"""Running ButtercupQL on databases through their own drivers: `run()`, `read_schema()` and the statements of a
compiled text. SQLite is always there; DuckDB, PostgreSQL and ClickHouse are used when their drivers are installed
and, for the servers, when `BCQL_POSTGRES_URL` and `BCQL_CLICKHOUSE_URL` say where they are."""

import os
import sqlite3
import urllib.parse

import pytest

import buttercupql

SCRIPT = """
create table accounts:
    id: str = field(primary_key=True)
    operator: str
    balance: int = 0

from [{"id": "A1", "operator": "Acme"}, {"id": "A2", "operator": "Beta"}]
insert into accounts

from accounts
where id == "A1"
update balance += 100

from accounts
sort id
select id, operator, balance
"""

ROWS = [("A1", "Acme", 100), ("A2", "Beta", 0)]

# A script whose last statement fails: a second row with the same key.
FAILS = """
create table t:
    id: int = field(primary_key=True)

from [{"id": 1}]
insert into t

from [{"id": 1}]
insert into t
"""


def postgres():
    psycopg = pytest.importorskip("psycopg")
    url = os.environ.get("BCQL_POSTGRES_URL")
    if not url:
        pytest.skip("BCQL_POSTGRES_URL names no PostgreSQL database")
    connection = psycopg.connect(url)
    schema = f"bcql_python_{os.getpid()}"
    connection.execute(f"DROP SCHEMA IF EXISTS {schema} CASCADE")
    connection.execute(f"CREATE SCHEMA {schema}")
    connection.execute(f"SET search_path TO {schema}")
    connection.commit()
    return connection, lambda: (connection.rollback(), connection.execute(f"DROP SCHEMA {schema} CASCADE"), connection.commit())


def clickhouse():
    clickhouse_connect = pytest.importorskip("clickhouse_connect")
    url = os.environ.get("BCQL_CLICKHOUSE_URL")
    if not url:
        pytest.skip("BCQL_CLICKHOUSE_URL names no ClickHouse server")
    place = urllib.parse.urlparse(url)
    login = {
        "host": place.hostname,
        "port": place.port or 8123,
        "username": os.environ.get("BCQL_CLICKHOUSE_USER", "default"),
        "password": os.environ.get("BCQL_CLICKHOUSE_PASSWORD", ""),
    }
    database = f"bcql_python_{os.getpid()}"
    admin = clickhouse_connect.get_client(**login)
    admin.command(f"DROP DATABASE IF EXISTS {database}")
    admin.command(f"CREATE DATABASE {database}")
    return clickhouse_connect.get_client(database=database, **login), lambda: admin.command(f"DROP DATABASE {database}")


def duckdb():
    duckdb = pytest.importorskip("duckdb")
    return duckdb.connect(), lambda: None


DATABASES = {
    "sqlite": lambda: (sqlite3.connect(":memory:"), lambda: None),
    "duckdb": duckdb,
    "postgres": postgres,
    "clickhouse": clickhouse,
}


@pytest.fixture(params=list(DATABASES))
def database(request):
    connection, drop = DATABASES[request.param]()
    yield request.param, connection
    drop()


def test_a_script_runs_on_every_database(database):
    name, connection = database
    rows = buttercupql.run(connection, SCRIPT)
    assert rows == ROWS, name
    assert rows.columns == ["id", "operator", "balance"]


def test_the_tables_are_read_from_every_database(database):
    name, connection = database
    buttercupql.run(connection, SCRIPT)
    schema = buttercupql.read_schema(connection)
    assert "accounts" in schema.tables, name
    with pytest.raises(buttercupql.CompileError) as raised:
        buttercupql.compile("from accounts\nsort -balanse\n", dialect=name, schema=schema)
    assert raised.value.diagnostics[0].label == "did you mean `balance`?"


def test_a_percent_sign_in_the_sql_reaches_the_database(database):
    name, connection = database
    buttercupql.run(connection, SCRIPT)
    # `%` is Python's remainder; with psycopg's `%s` placeholders, it is written `%%` in the SQL.
    rows = buttercupql.run(connection, "from accounts\nwhere balance is not None\nsort id\nselect id, left = balance % 7\n")
    assert rows == [("A1", 2), ("A2", 0)], name


def test_a_script_that_fails_changes_nothing(database):
    name, connection = database
    if name == "clickhouse":
        pytest.skip("ClickHouse has no transactions, nor keys that refuse a row")
    with pytest.raises(Exception):
        buttercupql.run(connection, FAILS)
    # The table made before the statement that failed is gone with it.
    with pytest.raises(buttercupql.CompileError):
        buttercupql.compile("from t\n", dialect=name, schema=buttercupql.read_schema(connection))


def test_a_transaction_block_runs_on_the_databases_that_have_transactions(database):
    name, connection = database
    text = SCRIPT.replace(
        'from accounts\nwhere id == "A1"\nupdate balance += 100\n',
        'with transaction:\n    from accounts\n    where id == "A1"\n    update balance += 100\n',
    )
    if name == "clickhouse":
        with pytest.raises(buttercupql.CompileError, match="ClickHouse runs no transactions"):
            buttercupql.run(connection, text)
    else:
        assert buttercupql.run(connection, text) == ROWS


def test_sqlite_in_autocommit_mode_rolls_a_failed_script_back():
    connection = sqlite3.connect(":memory:", isolation_level=None)
    with pytest.raises(sqlite3.IntegrityError):
        buttercupql.run(connection, FAILS)
    assert connection.execute("SELECT count(*) FROM sqlite_master WHERE name = 't'").fetchone() == (0,)


def test_changes_not_yet_committed_are_run_with_the_script():
    connection = sqlite3.connect(":memory:")
    connection.execute("CREATE TABLE notes (n INTEGER)")
    connection.execute("INSERT INTO notes VALUES (1)")
    assert connection.in_transaction
    assert buttercupql.run(connection, "from notes\nsummarize:\n    n = count()\n") == [(1,)]
    assert not connection.in_transaction


def test_a_query_def_runs_with_its_arguments():
    connection = sqlite3.connect(":memory:")
    connection.executescript(
        "CREATE TABLE cases (key TEXT, account_id TEXT, risk INTEGER);"
        "INSERT INTO cases VALUES ('C1', 'A1', 95), ('C2', 'A1', 40), ('C3', 'A2', 90);"
    )
    text = (
        "def risky_for(account: str, at_least: int = 50):\n"
        "    from cases\n"
        "    where account_id == account and risk >= at_least\n"
        "    select key, risk\n"
    )
    assert buttercupql.run(connection, text, query="risky_for", account="A1") == [("C1", 95)]
    assert buttercupql.run(connection, text, query="risky_for", account="A1", at_least=10) == [
        ("C1", 95),
        ("C2", 40),
    ]
    with pytest.raises(TypeError, match="no query is named: account"):
        buttercupql.run(connection, "from cases\n", account="A1")


def test_a_text_that_gives_no_rows_gives_none():
    connection = sqlite3.connect(":memory:")
    assert buttercupql.run(connection, "create table t:\n    n: int\n") is None
    assert connection.execute("SELECT count(*) FROM t").fetchone() == (0,)


def test_a_driver_it_does_not_know_needs_its_database():
    class Connection:
        pass

    with pytest.raises(ValueError, match="say it with dialect="):
        buttercupql.run(Connection(), "from t\n")


def test_each_statement_is_given_to_run_in_order():
    compiled = buttercupql.compile(SCRIPT, dialect="sqlite")
    statements = compiled.statements
    assert [statement.kind for statement in statements] == ["definition", "change", "change", "query"]
    update = statements[2]
    assert update.sql == "UPDATE accounts SET balance = balance + 100\nWHERE id = ?"
    assert update.params == ["A1"]
    assert update.names == ["p1"]
    assert str(update) == update.sql
    assert repr(update).startswith("<Statement change ")
    one = buttercupql.compile("def f(a: int):\n    from t\n    where n == a\n", query="f").statements
    with pytest.raises(ValueError, match="the statement needs a value for `a`"):
        one[0].params


def test_rows_show_as_a_table():
    rows = buttercupql.Rows([("<b>", None)], ["text", "missing"])
    assert rows == [("<b>", None)]
    shown = rows._repr_html_()
    assert "<th>text</th>" in shown and "<td>&lt;b&gt;</td>" in shown and "<td>None</td>" in shown


def test_rows_become_a_dataframe():
    pytest.importorskip("pandas")
    frame = buttercupql.Rows([(1, "a"), (2, "b")], ["n", "s"]).to_pandas()
    assert list(frame.columns) == ["n", "s"]
    assert frame["n"].tolist() == [1, 2]


def test_sqlite_tables_are_read_with_their_links():
    connection = sqlite3.connect(":memory:")
    connection.executescript(
        'CREATE TABLE accounts (id TEXT PRIMARY KEY, operator TEXT NOT NULL);'
        'CREATE TABLE cases (id INTEGER PRIMARY KEY, "the key" TEXT, account_id TEXT REFERENCES accounts, '
        "risk INTEGER, tags BLOB);"
    )
    schema = buttercupql.read_schema(connection)
    assert schema.tables == ["accounts", "cases"]
    query = buttercupql.compile("from cases\nselect id, operator = account.operator\n", dialect="sqlite", schema=schema)
    assert query.sql == (
        "SELECT cases.id, account.operator\nFROM cases\nLEFT JOIN accounts AS account ON cases.account_id = account.id"
    )
    with pytest.raises(ValueError, match="can't read the tables"):
        buttercupql.read_schema(object())
