"""The compiled part of ButtercupQL."""

from collections.abc import Mapping
from typing import Optional, Union

__version__: str

Value = Union[None, bool, int, float, str]

class Diagnostic:
    """A problem found in a text: what it is, where, and how to put it right."""

    @property
    def severity(self) -> str: ...
    @property
    def message(self) -> str: ...
    @property
    def label(self) -> str: ...
    @property
    def help(self) -> list[str]: ...
    @property
    def file(self) -> Optional[str]: ...
    @property
    def line(self) -> int: ...
    @property
    def column(self) -> int: ...
    def render(self, color: bool = False) -> str: ...

class CompileError(ValueError):
    """A text that does not compile. Its message shows every problem under its line."""

    diagnostics: list[Diagnostic]

class Arg:
    """An argument of a query `def` compiled with its parameters open."""

    @property
    def name(self) -> str: ...
    @property
    def type(self) -> str: ...
    @property
    def default(self) -> Value: ...

class Statement:
    """One statement of a compiled text: its SQL, and the values for its placeholders."""

    @property
    def sql(self) -> str: ...
    @property
    def params(self) -> list[Value]: ...
    @property
    def names(self) -> list[str]: ...
    @property
    def kind(self) -> str: ...

class Compiled:
    """A query compiled to SQL."""

    @property
    def sql(self) -> str: ...
    @property
    def params(self) -> list[Value]: ...
    @property
    def names(self) -> list[str]: ...
    @property
    def statements(self) -> list[Statement]: ...
    @property
    def args(self) -> list[Arg]: ...
    @property
    def warnings(self) -> list[Diagnostic]: ...
    def bind(self, **args: object) -> list[Value]: ...
    def bind_named(self, **args: object) -> dict[str, Value]: ...

class Schema:
    """The tables of a database, to check names against."""

    @staticmethod
    def from_sql(text: str) -> "Schema": ...
    @property
    def tables(self) -> list[str]: ...

def compile(
    text: str,
    dialect: str = "postgres",
    schema: Union[None, str, Schema, Mapping[str, Mapping[str, str]]] = None,
    params: Optional[str] = None,
    query: Optional[str] = None,
    path: Optional[str] = None,
    modules: Optional[Mapping[str, str]] = None,
) -> Compiled: ...
def check(
    text: str,
    dialect: str = "postgres",
    schema: Union[None, str, Schema, Mapping[str, Mapping[str, str]]] = None,
    path: Optional[str] = None,
    modules: Optional[Mapping[str, str]] = None,
) -> list[Diagnostic]: ...
def _run(args: list[str]) -> int: ...
