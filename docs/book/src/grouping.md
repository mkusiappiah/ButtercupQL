# Grouping

`group by` gives one row for each group, and its indented block says what to work out for each: the group's totals.

```python
from transactions
where time > now() - days(30)
group by operator:
    total = count()
    money = sum(amount)
    big = count(where=amount > 1_000_000)
where money > 1_000_000          # after grouping: the same `where` becomes HAVING
sort -money
```

```sql
SELECT operator, count(*) AS total, coalesce(sum(amount), 0) AS money, count(*) FILTER (WHERE amount > 1000000) AS big
FROM transactions
WHERE time > now() - INTERVAL '30 days'
GROUP BY operator
HAVING coalesce(sum(amount), 0) > 1000000
ORDER BY money DESC
```

- The `by` columns and the block's names are the rows' columns. Nothing is written twice, as SQL's `SELECT` and
  `GROUP BY` need.
- A `where` after the block keeps groups by their totals: SQL's `HAVING`, with the same word.
- `group by operator` with no block keeps one row for each group, with no totals.
- A `by` column can be worked out and named in place: `group by month = raised.month:`.

## Totals

| Total | Gives |
|---|---|
| `count()` | the number of rows |
| `count(x)` | the number of rows where `x` is not missing |
| `count_distinct(x)` | the number of different values of `x` |
| `sum(x)` | the sum; `0` for no rows |
| `mean(x)`, also `avg(x)` | the mean; missing for no rows |
| `min(x)`, `max(x)` | the smallest and the largest; missing for no rows |
| `any(condition)`, `all(condition)` | whether the condition is true on any row, or on every row |

Each total can take `where=`, a condition of its own. It becomes `FILTER (WHERE ...)` on PostgreSQL, SQLite and
DuckDB, and `countIf()` or `sumIf()` on ClickHouse:

```python
from transactions
group by operator:
    big = count(where=amount > 1_000_000)
    big_money = sum(amount, where=amount > 1_000_000)
```

```sql clickhouse
SELECT operator, countIf(amount > 1000000) AS big, coalesce(sumIf(amount, amount > 1000000), 0) AS big_money
FROM transactions
GROUP BY operator
```

## `summarize`

`summarize` works out totals for all the rows at once, in one row:

```python
from cases
where status == "open"
summarize:
    open = count()
    riskiest = max(risk)
    unassigned = count(where=assignee is None)
```

```sql
SELECT count(*) AS open, max(risk) AS riskiest, count(*) FILTER (WHERE assignee IS NULL) AS unassigned
FROM cases
WHERE status = $1
-- parameters: ["open"]
```

## Grouping twice

A grouped query can be grouped again, with no subquery to write:

```python
# How many operators had more than 100 cases?
from cases
group by operator:
    n = count()
where n > 100
summarize:
    operators = count()
```

```sql
SELECT count(*) AS operators
FROM (
  SELECT operator, count(*) AS n
  FROM cases
  GROUP BY operator
  HAVING count(*) > 100
) AS _q1
```

## Pivots

`pivot` makes a column of each total for each value of a column: one row for each group of `by`, with the open
cases of each band side by side.

```python
from cases
where status == "open"
pivot band in ["critical", "high", "low"] by operator:
    open = count()
sort operator
```

```sql
SELECT operator, count(*) FILTER (WHERE band = $1) AS critical, count(*) FILTER (WHERE band = $2) AS high, count(*) FILTER (WHERE band = $3) AS low
FROM cases
WHERE status = $4
GROUP BY operator
ORDER BY operator
-- parameters: ["critical", "high", "low", "open"]
```

The values are written out, so the columns are known before the query runs. With one total, each column is named by
its value; with several, by the value and the total's name: `critical_open`, `critical_riskiest`. With no `by`, the
totals are of every row, in one row.

```python
from cases
pivot band in ["critical", "high"]:
    open = count(where=status == "open")
    riskiest = max(risk)
```

```sql clickhouse
SELECT countIf(status = {p1:String} AND band = {p2:String}) AS critical_open, maxIfOrNull(risk, band = {p3:String}) AS critical_riskiest, countIf(status = {p4:String} AND band = {p5:String}) AS high_open, maxIfOrNull(risk, band = {p6:String}) AS high_riskiest
FROM cases
-- parameters: ["open", "critical", "critical", "open", "high", "high"]
```
