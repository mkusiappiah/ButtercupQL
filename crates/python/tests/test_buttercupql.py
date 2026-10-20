"""The Python package, used as a program would use it."""

import datetime
import sqlite3

import pytest

import buttercupql


def test_a_query_compiles_and_runs_on_sqlite():
    connection = sqlite3.connect(":memory:")
    connection.executescript(
        """
        CREATE TABLE cases (key TEXT, status TEXT, band TEXT, risk INTEGER);
        INSERT INTO cases VALUES ('C1', 'open', 'critical', 95), ('C2', 'open', 'low', 40),
                                 ('C3', 'closed', 'high', 90), ('C4', 'open', 'high', NULL);
        """
    )
    query = buttercupql.compile(
        """
        from cases
        where status == "open" and band in ["critical", "high"]
        sort -risk
        select key, risk
        """,
        dialect="sqlite",
    )
    assert query.sql == "SELECT key, risk\nFROM cases\nWHERE status = ? AND band IN (?, ?)\nORDER BY risk DESC"
    assert query.params == ["open", "critical", "high"]
    assert connection.execute(query.sql, query.params).fetchall() == [("C1", 95), ("C4", None)]
    assert str(query) == query.sql


def test_each_database_gets_its_own_placeholders():
    text = 'from t\nwhere a == "x"\n'
    assert buttercupql.compile(text).sql.endswith("a = $1")
    assert buttercupql.compile(text, dialect="duckdb").sql.endswith("a = $1")
    assert buttercupql.compile(text, dialect="clickhouse").sql.endswith("a = {p1:String}")
    assert buttercupql.compile(text, params="format").sql.endswith("a = %s")
    with pytest.raises(ValueError, match="unknown dialect `oracle`"):
        buttercupql.compile(text, dialect="oracle")


def test_a_mistake_is_shown_at_its_place():
    with pytest.raises(buttercupql.CompileError) as caught:
        buttercupql.compile('from cases\nwhere status = "open"\n', path="open.bcql")
    error = caught.value
    assert str(error).startswith("error: use `==` to compare\n  ┌─ open.bcql:2:14\n")
    [problem] = error.diagnostics
    assert (problem.severity, problem.line, problem.column) == ("error", 2, 14)
    assert problem.label == "`=` gives a column a value; `==` compares two values"
    assert "^" in problem.render()
    assert isinstance(error, ValueError)


def test_a_schema_checks_every_name():
    schema = "CREATE TABLE cases (key text NOT NULL, status text NOT NULL, risk integer);"
    with pytest.raises(buttercupql.CompileError) as caught:
        buttercupql.compile('from cases\nwhere stauts == "open"\n', schema=schema)
    assert caught.value.diagnostics[0].label == "did you mean `status`?"
    as_dict = {"cases": {"key": "text not null", "risk": "integer"}}
    assert buttercupql.compile("from cases\nsort -key\n", schema=as_dict).sql.endswith("ORDER BY key DESC")
    parsed = buttercupql.Schema.from_sql(schema)
    assert parsed.tables == ["cases"]
    assert buttercupql.compile("from cases\nsort -key\n", schema=parsed).sql.endswith("ORDER BY key DESC")


def test_a_query_def_takes_its_arguments_when_it_runs():
    text = """
def open_for(account: str, rows: int = 10, since: datetime = None):
    from cases
    where account_id == account
    take rows
"""
    query = buttercupql.compile(text, dialect="sqlite", query="open_for")
    assert query.sql == "SELECT *\nFROM cases\nWHERE account_id = ?\nLIMIT ?"
    assert [(arg.name, arg.type, arg.default) for arg in query.args] == [
        ("account", "str", None),
        ("rows", "int", 10),
        ("since", "datetime", None),
    ]
    assert query.bind(account="A1") == ["A1", 10]
    assert query.bind(account="A1", rows=3) == ["A1", 3]
    with pytest.raises(ValueError, match="needs a value for `account`"):
        query.params
    with pytest.raises(ValueError, match="no argument `acount`"):
        query.bind(acount="A1")
    clickhouse = buttercupql.compile(text, dialect="clickhouse", query="open_for")
    assert clickhouse.bind_named(account="A1") == {"account": "A1", "rows": 10}
    moment = buttercupql.compile(
        "def after(when: datetime):\n    from t\n    where raised > when\n", query="after"
    )
    assert moment.bind(when=datetime.datetime(2026, 10, 20, 14, 30)) == ["2026-10-20 14:30:00"]


def test_modules_come_from_texts_or_files(tmp_path):
    shared = "threshold = 1_000_000\ndef recent(days_back=7):\n    where raised > now() - days(days_back)\n"
    text = "from shared import recent, threshold\nfrom cases\nrecent(30)\nwhere amount > threshold\n"
    from_texts = buttercupql.compile(text, modules={"shared": shared})
    assert from_texts.sql == "SELECT *\nFROM cases\nWHERE raised > now() - INTERVAL '30 days' AND amount > 1000000"
    (tmp_path / "shared.bcql").write_text(shared)
    report = tmp_path / "report.bcql"
    report.write_text(text)
    from_files = buttercupql.compile(report.read_text(), path=str(report))
    assert from_files.sql == from_texts.sql
    with pytest.raises(buttercupql.CompileError) as caught:
        buttercupql.compile("from shared import recnt\nfrom cases\n", modules={"shared": shared})
    assert caught.value.diagnostics[0].label == "did you mean `recent`?"


def test_check_finds_every_problem_without_compiling():
    found = buttercupql.check("from cases\nwehre x\nsort by risk\n")
    assert [problem.message for problem in found] == [
        "there is no step `wehre`",
        "`sort` takes its columns straight after it, without `by`",
    ]
    assert buttercupql.check("from cases\nsort -risk\n") == []
    warned = buttercupql.check("from t\nsort a\ngroup by b:\n    n = count()\n")
    assert [problem.severity for problem in warned] == ["warning"]


def test_the_command_comes_with_the_package(capfd):
    from buttercupql._buttercupql import _run

    # The command writes to the process's own output, as a program does.
    assert _run(["--version"]) == 0
    assert capfd.readouterr().out == f"bcql {buttercupql.__version__}\n"
    assert _run(["frobnicate"]) == 2


def test_the_documentation_examples_run():
    import doctest

    assert doctest.testmod(buttercupql).failed == 0
