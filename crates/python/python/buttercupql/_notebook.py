"""ButtercupQL in Jupyter and IPython: `%load_ext buttercupql`, then a cell of ButtercupQL under `%%bcql`."""

import json
import os
import sys

from IPython.core.magic import Magics, cell_magic, magics_class
from IPython.core.magic_arguments import argument, magic_arguments, parse_argstring

from ._buttercupql import CompileError, compile
from ._database import dialect_of, run
from ._schema import read_schema


class Sql:
    """The SQL a cell compiles to, shown as SQL."""

    def __init__(self, compiled):
        parts = []
        statements = compiled.statements
        several = len(statements) > 1
        for statement in statements:
            text = statement.sql + (";" if several else "")
            try:
                values = statement.params
            except ValueError:
                values = None
            if values:
                text += "\n-- parameters: " + json.dumps(values)
            parts.append(text)
        self.text = "\n\n".join(parts)

    def __repr__(self):
        return self.text

    def _repr_markdown_(self):
        return f"```sql\n{self.text}\n```"


@magics_class
class ButtercupQLMagics(Magics):
    """The `%%bcql` cell magic."""

    def __init__(self, shell):
        super().__init__(shell)
        # The connection named last, for the cells that name none.
        self.connection = None

    @magic_arguments()
    @argument(
        "connection",
        nargs="?",
        help="The name of the connection to the database: a sqlite3, psycopg, psycopg2, pg8000 or DuckDB "
        "connection, or a clickhouse_connect client. Without one, the one named last.",
    )
    @argument("-d", "--dialect", help="The database, when its driver does not say: sqlite, postgres, duckdb or clickhouse.")
    @argument(
        "-s",
        "--schema",
        help="The tables, to check names against: the name of a variable holding CREATE TABLE statements, a "
        "Schema or a dict of tables, or a file of CREATE TABLE statements. Without one, the database's own tables "
        "are read.",
    )
    @argument("--sql", action="store_true", help="Show the SQL the cell compiles to, without running it.")
    @argument("-o", "--out", help="Keep the rows in a variable of this name, as well as showing them.")
    @cell_magic
    def bcql(self, line, cell):
        """Runs a cell of ButtercupQL on a database, and shows its rows: as a pandas DataFrame when pandas is
        installed. With `--sql`, shows the SQL instead."""
        args = parse_argstring(self.bcql, line)
        namespace = self.shell.user_ns
        connection = None
        if args.connection is not None:
            if args.connection not in namespace:
                print(f"There is no variable named {args.connection!r} to run the cell on.", file=sys.stderr)
                return None
            connection = namespace[args.connection]
            self.connection = connection
        else:
            # The connection named last: for `--sql`, its database's SQL.
            connection = self.connection
            if connection is None and not args.sql:
                print(
                    "Name the connection to run the cell on, as in `%%bcql db`: it is kept for the cells after.",
                    file=sys.stderr,
                )
                return None
        schema = args.schema
        if schema is not None:
            if schema in namespace:
                schema = namespace[schema]
            elif os.path.exists(schema):
                with open(schema, encoding="utf-8") as file:
                    schema = file.read()
            else:
                print(f"--schema names no variable and no file: {args.schema!r}", file=sys.stderr)
                return None
        elif connection is not None:
            # The database's own tables, for names to be checked against, unless they can't be read.
            try:
                schema = read_schema(connection)
            except Exception:
                schema = None
        dialect = args.dialect or (dialect_of(connection) if connection is not None else None)
        try:
            compiled = compile(cell, dialect=dialect or "postgres", schema=schema)
            for warning in compiled.warnings:
                print(warning.render(), file=sys.stderr)
            if args.sql:
                return Sql(compiled)
            rows = run(connection, cell, dialect=dialect, schema=schema)
        except CompileError as error:
            # The problems, each under its line, without a traceback of the magic's own code.
            print(str(error), file=sys.stderr)
            return None
        if rows is None:
            count = len([s for s in compiled.statements if s.kind not in ("begin", "commit")])
            print(f"{count} statement{'' if count == 1 else 's'} run.")
            return None
        try:
            shown = rows.to_pandas()
        except ImportError:
            shown = rows
        if args.out:
            namespace[args.out] = shown
        return shown


def load(ipython):
    ipython.register_magics(ButtercupQLMagics)
