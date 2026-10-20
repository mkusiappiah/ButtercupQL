# Steps

A query is a list of steps, one per line. The first is `from`; the rest can come in any order, as many times as needed.
Each step takes rows and gives rows.

| Step | Does | Becomes |
|---|---|---|
| `from cases` | Start from a table, a view or a named query | `FROM` |
| `where condition` | Keep the rows where the condition is true, before or after grouping | `WHERE`, `HAVING`, or a subquery |
| `name = expression` | Add a column, or replace one | an expression in `SELECT` |
| `select a, b, total = x + y` | Keep these columns, in this order, naming any computed ones | `SELECT` |
| `del a, b` | Drop columns | `SELECT` of the others |
| `sort -risk, raised` | Order the rows; `-` sorts that column high to low | `ORDER BY` |
| `take 20` | Keep the first 20 rows | `LIMIT` |
| `skip 40` | Leave out the first 40 rows | `OFFSET` |
| `distinct` | Drop repeated rows | `SELECT DISTINCT` |
| `group by a, b:` and an indented block | One row per group, with the block's totals | `GROUP BY` |
| `summarize:` and an indented block | One row of totals for all the rows | an aggregate `SELECT` |
| `join accounts on account_id == accounts.id` | Add another table's columns | `JOIN` |
| `left join accounts on ...` | The same, keeping rows that have no match | `LEFT JOIN` |

## `from` and `where`

`from` names the table. Each `where` keeps the rows where its condition is true; several are joined with `AND`:

```python
from cases
where status == "open"
where risk >= 90
```

```sql
SELECT *
FROM cases
WHERE status = $1 AND risk >= 90
-- parameters: ["open"]
```

## Adding columns

A step that gives a name a value adds a column, or replaces one of the same name. Later steps can use it:

```python
from transactions
fee = amount * 0.01
net = amount - fee
where net > 1_000
```

```sql
SELECT *, amount * 0.01 AS fee, amount - amount * 0.01 AS net
FROM transactions
WHERE amount - amount * 0.01 > 1000
```

## `select` and `del`

`select` keeps the columns named, in that order. A column can be computed and named in place:

```python
from cases
select key, risk, urgent = risk >= 90
```

```sql
SELECT key, risk, risk >= 90 AS urgent
FROM cases
```

Long lists go in brackets, over as many lines as needed:

```python
from cases
select (
    key,
    status,
    risk,
)
```

```sql
SELECT key, status, risk
FROM cases
```

`del` drops columns instead. With a schema, ButtercupQL knows the other columns and writes them:

```python
from cases
del notes, assignee
```

```sql schema
SELECT id, key, account_id, status, band, risk, raised, account, operator
FROM cases
```

## `sort`

`sort` orders the rows by one column or more. A `-` sorts that column from high to low:

```python
from cases
sort -risk, raised
```

```sql
SELECT *
FROM cases
ORDER BY risk DESC NULLS LAST, raised
```

Missing values come last whichever way a column is sorted, on every database: SQL's own default puts them first on
PostgreSQL when sorting high to low. With a schema that says a column is never missing, `NULLS LAST` is left out.

`reverse=True` turns the whole order around, as Python's `sorted(..., reverse=True)` does:

```python
from cases
sort raised, reverse=True
```

```sql
SELECT *
FROM cases
ORDER BY raised DESC NULLS LAST
```

## `take` and `skip`

`take` keeps the first rows, and `skip` leaves out the first rows:

```python
from cases
sort -risk
skip 40
take 20
```

```sql
SELECT *
FROM cases
ORDER BY risk DESC NULLS LAST
LIMIT 20 OFFSET 40
```

Steps run in the order they are written, so a `where` after a `take` keeps rows from those taken, and ButtercupQL
writes the subquery that means:

```python
from cases
sort -risk
take 100
where status == "open"
```

```sql
SELECT *
FROM (
  SELECT *
  FROM cases
  ORDER BY risk DESC NULLS LAST
  LIMIT 100
) AS _q1
WHERE status = $1
ORDER BY risk DESC NULLS LAST
-- parameters: ["open"]
```

## `distinct`

```python
from cases
select operator, band
distinct
```

```sql
SELECT DISTINCT operator, band
FROM cases
```

## Several steps on a line

`;` puts several steps on one line, and a `\` at the end of a line joins the next one to it:

```python
from cases; where risk > 90; take 5
```

```sql
SELECT *
FROM cases
WHERE risk > 90
LIMIT 5
```

## Names that are not plain words

A name with spaces, or a name that is one of Python's keywords, goes in backticks. The step words (`where`, `sort` and
the others) are ordinary names everywhere except at the start of a line:

```python
from `order lines`
where `class` == "A" and sort > 3
```

```sql
SELECT *
FROM "order lines"
WHERE class = $1 AND sort > 3
-- parameters: ["A"]
```
