# ButtercupQL for Visual Studio Code

[ButtercupQL](https://github.com/mkusiappiah/ButtercupQL) is a Python-like language for SQL, named after the
buttercup flower. A query is written one step per line, in the order the data flows, and compiles to the SQL that
SQLite, PostgreSQL, DuckDB and ClickHouse run.

```python
from cases
where status == "open" and band in ["critical", "high"]
sort -risk
take 20
```

This extension gives `.bcql` files:

- **Colours** for steps, functions, strings, numbers and comments.
- **Problems as you type,** each at its place with a suggestion, as `bcql check` finds them. Every query in a file is
  checked: the main query, each named query and each query `def`.
- **Completion** of steps, functions, methods, the file's own definitions, and, with a schema, tables, columns and
  links (`account.` lists the columns of the account a row points to).
- **Descriptions** of a step, function, column or definition when the pointer rests on it.
- **Go to definition**, into the modules a file imports too.
- **An outline** of the file's definitions.
- **Show SQL** (the database button above the editor): the SQL the query compiles to, beside the file, kept up to date
  as you type. With the cursor in a query `def` or a named query, it shows that one, its arguments left as
  placeholders.

## The `bcql` command

The extension runs `bcql lsp`, the ButtercupQL language server. The extension built for your platform brings `bcql`
with it. Otherwise, install it in one of these ways and the extension finds it on the `PATH`:

- `pip install git+https://github.com/mkusiappiah/ButtercupQL`
- `cargo install --git https://github.com/mkusiappiah/ButtercupQL bcql`
- a build from the [releases](https://github.com/mkusiappiah/ButtercupQL/releases)

## Settings

| Setting | Default | What it does |
|---|---|---|
| `buttercupql.dialect` | `postgres` | The database: `postgres`, `sqlite`, `duckdb` or `clickhouse`. |
| `buttercupql.schema` | none | A file of the database's `CREATE TABLE` statements. Names are checked against it and completed from it. A relative path is read from the workspace's folder. |
| `buttercupql.server.path` | none | The `bcql` command to run, when it is not the one that comes with the extension or the one on the `PATH`. |

## Commands

- **ButtercupQL: Show SQL**
- **ButtercupQL: Restart the Language Server**

## Licence

MIT or Apache-2.0, at your option. Written by Michael Kusi-Appiah.
