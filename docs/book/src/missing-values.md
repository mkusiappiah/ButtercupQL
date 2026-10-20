# Missing values

In SQL, `x = NULL` is never true: a classic and silent bug. In ButtercupQL, `x == None` and `x is None` both mean what
they say:

```python
from cases
where assignee is None
where closed != None
```

```sql
SELECT *
FROM cases
WHERE assignee IS NULL AND closed IS NOT NULL
```

## Comparisons

A comparison with a missing value is never true, as in SQL: `where risk != 90` leaves out the rows with no risk. To keep
them, say so:

```python
from cases
where risk != 90 or risk is None
```

```sql
SELECT *
FROM cases
WHERE risk <> 90 OR risk IS NULL
```

## Sorting

Missing values sort last, whichever way the sort goes, on every database. PostgreSQL would put them first when sorting
high to low, and SQLite and DuckDB first when sorting low to high, so ButtercupQL says where they go:

```python
from cases
sort -risk
```

```sql
SELECT *
FROM cases
ORDER BY risk DESC NULLS LAST
```

```python
from cases
sort risk
```

```sql sqlite
SELECT *
FROM cases
ORDER BY risk NULLS LAST
```

## Totals of no rows

`sum()` of no rows is `0`, `any()` is `False` and `all()` is `True`, as in Python. `mean()`, `min()` and `max()` of no
rows are missing.

```python
from transactions
where amount > 1_000_000_000
summarize:
    money = sum(amount)
    largest = max(amount)
```

```sql
SELECT coalesce(sum(amount), 0) AS money, max(amount) AS largest
FROM transactions
WHERE amount > 1000000000
```

## Truth

A `where` takes a value's truth as Python does: `0`, `""` and `None` are false. With a schema that says `notes` is
text, ButtercupQL writes what that means:

```python
from cases
where notes
```

```sql schema
SELECT *
FROM cases
WHERE notes <> $1
-- parameters: [""]
```

A missing value is false, so `not notes` holds where `notes` is missing, as `not None` does in Python. SQL would
take the missing value as neither true nor false, and keep no row, so ButtercupQL says so:

```python
from cases
where not notes
```

```sql schema
SELECT *
FROM cases
WHERE NOT ((notes <> $1) IS TRUE)
-- parameters: [""]
```

A comparison is not a value's truth: with a missing side, it is neither true nor false, as in SQL, and so is its
`not`. `where not (risk > 90)` keeps no row with no risk.
