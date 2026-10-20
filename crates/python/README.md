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

query = buttercupql.compile("""
    from cases
    where status == "open" and band in ["critical", "high"]
    sort -risk
    take 20
""", dialect="sqlite")

rows = sqlite3.connect("cases.db").execute(query.sql, query.params).fetchall()
```

`dialect` is `sqlite`, `postgres`, `duckdb` or `clickhouse`. Each gets its own placeholders: `?` for SQLite, `$1` for
PostgreSQL and DuckDB, and `{p1:String}` for ClickHouse; `params="format"` writes psycopg's `%s`.

- **Mistakes are shown at their place**, every one at once: `buttercupql.CompileError` holds them, and its message
  shows each under its line, with a suggestion.
- **A schema checks every name:** `schema="CREATE TABLE cases (...)"` takes the database's own statements.
- **Queries with arguments:** `compile(text, query="cases_for")` compiles a query `def` with its parameters left as
  placeholders, and `query.bind(account="A1")` gives their values.
- **The command** comes with the package: `bcql compile query.bcql`, `bcql check *.bcql`, and `bcql lsp` for editors.

The language, its design and its documentation: <https://github.com/mkusiappiah/ButtercupQL>.
