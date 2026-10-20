# Getting started

## Installing `bcql`

`bcql` is the ButtercupQL command. It compiles queries, checks them, and runs the language server editors use. Any of
these installs it:

- **A release.** Each [release](https://github.com/mkusiappiah/ButtercupQL/releases) has `bcql` built for Linux,
  macOS and Windows: unpack it and put it on your `PATH`.
- **With Python**, which also gives you the `buttercupql` package: `pip install buttercupql`.
- **With Rust:** `cargo install --git https://github.com/mkusiappiah/ButtercupQL bcql`.

`bcql --version` says which version you have. You can also try ButtercupQL without installing anything, in
[the playground](playground.md).

## A first query

ButtercupQL files end in `.bcql`. Write this in `open.bcql`:

```python
# Open cases, riskiest first
from cases
where status == "open"
sort -risk
take 10
```

and compile it:

```text
$ bcql compile open.bcql
SELECT *
FROM cases
WHERE status = $1
ORDER BY risk DESC NULLS LAST
LIMIT 10
-- parameters: ["open"]
```

The SQL comes first, then the values for its placeholders. `--dialect` chooses the database: `postgres`, which is
the default, `sqlite`, `duckdb` or `clickhouse`. Each gets its own placeholders:

```python
from cases
where status == "open"
sort -risk
take 10
```

```sql sqlite
SELECT *
FROM cases
WHERE status = ?
ORDER BY risk DESC
LIMIT 10
-- parameters: ["open"]
```

## Checking names

Give `bcql` the database's own `CREATE TABLE` statements with `--schema`, and every name is checked against them. Had
line 3 said `stauts`:

```text
$ bcql check --schema schema.sql open.bcql
error: there is no column `stauts` in `cases`
  ┌─ open.bcql:3:7
  │
3 │ where stauts == "open"
  │       ^^^^^^ did you mean `status`?

checked 1 file: 1 error
```

With a schema, the SQL can also use what the tables say: a column that is never missing needs no `NULLS LAST`, and a
column that points to another table can be [followed with a dot](joins.md#links).

## Next

- [Steps](steps.md) shows each step a query is made of.
- [Python](python.md) shows how to compile and run queries from a program.
- [Editors](editors.md) sets up colours, completion and problems as you type.
