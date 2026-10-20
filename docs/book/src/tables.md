# Tables and permissions

Tables, indexes and permissions are statements of their own, each on its own line, with a block below it where it
has parts. Each is written in the form its database takes, and refused with a clear message where the database has
no such thing.

## `create table`

A table's columns are written as Python's annotations: a name, a colon and a type. A column is never missing unless
its type says it can be, with `| None` or `Optional[...]`. A value after `=` is its default.

```python
create table archive:
    key: str = field(primary_key=True)
    account_id: str | None = field(references=accounts.id)
    status: str = "closed"
    risk: int | None
    raised: datetime
    archived: datetime = now()
```

```sql
CREATE TABLE archive (
  key TEXT NOT NULL,
  account_id TEXT REFERENCES accounts (id),
  status TEXT NOT NULL DEFAULT 'closed',
  risk BIGINT,
  raised TIMESTAMP NOT NULL,
  archived TIMESTAMP NOT NULL DEFAULT (now()),
  PRIMARY KEY (key)
)
```

`field()` says the rest, as Python's dataclasses do: `primary_key=True`, `unique=True`, `references=accounts.id` for
a column that holds another table's key, and `default=`. A key of several columns is a line of its own:
`primary_key(account_id, day)`, or `unique(account_id, day)`. `create table archive, exist_ok=True:` creates the
table only if there is none of that name.

The types are Python's, and each database gets its own for them:

| Written | PostgreSQL | SQLite | DuckDB | ClickHouse |
|---|---|---|---|---|
| `int` | `BIGINT` | `INTEGER` | `BIGINT` | `Int64` |
| `float` | `DOUBLE PRECISION` | `REAL` | `DOUBLE` | `Float64` |
| `Decimal` | `NUMERIC` | `NUMERIC` | `DECIMAL(38, 10)` | `Decimal(38, 10)` |
| `str` | `TEXT` | `TEXT` | `VARCHAR` | `String` |
| `bool` | `BOOLEAN` | `BOOLEAN` | `BOOLEAN` | `Bool` |
| `date` | `DATE` | `TEXT` | `DATE` | `Date` |
| `datetime` | `TIMESTAMP` | `TEXT` | `TIMESTAMP` | `DateTime` |
| `bytes` | `BYTEA` | `BLOB` | `BLOB` | `String` |

A ClickHouse table keeps its rows in the order of its primary key, and a column that can be missing is `Nullable`.
ClickHouse keeps no unique keys and no references, so they are left out, with a warning:

```python
create table archive:
    key: str = field(primary_key=True)
    account_id: str | None = field(references=accounts.id)
    risk: int | None
    raised: datetime
```

```sql clickhouse
CREATE TABLE archive (
  key String,
  account_id Nullable(String),
  risk Nullable(Int64),
  raised DateTime
)
ENGINE = MergeTree
ORDER BY (key)
```

## Indexes

`create index on cases(-risk, raised)` makes an index of the columns, in the order a sort by them takes: `-` for
high to low.

```python
create index on cases(-risk, raised)
```

```sql
CREATE INDEX cases_risk_raised_idx ON cases (risk DESC NULLS LAST, raised)
```

`create unique index` makes one in which no two rows have the same values. A name can be given after `index`, and
`where=` keeps only the rows a condition chooses:

```python
create unique index open_key on cases(key), where=status == "open"
```

```sql
CREATE UNIQUE INDEX open_key ON cases (key) WHERE status = 'open'
```

ClickHouse has no indexes like these.

## `alter table`

`alter table` changes a table, a change on each line of its block: a column written as in `create table` is added,
`rename old to new` renames one, and `del` drops one.

```python
alter table cases:
    region: str = "east"
    rename risk to score
    del notes
```

```sql
ALTER TABLE cases ADD COLUMN region TEXT NOT NULL DEFAULT 'east';

ALTER TABLE cases RENAME COLUMN risk TO score;

ALTER TABLE cases DROP COLUMN notes;
```

`drop table archive` drops a table, and `drop index cases_risk_raised_idx` an index. `missing_ok=True` drops it
only if it is there.

## Checked as they are written

With a schema, the names of each statement are checked against the tables, and a `create table`, an `alter table`
or a `drop table` changes the tables the statements after it are checked against:

```python mistake
create table watched:
    key: str = field(primary_key=True)
    since: datetime

from watched
where sicne > now() - days(7)
```

```text
error: there is no column `sicne` in `watched`
  ┌─ query.bcql:6:7
  │
6 │ where sicne > now() - days(7)
  │       ^^^^^ did you mean `since`?

checked 1 file: 1 error
```

## Permissions

`grant` lets users or roles read or change tables, and `revoke` takes their permissions away again. The permissions
are `read`, `write`, which is `insert`, `update` and `delete`, each of those three alone, and `all`.

```python
grant read, write on cases, accounts to analyst
revoke write on accounts from analyst
```

```sql
GRANT SELECT, INSERT, UPDATE, DELETE ON TABLE cases, accounts TO analyst;

REVOKE INSERT, UPDATE, DELETE ON TABLE accounts FROM analyst;
```

ClickHouse names its permissions in its own way, one table at a time:

```python
grant read, write on cases, accounts to analyst
revoke write on accounts from analyst
```

```sql clickhouse
GRANT SELECT, INSERT, ALTER UPDATE, ALTER DELETE ON cases TO analyst;

GRANT SELECT, INSERT, ALTER UPDATE, ALTER DELETE ON accounts TO analyst;

REVOKE INSERT, ALTER UPDATE, ALTER DELETE ON accounts FROM analyst;
```

SQLite and DuckDB have no users, and so no permissions.

## Stored defs

A `def` written with `@stored` on the line before it is kept in the database, which then runs it by name. A query
`def` becomes a function on PostgreSQL, a macro on DuckDB, and a view with parameters on ClickHouse. PostgreSQL
needs the types of the parameters, and of the columns the query gives, which the schema says:

```python
@stored
def risky_for(account: str, at_least: int = 50):
    from cases
    where account_id == account and risk >= at_least
    select key, risk
```

```sql schema
CREATE OR REPLACE FUNCTION risky_for(account TEXT, at_least BIGINT DEFAULT 50)
RETURNS TABLE (key TEXT, risk BIGINT)
LANGUAGE sql STABLE
AS $bcql$
SELECT key, risk
FROM cases
WHERE account_id = risky_for.account AND risk >= risky_for.at_least
$bcql$
```

```python
@stored
def risky_for(account: str, at_least: int = 50):
    from cases
    where account_id == account and risk >= at_least
    select key, risk
```

```sql duckdb
CREATE OR REPLACE MACRO risky_for(account, at_least := 50) AS TABLE
SELECT key, risk
FROM cases
WHERE account_id = account AND risk >= at_least
```

A `def` that changes rows becomes a procedure on PostgreSQL:

```python
@stored
def close_stale(days_old: int = 90):
    from cases
    where status == "open" and raised < now() - days(days_old)
    update status = "closed"
```

```sql
CREATE OR REPLACE PROCEDURE close_stale(days_old BIGINT DEFAULT 90)
LANGUAGE sql
BEGIN ATOMIC
UPDATE cases SET status = 'closed'
WHERE status = 'open' AND raised < now() - close_stale.days_old * INTERVAL '1 day';
END
```

SQLite keeps no queries, DuckDB keeps none that change rows, and ClickHouse's views take no defaults: each says so.
