# Changelog

## Unreleased

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
