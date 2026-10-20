"""The `%%bcql` cell magic, run in an IPython shell as a notebook runs it."""

import pathlib
import sqlite3

import pytest

pytest.importorskip("IPython")

from IPython.core.interactiveshell import InteractiveShell  # noqa: E402

import buttercupql  # noqa: E402


@pytest.fixture(scope="module")
def shell():
    shell = InteractiveShell.instance()
    shell.run_line_magic("load_ext", "buttercupql")
    return shell


@pytest.fixture
def db(shell):
    connection = sqlite3.connect(":memory:")
    connection.executescript(
        "CREATE TABLE cases (key TEXT NOT NULL, status TEXT NOT NULL, risk INTEGER);"
        "INSERT INTO cases VALUES ('C1', 'open', 95), ('C2', 'open', 40), ('C3', 'closed', 90);"
    )
    shell.user_ns["db"] = connection
    return connection


def values(shown):
    """The rows a cell showed, as tuples: from a pandas DataFrame, or from `Rows` without pandas."""
    if hasattr(shown, "itertuples"):
        return [tuple(row) for row in shown.itertuples(index=False)]
    return list(shown)


def test_a_cell_runs_on_the_connection_it_names(shell, db):
    shown = shell.run_cell_magic("bcql", "db", 'from cases\nwhere status == "open"\nsort -risk\nselect key, risk\n')
    assert values(shown) == [("C1", 95), ("C2", 40)]
    # The cells after use it too.
    again = shell.run_cell_magic("bcql", "", "from cases\nsummarize:\n    n = count()\n")
    assert values(again) == [(3,)]


def test_the_rows_can_be_kept(shell, db):
    shell.run_cell_magic("bcql", "db -o open_cases", 'from cases\nwhere status == "open"\nselect key\n')
    assert values(shell.user_ns["open_cases"]) == [("C1",), ("C2",)]


def test_the_sql_is_shown_for_the_connections_database(shell, db):
    shown = shell.run_cell_magic("bcql", "db --sql", 'from cases\nwhere status == "open"\n')
    assert repr(shown) == 'SELECT *\nFROM cases\nWHERE status = ?\n-- parameters: ["open"]'
    assert shown._repr_markdown_().startswith("```sql\nSELECT *")
    other = shell.run_cell_magic("bcql", "--sql -d clickhouse", 'from cases\nwhere status == "open"\n')
    assert repr(other).splitlines()[2] == "WHERE status = {p1:String}"


def test_a_mistake_is_shown_with_its_suggestion_from_the_databases_tables(shell, db, capsys):
    assert shell.run_cell_magic("bcql", "db", 'from cases\nwhere stauts == "open"\n') is None
    shown = capsys.readouterr().err
    assert "there is no column `stauts` in `cases`" in shown and "did you mean `status`?" in shown


def test_a_change_says_it_ran(shell, db, capsys):
    assert shell.run_cell_magic("bcql", "db", 'from cases\nwhere risk < 50\nupdate status = "low"\n') is None
    assert capsys.readouterr().out == "1 statement run.\n"
    assert db.execute("SELECT key FROM cases WHERE status = 'low'").fetchall() == [("C2",)]


def test_a_schema_can_be_given(shell, db):
    shell.user_ns["tables"] = {"cases": {"key": "text not null", "status": "text", "risk": "integer"}}
    shown = shell.run_cell_magic("bcql", "db --schema tables", "from cases\nsort -risk\ntake 1\nselect key\n")
    assert values(shown) == [("C1",)]


def test_a_cell_with_no_connection_says_so(capsys):
    shell = InteractiveShell.instance()
    magics = shell.magics_manager.registry["ButtercupQLMagics"]
    kept, magics.connection = magics.connection, None
    try:
        assert shell.run_cell_magic("bcql", "", "from cases\n") is None
        assert "Name the connection" in capsys.readouterr().err
        assert shell.run_cell_magic("bcql", "nowhere", "from cases\n") is None
        assert "There is no variable named 'nowhere'" in capsys.readouterr().err
    finally:
        magics.connection = kept


def test_the_package_loads_as_an_extension():
    assert callable(buttercupql.load_ipython_extension)


CHAPTER = pathlib.Path(__file__).resolve().parents[3] / "docs" / "book" / "src" / "notebooks.md"


def pieces(text):
    """The chapter's code blocks and tables, in order, each with its kind, its text or its rows, and the lines it
    starts and ends on."""
    lines = text.split("\n")
    found = []
    at = 0
    while at < len(lines):
        line = lines[at]
        if line.startswith("```"):
            start = at
            body = []
            at += 1
            while not lines[at].startswith("```"):
                body.append(lines[at])
                at += 1
            found.append((line[3:].strip(), "\n".join(body) + "\n", start, at))
        elif line.startswith("|") and at + 1 < len(lines) and lines[at + 1].startswith("|---"):
            start = at
            rows = []
            at += 2
            while at < len(lines) and lines[at].startswith("|"):
                rows.append(tuple(cell.strip() for cell in lines[at].strip("|").split("|")))
                at += 1
            found.append(("table", rows, start, at - 1))
            continue
        at += 1
    return found, lines


def test_the_notebook_chapter_runs(shell, capsys):
    found, lines = pieces(CHAPTER.read_text(encoding="utf-8"))
    ran = None
    checked = 0
    for kind, body, start, end in found:
        if kind == "py":
            capsys.readouterr()
            result = shell.run_cell(body, store_history=False)
            assert result.success, (body, result.error_in_exec)
            ran = (end, result.result, capsys.readouterr().err)
            continue
        # What a cell shows comes right after it, with nothing but blank lines between.
        if ran is None or any(line.strip() for line in lines[ran[0] + 1 : start]):
            continue
        _, shown, err = ran
        if kind == "table":
            assert [tuple(str(value) for value in row) for row in values(shown)] == body
        elif kind == "sql":
            assert repr(shown) + "\n" == body
        elif kind == "text":
            assert err == body
        checked += 1
    assert checked >= 4
