# Python

The `buttercupql` package compiles ButtercupQL from Python, and brings the `bcql` command with it. It is the Rust
compiler itself, built for Python, so it compiles a query in microseconds: fast enough to compile on every request.

```sh
pip install buttercupql
```

The package is [on PyPI](https://pypi.org/project/buttercupql/). One wheel serves every Python from 3.9, on Linux,
macOS and Windows.

## Compiling and running a query

`compile()` gives the SQL and the values for its placeholders, ready for the database's own driver:

```py
import sqlite3
import buttercupql

db = sqlite3.connect(":memory:")
db.executescript("""
    CREATE TABLE cases (key TEXT, status TEXT, band TEXT, risk INTEGER);
    INSERT INTO cases VALUES ('C1', 'open', 'critical', 95), ('C2', 'open', 'low', 40),
                             ('C3', 'closed', 'high', 90), ('C4', 'open', 'high', NULL);
""")

query = buttercupql.compile("""
    from cases
    where status == "open" and band in ["critical", "high"]
    sort -risk
    select key, risk
""", dialect="sqlite")

assert query.sql == "SELECT key, risk\nFROM cases\nWHERE status = ? AND band IN (?, ?)\nORDER BY risk DESC"
assert query.params == ["open", "critical", "high"]
assert db.execute(query.sql, query.params).fetchall() == [("C1", 95), ("C4", None)]
```

A query indented inside a Python string reads as it is: the first line sets the column.

`dialect` is `"postgres"`, the default, `"sqlite"`, `"duckdb"` or `"clickhouse"`. `params` chooses another style of
placeholder: `"format"` writes `%s`, as psycopg takes them.

```py
query = buttercupql.compile('from cases\nwhere status == "open"\n', params="format")
assert query.sql.endswith("WHERE status = %s")
```

## Running on a database

`run()` compiles a text and runs it with the database's own driver, and gives the rows of its query: a list of
tuples, as `fetchall()` gives them, with the names of the columns in `columns`.

```py
rows = buttercupql.run(db, """
    from cases
    where status == "open"
    sort -risk
    select key, risk
""")
assert rows == [("C1", 95), ("C2", 40), ("C4", None)]
assert rows.columns == ["key", "risk"]
```

The driver says which database it is, and which placeholders it takes: `sqlite3`; `psycopg`, `psycopg2` or `pg8000`
for PostgreSQL; `duckdb`; or a `clickhouse_connect` client. For another DB-API driver, `dialect=` names the
database. `rows.to_pandas()` gives the rows as a pandas DataFrame.

A text that changes the database runs statement by statement, in order, as one transaction: all of it, or none of it
if a statement fails. ClickHouse has no transactions, so there each statement stands once it has run. The rows given
are the last query's:

```py
rows = buttercupql.run(db, """
    create table archive:
        key: str = field(primary_key=True)
        risk: int | None

    from cases
    where status == "closed"
    select key, risk
    insert into archive

    from cases
    where status == "closed"
    delete

    from archive
""")
assert rows == [("C3", 90)]
```

`query=` runs a query `def` of the text, with its arguments given by name, as [`bind()`](#queries-with-arguments)
takes them.

## The database's own tables

`read_schema()` reads the tables of the database a connection is to, with their columns and the links between them,
for names to be checked against: a `Schema` for `schema=`.

```py
schema = buttercupql.read_schema(db)
assert schema.tables == ["archive", "cases"]
try:
    buttercupql.run(db, "from archive\nsort -rsik\n", schema=schema)
except buttercupql.CompileError as error:
    assert error.diagnostics[0].label == "did you mean `risk`?"
```

It reads SQLite's, DuckDB's and PostgreSQL's tables and views, and ClickHouse's. In a notebook, the `%%bcql` magic
reads them for each cell: see [Notebooks](notebooks.md).

## Statements

A text compiles to one statement for a query, and to one for each change in a text that changes the database.
`statements` gives each, to run one by one in order, with its SQL, its values and its `kind`: `"query"`,
`"change"`, `"definition"` for a change to the database itself, and `"begin"` and `"commit"` around a
`with transaction:` block. `sql` and `params` are all the statements together.

```py
script = buttercupql.compile("""
    create table seen:
        key: str

    from cases
    where risk < 50
    update band = "low"
""", dialect="sqlite")
assert [statement.kind for statement in script.statements] == ["definition", "change"]
update = script.statements[1]
assert (update.sql, update.params) == ("UPDATE cases SET band = ?\nWHERE risk < 50", ["low"])
```

## Mistakes

A text that does not compile raises `buttercupql.CompileError`, a `ValueError`. It holds every problem found, and its
message shows each under its line, as `bcql` does:

```py
try:
    buttercupql.compile('from cases\nwhere status = "open"\n', path="open.bcql")
except buttercupql.CompileError as error:
    [problem] = error.diagnostics
    assert (problem.severity, problem.line, problem.column) == ("error", 2, 14)
    assert problem.message == "use `==` to compare"
    assert str(error).startswith("error: use `==` to compare\n  ┌─ open.bcql:2:14\n")
```

`check()` gives the problems of a text without raising, and an empty list when there are none:

```py
assert [p.message for p in buttercupql.check("from cases\nwehre risk > 90\n")] == ["there is no step `wehre`"]
assert buttercupql.check("from cases\nsort -risk\n") == []
```

## Schemas

`schema=` checks every name against the database's tables. It takes the `CREATE TABLE` statements, a
`buttercupql.Schema`, or a dict of tables and their columns' types:

```py
schema = "CREATE TABLE cases (key text NOT NULL, status text NOT NULL, risk integer);"
try:
    buttercupql.compile('from cases\nwhere stauts == "open"\n', schema=schema)
except buttercupql.CompileError as error:
    assert error.diagnostics[0].label == "did you mean `status`?"

tables = {"cases": {"key": "text not null", "risk": "integer"}}
assert buttercupql.compile("from cases\nsort -key\n", schema=tables).sql.endswith("ORDER BY key DESC")
```

## Queries with arguments

`query=` compiles a named query or a query `def` in place of the main query. A `def`'s parameters become placeholders,
and `bind()` gives their values when the query runs, with the defaults for those not given:

```py
text = """
def open_for(account: str, rows: int = 10):
    from cases
    where account_id == account
    take rows
"""
query = buttercupql.compile(text, dialect="sqlite", query="open_for")
assert query.sql == "SELECT *\nFROM cases\nWHERE account_id = ?\nLIMIT ?"
assert [(arg.name, arg.type, arg.default) for arg in query.args] == [("account", "str", None), ("rows", "int", 10)]
assert query.bind(account="A1") == ["A1", 10]
assert query.bind(account="A1", rows=3) == ["A1", 3]
```

The query can be compiled once and run with different arguments as often as needed. `bind_named()` gives the values
by name, for named placeholders such as ClickHouse's:

```py
query = buttercupql.compile(text, dialect="clickhouse", query="open_for")
assert query.bind_named(account="A1") == {"account": "A1", "rows": 10}
```

A date or a time given to `bind()` is passed as text, in the form databases read: `2026-10-20 14:30:00`.

## Modules

A text that imports modules needs to know where they are. `path=` names the file the text is from, and its imports
are read from files beside it; `modules=` gives the modules' texts instead:

```py
shared = "threshold = 1_000_000\ndef recent(days_back=7):\n    where raised > now() - days(days_back)\n"
query = buttercupql.compile(
    "from shared import recent, threshold\nfrom cases\nrecent(30)\nwhere amount > threshold\n",
    modules={"shared": shared},
)
assert query.sql == "SELECT *\nFROM cases\nWHERE raised > now() - INTERVAL '30 days' AND amount > 1000000"
```

## The command

The package installs `bcql`, which `python -m buttercupql` also runs: see [The bcql command](command.md).
