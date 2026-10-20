# Python

The `buttercupql` package compiles ButtercupQL from Python, and brings the `bcql` command with it. It is the Rust
compiler itself, built for Python, so it compiles a query in microseconds: fast enough to compile on every request.

```sh
pip install git+https://github.com/mkusiappiah/ButtercupQL   # Rust must be installed to build it
```

One package serves every Python from 3.9.

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
