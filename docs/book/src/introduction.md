# ButtercupQL

<img class="buttercup-logo" src="buttercup.svg" alt="">

**ButtercupQL is a Python-like language for SQL**, named after the buttercup flower. A query is written one step per
line, in the order the data flows, with Python's own expressions. ButtercupQL compiles it to the SQL your database
already runs: PostgreSQL, SQLite, DuckDB and ClickHouse.

```python
# Critical cases nobody has taken, riskiest first
from cases
where status == "open" and band in ["critical", "high"]
where raised > now() - days(7)
sort -risk
take 20
select key, account, risk
```

becomes, for PostgreSQL,

```sql
SELECT key, account, risk
FROM cases
WHERE status = $1 AND band IN ($2, $3) AND raised > now() - INTERVAL '7 days'
ORDER BY risk DESC NULLS LAST
LIMIT 20
-- parameters: ["open", "critical", "high"]
```

The values are passed as parameters, never pasted into the SQL, so a query can't be turned into another by what it is
given. Cases with no risk come last, as they do on every database.

## Why

- **It reads like Python.** Indentation, `#` comments, `==`, `and`, `in`, `None`, f-strings, `def` and `import`
  mean what they mean in Python, and give the rows Python's meaning gives: `7 / 2` is `3.5` and `sort -risk` puts
  missing values last, on every database.
- **It is faster to write.** The table comes first, so an editor can complete column names from the first word.
  Grouping columns are written once, one `where` works before and after grouping, and logic in several steps needs no
  subqueries.
- **It runs as fast as SQL written by hand.** The compiler writes one flat `SELECT` where it can, uses each
  database's own forms, and passes values as parameters so plans can be reused.
- **It is safer and clearer.** `x == None` means `IS NULL`. Names are checked against the schema, and every mistake is
  shown at its place, with what to write instead.

## In this book

- [Getting started](getting-started.md) installs the `bcql` command and compiles a first query.
- [Steps](steps.md) to [Mistakes and their messages](errors.md) teach the language, each part with the SQL it
  becomes.
- [The databases](databases.md) to [The playground](playground.md) are about running it: from the command line, from
  Python and Rust, in an editor and in a browser.
- [Functions and methods](functions.md) lists everything built in.

Every example in this book is checked by ButtercupQL's tests: each query compiles to exactly the SQL shown beside it.
