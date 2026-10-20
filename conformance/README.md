# The ButtercupQL conformance suite

The cases here say what ButtercupQL texts mean: the rows each gives, and the texts that are mistakes. They are
written for any implementation of the language, and they hold on every database: ButtercupQL's own tests run each
case on SQLite, PostgreSQL, DuckDB and ClickHouse, and the rows must be the same on all four.
[The specification](../docs/book/src/spec/index.md) is the language they test.

## The tables

Every case starts from the tables and rows of [tables.bcql](tables.bcql), itself a ButtercupQL script: four tables,
`accounts`, `cases`, `payments` and `transfers`, with a few rows each, missing values among them. A case runs on a
database of its own, so a case that changes rows changes them for itself alone. Each case's names are checked against
these tables: they are its schema.

## The cases

Each file holds the cases of a part of the language. A case is its name after `===`, its text on the lines below,
and then what it gives, in sections that start with `---`:

```text
=== a where keeps the rows its condition is true for
from cases
where risk > 50
sort key
select key, risk
--- rows
["C1", 95]
["C2", 90]
["C6", 70]
```

| Section | What it says |
|---|---|
| `--- rows` | The rows of the text's last statement, one JSON list on each line, in order. A text that changes rows ends with a query that reads them. |
| `--- rows in any order` | The same rows, in any order, for a text that does not sort them. |
| `--- error` | The text is a mistake: it does not compile, on any database. Each line is the place of an error, `line:column`, counted from 1 in characters, and its message. |
| `--- warning` | The text compiles, with these warnings, written as errors are. |
| `--- refused on clickhouse` | The databases named, with `, ` between them, refuse the text, with the message on the next line. The others run it. |

A case with no `--- rows` is checked only for compiling, and for its warnings: a statement that changes the
database's users, for one, gives no rows.

Values are written as JSON writes them. A number is the same as another of the same value, whole or not, within a
billionth of it; `true` and `false` are `1` and `0`, as SQLite gives them; a date is text, `"2026-10-01"`. `null` is
a missing value.

The messages are those of ButtercupQL's own compiler. Another implementation can hold to the places alone, and to
which texts are refused.

## Running it

```sh
cargo test -p db-tests --test databases the_conformance_suite
```

runs every case on SQLite, and on PostgreSQL, DuckDB and ClickHouse when `BCQL_POSTGRES_URL`, `BCQL_DUCKDB_PYTHON`
and `BCQL_CLICKHOUSE_URL` say where they are, as [crates/db-tests](../crates/db-tests) describes.
