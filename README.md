# ButtercupQL

[![CI](https://github.com/mkusiappiah/ButtercupQL/actions/workflows/ci.yml/badge.svg)](https://github.com/mkusiappiah/ButtercupQL/actions/workflows/ci.yml)

**A Python-like language for SQL**, named after the buttercup flower.

Write a query the way you would write Python, one step per line in the order the data flows. ButtercupQL compiles it to
the SQL your database already runs: SQLite, PostgreSQL, DuckDB and ClickHouse.

**[Read the book](https://mkusiappiah.github.io/ButtercupQL/)** · **[Try it in the playground](https://mkusiappiah.github.io/ButtercupQL/playground/)**,
in your browser, with nothing to install

```python
# Critical cases nobody has taken, riskiest first
from cases
where status == "open" and band in ["critical", "high"]
where raised > now() - days(7)
sort -risk
take 20
select key, account, risk
```

becomes

```sql
SELECT key, account, risk
FROM cases
WHERE status = $1 AND band IN ($2, $3) AND raised > now() - INTERVAL '7 days'
ORDER BY risk DESC NULLS LAST
LIMIT 20
-- parameters: ["open", "critical", "high"]
```

on PostgreSQL, with the values `"open"`, `"critical"` and `"high"` passed as parameters, never pasted into the SQL.
Cases with no risk come last, on every database.

## Why

- **Faster to write.** The table comes first, so an editor can complete column names from the first word. Grouping
  columns are written once, one `where` works before and after grouping, and multi-step logic needs no subqueries.
- **As fast to run.** The compiler writes one flat `SELECT` where it can, uses each database's own forms, and passes
  values as parameters so plans can be reused.
- **Safer and clearer.** `x == None` means `IS NULL`, as it reads. Names are checked against the schema, and every error
  points at its place with a suggestion.
- **Reusable.** Named queries and `def`, as in Python.

Grouping, for example, writes each column once and filters the totals with the same `where`:

```python
from transactions
where time > now() - days(30)
group by operator:
    total = count()
    money = sum(amount)
    big = count(where=amount > 1_000_000)
where money > 1_000_000
sort -money
```

becomes

```sql
SELECT operator, count(*) AS total, coalesce(sum(amount), 0) AS money, count(*) FILTER (WHERE amount > 1000000) AS big
FROM transactions
WHERE time > now() - INTERVAL '30 days'
GROUP BY operator
HAVING coalesce(sum(amount), 0) > 1000000
ORDER BY money DESC
```

## Python's meaning, on every database

The same query gives the same rows on SQLite, PostgreSQL, DuckDB and ClickHouse, and they are the rows Python's meaning
gives. Every rule below is checked by running queries on all four databases:

| ButtercupQL | Gives | Where SQL alone would give |
|---|---|---|
| `7 / 2` | `3.5` | `3`: whole numbers divide to a whole number |
| `-5 // 2` | `-3`, rounded down | `-2`, rounded towards zero |
| `-5 % 3` | `1`, the divisor's sign | `-2` |
| `x not in [1, None]` | the rows where `x` is neither | no rows at all |
| `sort -risk` | missing values last | first on PostgreSQL |
| `notes.startswith("50%")` | notes that start `50%` | notes that start `50`: `%` matches anything in `LIKE` |
| `notes.startswith("ab")` | notes that start `ab` | on SQLite, `AB` too: its `LIKE` ignores case |
| `sum(x)` of no rows | `0` | `NULL` |

## Window functions, links and queries with arguments

```python
from cases
where account.operator == "Acme"              # a link: cases.account_id points to accounts
place = rank(over=account_id, sort=-risk)     # a window: 1 for each account's riskiest case
where place <= 3
```

- **Window functions** are totals with `over=`, `sort=` or `rows=`: `sum(amount, sort=time)` is a running total and
  `mean(amount, sort=day, rows=7)` a moving one. `row_number()`, `rank()`, `lag()`, `lead()`, `first()` and `last()`
  work the same way.
- **Links** follow the schema's `REFERENCES`: `account.operator` reads the operator of the account a case points to, and
  the compiler writes the join.
- **Queries with arguments.** A query `def` compiled with `--query` keeps its parameters as placeholders, given when the
  query runs:

```python
def for_operator(operator: str, rows: int = 10):
    from cases
    where account.operator == operator
    take rows
```

```text
$ bcql compile report.bcql --schema schema.sql --query for_operator
SELECT cases.*
FROM cases
LEFT JOIN accounts AS account ON cases.account_id = account.id
WHERE account.operator = $1
LIMIT $2
-- parameters: [{"arg": "operator"}, {"arg": "rows"}]
-- arguments: operator, rows = 10
```

- **Modules.** `from shared import recent` reads `shared.bcql` beside the file, as Python's imports do.

## Try it

[Each release](https://github.com/mkusiappiah/ButtercupQL/releases) has `bcql` built for Linux, macOS and Windows.
Or, with [Rust](https://rustup.rs) installed:

```sh
cargo install --git https://github.com/mkusiappiah/ButtercupQL bcql
bcql compile query.bcql                        # PostgreSQL, with $1 placeholders
bcql compile query.bcql --dialect clickhouse   # also sqlite and duckdb
bcql compile query.bcql --query for_operator   # a query def, its arguments left open
bcql check --schema schema.sql *.bcql          # names checked against the tables
bcql lsp                                       # the language server, for editors
```

or, in a copy of this repository, `cargo run -p bcql -- compile examples/open_critical.bcql`.

`--schema` takes the database's own `CREATE TABLE` statements. With it, every name is checked, and the SQL can use what
the tables say, such as which columns are never missing.

`bcql check` reports every mistake in a file in one go, each under its line, and says how to write it the ButtercupQL
way:

```text
error: use `==` to compare
  ┌─ query.bcql:3:14
  │
3 │ where status = "open"
  │              ^ `=` gives a column a value; `==` compares two values

error: there is no step `limit`
  ┌─ query.bcql:5:1
  │
5 │ limit 20
  │ ^^^^^ ButtercupQL calls this `take`
  = help: for example: `take 20`

checked 1 file: 2 errors
```

## In your editor

`bcql lsp` is a language server: an editor that speaks the Language Server Protocol shows each problem as it is
typed, completes steps, functions, tables, columns and links, describes each when the pointer rests on it, and goes to
where a name is defined, in an imported module too.

- **VS Code:** the extension in [editors/vscode](editors/vscode) adds colours and **Show SQL**, the SQL of the query
  beside the file, kept up to date as you type. Each [release](https://github.com/mkusiappiah/ButtercupQL/releases)
  has it for each platform, with `bcql` in it: install the `.vsix` with `code --install-extension`.
- **Other editors:** run `bcql lsp` for `.bcql` files. It takes the settings `{"dialect": "sqlite", "schema":
  "schema.sql"}`, when it starts or when they change.

## Using it from Python

```sh
pip install buttercupql
```

The package is [on PyPI](https://pypi.org/project/buttercupql/), with one wheel for every Python from 3.9 on Linux,
macOS and Windows.

```py
import sqlite3
import buttercupql

query = buttercupql.compile(open("open_critical.bcql").read(), dialect="sqlite")
rows = sqlite3.connect("cases.db").execute(query.sql, query.params).fetchall()
```

A mistake raises `buttercupql.CompileError`, which holds every problem found and shows each under its line. A query
`def` compiled with `query="for_operator"` gives its arguments with `query.bind(operator="Acme")`. The package also
installs the `bcql` command. [crates/python](crates/python) describes it.

## Using it from Rust

```rust
use buttercupql::{Dialect, Options};

let compiled = buttercupql::compile(text, &Options::new(Dialect::Postgres)).expect("the query compiles");
// compiled.sql, and compiled.params for its placeholders
```

On failure, `compile` returns every problem found; `render` shows one as `bcql` does.

## Status

Version 0.1.0, released on 20 October 2026, is the first. The language design is approved:
[docs/design.md](docs/design.md) describes it, and [docs/research.md](docs/research.md) the research behind it. [The
book](https://mkusiappiah.github.io/ButtercupQL/) teaches the language and is its reference; its source is in
[docs/book](docs/book), and every example in it is checked by the tests. The compiler is built for SQLite, PostgreSQL,
DuckDB and ClickHouse. Each example compiles to the SQL kept beside it in [examples/](examples), and the test suite runs
its queries on all four databases.

## Roadmap

1. **Language design** (approved 20 October 2026)
2. **Core:** tokens, the parser, error messages and `bcql check` (done)
3. **Compiler:** filtering, sorting, selecting, grouping, joins, named queries and `def`, compiling to SQLite and
   PostgreSQL, tested against both (done)
4. **More:** imports, window functions, links and queries with arguments; DuckDB and ClickHouse (done)
5. **Publishing:** the `buttercupql` Python package, the book, editor support and the playground (done: version 0.1.0,
   20 October 2026)

## Building

ButtercupQL is written in Rust (edition 2024). The library and the command have no dependencies. The workspace holds
five crates: [`buttercupql`](crates/buttercupql), the library; [`bcql`](crates/bcql), the command and the language
server; [`buttercupql-python`](crates/python), the Python package; [`buttercupql-wasm`](crates/wasm), the compiler
built for WebAssembly, for the playground; and [`db-tests`](crates/db-tests), which runs the SQL on real databases:
SQLite always, and the others when they are given.

```sh
cargo test --workspace --exclude buttercupql-python
BCQL_POSTGRES_URL=postgresql://localhost/test \
BCQL_DUCKDB_PYTHON=python3 \
BCQL_CLICKHOUSE_URL=http://localhost:8123 \
cargo test -p db-tests
```

The Python package is built with [maturin](https://www.maturin.rs), in a virtual environment:

```sh
pip install maturin pytest
maturin develop
pytest crates/python/tests
```

The VS Code extension is in [editors/vscode](editors/vscode) (`npm ci && npm test`), the playground in
[playground](playground), and the book in [docs/book](docs/book), built with [mdBook](https://rust-lang.github.io/mdBook/):

```sh
cargo build -p buttercupql-wasm --target wasm32-unknown-unknown --profile wasm
mdbook build docs/book -d "$PWD/site"
node playground/build.mjs target/wasm32-unknown-unknown/wasm/buttercupql_wasm.wasm site/playground
```

A tag `vX.Y.Z` makes a release, with [the release workflow](.github/workflows/release.yml).

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

## Author

Michael Kusi-Appiah ([@mkusiappiah](https://github.com/mkusiappiah))
