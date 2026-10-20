"""The `bcql` command, as the Python package installs it."""

import sys

from ._buttercupql import _run


def main() -> None:
    """Runs `bcql` with the command line's arguments."""
    sys.exit(_run(sys.argv[1:]))
