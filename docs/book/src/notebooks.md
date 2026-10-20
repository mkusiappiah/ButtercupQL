# Notebooks

In Jupyter, IPython, and the notebooks of VS Code and other editors, a cell of ButtercupQL runs on a database and
shows its rows. The `buttercupql` package brings the cell magic: `pip install buttercupql`, then load it once in the
notebook.

```py
%load_ext buttercupql
```

A cell that starts with `%%bcql` and the name of a connection runs on that database. Any connection that
[`run()`](python.md#running-on-a-database) takes will do:

```py
import sqlite3

db = sqlite3.connect(":memory:")
db.executescript("""
    CREATE TABLE cases (key TEXT NOT NULL, status TEXT NOT NULL, band TEXT, risk INTEGER);
    INSERT INTO cases VALUES ('C1', 'open', 'critical', 95), ('C2', 'open', 'low', 40),
                             ('C3', 'closed', 'high', 90), ('C4', 'open', 'high', NULL);
""")
```

```py
%%bcql db
from cases
where status == "open" and risk > 30
sort -risk
select key, band, risk
```

| key | band | risk |
|---|---|---|
| C1 | critical | 95 |
| C2 | low | 40 |

The rows are a pandas DataFrame when pandas is installed, and a table of their own when it is not. The cells after it
run on the same connection without naming it again.

## Mistakes, before anything runs

Each cell's names are checked against the database's own tables, read from the database as the cell runs. A mistake
is shown with its suggestion, and nothing runs:

```py
%%bcql
from cases
where stauts == "open"
```

```text
error: there is no column `stauts` in `cases`
  ┌─ <query>:2:7
  │
2 │ where stauts == "open"
  │       ^^^^^^ did you mean `status`?

```

## The SQL

`--sql` shows the SQL the cell compiles to, for the connection's database, without running it:

```py
%%bcql --sql
from cases
where status == "open"
sort -risk
```

```sql
SELECT *
FROM cases
WHERE status = ?
ORDER BY risk DESC
-- parameters: ["open"]
```

## Keeping the rows

`-o` keeps the rows in a variable, for the cells after to use:

```py
%%bcql -o bands
from cases
group by band:
    cases = count()
    riskiest = max(risk)
sort band
```

| band | cases | riskiest |
|---|---|---|
| critical | 1 | 95 |
| high | 2 | 90 |
| low | 1 | 40 |

```py
assert len(bands) == 3
```

A cell that changes the database runs as [`run()`](python.md#running-on-a-database) runs a text: statement by
statement, as one transaction. With no query to give rows, it says how many statements ran.

## Options

| Written | Does |
|---|---|
| `%%bcql db` | Runs the cell on the connection `db`, and makes it the one the cells after run on. |
| `%%bcql --sql` | Shows the SQL the cell compiles to, without running it. |
| `%%bcql -o name` | Keeps the rows in the variable `name`, as well as showing them. |
| `%%bcql --schema tables` | Checks the names against `tables`, in place of the database's own: a variable holding `CREATE TABLE` statements, a `Schema` or a dict of tables, or a file of `CREATE TABLE` statements. |
| `%%bcql -d duckdb` | Names the database, for a driver that does not say which it is. |

`%%bcql?` shows them in the notebook.
