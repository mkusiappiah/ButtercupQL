# Changelog

## Unreleased

- The parser. Every form in the language design is read into a syntax tree, and every mistake in a file is reported in
  one reading, each at its place, with a suggestion where one helps. Habits from SQL and pandas, such as `order by`,
  `limit` or `=` in a condition, are reported with the ButtercupQL way to write them.
- `bcql check`, the first command: it checks ButtercupQL files and shows each problem under its line.
- The language design, v0.1, approved ([docs/design.md](docs/design.md)), and the research behind it
  ([docs/research.md](docs/research.md)).
- The Rust workspace, the dialects ButtercupQL compiles to, and automated tests on GitHub.
