# Combining queries

Two queries combine with Python's operators, as lists and sets do:

| Written | Gives | SQL |
|---|---|---|
| `a + b` | The rows of both, as `+` joins two lists. | `UNION ALL` |
| `a \| b` | The rows of either, each once, as `\|` joins two sets. | `UNION` |
| `a & b` | The rows of both that are in each. | `INTERSECT` |
| `a - b` | The rows of `a` that are not in `b`. | `EXCEPT` |

```python
watched = (
    from cases
    where band == "critical"
    select key
) | (
    from cases
    where assignee is None
    select key
)

from watched
sort key
```

```sql
SELECT *
FROM (
  SELECT key
  FROM cases
  WHERE band = $1
  UNION
  SELECT key
  FROM cases
  WHERE assignee IS NULL
) AS _q1
ORDER BY key
-- parameters: ["critical"]
```

The second query's columns are matched to the first's by name, so their order does not matter. Two queries with
different columns are a mistake:

```python mistake
both = (from cases; select key) + (from accounts; select id)
from both
```

```text
error: the queries have different columns
  ┌─ query.bcql:1:35
  │
1 │ both = (from cases; select key) + (from accounts; select id)
  │                                   ^^^^^^^^^^^^^^^^^^^^^^^^^^ the first gives `key`, and this one `id`
  = help: give both the same columns with `select`; they are matched by name

checked 1 file: 1 error
```

## Recursive queries

A named query that reads itself is recursive. Its first part gives the first rows, and its second part reads the
rows found so far to give the next, until it gives no more. These are the accounts money reaches from `A1`, through
the transfers between accounts, and how many transfers away each is:

```python
reached = (
    from [{"id": "A1", "steps": 0}]
) + (
    from transfers
    join reached on source == reached.id
    where reached.steps < 5
    select id = target, steps = reached.steps + 1
)

from reached
```

```sql
WITH RECURSIVE reached AS (
  SELECT *
  FROM (
    SELECT $1 AS id, 0 AS steps
  ) AS _q1
  UNION ALL
  SELECT transfers.target AS id, reached.steps + 1 AS steps
  FROM transfers
  JOIN reached ON transfers.source = reached.id
  WHERE reached.steps < 5
)
SELECT *
FROM reached
-- parameters: ["A1"]
```

With `+`, every row found is kept: a loop in the transfers is followed until the condition on `steps` ends it. With
`|`, a row the query has found already is not added again, so where the rows hold only the accounts,
`select id = target`, a loop ends by itself. ClickHouse keeps every row, and takes `+` alone.

The first part names the columns. A recursive query can make rows of its own, with no table to read: a row for each
of the last seven days.

```python
week = (
    from [{"n": 0}]
) + (
    from week
    where n < 6
    select n = n + 1
)

from week
select n, day = today() - days(n)
```

```sql
WITH RECURSIVE week AS (
  SELECT *
  FROM (
    SELECT 0 AS n
  ) AS _q1
  UNION ALL
  SELECT n + 1 AS n
  FROM week
  WHERE n < 6
)
SELECT n, CURRENT_DATE - n AS day
FROM week
```
