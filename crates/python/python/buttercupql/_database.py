"""Running ButtercupQL on a database, through the database's own Python driver."""

import html
import sys

from ._buttercupql import compile

# The database each driver is for, by the name of its module.
_DIALECTS = {
    "sqlite3": "sqlite",
    "psycopg": "postgres",
    "psycopg2": "postgres",
    "pg8000": "postgres",
    "duckdb": "duckdb",
    "_duckdb": "duckdb",
    "clickhouse_connect": "clickhouse",
}

# The placeholders each driver takes, where they are not the database's own: psycopg takes `%s`, not `$1`.
_PARAMS = {
    "psycopg": "format",
    "psycopg2": "format",
    "pg8000": "format",
}

# The placeholders a DB-API driver says it takes, in its module's `paramstyle`.
_PARAMSTYLES = {"qmark": "question", "format": "format", "pyformat": "format", "named": "colon"}


class Rows(list):
    """The rows a query gave: a list of tuples, as `fetchall()` gives them, with the names of their columns in
    `columns`."""

    def __init__(self, rows=(), columns=()):
        super().__init__(tuple(row) for row in rows)
        self.columns = list(columns)

    def to_pandas(self):
        """The rows as a pandas `DataFrame`."""
        import pandas

        return pandas.DataFrame(list(self), columns=self.columns)

    def _repr_html_(self):
        # A notebook shows the rows as a table.
        head = "".join(f"<th>{html.escape(str(name))}</th>" for name in self.columns)
        body = "".join(
            "<tr>" + "".join(f"<td>{html.escape(_shown(value))}</td>" for value in row) + "</tr>" for row in self
        )
        return f"<table><thead><tr>{head}</tr></thead><tbody>{body}</tbody></table>"


def _shown(value):
    return "None" if value is None else str(value)


def _driver(connection):
    """The name of the module a connection's driver is, such as `sqlite3` or `psycopg`."""
    return type(connection).__module__.split(".")[0]


def dialect_of(connection):
    """The database a connection is to, by its driver: `"sqlite"`, `"postgres"`, `"duckdb"` or `"clickhouse"`, or
    `None` for a driver ButtercupQL does not know."""
    return _DIALECTS.get(_driver(connection))


def run(connection, text, *, dialect=None, schema=None, query=None, path=None, modules=None, **args):
    """Compiles a ButtercupQL text and runs it on a database, and gives the rows of its query.

    `connection` is a connection of the database's own driver: `sqlite3`, `psycopg`, `psycopg2` or `pg8000` for
    PostgreSQL, `duckdb`, any other DB-API driver with `dialect=`, or a `clickhouse_connect` client. The database is
    told by the driver, unless `dialect=` says it.

    The statements run one by one, in order, as one transaction: all of them, or none of them if one fails. On
    ClickHouse, which has no transactions, each statement stands once it has run. The answer is the rows of the
    last statement that gives rows, as `Rows`: a list of tuples with the names of the columns in `columns`, or
    `None` when no statement gives rows.

    `query=` runs a named query or a query `def` of the text, its arguments given by name: `query="open_for",
    account="A1"`. `schema=`, `path=` and `modules=` are as `compile()` takes them.
    """
    driver = _driver(connection)
    if dialect is None:
        dialect = _DIALECTS.get(driver)
        if dialect is None:
            raise ValueError(
                f"ButtercupQL does not know the {driver!r} driver's database: say it with dialect=, "
                'such as dialect="postgres"'
            )
    if args and query is None:
        raise TypeError(
            f"arguments are given to a query def named with query=, but no query is named: {', '.join(args)}"
        )
    clickhouse = driver == "clickhouse_connect"
    params = None if clickhouse else _PARAMS.get(driver) or _paramstyle(driver, dialect)
    compiled = compile(text, dialect=dialect, schema=schema, params=params, query=query, path=path, modules=modules)
    statements = compiled.statements
    if query is not None:
        # A query `def` compiled on its own is one statement, its placeholders filled with the arguments given.
        values = [compiled.bind(**args)]
    else:
        values = [statement.params for statement in statements]
    named = params == "colon" or clickhouse
    if named:
        values = [dict(zip(statement.names, given)) for statement, given in zip(statements, values)]
    if clickhouse:
        return _run_clickhouse(connection, statements, values)
    return _run_dbapi(connection, driver, statements, values)


def _paramstyle(driver, dialect):
    """The placeholders a DB-API driver takes, from its module's `paramstyle`; the database's own if it does not
    say, or if they are the same."""
    module = sys.modules.get(driver)
    style = _PARAMSTYLES.get(getattr(module, "paramstyle", None))
    own = {"sqlite": "question", "clickhouse": "braces"}.get(dialect, "dollar")
    return None if style in (None, own) else style


def _begins_itself(connection, driver):
    """Whether the driver starts a transaction itself, as DB-API drivers do unless told not to, so that its
    `commit()` ends it; otherwise `BEGIN` starts one."""
    if driver in ("duckdb", "_duckdb"):
        return False
    if driver == "sqlite3":
        # Before Python 3.12's `autocommit`, sqlite3 starts a transaction only before a change to rows, not before
        # `CREATE TABLE`, so `BEGIN` starts it.
        return getattr(connection, "autocommit", None) is False
    return not getattr(connection, "autocommit", False)


def _run_dbapi(connection, driver, statements, values):
    duckdb = driver in ("duckdb", "_duckdb")
    # A DuckDB cursor is a connection of its own, with transactions of its own: statements run on the connection.
    cursor = connection if duckdb else connection.cursor()
    # The text runs as one transaction: all of it, or none of it if a statement fails.
    begun = not _begins_itself(connection, driver)
    if begun and getattr(connection, "in_transaction", False):
        # sqlite3 has a transaction open already, for changes not yet committed: the text runs in it.
        begun = False
    rows = None
    try:
        if begun:
            cursor.execute("BEGIN")
        for statement, given in zip(statements, values):
            if statement.kind in ("begin", "commit"):
                # A `with transaction:` block runs in the text's own transaction.
                continue
            # The values go with every statement, none too: with `%s` placeholders, a `%` in the SQL is written
            # `%%`, which a driver reads back only when it is given values.
            cursor.execute(statement.sql, given)
            if cursor.description is not None:
                rows = Rows(cursor.fetchall(), [column[0] for column in cursor.description])
        if begun:
            cursor.execute("COMMIT")
        else:
            connection.commit()
    except BaseException:
        try:
            if begun:
                cursor.execute("ROLLBACK")
            else:
                connection.rollback()
        except Exception:
            # The database may have ended the transaction itself: the error that failed the text is the one shown.
            pass
        raise
    finally:
        if not duckdb:
            cursor.close()
    return rows


def _run_clickhouse(client, statements, values):
    rows = None
    for statement, given in zip(statements, values):
        if statement.kind == "query":
            result = client.query(statement.sql, parameters=given)
            rows = Rows(result.result_rows, result.column_names)
        else:
            client.command(statement.sql, parameters=given)
    return rows
