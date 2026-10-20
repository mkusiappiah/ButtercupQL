"""ButtercupQL, a Python-like language for SQL, named after the buttercup flower.

Write a query one step per line, in the order the data flows, and compile it to the SQL a database already runs:

>>> import buttercupql
>>> query = buttercupql.compile('from cases\\nwhere status == "open"\\ntake 5', dialect="sqlite")
>>> print(query.sql)
SELECT *
FROM cases
WHERE status = ?
LIMIT 5
>>> query.params
['open']

`run()` compiles a text and runs it on a database, through the database's own driver:

>>> import sqlite3
>>> db = sqlite3.connect(":memory:")
>>> buttercupql.run(db, 'from [{"n": 1}, {"n": 2}]\\nselect n, twice = n * 2')
[(1, 2), (2, 4)]

In Jupyter, `%load_ext buttercupql` gives the `%%bcql` cell magic.
"""

from ._buttercupql import (
    Arg,
    Compiled,
    CompileError,
    Diagnostic,
    Schema,
    Statement,
    __version__,
    check,
    compile,
)
from ._database import Rows, run
from ._schema import read_schema

__all__ = [
    "Arg",
    "Compiled",
    "CompileError",
    "Diagnostic",
    "Rows",
    "Schema",
    "Statement",
    "__version__",
    "check",
    "compile",
    "read_schema",
    "run",
]


def load_ipython_extension(ipython):
    """`%load_ext buttercupql`: the `%%bcql` cell magic, which runs a cell of ButtercupQL on a database and shows its
    rows."""
    from ._notebook import load

    load(ipython)
