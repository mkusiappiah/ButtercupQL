# The databases

The same query gives the same rows on PostgreSQL, SQLite, DuckDB and ClickHouse, and they are the rows Python's meaning
gives. Each database gets the SQL written in its own forms. ButtercupQL's tests run the SQL on all four and compare
the rows.

```python
from transactions
where time > now() - days(30)
group by operator:
    payments = count()
    big = count(where=amount > 1_000_000)
    average = mean(amount)
sort -payments
```

## PostgreSQL

```sql
SELECT operator, count(*) AS payments, count(*) FILTER (WHERE amount > 1000000) AS big, avg(amount) AS average
FROM transactions
WHERE time > now() - INTERVAL '30 days'
GROUP BY operator
ORDER BY payments DESC
```

- Placeholders are `$1`, `$2`. Python's psycopg takes `%s`, which `--params format` writes.
- Missing values are put last with `NULLS LAST`, which an index made the same way can serve:
  `CREATE INDEX ON cases (risk DESC NULLS LAST)`.
- Text is sorted in the database's own order, its collation: by the locale of the database, unless it or the column
  uses the `C` collation. ButtercupQL does not write `COLLATE`, which would keep PostgreSQL from using its indexes for
  the sort.

## SQLite

```python
from transactions
where time > now() - days(30)
group by operator:
    payments = count()
    big = count(where=amount > 1_000_000)
    average = mean(amount)
sort -payments
```

```sql sqlite
SELECT operator, count(*) AS payments, count(*) FILTER (WHERE amount > 1000000) AS big, avg(amount) AS average
FROM transactions
WHERE time > datetime('now', '-30 days')
GROUP BY operator
ORDER BY payments DESC
```

- Placeholders are `?`.
- SQLite keeps dates and times as text. ButtercupQL compares them in the form SQLite's own date functions write,
  `2026-10-20 14:30:00`, and moves them with those functions: `datetime('now', '-30 days')`.
- SQLite builds often leave out the maths functions, so ButtercupQL does without them: `//` and `%` are worked out from
  casts, and `x ** 2` to `x ** 4` are multiplications. Other powers need SQLite's `power()`.
- `startswith()` becomes `GLOB`, which matches capitals exactly as Python does; SQLite's `LIKE` ignores case.

## DuckDB

```python
from transactions
where time > now() - days(30)
group by operator:
    payments = count()
    big = count(where=amount > 1_000_000)
    average = mean(amount)
sort -payments
```

```sql duckdb
SELECT operator, count(*) AS payments, count(*) FILTER (WHERE amount > 1000000) AS big, avg(amount) AS average
FROM transactions
WHERE time > now() - INTERVAL '30 days'
GROUP BY operator
ORDER BY payments DESC
```

- Placeholders are `$1`, `$2`.
- DuckDB's `/` already divides exactly and it puts missing values last by itself, so its SQL is the plainest.
- A `where` on a window value is a `QUALIFY`, in the same query.

## ClickHouse

```python
from transactions
where time > now() - days(30)
group by operator:
    payments = count()
    big = count(where=amount > 1_000_000)
    average = mean(amount)
sort -payments
```

```sql clickhouse
SELECT operator, count(*) AS payments, countIf(amount > 1000000) AS big, avgOrNull(amount) AS average
FROM transactions
WHERE time > now() - INTERVAL 30 DAY
GROUP BY operator
ORDER BY payments DESC
```

- Placeholders are named and typed: `{p1:String}`, and a query argument's own name, `{account:String}`.
- Totals with a condition are `countIf()` and `sumIf()`, and `avgOrNull()`, `minOrNull()` and `maxOrNull()` make the
  total of no rows missing, as on the other databases.
- A query with a `LEFT JOIN` gets `SETTINGS join_use_nulls = 1`, so that a row with no match has missing values rather
  than empty ones.
- Text functions are ClickHouse's own: `startsWith()`, `lowerUTF8()`, `lengthUTF8()`.

## Placeholders

`--params` writes another style of placeholder, for drivers that take it:

| Style | Placeholder | For |
|---|---|---|
| `dollar` | `$1` | PostgreSQL and DuckDB |
| `question` | `?` | SQLite, and drivers that number them |
| `format` | `%s` | Python's psycopg |
| `colon` | `:p1` | drivers that take names |
| `braces` | `{p1:String}` | ClickHouse |

With the `colon` and `braces` styles, each value is passed by name, and a query's argument used twice is passed
once. With `dollar`, an argument used twice is one `$n`.
