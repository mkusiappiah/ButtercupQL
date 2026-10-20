# The databases

A text means the same on every database. This chapter says how ButtercupQL's compiler gives each database that
meaning, which another implementation can do in its own way, and what each database can't do, which every
implementation refuses alike.

## SQLite

- Placeholders are `?`.
- `/` is written `x * 1.0 / y`, so that whole numbers divide exactly. SQLite's builds may have no `floor()` and no
  `power()`, so `//` is written with `CAST`, and a whole power up to 16 is multiplied out. Any other power needs a
  build with SQLite's math functions, as Python's `sqlite3` has.
- SQLite puts missing values first when sorting low to high, so `NULLS LAST` is written there.
- Dates and times are text, `'2026-10-20 14:30:00'`, worked on with `date()`, `datetime()` and `strftime()`; the
  columns of `date` and `datetime` are `TEXT`.
- SQLite has no users, so `grant` and `revoke` are refused, and keeps no queries, so `@stored` is.

## PostgreSQL

- Placeholders are `$1`, or `%s` for psycopg.
- PostgreSQL puts missing values first when sorting high to low, so `DESC NULLS LAST` is written there, and in an
  index's keys.
- A stored query `def` is a SQL function, `RETURNS TABLE` with its columns' types, which the schema says, or
  `RETURNS SETOF` a table for its whole rows. A stored `def` that changes rows is a procedure, written with
  `BEGIN ATOMIC`, which needs PostgreSQL 14 or later. Its parameters are written with their types.

## DuckDB

- Placeholders are `$1`.
- `/` already divides exactly, and missing values already sort last.
- A condition on a window value is written in `QUALIFY`.
- A stored query `def` is a table macro. DuckDB keeps no procedures, so a stored `def` that changes rows is refused.
- DuckDB has no users, so `grant` and `revoke` are refused. Its indexes take no condition and no direction, so
  `where=` on an index is refused, and its keys are written low to high.
- DuckDB refuses some changes to tables that other tables' keys point to, and says so as it runs them: renaming one
  of their columns, making one never missing, and giving back the rows an `update` changed.

## ClickHouse

- Placeholders are named and typed, `{p1:String}`, and their values are passed by name.
- A total's condition is written in ClickHouse's own functions, `countIf()` and `sumIf()`, and a total of no rows that
  is missing in Python, `max()`, with `maxIfOrNull()`.
- A query with a `left join` is given `SETTINGS join_use_nulls = 1`, so that a row with no match has missing values,
  not empty ones.
- ClickHouse reads a name as a column the query gives wherever the name is written alone, so where a `select` gives
  a column the name of one it reads, every column of the query is named with its table.
- An `update` is a mutation, `ALTER TABLE ... UPDATE`, with `mutations_sync = 1` so that it is done before the
  statement returns. A `delete` with no `where` is written `WHERE true`.
- A table is a `MergeTree`, its rows kept in the order of its primary key, and a column that can be missing is
  `Nullable`. ClickHouse keeps no unique keys and no references, so they are left out, with a warning.
- A stored query `def` is a view with parameters, which take no defaults: they are left out, with a warning.
- Permissions are granted one table at a time, and `write` is `INSERT`, `ALTER UPDATE` and `ALTER DELETE`.
- ClickHouse runs no transactions, has no indexes like `create index`, can't give back the rows a change makes, and
  keeps every row of a recursive query, so `with transaction:`, `create index`, a `select` after a change, and `|` in
  a recursive query are refused.
