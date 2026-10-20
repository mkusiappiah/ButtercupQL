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
ORDER BY risk DESC
LIMIT 20
```

with the values `"open"`, `"critical"` and `"high"` passed as parameters, never pasted into the SQL.

## Why

- **Faster to write.** The table comes first, so an editor can complete column names from the first word. Grouping
  columns are written once, one `where` works before and after grouping, and multi-step logic needs no subqueries.
- **As fast to run.** The compiler writes one flat `SELECT` where it can, uses each database's own forms, and passes
  values as parameters so plans can be reused.
- **Safer and clearer.** `x == None` means `IS NULL`, as it reads. Names are checked against the schema, and every error
  points at its place with a suggestion.
- **Reusable.** Named queries, `def` and imports, as in Python.

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

## Try it

With [Rust](https://rustup.rs) installed:

```sh
cargo install --git https://github.com/mkusiappiah/ButtercupQL bcql
bcql check query.bcql
```

or, in a copy of this repository, `cargo run -p bcql -- check examples/*.bcql`.

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

## Status

The language design is approved: [docs/design.md](docs/design.md) describes it, and
[docs/research.md](docs/research.md) the research behind it. The parser is built: it reads every form in the design,
and `bcql check` shows the mistakes it finds. The compiler, which writes the SQL, is built next.

## Roadmap

1. **Language design** (approved 20 October 2026)
2. **Core:** tokens, the parser, error messages and `bcql check` (done)
3. **Compiler:** filtering, sorting, selecting, grouping and joins, compiling to SQLite and PostgreSQL, tested against
   real databases
4. **More:** `def`, imports, window functions and links; DuckDB and ClickHouse
5. **Publishing:** the `buttercupql` Python package on PyPI, documentation, editor support and a browser playground

## Building

ButtercupQL is written in Rust (edition 2024), with no dependencies. The workspace holds two crates:
[`buttercupql`](crates/buttercupql), the library, and [`bcql`](crates/bcql), the command.

```sh
cargo test --workspace
```

## Licence

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT licence ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

## Author

Michael Kusi-Appiah ([@mkusiappiah](https://github.com/mkusiappiah))
