# ButtercupQL language design — v0.1

**Status:** approved on 20 October 2026. The parser reads every form in this document, and the compiler is being built
from it. The research behind each choice is in [research.md](research.md).

## Goals

1. **Read like Python.** Indentation, `#` comments, and Python's own expressions and values.
2. **One step per line, in the order the data flows.** Each step takes rows and gives rows: start from a table, filter,
   compute, group, sort, choose columns.
3. **Faster to write than SQL, and never slower to run.** The compiler writes the SQL a careful person would write.
4. **Compile to SQL; never replace the database.** SQLite and PostgreSQL first, then DuckDB and ClickHouse.
5. **Safe by default.** Every value becomes a parameter, so a query can never inject SQL; names are checked against the
   schema when one is given.

## A first look

```python
# Critical cases nobody has taken, riskiest first
from cases
where status == "open" and band in ["critical", "high"]
where raised > now() - days(7)
sort -risk
take 20
select key, account, risk
```

compiles to (PostgreSQL)

```sql
SELECT key, account, risk
FROM cases
WHERE status = $1 AND band IN ($2, $3) AND raised > now() - INTERVAL '7 days'
ORDER BY risk DESC NULLS LAST
LIMIT 20
```

with the parameters `["open", "critical", "high"]`. Missing values sort last on every database, so cases with no risk
come after the others; with a schema that says `risk` is never missing, `NULLS LAST` is left out.

## Steps

A query is a list of steps, one per line. The first is `from`; the rest can come in any order, as many times as needed.

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

`sort` also takes Python's `reverse=True` to turn the whole order around: `sort raised, reverse=True`. The `-` form is
the main one because it sets each column's direction on its own, like Python's `key=lambda c: (-c.risk, c.raised)`.

## Expressions are Python's

| Python | Example | SQL |
|---|---|---|
| Comparison, chained too | `risk >= 90`, `1 <= risk <= 100` | `risk >= 90`, `risk BETWEEN 1 AND 100` |
| `and`, `or`, `not` | `band == "high" and not closed` | `AND`, `OR`, `NOT` |
| Membership | `band in ["critical", "high"]`, `operator not in ["X"]` | `IN (...)`, `NOT IN (...)` |
| Missing values | `assignee is None`, `assignee == None` | `assignee IS NULL` |
| Arithmetic | `+ - * / // % **`, with Python's meaning: `/` divides exactly, `//` rounds down, `%` takes the divisor's sign | the database's operators, with what makes them mean the same |
| Conditional | `"urgent" if risk >= 90 else "normal"` | `CASE WHEN ... THEN ... ELSE ... END` |
| Text | `"Acme" in operator`, `name.lower()`, `name.startswith("A")`, `len(name)` | `LIKE`, `lower()`, `length()` |
| f-strings | `f"{first} {last}"` | concatenation |
| Numbers and values | `1_000_000`, `3.5`, `True`, `False`, `None` | numbers, booleans, `NULL` |
| Dates and times | `now()`, `today()`, `date("2026-10-20")`, `days(7)`, `hours(6)`, `weeks(2)` | the database's own date arithmetic |

## Grouping

```python
from transactions
where time > now() - days(30)
group by operator:
    total = count()
    money = sum(amount)
    big = count(where=amount > 1_000_000)
where money > 1_000_000          # after grouping: the same `where` becomes HAVING
sort -money
```

- The `by` columns and the block's names are the output. Nothing is written twice, unlike SQL's `SELECT` and `GROUP BY`.
- The block holds totals: `count()`, `count(x)`, `count_distinct(x)`, `sum(x)`, `mean(x)` (also accepted as `avg`),
  `min(x)`, `max(x)`, `any(x)`, `all(x)`.
- Each total can take `where=`, a condition of its own. It compiles to `FILTER (WHERE ...)` on PostgreSQL, SQLite and
  DuckDB, and to `countIf` or `sumIf` on ClickHouse.

Grouping twice needs no subquery to write:

```python
# How many operators had more than 100 cases?
from cases
group by operator:
    n = count()
where n > 100
summarize:
    operators = count()
```

## Joins and links

An explicit join is one line:

```python
from cases
join accounts on account_id == accounts.id
where accounts.operator == "Acme"
```

**Links (v0.2).** When the schema says which column points to which table, a query follows the link with a dot, and the
compiler writes the join:

```python
from cases
where account.operator == "Acme"          # cases.account_id → accounts.id
```

## Reuse

**Named queries.** Brackets make a query a value, as they make any long Python expression span lines:

```python
open_cases = (
    from cases
    where status == "open"
)

from open_cases
group by band:
    n = count()
```

**Functions of steps.** A `def` holds steps; calling it puts them in place:

```python
def recent(days_back=7):
    """The rows raised in the last `days_back` days."""
    where raised > now() - days(days_back)

from cases
recent(30)
take 10
```

**Queries with parameters.** A `def` whose body starts with `from` is a query; its arguments become the SQL's
parameters when it is compiled:

```python
def cases_for(account: str, days_back: int = 30):
    from cases
    where account_id == account
    recent(days_back)
```

**Modules.** Python's own import lines: `from shared import recent`, or `import shared` and then `shared.recent()`.

## Missing values done right

In SQL, `x = NULL` is never true, a classic and silent bug. In ButtercupQL, `x == None` and `x is None` both compile to
`x IS NULL`. A list given to `not in` that holds `None` is compiled so that the result still means what it says, where
SQL would quietly return no rows.

- **Sorting.** Missing values sort last, whichever way the sort goes, on every database.
- **Comparisons.** A comparison with a missing value is never true, as in SQL: `where risk != 90` leaves out the rows
  with no risk. To keep them, say so: `where risk != 90 or risk is None`.
- **Totals of no rows.** `sum()` is `0`, `any()` is `False` and `all()` is `True`, as in Python; `mean()`, `min()` and
  `max()` of no rows are missing.
- **Truth.** A `where` takes a value's truth as Python does: `0`, `""` and `None` are false.

## What the compiler gives back

The SQL text and its parameters, in order. From Python:

```python
import buttercupql

query = buttercupql.compile(text, dialect="postgres")
cursor.execute(query.sql, query.params)
```

From the command line: `bcql compile query.bcql --dialect sqlite` and `bcql check query.bcql`. ButtercupQL files end in
`.bcql`. Both take `--schema schema.sql`, the database's own `CREATE TABLE` statements, to check every name against the
tables.

**Each database.** The same query gives the same rows on SQLite and PostgreSQL:

- **SQLite** keeps dates and times as text. ButtercupQL compares them in the form SQLite's own date functions write,
  `2026-10-20 14:30:00`, and moves them with those functions: `datetime('now', '-7 days')`.
- **SQLite** builds often leave out the maths functions, so ButtercupQL does without them: `//` and `%` are worked out
  from casts, and `x ** 2` to `x ** 4` are multiplications. Other powers need SQLite's `power()`.
- **PostgreSQL** gets `$1` placeholders and **SQLite** `?`, as their own drivers take them; Python's psycopg takes
  `%s`, which `--params format` writes.

## Errors that help

```text
error: there is no column `stauts` in `cases`
  ┌─ open.bcql:2:7
  │
2 │ where stauts == "open"
  │       ^^^^^^ did you mean `status`?
```

Every error found is reported at once, each pointing at its place, rather than only the first.

## Queries that run fast

- **One flat `SELECT`** whenever the steps allow. A subquery is written only when the meaning needs one, for example a
  `where` after `take`.
- **No optimisation fences.** A named query used once is written inline, where the database can optimise across it.
- **Each database's own forms:** `FILTER (WHERE ...)` or `countIf`, and `QUALIFY` where the database has it.
- **Parameters,** so databases can reuse a query's plan. `literal(x)` writes a value into the SQL instead, for the cases
  where a partial index needs to see it.
- **Patterns an index can use.** `name.startswith("A")` is `name LIKE 'A%'` on PostgreSQL and `name GLOB 'A*'` on
  SQLite, with any wildcard in the text made plain.
- **Sorting with missing values last.** PostgreSQL can serve `ORDER BY risk DESC NULLS LAST` from an index made the same
  way, `CREATE INDEX ON cases (risk DESC NULLS LAST)`, or from a plain index when the column is `NOT NULL` and the schema
  says so.
- **Compiling takes microseconds,** so it can run on every request, and in a browser through WebAssembly.

## Writing details

Settled while building the parser, each as Python does it where Python has an answer:

- **Lines.** A step ends at the end of its line. Inside brackets a step can run over several lines, and a `\` at the end
  of a line joins the next one to it. `;` puts several steps on one line: `from cases; where risk > 90; take 5`.
- **Indentation.** The first line sets the column, so a query indented inside a Python string reads as it is. Only a
  block after a `:` is indented further. Tabs and spaces mixed so that the meaning depends on a tab's width are refused,
  as in Python.
- **Names that are not plain words** go in backticks: ``from `order lines` ``, ``where `from` == "A"``. Python's
  keywords need them too. The step words (`where`, `sort` and the others) are ordinary names everywhere except at the
  start of a line.
- **Long lists of columns** go in brackets, over as many lines as needed: `select (key, risk, total = x + y)`.
- **`group by operator`** with no block keeps one row per group, with no totals.
- **Values before the query.** `threshold = 1_000_000` names a value for the query to use, as a named query names a
  query. In the query, the name means the value even where a table has a column of the same name, so a new column can
  never change what a query does.
- **`risk += 1`** is `risk = risk + 1`, as in Python.
- **A query in brackets is a value:** `where account_id in (from blocked; select account_id)`.
- **Other languages' habits are caught.** `order by`, `limit`, `having`, `WHERE`, `=` in a condition, `&&`, `||`, `!`,
  `<>`, `desc` and `x as y` are each reported with the ButtercupQL way to write them.

## Not in v0.1

- Writing data (`insert`, `update`, `delete`) and creating tables.
- Window functions, links, `match`, recursive queries and pivots: planned for v0.2 and later.

## Decisions

Approved on 20 October 2026:

1. `sort -risk` as the main form, with `reverse=True` accepted.
2. `mean(x)` for averages, with `avg(x)` accepted.
3. The `.bcql` extension and the `bcql` command (`bql` is taken by another project).
4. Links from the schema in v0.2, after the first release.
