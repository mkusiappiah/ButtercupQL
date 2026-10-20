# Named queries and `def`

## Named queries

Brackets make a query a value, as they make any long Python expression span lines. A named query is read with `from`,
like a table:

```python
open_cases = (
    from cases
    where status == "open"
)

from open_cases
group by band:
    n = count()
```

```sql
SELECT band, count(*) AS n
FROM cases
WHERE status = $1
GROUP BY band
-- parameters: ["open"]
```

A named query used once is written in place, where the database can optimise across it, rather than as a separate
step it must work out first.

## Values

A name given a value before the query can be used in it. The name means the value even where a table has a column of
the same name, so a new column can never change what a query does:

```python
threshold = 1_000_000

from transactions
where amount > threshold
```

```sql
SELECT *
FROM transactions
WHERE amount > 1000000
```

## `def`: steps written once

A `def` holds steps. Calling it on a line of its own puts them in place:

```python
def recent(days_back=7):
    """The rows raised in the last `days_back` days."""
    where raised > now() - days(days_back)

from cases
recent(30)
where assignee is None
take 10
```

```sql
SELECT *
FROM cases
WHERE raised > now() - INTERVAL '30 days' AND assignee IS NULL
LIMIT 10
```

Its parameters take defaults, and arguments by name, as in Python: `recent()`, `recent(30)` and
`recent(days_back=30)`.

## Queries with arguments

A `def` whose body starts with `from` is a query. Called in a `from`, it is read in place:

```python
def cases_for(account: str, days_back: int = 30):
    from cases
    where account_id == account
    where raised > now() - days(days_back)

from cases_for("A1", 7)
sort -risk
```

```sql
SELECT *
FROM cases
WHERE account_id = $1 AND raised > now() - INTERVAL '7 days'
ORDER BY risk DESC NULLS LAST
-- parameters: ["A1"]
```

Compiled on its own, its parameters become the SQL's placeholders, given values when the query runs. With `bcql`, that
is `--query cases_for`:

```python
def cases_for(account: str, days_back: int = 30):
    from cases
    where account_id == account
    where raised > now() - days(days_back)
```

```sql query=cases_for
SELECT *
FROM cases
WHERE account_id = $1 AND raised > now() - $2 * INTERVAL '1 day'
-- parameters: [{"arg": "account"}, {"arg": "days_back"}]
-- arguments: account, days_back = 30
```

From Python, `compile(text, query="cases_for")` gives the query, and `bind(account="A1")` the values for its
placeholders, with `days_back`'s default: see [Python](python.md#queries-with-arguments). On ClickHouse, the
placeholders are named and typed:

```python
def cases_for(account: str, days_back: int = 30):
    from cases
    where account_id == account
    where raised > now() - days(days_back)
```

```sql clickhouse query=cases_for
SELECT *
FROM cases
WHERE account_id = {account:String} AND raised > now() - toIntervalDay({days_back:Int64})
-- parameters: [{"arg": "account"}, {"arg": "days_back"}]
-- arguments: account, days_back = 30
```
