# Joins and links

## Joins

A join is one line: the table, and how its rows match. A table's own columns are read with its name and a dot.

```python
from cases
join accounts on account_id == accounts.id
where accounts.operator == "Acme"
```

```sql
SELECT *
FROM cases
JOIN accounts ON account_id = accounts.id
WHERE accounts.operator = $1
-- parameters: ["Acme"]
```

`left join` keeps the rows that have no match, with the other table's columns missing:

```python
from cases
left join accounts on account_id == accounts.id
where accounts.id is None          # the cases with no account
```

```sql
SELECT *
FROM cases
LEFT JOIN accounts ON account_id = accounts.id
WHERE accounts.id IS NULL
```

## Links

When the schema says which column points to which table, a query follows the link with a dot, and ButtercupQL writes
the join. A column that `REFERENCES` another table, such as `account_id`, is followed by its name without `_id`:

```python
from cases
where account.operator == "Acme"          # cases.account_id → accounts.id
select key, risk, account.balance
```

```sql schema
SELECT cases.key, cases.risk, account.balance
FROM cases
LEFT JOIN accounts AS account ON cases.account_id = account.id
WHERE account.operator = $1
-- parameters: ["Acme"]
```

- Each link is joined once, with `LEFT JOIN`, so a row whose link leads nowhere stays, with its linked values missing:
  `where account.id is None` finds the cases with no account.
- A link from the linked table goes on: `account.bank.name`.
- Links need a schema, with the database's `REFERENCES` or `FOREIGN KEY` constraints in it. The
  [Schemas](schemas.md) chapter shows the one these examples use.

A link can be grouped by, like any value:

```python
from cases
where status == "open"
group by operator = account.operator:
    open = count()
    riskiest = max(risk)
sort -open
```

```sql schema
SELECT account.operator, count(*) AS open, max(cases.risk) AS riskiest
FROM cases
LEFT JOIN accounts AS account ON cases.account_id = account.id
WHERE cases.status = $1
GROUP BY account.operator
ORDER BY open DESC
-- parameters: ["open"]
```
