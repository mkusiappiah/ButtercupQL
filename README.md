# ButtercupQL

[![CI](https://github.com/mkusiappiah/ButtercupQL/actions/workflows/ci.yml/badge.svg)](https://github.com/mkusiappiah/ButtercupQL/actions/workflows/ci.yml)

**A Python-like language for SQL**, named after the buttercup flower.

Write a query the way you would write Python, one step per line in the order the data flows. ButtercupQL compiles it to
the SQL your database already runs: SQLite and PostgreSQL first, then DuckDB and ClickHouse.

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

The same query gives the same rows on SQLite and PostgreSQL, and they are the rows Python's meaning gives. Every rule
below is checked by running queries on both databases:

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

## Try it

With [Rust](https://rustup.rs) installed:

```sh
cargo install --git https://github.com/mkusiappiah/ButtercupQL bcql
bcql compile query.bcql                    # PostgreSQL, with $1 placeholders
bcql compile query.bcql --dialect sqlite   # SQLite, with ?
bcql check --schema schema.sql *.bcql      # names checked against the tables
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

## Using it from Rust

```rust
use buttercupql::{Dialect, Options};

let compiled = buttercupql::compile(text, &Options::new(Dialect::Postgres)).expect("the query compiles");
// compiled.sql, and compiled.params for its placeholders
```

On failure, `compile` returns every problem found; `render` shows one as `bcql` does.

## Status

The language design is approved: [docs/design.md](docs/design.md) describes it, and
[docs/research.md](docs/research.md) the research behind it. The compiler is built for SQLite and PostgreSQL. Each
example compiles to the SQL kept beside it in [examples/](examples), and the test suite runs its queries on both
databases. DuckDB and ClickHouse come next.

## Roadmap

1. **Language design** (approved 20 October 2026)
2. **Core:** tokens, the parser, error messages and `bcql check` (done)
3. **Compiler:** filtering, sorting, selecting, grouping, joins, named queries and `def`, compiling to SQLite and
   PostgreSQL, tested against both (done)
4. **More:** imports, window functions and links; DuckDB and ClickHouse
5. **Publishing:** the `buttercupql` Python package on PyPI, documentation, editor support and a browser playground

## Building

ButtercupQL is written in Rust (edition 2024). The library and the command have no dependencies. The workspace holds
three crates: [`buttercupql`](crates/buttercupql), the library; [`bcql`](crates/bcql), the command; and
[`db-tests`](crates/db-tests), which runs the SQL on SQLite, and on PostgreSQL when `BCQL_POSTGRES_URL` names one.

```sh
cargo test --workspace
BCQL_POSTGRES_URL=postgresql://localhost/test cargo test -p db-tests
```

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

## Author

Michael Kusi-Appiah ([@mkusiappiah](https://github.com/mkusiappiah))
