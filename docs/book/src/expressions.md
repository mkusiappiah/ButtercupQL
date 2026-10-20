# Expressions

Expressions are Python's, and they mean what they mean in Python, on every database. Where SQL alone would give
something else, ButtercupQL writes what makes it the same.

| Python | Example | SQL |
|---|---|---|
| Comparison, chained too | `risk >= 90`, `1 <= risk <= 100` | `risk >= 90`, `risk BETWEEN 1 AND 100` |
| `and`, `or`, `not` | `band == "high" and not closed` | `AND`, `OR`, `NOT` |
| Membership | `band in ["critical", "high"]` | `IN (...)` |
| Missing values | `assignee is None`, `assignee == None` | `assignee IS NULL` |
| Arithmetic | `+ - * / // % **` | the database's operators, made to mean what Python means |
| Conditional | `"urgent" if risk >= 90 else "normal"` | `CASE WHEN ... THEN ... ELSE ... END` |
| Text | `"Acme" in operator`, `name.lower()`, `len(name)` | `LIKE`, `lower()`, `length()` |
| f-strings | `f"{first} {last}"` | concatenation |
| Values | `1_000_000`, `3.5`, `True`, `False`, `None` | numbers, booleans, `NULL` |
| Dates and times | `now()`, `today()`, `date("2026-10-20")`, `days(7)` | the database's own date arithmetic |

## Comparisons

`==` compares; a single `=` gives a column a value. Comparisons chain as in Python:

```python
from cases
where 1 <= risk <= 100 and status != "closed"
```

```sql
SELECT *
FROM cases
WHERE risk BETWEEN 1 AND 100 AND status <> $1
-- parameters: ["closed"]
```

## Membership

`in` and `not in` take a list. `not in` with `None` in the list still means what it says, where SQL's `NOT IN` would
quietly give no rows:

```python
from cases
where band in ["critical", "high"]
where operator not in ["Acme", None]
```

```sql
SELECT *
FROM cases
WHERE band IN ($1, $2) AND operator NOT IN ($3) AND operator IS NOT NULL
-- parameters: ["critical", "high", "Acme"]
```

A query in brackets is a list too:

```python
from cases
where account_id in (from accounts; where operator == "Acme"; select id)
```

```sql
SELECT *
FROM cases
WHERE account_id IN (
  SELECT id
  FROM accounts
  WHERE operator = $1
)
-- parameters: ["Acme"]
```

## Arithmetic

`/` divides exactly, `//` rounds down and `%` takes the divisor's sign, as in Python. SQL's own operators give `3`
for `7 / 2` on PostgreSQL and SQLite, and round towards zero:

```python
from transactions
share = amount / 3
blocks = amount // 1_000
rest = amount % 1_000
```

```sql
SELECT *, amount * 1.0 / 3 AS share, floor(amount * 1.0 / 1000) AS blocks, (amount % 1000 + 1000) % 1000 AS rest
FROM transactions
```

## Conditionals

```python
from cases
level = "urgent" if risk >= 90 else "normal"
```

```sql
SELECT *, CASE WHEN risk >= 90 THEN $1 ELSE $2 END AS level
FROM cases
-- parameters: ["urgent", "normal"]
```

## Text

Text has Python's methods, and `in` finds one text in another. `startswith()` becomes a `LIKE` pattern that an index
can serve, passed as a parameter, with any `%` or `_` in the text matched as it is:

```python
from cases
where notes.startswith("50%") and "fraud" in notes.lower()
title = f"{key}: {status.upper()}"
```

```sql
SELECT *, CAST(key AS TEXT) || $1 || upper(status) AS title
FROM cases
WHERE notes LIKE $2 ESCAPE '\' AND strpos(lower(notes), $3) > 0
-- parameters: [": ", "50\\%%", "fraud"]
```

| Method | Gives |
|---|---|
| `text.lower()`, `text.upper()` | the text in small letters, or in capitals |
| `text.strip()`, `lstrip()`, `rstrip()` | the text without spaces at either end, its start or its end |
| `text.startswith(prefix)`, `text.endswith(suffix)` | whether the text starts or ends so, matching capitals exactly |
| `text.replace(old, new)` | the text with every `old` replaced by `new` |
| `len(text)` | the number of characters |

## Dates and times

`now()` and `today()` are the moment and the day the query runs. `days()`, `hours()`, `minutes()`, `seconds()` and
`weeks()` are lengths of time to add or take away, and `.year`, `.month`, `.day`, `.hour`, `.minute` and `.second`
read a part of a date:

```python
from cases
where raised > now() - days(7)
where raised.year == 2026
day = raised.date()
```

```sql
SELECT *, CAST(raised AS DATE) AS day
FROM cases
WHERE raised > now() - INTERVAL '7 days' AND CAST(EXTRACT(YEAR FROM raised) AS INTEGER) = 2026
```

On SQLite, which keeps dates as text, the same query moves dates with SQLite's own functions:

```python
from cases
where raised > now() - days(7)
```

```sql sqlite
SELECT *
FROM cases
WHERE raised > datetime('now', '-7 days')
```

`date("2026-10-20")` and `datetime("2026-10-20 14:30")` write a date or a moment into a query. A date that does not
exist is a mistake ButtercupQL finds, rather than the database.

## Values in the SQL

Text becomes a parameter; numbers, `True`, `False` and `None` are written into the SQL. `literal()` writes text into
the SQL too, for a partial index that needs to see the value:

```python
from cases
where status == literal("open") and band == "high"
```

```sql
SELECT *
FROM cases
WHERE status = 'open' AND band = $1
-- parameters: ["high"]
```
