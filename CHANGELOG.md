# Changelog

## Unreleased

### Added

- Python: `buttercupql.run(connection, text)` compiles a text for the database a connection is to and runs it with
  the database's own driver (`sqlite3`, `psycopg`, `psycopg2`, `pg8000`, `duckdb`, or a `clickhouse_connect`
  client), as one transaction, and gives the rows with their columns' names. `read_schema(connection)` reads the
  database's tables and the links between them. `Compiled.statements` gives each statement of a text.
- Notebooks: `%load_ext buttercupql`, then a cell under `%%bcql db` runs on the connection `db` and shows its rows,
  as a pandas DataFrame when pandas is installed, with its names checked against the database's own tables first.

### Fixed

- A sort before a `select` that gives a column the name of one it reads, as in `sort risk` and then
  `select key, risk = -risk`, sorted the rows by the new column on every database. In `ORDER BY`, the old column is
  now named with its table: `ORDER BY cases.risk`. ClickHouse reads such a name as the new column wherever it is
  written alone, so there the query's columns are all named with their tables, and a `where` before the `select`
  keeps the rows it means.

## 0.1.0 — 20 October 2026

The first release of ButtercupQL, a Python-like language for SQL.

### The language

- The language design, v0.1 ([docs/design.md](docs/design.md)), and the research behind it
  ([docs/research.md](docs/research.md)).
- Steps, one per line in the order the data flows: `from`, `where`, a step that adds a column, `select`, `del`,
  `sort`, `take`, `skip`, `distinct`, `group by`, `summarize`, `join` and `left join`.
- Python's expressions with Python's meaning, on every database: `/` divides exactly, `//` rounds down, `%` takes the
  divisor's sign, `x == None` is `IS NULL`, `not in` with `None` means what it says, missing values sort last, and
  `sum()` of no rows is `0`.
- Grouping with totals and their own conditions, `where=`, and grouping twice with no subquery to write.
- Window functions: totals with `over=`, `sort=` and `rows=` for running and moving totals, and `row_number()`,
  `rank()`, `dense_rank()`, `lag()`, `lead()`, `first()` and `last()`.
- Links: `account.operator` follows the schema's `REFERENCES` and writes the join.
- Named queries, values and `def`. A query `def` compiled on its own keeps its parameters as placeholders, and
  `bind()` gives their values, with defaults.
- Modules: `import shared` and `from shared import recent` read other files' definitions, through a loader; a mistake
  in a module is shown in the module's own text.

### Compiling

- SQL for SQLite, PostgreSQL, DuckDB and ClickHouse, each in its own forms: ClickHouse's `{p1:String}` placeholders,
  `countIf()`, `sumIf()`, `maxOrNull()` and `join_use_nulls`, and DuckDB's `QUALIFY`. One flat `SELECT` is written
  where the steps allow, and a subquery only where the meaning needs one. Values become parameters.
- Schemas, read from the database's own `CREATE TABLE` statements, to check every name with a suggestion for a
  misspelt one, and to write SQL that uses what the tables say.
- Every mistake in a file is reported in one reading, each at its place, with a suggestion where one helps. Habits
  from SQL and pandas, such as `order by`, `limit` or `=` in a condition, are reported with the ButtercupQL way to
  write them.
- Tests that run the SQL on real SQLite, PostgreSQL, DuckDB and ClickHouse, and check that each gives the rows
  Python's meaning gives.

### Tools

- `bcql`, the command: `bcql compile` writes the SQL and its parameters, and `bcql check` reports every mistake in
  files, compiling each query, named query and query `def` in them. Each release has it for Linux, macOS and Windows.
- `bcql lsp`, a language server. It shows each problem as it is typed, a problem in an imported module on the import
  that reads it; completes steps, functions, methods, the file's own definitions and, with a schema, tables, columns
  and links; describes them on hover; goes to a definition, in another module too; lists a file's definitions; and
  gives the SQL of a query for an editor to show.
- The VS Code extension: colours, the language server, and **Show SQL** beside the file. Each release has it for each
  platform, with `bcql` in it, and for any platform, using the `bcql` on the `PATH`.
- The Python package, `buttercupql`: `compile()` and `check()`, with every problem found raised as one
  `CompileError`; schemas as `CREATE TABLE` text, a dict or a `Schema`; a query `def`'s arguments given with
  `bind()`; modules from files or from texts; and the `bcql` command. One wheel serves every Python from 3.9.
- The Rust crate, `buttercupql`, with no dependencies. `buttercupql::builtins` describes every step, function, method
  and part of a date.
- The playground, at <https://mkusiappiah.github.io/ButtercupQL/playground/>: ButtercupQL compiled in the browser,
  built for WebAssembly. It shows the SQL for each database as you type, underlines each mistake with its message,
  compiles a query `def` on its own with its arguments, checks names against a schema, and shares a query as a link.
- The book, at <https://mkusiappiah.github.io/ButtercupQL/>: a guide to the language and its reference. The tests check
  every example in it: each query compiles to the SQL shown, each mistake gets the message shown, and the Python and
  Rust examples run.
