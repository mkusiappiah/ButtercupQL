# Changing data

A query that changes rows ends with the change. The steps before it choose the rows, as they choose the rows a query
gives.

## `update`

`update` gives columns new values: with `=`, or with `+=`, `-=` and the others, as in Python.

```python
from cases
where status == "open" and raised < now() - days(90)
update status = "stale", risk += 1
```

```sql
UPDATE cases SET status = $1, risk = risk + 1
WHERE status = $2 AND raised < now() - INTERVAL '90 days'
-- parameters: ["stale", "open"]
```

The new values keep Python's meaning, as every value does: `n / 2` divides exactly, and `n // 2` rounds down. On
ClickHouse, an update is a mutation, which `mutations_sync = 1` makes finish before the statement returns:

```python
from cases
where status == "open" and raised < now() - days(90)
update status = "stale", risk += 1
```

```sql clickhouse
ALTER TABLE cases UPDATE status = {p1:String}, risk = risk + 1
WHERE status = {p2:String} AND raised < now() - INTERVAL 90 DAY
SETTINGS mutations_sync = 1
-- parameters: ["stale", "open"]
```

Only `where` chooses the rows to change. A `sort` or a `take` before a change is a mistake, since a database changes
the rows a condition chooses, in no order:

```python mistake
from cases
sort -risk
update status = "urgent"
```

```text
error: only `where` can choose the rows to update
  ┌─ query.bcql:2:1
  │
2 │ sort -risk
  │ ^^^^^^^^^^ this can't come before `update`
  = help: `update` works on every row the `where`s keep, in no order

checked 1 file: 1 error
```

A condition can follow a link: the rows are then chosen by their key.

```python
from cases
where account.operator == "Acme"
update risk = 0
```

```sql schema
UPDATE cases SET risk = 0
WHERE id IN (
  SELECT cases.id
  FROM cases
  LEFT JOIN accounts AS account ON cases.account_id = account.id
  WHERE account.operator = $1
)
-- parameters: ["Acme"]
```

## `delete`

```python
from cases
where status == "closed" and raised < now() - days(365)
delete
```

```sql
DELETE FROM cases
WHERE status = $1 AND raised < now() - INTERVAL '365 days'
-- parameters: ["closed"]
```

A `delete` with no `where` deletes every row of the table. It is what was written, so it compiles, with a warning:

```python mistake
from cases
delete
```

```text
warning: this deletes every row
  ┌─ query.bcql:2:1
  │
2 │ delete
  │ ^^^^^^ no `where` chooses the rows of `cases`

checked 1 file: 1 warning
```

## `insert into`

`insert into` adds the rows of the steps before to a table. Their columns go into the table's columns of the same
names, so a `select` chooses them:

```python
from cases
where status == "closed"
select key, risk, raised
insert into archive
```

```sql
INSERT INTO archive (key, risk, raised)
SELECT key, risk, raised
FROM cases
WHERE status = $1
-- parameters: ["closed"]
```

Rows can be written in the query, as a list of dicts. Every row has the same keys, which are the columns:

```python
from [
    {"id": "A9", "operator": "Acme", "balance": 0},
    {"id": "B4", "operator": "Beta", "balance": 120},
]
insert into accounts
```

```sql sqlite
INSERT INTO accounts (id, operator, balance)
VALUES (?, ?, 0), (?, ?, 120)
-- parameters: ["A9", "Acme", "B4", "Beta"]
```

## The rows changed

A `select` after a change gives the rows it changed, as they are after it:

```python
from cases
where key == "C1"
update status = "closed"
select key, status
```

```sql
UPDATE cases SET status = $1
WHERE key = $2
RETURNING key, status
-- parameters: ["closed", "C1"]
```

ClickHouse can't give them, and says so. Only a `select` can follow a change.

## Changes in a `def`

A `def` whose steps change rows is called on a line of its own, and its change is made there:

```python
def close_stale(days_old: int = 90):
    from cases
    where status == "open" and raised < now() - days(days_old)
    update status = "closed"

close_stale(30)
```

```sql
UPDATE cases SET status = $1
WHERE status = $2 AND raised < now() - INTERVAL '30 days'
-- parameters: ["closed", "open"]
```

Compiled on its own, its parameters are placeholders, as a query `def`'s are, for a program to give each time it
runs it: with `bcql`, `--query close_stale`.

```python
def close_stale(days_old: int = 90):
    from cases
    where status == "open" and raised < now() - days(days_old)
    update status = "closed"
```

```sql query=close_stale
UPDATE cases SET status = $1
WHERE status = $2 AND raised < now() - $3 * INTERVAL '1 day'
-- parameters: ["closed", "open", {"arg": "days_old"}]
-- arguments: days_old = 90
```

To keep it in the database itself, as a procedure, write `@stored` on the line before it: see
[Stored defs](tables.md#stored-defs).

## Transactions

The statements under `with transaction:` run together: all of them, or none of them if one fails.

```python
with transaction:
    from accounts
    where id == "A1"
    update balance -= 100

    from accounts
    where id == "B4"
    update balance += 100
```

```sql
BEGIN;

UPDATE accounts SET balance = balance - 100
WHERE id = $1;
-- parameters: ["A1"]

UPDATE accounts SET balance = balance + 100
WHERE id = $1;
-- parameters: ["B4"]

COMMIT;
```

ClickHouse runs no transactions, so it refuses a `with transaction:` block rather than run its statements one by
one. From Python, [`run()`](python.md#running-on-a-database) runs a whole text as one transaction.

## Several statements

A file can hold several statements: changes to rows, [changes to the tables](tables.md), and queries. They run in
the order they are written, and `bcql compile` writes each, ended with `;`, with its own parameters. A file gives
the rows of one query; the changes before it are done first.

```python
from cases
where status == "open" and raised < now() - days(90)
update status = "stale"

from cases
group by status:
    n = count()
```

```sql
UPDATE cases SET status = $1
WHERE status = $2 AND raised < now() - INTERVAL '90 days';
-- parameters: ["stale", "open"]

SELECT status, count(*) AS n
FROM cases
GROUP BY status;
```
