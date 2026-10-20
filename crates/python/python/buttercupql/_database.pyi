from collections.abc import Iterable, Mapping
from typing import Any, Optional, Union

from ._buttercupql import Schema

class Rows(list[tuple[Any, ...]]):
    """The rows a query gave: a list of tuples, with the names of their columns in `columns`."""

    columns: list[str]
    def __init__(self, rows: Iterable[Iterable[Any]] = ..., columns: Iterable[str] = ...) -> None: ...
    def to_pandas(self) -> Any: ...

def dialect_of(connection: object) -> Optional[str]: ...
def run(
    connection: object,
    text: str,
    *,
    dialect: Optional[str] = None,
    schema: Union[None, str, Schema, Mapping[str, Mapping[str, str]]] = None,
    query: Optional[str] = None,
    path: Optional[str] = None,
    modules: Optional[Mapping[str, str]] = None,
    **args: object,
) -> Optional[Rows]: ...
