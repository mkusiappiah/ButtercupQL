# Window functions

A total given `over=`, `sort=` or `rows=` keeps every row, with the total of the rows around it beside each one. The
window functions work the same way.

```python
from transactions
share = amount / sum(amount, over=operator)        # each row's share of its operator's total
so_far = sum(amount, sort=time)                    # a running total
place = rank(over=operator, sort=-amount)          # 1 for each operator's largest
before = lag(amount, sort=time)                    # the value on the row before
```

```sql
SELECT *, amount * 1.0 / coalesce(sum(amount) OVER (PARTITION BY operator), 0) AS share, coalesce(sum(amount) OVER (ORDER BY time ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW), 0) AS so_far, rank() OVER (PARTITION BY operator ORDER BY amount DESC NULLS LAST) AS place, lag(amount, 1) OVER (ORDER BY time) AS before
FROM transactions
```

| Argument | Means | SQL |
|---|---|---|
| `over=operator`, `over=[a, b]` | Work over the rows of each operator; `over=()` is all the rows | `PARTITION BY` |
| `sort=-risk`, `sort=[a, -b]` | In this order, `-` for high to low | `ORDER BY` in the window |
| `rows=7` | Only this row and the 6 before it | `ROWS BETWEEN 6 PRECEDING AND CURRENT ROW` |

- The window functions are `row_number()`, `rank()`, `dense_rank()`, `lag(x, n)` and `lead(x, n)`, which take
  `default=`, and `first(x)` and `last(x)`. Every one but `row_number()` needs `sort=`.
- A sorted total runs from the first row to the current one, as pandas' `cumsum` does. SQL's own default also takes in
  the rows that tie with the current one.
- A window value counts the rows the steps before kept: after `take 5`, `row_number()` numbers those 5.

## Moving totals

`rows=` keeps the window to the last rows: here the mean of each day and the six before it.

```python
from transactions
week = mean(amount, sort=time, rows=7)
```

```sql
SELECT *, avg(amount) OVER (ORDER BY time ROWS BETWEEN 6 PRECEDING AND CURRENT ROW) AS week
FROM transactions
```

## Keeping rows by a window value

A `where` after a window value keeps rows by it. On DuckDB it is a `QUALIFY` in the same query:

```python
# The three riskiest open cases of each operator
from cases
where status == "open"
place = rank(over=operator, sort=-risk)
where place <= 3
```

```sql duckdb
SELECT *, rank() OVER (PARTITION BY operator ORDER BY risk DESC) AS place
FROM cases
WHERE status = $1
QUALIFY rank() OVER (PARTITION BY operator ORDER BY risk DESC) <= 3
-- parameters: ["open"]
```

and on the others, a query around it:

```python
# The three riskiest open cases of each operator
from cases
where status == "open"
place = rank(over=operator, sort=-risk)
where place <= 3
```

```sql
SELECT *
FROM (
  SELECT *, rank() OVER (PARTITION BY operator ORDER BY risk DESC NULLS LAST) AS place
  FROM cases
  WHERE status = $1
) AS _q1
WHERE place <= 3
-- parameters: ["open"]
```
