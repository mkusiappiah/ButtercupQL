# Schemas

A schema is the database's own `CREATE TABLE` statements. Given one, ButtercupQL checks every name against it, and
writes SQL that uses what the tables say. The examples in this book that use a schema, and [the playground](playground.md),
use this one:

```sql
CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    operator TEXT NOT NULL,
    balance NUMERIC
);

CREATE TABLE cases (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL,
    account_id TEXT REFERENCES accounts (id),
    status TEXT NOT NULL,
    band TEXT,
    risk INTEGER,
    raised TIMESTAMP NOT NULL,
    assignee TEXT,
    notes TEXT,
    account TEXT,
    operator TEXT
);

CREATE TABLE transactions (
    id INTEGER PRIMARY KEY,
    account_id TEXT REFERENCES accounts (id),
    operator TEXT NOT NULL,
    amount INTEGER NOT NULL,
    time TIMESTAMP NOT NULL
);
```

`bcql` takes it with `--schema schema.sql`, the Python package with `schema=`, and an editor from its settings.

## Every name is checked

A name that is not in the table is a mistake, with the name most likely meant:

```python mistake
from cases
where stauts == "open"
```

```text schema
error: there is no column `stauts` in `cases`
  ┌─ query.bcql:2:7
  │
2 │ where stauts == "open"
  │       ^^^^^^ did you mean `status`?

checked 1 file: 1 error
```

Types are checked too: text can't be compared with a number.

```python mistake
from cases
where risk == "high"
```

```text schema
error: A whole number can't be compared with text
  ┌─ query.bcql:2:7
  │
2 │ where risk == "high"
  │       ^^^^^^^^^^^^^^

checked 1 file: 1 error
```

## SQL that uses what the tables say

- **A column that is never missing** needs no `NULLS LAST` when it is sorted, so an ordinary index serves the sort:

```python
from cases
sort -raised
```

```sql schema
SELECT *
FROM cases
ORDER BY raised DESC
```

- **A column that points to another table** can be followed with a dot: see [Links](joins.md#links).
- **`del`** writes the columns that are left, by name.

## Writing a schema

ButtercupQL reads `CREATE TABLE` statements as PostgreSQL, SQLite and DuckDB write them: each column's name and type,
`NOT NULL`, `PRIMARY KEY`, and `REFERENCES` in the column or as a `FOREIGN KEY` of the table. A type it does not know
is taken as a value of any type, and what else a statement says is left to the database.
