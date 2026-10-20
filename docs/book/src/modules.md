# Modules

A file's `def`s, named queries and values can be used by other files, with Python's own import lines. Say
`shared.bcql` holds:

```python file=shared.bcql
# Steps the reports share
threshold = 1_000_000

def recent(days_back=7):
    """The payments of the last `days_back` days."""
    where time > now() - days(days_back)

busy = (
    from transactions
    where amount > threshold
)
```

Then another file in the same folder imports from it:

```python
from shared import recent, threshold

from transactions
recent(30)
where amount > threshold
```

```sql
SELECT *
FROM transactions
WHERE time > now() - INTERVAL '30 days' AND amount > 1000000
```

or imports it whole, and reads its names with a dot:

```python
import shared

from shared.busy
summarize:
    money = sum(amount)
```

```sql
SELECT coalesce(sum(amount), 0) AS money
FROM transactions
WHERE amount > 1000000
```

## Where modules are found

- `import shared` reads `shared.bcql` in the folder of the file compiled, and `import reports.daily` reads
  `reports/daily.bcql` there.
- `from .lib import recent` reads `lib.bcql` beside the importing file; each further dot is a folder up.
- `import shared as s` and `from shared import recent as last` give a name of your own.
- A module that imports itself, through others or directly, is a mistake ButtercupQL reports.

## A module's names are its own

A module's `def`s, named queries and values mean what its own names say, as a Python function sees its own module's
names: `busy` above uses `shared`'s `threshold`, whatever the importing file calls `threshold`.

A mistake in a module is shown in the module's own text, with its own name, and an editor shows it on the import
that reads the module.

## From programs

`bcql` reads modules from files beside the one it compiles. A program gives the compiler a loader instead: the Python
package takes `path=`, for files, or `modules=`, a dict of texts (see [Python](python.md#modules)), and Rust takes
any `Loader` (see [Rust](rust.md)).
