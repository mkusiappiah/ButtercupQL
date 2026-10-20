# Changelog

## Unreleased

- The book, at <https://mkusiappiah.github.io/ButtercupQL/>: a guide to the language and its reference, published with
  the playground on GitHub Pages. The tests check every example in it, and in the README and the design: each query
  compiles to the SQL shown, each mistake gets the message shown, and the Python and Rust examples run.
- The playground: ButtercupQL compiled in the browser, built for WebAssembly (`crates/wasm`, 454 KB). It shows the SQL
  for each database as you type, underlines each mistake with its message, compiles a query `def` on its own with
  its arguments, checks names against a schema, and shares a query as a link.
- `bcql lsp`, a language server. It shows each problem as it is typed, a problem in an imported module on the import
  that reads it; completes steps, functions, methods, the file's own definitions and, with a schema, tables, columns
  and links; describes them on hover; goes to a definition, in another module too; lists a file's definitions; and
  gives the SQL of a query for an editor to show. Places are counted in UTF-16, or in UTF-8 when the editor asks.
- The VS Code extension, in `editors/vscode`: colours, the language server, and **Show SQL** beside the file.
- `bcql check` also compiles a file's named queries and query `def`s, so a file of definitions is checked as fully
  as a query.
- `buttercupql::builtins`, which describes every step, function, method and part of a date, and `FileLoader::path`.
- The Python package, `buttercupql`: `compile()` and `check()`, with every problem found raised as one `CompileError`;
  schemas as `CREATE TABLE` text, a dict or a `Schema`; a query `def`'s arguments given with `bind()`; modules from
  files or from texts; and the `bcql` command. One wheel serves every Python from 3.9.
- DuckDB and ClickHouse, in their own forms: ClickHouse's `{p1:String}` placeholders, `countIf()` and `sumIf()`,
  `maxOrNull()` for the total of no rows, and `join_use_nulls` for left joins; DuckDB's `QUALIFY`.
- Window functions: totals with `over=`, `sort=` and `rows=` for running and moving totals, and `row_number()`,
  `rank()`, `dense_rank()`, `lag()`, `lead()`, `first()` and `last()`.
- Links: `account.operator` follows the schema's `REFERENCES` and writes the join.
- Queries with arguments: `--query` compiles a query `def` with its parameters as placeholders, and `bind()` gives
  their values, with defaults.
- Modules: `import shared` and `from shared import recent` read other files' definitions, through a loader; a mistake
  in a module is shown in the module's text.
- Tests on real DuckDB and ClickHouse, beside SQLite and PostgreSQL.
- The compiler, for SQLite and PostgreSQL: every step, Python's expressions with Python's meaning, grouping with
  totals and their conditions, joins, named queries and `def`. One flat `SELECT` is written where the steps allow, and
  a subquery only where the meaning needs one. Strings become parameters.
- Schemas, read from the database's own `CREATE TABLE` statements, to check every name with a suggestion for a
  misspelt one, and to write SQL that uses what the tables say.
- `bcql compile`, which writes the SQL and its parameters, and `bcql check --schema`.
- Tests that run the SQL on SQLite and PostgreSQL, and check that both give the rows Python's meaning gives.
- The parser. Every form in the language design is read into a syntax tree, and every mistake in a file is reported in
  one reading, each at its place, with a suggestion where one helps. Habits from SQL and pandas, such as `order by`,
  `limit` or `=` in a condition, are reported with the ButtercupQL way to write them.
- `bcql check`, the first command: it checks ButtercupQL files and shows each problem under its line.
- The language design, v0.1, approved ([docs/design.md](docs/design.md)), and the research behind it
  ([docs/research.md](docs/research.md)).
- The Rust workspace, the dialects ButtercupQL compiles to, and automated tests on GitHub.
