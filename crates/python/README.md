# ButtercupQL for Python

**A Python-like language for SQL**, named after the buttercup flower. Write a query the way you write Python, one step
per line in the order the data flows; ButtercupQL compiles it to the SQL your database runs, with every value passed as
a parameter.

```sh
pip install buttercupql
```

```python
import sqlite3
import buttercupql

db = sqlite3.connect("cases.db")
rows = buttercupql.run(db, """
    from cases
    where status == "open" and band in ["critical", "high"]
    sort -risk
    take 20
""")
```

`run()` compiles the text for the database the connection is to, runs it with the database's own driver, and gives
the rows, with their columns' names in `rows.columns` and `rows.to_pandas()` for a DataFrame. It takes `sqlite3`,
`psycopg`, `psycopg2`, `pg8000` and `duckdb` connections, and `clickhouse_connect` clients. A text that changes the
database runs as one transaction: all of it, or none of it if a statement fails.

`compile()` gives the SQL and its parameters, for any driver:

```python
query = buttercupql.compile('from cases\nwhere status == "open"\n', dialect="sqlite")
rows = db.execute(query.sql, query.params).fetchall()
```

`dialect` is `sqlite`, `postgres`, `duckdb` or `clickhouse`. Each gets its own placeholders: `?` for SQLite, `$1` for
PostgreSQL and DuckDB, and `{p1:String}` for ClickHouse; `params="format"` writes psycopg's `%s`.

In Jupyter and IPython, `%load_ext buttercupql` gives a cell magic: a cell under `%%bcql db` runs on the connection
`db` and shows its rows, with its names checked against the database's own tables first.

- **Mistakes are shown at their place**, every one at once: `buttercupql.CompileError` holds them, and its message
  shows each under its line, with a suggestion.
- **A schema checks every name:** `schema="CREATE TABLE cases (...)"` takes the database's own statements, and
  `buttercupql.read_schema(db)` reads them from the database.
- **Changes to the database:** `update`, `delete` and `insert into` at the end of a query, `create table` with Python's
  annotations, indexes, `with transaction:`, permissions, recursive queries and pivots.
- **Queries with arguments:** `compile(text, query="cases_for")` compiles a query `def` with its parameters left as
  placeholders, and `query.bind(account="A1")` gives their values.
- **The command** comes with the package: `bcql compile query.bcql`, `bcql check *.bcql`, and `bcql lsp` for editors.

The language, its design and its documentation: <https://github.com/mkusiappiah/ButtercupQL>.
