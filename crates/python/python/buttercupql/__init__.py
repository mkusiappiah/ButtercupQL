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
"""

from ._buttercupql import (
    Arg,
    Compiled,
    CompileError,
    Diagnostic,
    Schema,
    __version__,
    check,
    compile,
)

__all__ = [
    "Arg",
    "Compiled",
    "CompileError",
    "Diagnostic",
    "Schema",
    "__version__",
    "check",
    "compile",
]
