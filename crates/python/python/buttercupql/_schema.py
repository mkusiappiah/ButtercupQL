"""The tables of a database, read from the database itself through its driver."""

import re

from ._buttercupql import Schema
from ._database import _driver, dialect_of

# A name written as it is in SQL; any other is quoted.
_PLAIN = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")


def read_schema(connection):
    """The tables of the database a connection is to, with their columns, the kinds of values they hold, whether
    they can be missing, and the links between tables, for names to be checked against: a `Schema`.

    It reads SQLite's, DuckDB's and PostgreSQL's tables and views through their DB-API drivers, and ClickHouse's
    through a `clickhouse_connect` client. Tables of the schemas a query reads without naming one are named alone;
    those of other schemas, or of other attached databases, are named with their schema: `analytics.daily`.
    """
    dialect = dialect_of(connection)
    readers = {
        "sqlite": _sqlite,
        "duckdb": _duckdb,
        "postgres": _postgres,
        "clickhouse": _clickhouse,
    }
    if dialect not in readers:
        raise ValueError(f"ButtercupQL can't read the tables of a {_driver(connection)!r} connection")
    tables = readers[dialect](connection)
    return Schema.from_sql(_ddl(tables))


class _Column:
    def __init__(self, name, kind, not_null=False, references=None):
        self.name = name
        self.kind = kind
        self.not_null = not_null
        # The table and the column it points to, for a column that holds another row's key.
        self.references = references


def _ddl(tables):
    """`CREATE TABLE` statements for the tables, as the schema reader reads them."""
    statements = []
    for table, columns in tables.items():
        lines = []
        for column in columns:
            line = f"{_name(column.name)} {_kind(column.kind)}"
            if column.not_null:
                line += " NOT NULL"
            if column.references is not None:
                target, key = column.references
                line += f" REFERENCES {_dotted(target)} ({_name(key)})"
            lines.append(line)
        if lines:
            statements.append(f"CREATE TABLE {_dotted(table)} ({', '.join(lines)});")
    return "\n".join(statements)


def _name(name):
    return name if _PLAIN.match(name) else '"' + name.replace('"', '""') + '"'


def _dotted(name):
    return ".".join(_name(part) for part in name.split("."))


def _kind(kind):
    """A type's name, plain enough for the schema reader: `numeric` of `numeric(12, 2)`. A list, a map or a
    structure is a kind of value ButtercupQL does not know."""
    kind = (kind or "").strip()
    if not kind or "[" in kind or kind.upper().startswith(("ARRAY", "MAP", "STRUCT", "TUPLE", "UNION", "LIST")):
        return "unknown"
    base = kind.split("(")[0]
    words = re.findall(r"[A-Za-z0-9_]+", base)
    return " ".join(words) or "unknown"


def _qualified(schema, table, plain_schemas):
    return table if schema in plain_schemas else f"{schema}.{table}"


def _sqlite(connection):
    tables = {}
    databases = [row[1] for row in connection.execute("PRAGMA database_list").fetchall()]
    for database in databases:
        quoted = _name(database)
        names = connection.execute(
            f"SELECT name FROM {quoted}.sqlite_master WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' "
            "ORDER BY name"
        ).fetchall()
        for (table,) in names:
            info = connection.execute(f"PRAGMA {quoted}.table_info({_name(table)})").fetchall()
            keys = {}
            for row in connection.execute(f"PRAGMA {quoted}.foreign_key_list({_name(table)})").fetchall():
                number, _, target, source, key = row[:5]
                keys.setdefault(number, []).append((source, target, key))
            links = {}
            for parts in keys.values():
                # A key of one column is a link; a key of several is not followed.
                if len(parts) == 1:
                    source, target, key = parts[0]
                    links[source] = (target, key)
            columns = []
            for _, name, kind, not_null, _, primary in info:
                # An INTEGER PRIMARY KEY holds the row's number, which is never missing.
                not_null = bool(not_null) or (bool(primary) and (kind or "").upper() == "INTEGER")
                columns.append(_Column(name, kind, not_null, links.get(name)))
            tables[_qualified(database, table, {"main", "temp"})] = columns
    # A link to a table's key with no column named is to its primary key.
    for columns in tables.values():
        for column in columns:
            if column.references is not None and column.references[1] is None:
                target = column.references[0]
                primary = connection.execute(f"PRAGMA table_info({_name(target)})").fetchall()
                keys = [row[1] for row in primary if row[5]]
                column.references = (target, keys[0]) if len(keys) == 1 else None
    return tables


def _duckdb(connection):
    current = connection.execute("SELECT current_database(), current_schema()").fetchone()
    rows = connection.execute(
        "SELECT table_catalog, table_schema, table_name, column_name, data_type, is_nullable "
        "FROM information_schema.columns "
        "WHERE table_schema NOT IN ('information_schema', 'pg_catalog') "
        "ORDER BY table_catalog, table_schema, table_name, ordinal_position"
    ).fetchall()
    links = {}
    try:
        keys = connection.execute(
            "SELECT database_name, schema_name, table_name, constraint_column_names, referenced_table, "
            "referenced_column_names FROM duckdb_constraints() WHERE constraint_type = 'FOREIGN KEY'"
        ).fetchall()
    except Exception:
        # An older DuckDB does not say which table a key points to.
        keys = []
    for database, schema, table, sources, target, targets in keys:
        if len(sources) == 1 and len(targets) == 1:
            links[(database, schema, table, sources[0])] = (target, targets[0])
    tables = {}
    for database, schema, table, column, kind, nullable in rows:
        if database == current[0]:
            name = _qualified(schema, table, {current[1]})
        else:
            name = f"{database}.{schema}.{table}"
        tables.setdefault(name, []).append(
            _Column(column, kind, nullable == "NO", links.get((database, schema, table, column)))
        )
    return tables


def _idle(connection):
    """Whether a PostgreSQL connection has no transaction open, by psycopg's or psycopg2's word; `False` when the
    driver does not say."""
    status = getattr(getattr(connection, "info", None), "transaction_status", None)
    if status is None and hasattr(connection, "get_transaction_status"):
        status = connection.get_transaction_status()
    return status is not None and int(status) == 0


def _postgres(connection):
    # The catalogue is read in a transaction of its own, ended after, unless one is open already.
    ends = _idle(connection) and not getattr(connection, "autocommit", False)
    cursor = connection.cursor()
    try:
        cursor.execute("SELECT current_schemas(false)")
        plain = set(cursor.fetchone()[0])
        cursor.execute(
            "SELECT n.nspname, c.relname, a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull "
            "FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace "
            "WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') AND a.attnum > 0 AND NOT a.attisdropped "
            "AND n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg_toast%' "
            "AND n.nspname NOT LIKE 'pg_temp%' "
            "ORDER BY n.nspname, c.relname, a.attnum"
        )
        rows = cursor.fetchall()
        cursor.execute(
            "SELECT n.nspname, c.relname, a.attname, rn.nspname, rc.relname, ra.attname "
            "FROM pg_constraint k "
            "JOIN pg_class c ON c.oid = k.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace "
            "JOIN pg_class rc ON rc.oid = k.confrelid JOIN pg_namespace rn ON rn.oid = rc.relnamespace "
            "JOIN pg_attribute a ON a.attrelid = k.conrelid AND a.attnum = k.conkey[1] "
            "JOIN pg_attribute ra ON ra.attrelid = k.confrelid AND ra.attnum = k.confkey[1] "
            "WHERE k.contype = 'f' AND cardinality(k.conkey) = 1"
        )
        keys = cursor.fetchall()
    finally:
        cursor.close()
        if ends:
            connection.rollback()
    links = {
        (schema, table, column): (_qualified(target_schema, target, plain), key)
        for schema, table, column, target_schema, target, key in keys
    }
    tables = {}
    for schema, table, column, kind, not_null in rows:
        tables.setdefault(_qualified(schema, table, plain), []).append(
            _Column(column, kind, bool(not_null), links.get((schema, table, column)))
        )
    return tables


def _clickhouse(client):
    current = client.query("SELECT currentDatabase()").result_rows[0][0]
    rows = client.query(
        "SELECT database, table, name, type FROM system.columns "
        "WHERE database NOT IN ('system', 'INFORMATION_SCHEMA', 'information_schema') "
        "ORDER BY database, table, position"
    ).result_rows
    tables = {}
    for database, table, column, kind in rows:
        # ClickHouse's columns are never missing unless they are `Nullable`.
        nullable = False
        while True:
            wrapped = re.match(r"^(Nullable|LowCardinality)\((.*)\)$", kind)
            if not wrapped:
                break
            nullable = nullable or wrapped.group(1) == "Nullable"
            kind = wrapped.group(2)
        kind = {"DateTime64": "timestamp", "Date32": "date", "Bool": "boolean"}.get(kind.split("(")[0], kind)
        tables.setdefault(_qualified(database, table, {current}), []).append(_Column(column, kind, not nullable))
    return tables
