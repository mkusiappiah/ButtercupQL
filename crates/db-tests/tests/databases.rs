//! The SQL ButtercupQL writes, run on real databases.
//!
//! Each query must give the rows written beside it, which are the rows Python's meaning gives, on every database,
//! with and without a schema. SQLite runs in memory every time. The others run when they are given:
//!
//! - PostgreSQL, when `BCQL_POSTGRES_URL` names a database to use;
//! - DuckDB, when `BCQL_DUCKDB_PYTHON` names a Python that has the `duckdb` package;
//! - ClickHouse, when `BCQL_CLICKHOUSE_URL` names its HTTP interface, with `BCQL_CLICKHOUSE_USER` and
//!   `BCQL_CLICKHOUSE_PASSWORD` if it needs them.

use std::fmt;
use std::io::Write;
use std::process::{Command, Stdio};

use buttercupql::{Dialect, Options, Schema, Value};

/// A value read back from a database, in a form both databases give alike.
#[derive(Debug, Clone)]
enum Cell {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
}

impl PartialEq for Cell {
    fn eq(&self, other: &Cell) -> bool {
        let number = |cell: &Cell| match cell {
            Cell::Int(n) => Some(*n as f64),
            Cell::Float(f) => Some(*f),
            _ => None,
        };
        match (self, other) {
            (Cell::Null, Cell::Null) => true,
            (Cell::Text(a), Cell::Text(b)) => a == b,
            _ => match (number(self), number(other)) {
                (Some(a), Some(b)) => (a - b).abs() <= 1e-9 * a.abs().max(1.0),
                _ => false,
            },
        }
    }
}

impl fmt::Display for Cell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Cell::Null => f.write_str("None"),
            Cell::Int(n) => write!(f, "{n}"),
            Cell::Float(x) => write!(f, "{x}"),
            Cell::Text(text) => write!(f, "{text:?}"),
        }
    }
}

fn i(n: i64) -> Cell {
    Cell::Int(n)
}

fn x(n: f64) -> Cell {
    Cell::Float(n)
}

fn t(text: &str) -> Cell {
    Cell::Text(text.to_string())
}

const NONE: Cell = Cell::Null;

/// One column of rows.
fn column(cells: &[Cell]) -> Vec<Vec<Cell>> {
    cells.iter().map(|cell| vec![cell.clone()]).collect()
}

/// The rows of the tables, the same on every database: `raised` and `time` are a number of days ago.
const CASES: [&str; 6] = [
    "1, 'C1', 1, 'open', 'critical', 95, 1, NULL, '50% off', 'A1', 'Acme'",
    "2, 'C2', 1, 'open', 'high', 90, 2, 'ana', 'a_b', 'A1', 'Acme'",
    "3, 'C3', 2, 'open', 'low', 40, 3, NULL, 'A_B', 'A2', 'Beta'",
    "4, 'C4', 2, 'closed', 'critical', NULL, 4, 'ben', NULL, 'A2', 'Beta'",
    "5, 'C5', 3, 'open', NULL, -5, 10, '', 'plain', 'A3', 'acme'",
    "6, 'C6', NULL, 'open', 'high', 70, 20, NULL, '5000 off', NULL, NULL",
];

const TRANSACTIONS: [&str; 5] = [
    "1, 1, 'Acme', 2000000, 1",
    "2, 1, 'Acme', 500, 2",
    "3, 2, 'Beta', 1500000, 3",
    "4, 2, 'Beta', 300, 40",
    "5, 3, 'Gamma', 100, 5",
];

/// The SQL that makes the tables, as statements, for a database.
fn fixture(dialect: Dialect) -> Vec<String> {
    let ago = |days: i64| match dialect {
        Dialect::Sqlite => format!("datetime('now', '-{days} days')"),
        Dialect::ClickHouse => format!("now() - INTERVAL {days} DAY"),
        Dialect::Postgres | Dialect::DuckDb => format!("now() - interval '{days} days'"),
    };
    let mut sql: Vec<String> = match dialect {
        Dialect::ClickHouse => vec![
            "CREATE TABLE accounts (id Int64, operator String, balance Nullable(Decimal(12, 2))) ENGINE = MergeTree ORDER BY id".to_string(),
            "CREATE TABLE cases (id Int64, key String, account_id Nullable(Int64), status String, band Nullable(String), risk Nullable(Int64), raised DateTime, assignee Nullable(String), notes Nullable(String), account Nullable(String), operator Nullable(String)) ENGINE = MergeTree ORDER BY id".to_string(),
            "CREATE TABLE transactions (id Int64, account_id Nullable(Int64), operator String, amount Int64, time DateTime) ENGINE = MergeTree ORDER BY id".to_string(),
        ],
        _ => {
            let moment = if dialect == Dialect::Sqlite { "TEXT" } else { "TIMESTAMP" };
            vec![
                "CREATE TABLE accounts (id INTEGER PRIMARY KEY, operator TEXT NOT NULL, balance NUMERIC)".to_string(),
                format!(
                    "CREATE TABLE cases (id INTEGER PRIMARY KEY, key TEXT NOT NULL, account_id INTEGER REFERENCES accounts (id), status TEXT NOT NULL, band TEXT, risk INTEGER, raised {moment} NOT NULL, assignee TEXT, notes TEXT, account TEXT, operator TEXT)"
                ),
                format!(
                    "CREATE TABLE transactions (id INTEGER PRIMARY KEY, account_id INTEGER REFERENCES accounts (id), operator TEXT NOT NULL, amount INTEGER NOT NULL, time {moment} NOT NULL)"
                ),
            ]
        }
    };
    sql.push(
        "INSERT INTO accounts VALUES (1, 'Acme', 120.50), (2, 'Beta', NULL), (3, 'acme', 0)"
            .to_string(),
    );
    let rows = |table: &str, rows: &[&str], when: usize| {
        let values: Vec<String> = rows
            .iter()
            .map(|row| {
                let mut fields: Vec<String> = row.split(", ").map(str::to_string).collect();
                fields[when] = ago(fields[when].parse().expect("days ago"));
                format!("({})", fields.join(", "))
            })
            .collect();
        format!("INSERT INTO {table} VALUES {}", values.join(", "))
    };
    sql.push(rows("cases", &CASES, 6));
    sql.push(rows("transactions", &TRANSACTIONS, 4));
    sql
}

/// The schema of the tables, as PostgreSQL's statements say it.
fn schema() -> Schema {
    Schema::from_sql(&fixture(Dialect::Postgres).join(";\n")).expect("the fixture's schema reads")
}

/// A query, and the rows it must give.
struct Case {
    name: &'static str,
    query: &'static str,
    rows: Vec<Vec<Cell>>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the design's first example",
            query: "\
from cases
where status == \"open\" and band in [\"critical\", \"high\"]
where raised > now() - days(7)
sort -risk
take 20
select key, risk",
            rows: vec![vec![t("C1"), i(95)], vec![t("C2"), i(90)]],
        },
        Case {
            name: "missing values sort last, high to low",
            query: "from cases\nsort -risk, key\nselect key, risk",
            rows: vec![
                vec![t("C1"), i(95)],
                vec![t("C2"), i(90)],
                vec![t("C6"), i(70)],
                vec![t("C3"), i(40)],
                vec![t("C5"), i(-5)],
                vec![t("C4"), NONE],
            ],
        },
        Case {
            name: "missing values sort last, low to high",
            query: "from cases\nsort risk, key\nselect key",
            rows: column(&[t("C5"), t("C3"), t("C6"), t("C2"), t("C1"), t("C4")]),
        },
        Case {
            name: "a sort before a select that gives a column the name of one it reads",
            query: "from cases\nwhere risk is not None\nsort risk\nselect key, risk = -risk",
            rows: vec![
                vec![t("C5"), i(5)],
                vec![t("C3"), i(-40)],
                vec![t("C6"), i(-70)],
                vec![t("C2"), i(-90)],
                vec![t("C1"), i(-95)],
            ],
        },
        Case {
            name: "a where before a select that gives a column the name of one it reads",
            query: "from cases\nwhere risk > 50\nselect key, risk = risk - 40\nsort key",
            rows: vec![
                vec![t("C1"), i(55)],
                vec![t("C2"), i(50)],
                vec![t("C6"), i(30)],
            ],
        },
        Case {
            name: "a select's columns read the columns before it, not each other",
            query: "from cases\nwhere risk > 50\nselect key, risk = risk * 2, before = risk\nsort key",
            rows: vec![
                vec![t("C1"), i(190), i(95)],
                vec![t("C2"), i(180), i(90)],
                vec![t("C6"), i(140), i(70)],
            ],
        },
        Case {
            name: "a total named as the column it totals, and a condition on it",
            query: "from cases\ngroup by band:\n    risk = max(risk)\nwhere risk > 50\nsort band\nselect band, risk",
            rows: vec![vec![t("critical"), i(95)], vec![t("high"), i(90)]],
        },
        Case {
            name: "not in a list that holds None",
            query: "from cases\nwhere band not in [\"low\", None]\nsort key\nselect key",
            rows: column(&[t("C1"), t("C2"), t("C4"), t("C6")]),
        },
        Case {
            name: "in a list that holds None",
            query: "from cases\nwhere band in [\"low\", None]\nsort key\nselect key",
            rows: column(&[t("C3"), t("C5")]),
        },
        Case {
            name: "== None",
            query: "from cases\nwhere assignee == None\nsort key\nselect key",
            rows: column(&[t("C1"), t("C3"), t("C6")]),
        },
        Case {
            name: "a % sign in text is a % sign",
            query: "from cases\nwhere \"%\" in notes\nselect key",
            rows: column(&[t("C1")]),
        },
        Case {
            name: "startswith takes its text as it is",
            query: "from cases\nwhere notes.startswith(\"50%\")\nselect key",
            rows: column(&[t("C1")]),
        },
        Case {
            name: "startswith minds case and underscores",
            query: "from cases\nwhere notes.startswith(\"a_\")\nselect key",
            rows: column(&[t("C2")]),
        },
        Case {
            name: "endswith minds case",
            query: "from cases\nwhere notes.endswith(\"_B\")\nselect key",
            rows: column(&[t("C3")]),
        },
        Case {
            name: "text in text minds case",
            query: "from accounts\nwhere \"Ac\" in operator\nselect id",
            rows: column(&[i(1)]),
        },
        Case {
            name: "lower()",
            query: "from accounts\nwhere operator.lower() == \"acme\"\nsort id\nselect id",
            rows: column(&[i(1), i(3)]),
        },
        Case {
            name: "Python's arithmetic",
            query: "\
from cases
where risk is not None
sort key
select key, half = risk / 2, whole = risk // 2, rest = risk % 3",
            rows: vec![
                vec![t("C1"), x(47.5), i(47), i(2)],
                vec![t("C2"), x(45.0), i(45), i(0)],
                vec![t("C3"), x(20.0), i(20), i(1)],
                vec![t("C5"), x(-2.5), i(-3), i(1)],
                vec![t("C6"), x(35.0), i(35), i(1)],
            ],
        },
        Case {
            name: "between",
            query: "from cases\nwhere 40 <= risk <= 90\nsort key\nselect key",
            rows: column(&[t("C2"), t("C3"), t("C6")]),
        },
        Case {
            name: "a value that depends on a condition",
            query: "from cases\nsort key\nselect key, level = \"urgent\" if risk >= 90 else \"normal\"",
            rows: vec![
                vec![t("C1"), t("urgent")],
                vec![t("C2"), t("urgent")],
                vec![t("C3"), t("normal")],
                vec![t("C4"), t("normal")],
                vec![t("C5"), t("normal")],
                vec![t("C6"), t("normal")],
            ],
        },
        Case {
            name: "f-strings",
            query: "from cases\nwhere risk is not None\nsort key\ntake 2\nselect label = f\"{key}:{risk}\"",
            rows: column(&[t("C1:95"), t("C2:90")]),
        },
        Case {
            name: "grouping with totals and a condition on them",
            query: "\
from transactions
where time > now() - days(30)
group by operator:
    total = count()
    money = sum(amount)
    big = count(where=amount > 1_000_000)
where money > 1_000
sort -money",
            rows: vec![
                vec![t("Acme"), i(2), i(2_000_500), i(1)],
                vec![t("Beta"), i(1), i(1_500_000), i(1)],
            ],
        },
        Case {
            name: "grouping twice",
            query: "from cases\ngroup by account_id:\n    n = count()\nwhere n > 1\nsummarize:\n    accounts = count()",
            rows: column(&[i(2)]),
        },
        Case {
            name: "totals of no rows",
            query: "\
from transactions
where amount > 1_000_000_000
summarize:
    total = sum(amount)
    n = count()
    biggest = max(amount)",
            rows: vec![vec![i(0), i(0), NONE]],
        },
        Case {
            name: "the mean",
            query: "from transactions\nsummarize:\n    average = mean(amount)",
            rows: column(&[x(700_180.0)]),
        },
        Case {
            name: "join",
            query: "\
from cases
join accounts on account_id == accounts.id
where accounts.operator == \"Acme\"
sort key
select key, operator = accounts.operator",
            rows: vec![vec![t("C1"), t("Acme")], vec![t("C2"), t("Acme")]],
        },
        Case {
            name: "left join",
            query: "from cases\nleft join accounts on account_id == accounts.id\nwhere accounts.id is None\nselect key",
            rows: column(&[t("C6")]),
        },
        Case {
            name: "a named query and a def",
            query: "\
def recent(days_back=7):
    where raised > now() - days(days_back)

open_cases = (
    from cases
    where status == \"open\"
)

from open_cases
recent(5)
sort key
select key",
            rows: column(&[t("C1"), t("C2"), t("C3")]),
        },
        Case {
            name: "where after take",
            query: "from cases\nsort -risk, key\ntake 3\nwhere band == \"high\"\nselect key",
            rows: column(&[t("C2"), t("C6")]),
        },
        Case {
            name: "skip without take",
            query: "from cases\nsort key\nskip 4\nselect key",
            rows: column(&[t("C5"), t("C6")]),
        },
        Case {
            name: "distinct",
            query: "from cases\nselect status\ndistinct\nsort status",
            rows: column(&[t("closed"), t("open")]),
        },
        Case {
            name: "parts of dates",
            query: "from cases\nwhere raised.year >= 2000 and raised.month >= 1\nsummarize:\n    n = count()",
            rows: column(&[i(6)]),
        },
        Case {
            name: "a query in brackets after in",
            query: "\
from cases
where account_id in (from accounts; where operator == \"Beta\"; select id)
sort key
select key",
            rows: column(&[t("C3"), t("C4")]),
        },
        Case {
            name: "the first in each group, by a window",
            query: "from cases\nplace = rank(over=operator, sort=-risk)\nwhere place == 1\nsort key\nselect key",
            rows: column(&[t("C1"), t("C3"), t("C5"), t("C6")]),
        },
        Case {
            name: "a running total",
            query: "from transactions\nso_far = sum(amount, sort=id)\nsort id\nselect id, so_far",
            rows: vec![
                vec![i(1), i(2_000_000)],
                vec![i(2), i(2_000_500)],
                vec![i(3), i(3_500_500)],
                vec![i(4), i(3_500_800)],
                vec![i(5), i(3_500_900)],
            ],
        },
        Case {
            name: "the row before, and a moving mean",
            query: "from transactions\nbefore = lag(amount, sort=id)\npair = mean(amount, sort=id, rows=2)\nsort id\nselect id, before, pair",
            rows: vec![
                vec![i(1), NONE, x(2_000_000.0)],
                vec![i(2), i(2_000_000), x(1_000_250.0)],
                vec![i(3), i(500), x(750_250.0)],
                vec![i(4), i(1_500_000), x(750_150.0)],
                vec![i(5), i(300), x(200.0)],
            ],
        },
        Case {
            name: "a share of the group's total, on every row",
            query: "from transactions\nwhere operator == \"Acme\"\nshare = amount / sum(amount, over=operator)\nsort id\nselect id, share",
            rows: vec![
                vec![i(1), x(2_000_000.0 / 2_000_500.0)],
                vec![i(2), x(500.0 / 2_000_500.0)],
            ],
        },
        Case {
            name: "a window counts the rows kept before it",
            query: "from cases\nsort -risk, key\ntake 3\nplace = row_number(sort=key)\nsort key\nselect key, place",
            rows: vec![
                vec![t("C1"), i(1)],
                vec![t("C2"), i(2)],
                vec![t("C6"), i(3)],
            ],
        },
    ]
}

/// The cases that need a schema, which gives the kinds of the values.
fn schema_cases() -> Vec<Case> {
    vec![
        Case {
            name: "Python's truth: text that is not empty",
            query: "from cases\nwhere assignee\nsort key\nselect key",
            rows: column(&[t("C2"), t("C4")]),
        },
        Case {
            name: "a link, followed with a dot",
            query: "from cases\nwhere account.operator == \"Acme\"\nsort key\nselect key, operator = account.operator",
            rows: vec![vec![t("C1"), t("Acme")], vec![t("C2"), t("Acme")]],
        },
        Case {
            name: "a link that leads nowhere is missing",
            query: "from cases\nwhere account.id is None\nselect key",
            rows: column(&[t("C6")]),
        },
        Case {
            name: "grouping by a link's column",
            // Sorted by count first: how text with capitals sorts is the database's collation's to say.
            query: "from cases\ngroup by operator = account.operator:\n    n = count()\nsort -n, operator\nselect operator, n",
            rows: vec![
                vec![t("Acme"), i(2)],
                vec![t("Beta"), i(2)],
                vec![t("acme"), i(1)],
                vec![NONE, i(1)],
            ],
        },
    ]
}

/// A query `def` compiled with its arguments open, the arguments to give it, and the rows it must give.
struct Prepared {
    name: &'static str,
    text: &'static str,
    query: &'static str,
    args: Vec<(&'static str, Value)>,
    rows: Vec<Vec<Cell>>,
}

fn prepared() -> Vec<Prepared> {
    let text = "\
def by_band(band_wanted: str, rows: int = 10):
    from cases
    where band == band_wanted
    sort key
    take rows
    select key
";
    vec![
        Prepared {
            name: "a query's arguments, given when it runs",
            text,
            query: "by_band",
            args: vec![("band_wanted", Value::Text("high".to_string()))],
            rows: column(&[t("C2"), t("C6")]),
        },
        Prepared {
            name: "a query's argument that limits its rows",
            text,
            query: "by_band",
            args: vec![
                ("band_wanted", Value::Text("critical".to_string())),
                ("rows", Value::Int(1)),
            ],
            rows: column(&[t("C1")]),
        },
    ]
}

// ---- Running SQL

fn sqlite() -> rusqlite::Connection {
    let connection = rusqlite::Connection::open_in_memory().expect("an SQLite database in memory");
    connection
        .execute_batch(&(fixture(Dialect::Sqlite).join(";\n") + ";"))
        .expect("the tables are created in SQLite");
    connection
}

fn run_sqlite(
    connection: &rusqlite::Connection,
    sql: &str,
    params: &[Value],
) -> Result<Vec<Vec<Cell>>, String> {
    let mut statement = connection.prepare(sql).map_err(|error| error.to_string())?;
    let columns = statement.column_count();
    let params = params.iter().map(|param| match param {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(value) => rusqlite::types::Value::Integer(i64::from(*value)),
        Value::Int(value) => rusqlite::types::Value::Integer(*value),
        Value::Float(value) => rusqlite::types::Value::Real(*value),
        Value::Text(text) => rusqlite::types::Value::Text(text.clone()),
    });
    let rows = statement
        .query_map(rusqlite::params_from_iter(params), |row| {
            (0..columns)
                .map(|n| {
                    Ok(match row.get_ref(n)? {
                        rusqlite::types::ValueRef::Null => Cell::Null,
                        rusqlite::types::ValueRef::Integer(value) => Cell::Int(value),
                        rusqlite::types::ValueRef::Real(value) => Cell::Float(value),
                        rusqlite::types::ValueRef::Text(text) => {
                            Cell::Text(String::from_utf8_lossy(text).into_owned())
                        }
                        rusqlite::types::ValueRef::Blob(bytes) => Cell::Text(format!("{bytes:?}")),
                    })
                })
                .collect::<rusqlite::Result<Vec<Cell>>>()
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())
}

/// A PostgreSQL connection with the tables made in a schema of its own, when `BCQL_POSTGRES_URL` names a database.
fn postgres(test: &str) -> Option<postgres::Client> {
    let url = std::env::var("BCQL_POSTGRES_URL").ok()?;
    let mut client =
        postgres::Client::connect(&url, postgres::NoTls).expect("PostgreSQL can be reached");
    let schema = format!("bcql_{test}_{}", std::process::id());
    client
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema};\n{};",
            fixture(Dialect::Postgres).join(";\n")
        ))
        .expect("the tables are created in PostgreSQL");
    Some(client)
}

/// A JSON value read back from a database, as a cell.
fn json_cell(value: serde_json::Value) -> Cell {
    match value {
        serde_json::Value::Null => Cell::Null,
        serde_json::Value::Bool(value) => Cell::Int(i64::from(value)),
        serde_json::Value::Number(number) => match number.as_i64() {
            Some(n) => Cell::Int(n),
            None => Cell::Float(number.as_f64().unwrap_or(f64::NAN)),
        },
        serde_json::Value::String(text) => Cell::Text(text),
        other => Cell::Text(other.to_string()),
    }
}

fn run_postgres(
    client: &mut postgres::Client,
    sql: &str,
    params: &[Value],
) -> Result<Vec<Vec<Cell>>, String> {
    // Each row comes back as JSON, so that every kind of value reads the same way.
    let wrapped = format!("SELECT row_to_json(_r)::text FROM (\n{sql}\n) AS _r");
    run_postgres_json(client, &wrapped, params)
}

/// Runs SQL that gives each row as one JSON text, and reads the rows.
fn run_postgres_json(
    client: &mut postgres::Client,
    wrapped: &str,
    params: &[Value],
) -> Result<Vec<Vec<Cell>>, String> {
    let boxed: Vec<Box<dyn postgres::types::ToSql + Sync>> = params
        .iter()
        .map(|param| -> Box<dyn postgres::types::ToSql + Sync> {
            match param {
                Value::Text(text) => Box::new(text.clone()),
                Value::Int(value) => Box::new(*value),
                Value::Float(value) => Box::new(*value),
                Value::Bool(value) => Box::new(*value),
                Value::Null => Box::new(Option::<String>::None),
            }
        })
        .collect();
    let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
        boxed.iter().map(|param| param.as_ref()).collect();
    let rows = client.query(wrapped, &refs).map_err(|error| {
        let detail = error
            .as_db_error()
            .map_or(String::new(), |db| format!(": {}", db.message()));
        format!("{error}{detail}")
    })?;
    let mut out = Vec::new();
    for row in rows {
        let json: String = row.get(0);
        let object: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&json).map_err(|error| error.to_string())?;
        out.push(object.into_values().map(json_cell).collect());
    }
    Ok(out)
}

/// ClickHouse's HTTP interface, with a database of its own made with the tables, when `BCQL_CLICKHOUSE_URL` names
/// one.
struct ClickHouse {
    url: String,
    database: String,
    agent: ureq::Agent,
}

impl ClickHouse {
    fn connect(test: &str) -> Option<ClickHouse> {
        let url = std::env::var("BCQL_CLICKHOUSE_URL").ok()?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let clickhouse = ClickHouse {
            url,
            database: format!("bcql_{test}_{}", std::process::id()),
            agent,
        };
        clickhouse
            .send(
                &format!("DROP DATABASE IF EXISTS {}", clickhouse.database),
                &[],
                false,
            )
            .expect("ClickHouse can be reached");
        clickhouse
            .send(
                &format!("CREATE DATABASE {}", clickhouse.database),
                &[],
                false,
            )
            .expect("a database is made in ClickHouse");
        for statement in fixture(Dialect::ClickHouse) {
            clickhouse
                .send(&statement, &[], true)
                .expect("the tables are created in ClickHouse");
        }
        Some(clickhouse)
    }

    /// Sends a statement, with its parameters by name, and returns ClickHouse's answer.
    fn send(
        &self,
        sql: &str,
        params: &[(String, Value)],
        in_database: bool,
    ) -> Result<String, String> {
        let mut request = self.agent.post(&self.url);
        if let Ok(user) = std::env::var("BCQL_CLICKHOUSE_USER") {
            request = request.header("X-ClickHouse-User", &user);
        }
        if let Ok(password) = std::env::var("BCQL_CLICKHOUSE_PASSWORD") {
            request = request.header("X-ClickHouse-Key", &password);
        }
        if in_database {
            request = request.query("database", &self.database);
        }
        // Whole numbers come back as numbers, not as text.
        request = request.query("output_format_json_quote_64bit_integers", "0");
        for (name, value) in params {
            let text = match value {
                Value::Null => "\\N".to_string(),
                Value::Bool(value) => value.to_string(),
                Value::Int(value) => value.to_string(),
                Value::Float(value) => value.to_string(),
                Value::Text(text) => text.clone(),
            };
            request = request.query(format!("param_{name}"), text);
        }
        let mut response = request.send(sql).map_err(|error| error.to_string())?;
        let status = response.status();
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|error| error.to_string())?;
        if status.is_success() {
            Ok(body)
        } else {
            Err(format!("{status}: {}", body.trim()))
        }
    }

    fn run(&self, sql: &str, params: &[(String, Value)]) -> Result<Vec<Vec<Cell>>, String> {
        let query = format!("{sql}\nFORMAT JSONCompact");
        let body = self.send(&query, params, true)?;
        let answer: serde_json::Value =
            serde_json::from_str(&body).map_err(|error| format!("{error}: {body}"))?;
        let data = answer["data"].as_array().cloned().unwrap_or_default();
        Ok(data
            .into_iter()
            .map(|row| {
                row.as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(json_cell)
                    .collect()
            })
            .collect())
    }
}

/// DuckDB, through the `duckdb` package of the Python that `BCQL_DUCKDB_PYTHON` names. Its queries are run
/// together, after they are all compiled.
struct DuckDb {
    python: String,
    queries: Vec<Queued>,
}

/// A query kept for DuckDB: what it is, its SQL and parameters, and the rows it must give.
struct Queued {
    label: String,
    sql: String,
    params: Vec<Value>,
    wanted: Vec<Vec<Cell>>,
}

impl DuckDb {
    fn connect() -> Option<DuckDb> {
        let python = std::env::var("BCQL_DUCKDB_PYTHON").ok()?;
        Some(DuckDb {
            python,
            queries: Vec::new(),
        })
    }

    /// Runs the queries kept, and returns what went wrong.
    fn run(&self) -> Vec<String> {
        let to_json = |value: &Value| match value {
            Value::Null => serde_json::Value::Null,
            Value::Bool(value) => serde_json::Value::Bool(*value),
            Value::Int(value) => serde_json::Value::from(*value),
            Value::Float(value) => serde_json::Value::from(*value),
            Value::Text(text) => serde_json::Value::String(text.clone()),
        };
        let job = serde_json::json!({
            "setup": fixture(Dialect::DuckDb).join(";\n") + ";",
            "queries": self.queries.iter().map(|query| serde_json::json!({
                "sql": query.sql,
                "params": query.params.iter().map(to_json).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });
        let runner =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/duckdb_runner.py");
        let mut child = Command::new(&self.python)
            .arg(runner)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the Python for DuckDB starts");
        child
            .stdin
            .take()
            .expect("its input")
            .write_all(job.to_string().as_bytes())
            .expect("the queries are sent to DuckDB");
        let out = child.wait_with_output().expect("DuckDB's queries finish");
        if !out.status.success() {
            return vec![format!(
                "DuckDB's runner failed: {}",
                String::from_utf8_lossy(&out.stderr)
            )];
        }
        let results: Vec<serde_json::Value> =
            serde_json::from_slice(&out.stdout).expect("DuckDB's answer is JSON");
        let mut failures = Vec::new();
        for (
            Queued {
                label, sql, wanted, ..
            },
            result,
        ) in self.queries.iter().zip(results)
        {
            if let Some(error) = result.get("error").and_then(serde_json::Value::as_str) {
                failures.push(format!("{label}: the database refused\n{sql}\n{error}"));
                continue;
            }
            let rows: Vec<Vec<Cell>> = result["rows"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|row| {
                    row.as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .map(json_cell)
                        .collect()
                })
                .collect();
            if rows != *wanted {
                failures.push(format!(
                    "{label}: wrong rows\n{sql}\ngot:\n{}\nwanted:\n{}",
                    shown(&rows),
                    shown(wanted)
                ));
            }
        }
        failures
    }
}

fn shown(rows: &[Vec<Cell>]) -> String {
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = row.iter().map(ToString::to_string).collect();
            format!("  ({})", cells.join(", "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The databases a test runs on.
struct Databases {
    lite: rusqlite::Connection,
    pg: Option<postgres::Client>,
    duck: Option<DuckDb>,
    click: Option<ClickHouse>,
}

impl Databases {
    fn connect(test: &str) -> Databases {
        Databases {
            lite: sqlite(),
            pg: postgres(test),
            duck: DuckDb::connect(),
            click: ClickHouse::connect(test),
        }
    }

    /// Compiles a text for each database, runs it, and returns what went wrong. DuckDB's queries run in
    /// [`Databases::finish`].
    fn check(
        &mut self,
        label: &str,
        text: &str,
        options: Options<'_>,
        args: &[(&str, Value)],
        wanted: &[Vec<Cell>],
        failures: &mut Vec<String>,
    ) {
        for dialect in Dialect::ALL {
            let options = Options { dialect, ..options };
            let label = format!("{label} ({dialect})");
            let compiled = match buttercupql::compile(text, &options) {
                Ok(compiled) => compiled,
                Err(problems) => {
                    let messages: Vec<String> = problems.iter().map(ToString::to_string).collect();
                    failures.push(format!(
                        "{label}: does not compile: {}",
                        messages.join("; ")
                    ));
                    continue;
                }
            };
            let values = match compiled.bind(args) {
                Ok(values) => values,
                Err(error) => {
                    failures.push(format!("{label}: {error}"));
                    continue;
                }
            };
            let result = match dialect {
                Dialect::Sqlite => Some(run_sqlite(&self.lite, &compiled.sql, &values)),
                Dialect::Postgres => self
                    .pg
                    .as_mut()
                    .map(|client| run_postgres(client, &compiled.sql, &values)),
                Dialect::ClickHouse => self.click.as_ref().map(|click| {
                    let named = compiled.bind_named(args).expect("bound once already");
                    click.run(&compiled.sql, &named)
                }),
                Dialect::DuckDb => {
                    if let Some(duck) = self.duck.as_mut() {
                        duck.queries.push(Queued {
                            label: label.clone(),
                            sql: compiled.sql.clone(),
                            params: values,
                            wanted: wanted.to_vec(),
                        });
                    }
                    None
                }
            };
            match result {
                None => {}
                Some(Err(error)) => failures.push(format!(
                    "{label}: the database refused\n{}\n{error}",
                    compiled.sql
                )),
                Some(Ok(rows)) if rows != wanted => failures.push(format!(
                    "{label}: wrong rows\n{}\ngot:\n{}\nwanted:\n{}",
                    compiled.sql,
                    shown(&rows),
                    shown(wanted)
                )),
                Some(Ok(_)) => {}
            }
        }
    }

    fn finish(self, failures: &mut Vec<String>) {
        if let Some(duck) = &self.duck {
            failures.extend(duck.run());
        }
        let missing: Vec<&str> = [
            ("PostgreSQL", self.pg.is_none()),
            ("DuckDB", self.duck.is_none()),
            ("ClickHouse", self.click.is_none()),
        ]
        .into_iter()
        .filter(|(_, missing)| *missing)
        .map(|(name, _)| name)
        .collect();
        if !missing.is_empty() {
            eprintln!(
                "not tested on {}: see this file's first lines to test them",
                missing.join(", ")
            );
        }
    }
}

fn report(failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{} failures:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn every_query_gives_pythons_rows_on_every_database() {
    let schema = schema();
    let mut databases = Databases::connect("every");
    let mut failures = Vec::new();
    for case in cases() {
        let plain = Options::new(Dialect::Sqlite);
        databases.check(
            &format!("{}, without a schema", case.name),
            case.query,
            plain,
            &[],
            &case.rows,
            &mut failures,
        );
        let typed = plain.with_schema(&schema);
        databases.check(
            &format!("{}, with the schema", case.name),
            case.query,
            typed,
            &[],
            &case.rows,
            &mut failures,
        );
    }
    databases.finish(&mut failures);
    report(failures);
}

#[test]
fn the_schema_gives_kinds_of_values_and_links() {
    let schema = schema();
    let mut databases = Databases::connect("schema");
    let mut failures = Vec::new();
    for case in schema_cases() {
        let options = Options::new(Dialect::Sqlite).with_schema(&schema);
        databases.check(
            case.name,
            case.query,
            options,
            &[],
            &case.rows,
            &mut failures,
        );
    }
    databases.finish(&mut failures);
    report(failures);
}

#[test]
fn a_query_runs_with_the_arguments_given() {
    let mut databases = Databases::connect("args");
    let mut failures = Vec::new();
    for case in prepared() {
        let options = Options::new(Dialect::Sqlite).with_query(case.query);
        databases.check(
            case.name,
            case.text,
            options,
            &case.args,
            &case.rows,
            &mut failures,
        );
    }
    databases.finish(&mut failures);
    report(failures);
}

#[test]
fn every_example_runs() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut databases = Databases::connect("examples");
    let mut failures = Vec::new();
    for entry in std::fs::read_dir(folder).expect("the examples folder can be read") {
        let path = entry.expect("an entry").path();
        if path.extension().is_none_or(|extension| extension != "bcql") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("an example can be read");
        let label = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        // An example's rows are not checked here, only that every database runs its SQL: the rows are the cases'
        // work. A wanted value that no answer has makes every answer count as wrong, so it is left empty and
        // wrong rows are ignored below.
        let mut found = Vec::new();
        databases.check(
            &label,
            &text,
            Options::new(Dialect::Sqlite),
            &[],
            &[],
            &mut found,
        );
        failures.extend(
            found
                .into_iter()
                .filter(|failure| !failure.contains(": wrong rows")),
        );
    }
    let mut finished = Vec::new();
    databases.finish(&mut finished);
    failures.extend(
        finished
            .into_iter()
            .filter(|failure| !failure.contains(": wrong rows")),
    );
    report(failures);
}

// ---- Scripts: statements that change the database, run in order

/// A script: statements that make tables, add and change rows, then a last statement whose rows are checked.
struct Script {
    name: &'static str,
    text: String,
    /// The rows the last statement gives, if it gives any.
    rows: Vec<Vec<Cell>>,
    /// The databases that can't do what the script does, and what their error says.
    not_on: Vec<(Dialect, &'static str)>,
    /// SQL run after the script, on one database, and the rows it must give: for what a script keeps in the
    /// database, such as a stored `def`.
    checks: Vec<(Dialect, &'static str, Vec<Vec<Cell>>)>,
    /// SQL run before the script, on one database: the roles a `grant` needs.
    setup: Vec<(Dialect, &'static str)>,
}

/// The accounts most scripts start with.
const ACCOUNTS: &str = "\
create table accounts:
    id: str = field(primary_key=True)
    operator: str
    balance: int = 0

from [
    {\"id\": \"A1\", \"operator\": \"Acme\", \"balance\": 100},
    {\"id\": \"A2\", \"operator\": \"Beta\", \"balance\": 50},
    {\"id\": \"A3\", \"operator\": \"Gamma\", \"balance\": -7},
]
insert into accounts
";

fn script(name: &'static str, text: &str, rows: Vec<Vec<Cell>>) -> Script {
    Script {
        name,
        text: text.to_string(),
        rows,
        not_on: Vec::new(),
        checks: Vec::new(),
        setup: Vec::new(),
    }
}

fn scripts() -> Vec<Script> {
    let with_accounts = |name, rest: &str, rows| script(name, &format!("{ACCOUNTS}\n{rest}"), rows);
    let set_tables = "\
create table a:
    n: int = field(primary_key=True)
create table b:
    n: int = field(primary_key=True)
from [{\"n\": 1}, {\"n\": 2}, {\"n\": 3}]
insert into a
from [{\"n\": 2}, {\"n\": 3}, {\"n\": 4}]
insert into b
";
    let combined = |name, op: &str, rows| {
        script(
            name,
            &format!(
                "{set_tables}\nboth = (from a; select n) {op} (from b; select n)\n\nfrom both\nsort n\n"
            ),
            rows,
        )
    };
    vec![
        Script {
            not_on: vec![
                (Dialect::Sqlite, "SQLite has no users or permissions"),
                (Dialect::DuckDb, "DuckDB has no users or permissions"),
            ],
            setup: vec![
                (
                    Dialect::Postgres,
                    "DO $$ BEGIN CREATE ROLE bcql_reader; EXCEPTION WHEN duplicate_object THEN NULL; END $$",
                ),
                (Dialect::ClickHouse, "CREATE ROLE IF NOT EXISTS bcql_reader"),
            ],
            checks: vec![
                (
                    Dialect::Postgres,
                    "SELECT has_table_privilege('bcql_reader', 'accounts', 'SELECT')::int AS reads, has_table_privilege('bcql_reader', 'accounts', 'INSERT')::int AS writes",
                    vec![vec![i(1), i(0)]],
                ),
                (
                    Dialect::ClickHouse,
                    "SELECT access_type FROM system.grants WHERE role_name = 'bcql_reader' AND database = currentDatabase() AND table = 'accounts' ORDER BY access_type",
                    column(&[t("SELECT")]),
                ),
            ],
            ..with_accounts(
                "a grant lets a role read a table, and a revoke takes writing away",
                "grant read, write on accounts to bcql_reader\nrevoke write on accounts from bcql_reader\n",
                Vec::new(),
            )
        },
        with_accounts(
            "rows written in the query go in, with a column's default",
            "from [{\"id\": \"A4\", \"operator\": \"Delta\"}]\ninsert into accounts\n\nfrom accounts\nsort id\nselect id, operator, balance\n",
            vec![
                vec![t("A1"), t("Acme"), i(100)],
                vec![t("A2"), t("Beta"), i(50)],
                vec![t("A3"), t("Gamma"), i(-7)],
                vec![t("A4"), t("Delta"), i(0)],
            ],
        ),
        with_accounts(
            "an update gives Python's values",
            "from accounts\nwhere balance > 0\nupdate balance = balance // 3, operator = operator.upper()\n\nfrom accounts\nsort id\nselect id, operator, balance\n",
            vec![
                vec![t("A1"), t("ACME"), i(33)],
                vec![t("A2"), t("BETA"), i(16)],
                vec![t("A3"), t("Gamma"), i(-7)],
            ],
        ),
        with_accounts(
            "a delete takes the rows the wheres choose",
            "from accounts\nwhere balance < 60\ndelete\n\nfrom accounts\nselect id\n",
            column(&[t("A1")]),
        ),
        with_accounts(
            "an insert from a query matches columns by name",
            "create table rich:\n    operator: str = field(primary_key=True)\n    id: str\n\nfrom accounts\nwhere balance >= 50\nselect id, operator\ninsert into rich\n\nfrom rich\nsort operator\nselect operator, id\n",
            vec![vec![t("Acme"), t("A1")], vec![t("Beta"), t("A2")]],
        ),
        script(
            "an update through a link changes the rows it leads to",
            "\
create table accounts:
    id: str = field(primary_key=True)
    operator: str

create table cases:
    id: int = field(primary_key=True)
    account_id: str | None = field(references=accounts.id)
    status: str = \"open\"

from [{\"id\": \"A1\", \"operator\": \"Acme\"}, {\"id\": \"A2\", \"operator\": \"Beta\"}]
insert into accounts

from [{\"id\": 1, \"account_id\": \"A1\"}, {\"id\": 2, \"account_id\": \"A2\"}, {\"id\": 3, \"account_id\": \"A1\"}]
insert into cases

from cases
where account.operator == \"Acme\"
update status = \"closed\"

from cases
sort id
select id, status
",
            vec![
                vec![i(1), t("closed")],
                vec![i(2), t("open")],
                vec![i(3), t("closed")],
            ],
        ),
        Script {
            not_on: vec![(Dialect::ClickHouse, "ClickHouse can't give the rows a change makes")],
            ..with_accounts(
                "a select after a change gives the rows changed",
                "from accounts\nwhere id == \"A1\"\nupdate balance = balance + 1\nselect id, balance\n",
                vec![vec![t("A1"), i(101)]],
            )
        },
        Script {
            not_on: vec![(Dialect::ClickHouse, "ClickHouse runs no transactions")],
            ..with_accounts(
                "a transaction moves money between accounts",
                "with transaction:\n    from accounts; where id == \"A1\"; update balance -= 30\n    from accounts; where id == \"A2\"; update balance += 30\n\nfrom accounts\nsort id\nselect id, balance\n",
                vec![
                    vec![t("A1"), i(70)],
                    vec![t("A2"), i(80)],
                    vec![t("A3"), i(-7)],
                ],
            )
        },
        with_accounts(
            "a table is altered: a column added with its default, one renamed and one dropped",
            "alter table accounts:\n    region: str = \"east\"\n    note: str | None\n    rename operator to company\n    del note\n\nfrom accounts\nsort id\nselect id, company, region\n",
            vec![
                vec![t("A1"), t("Acme"), t("east")],
                vec![t("A2"), t("Beta"), t("east")],
                vec![t("A3"), t("Gamma"), t("east")],
            ],
        ),
        script(
            "a table is dropped and made again",
            "\
create table t:
    id: int = field(primary_key=True)
drop table t
drop table t, missing_ok=True
create table t, exist_ok=True:
    id: int = field(primary_key=True)
    name: str
create table t, exist_ok=True:
    id: int = field(primary_key=True)
    name: str
from [{\"id\": 1, \"name\": \"x\"}]
insert into t

from t
select id, name
",
            vec![vec![i(1), t("x")]],
        ),
        Script {
            not_on: vec![(Dialect::ClickHouse, "ClickHouse has no indexes like this")],
            ..with_accounts(
                "indexes are made, and a unique one keeps its rows apart",
                "create index on accounts(-balance)\ncreate unique index on accounts(operator)\n\nfrom accounts\nsort -balance\nselect id\n",
                column(&[t("A1"), t("A2"), t("A3")]),
            )
        },
        Script {
            not_on: vec![(Dialect::ClickHouse, "ClickHouse's recursive queries keep every row")],
            ..script(
            "a recursive query follows transfers round a loop, and ends",
            "\
create table transfers:
    id: int = field(primary_key=True)
    source: str
    target: str

from [
    {\"id\": 1, \"source\": \"A1\", \"target\": \"A2\"},
    {\"id\": 2, \"source\": \"A2\", \"target\": \"A3\"},
    {\"id\": 3, \"source\": \"A3\", \"target\": \"A1\"},
    {\"id\": 4, \"source\": \"B1\", \"target\": \"B2\"},
]
insert into transfers

reached = (
    from [{\"id\": \"A1\"}]
) | (
    from transfers
    join reached on source == reached.id
    select id = target
)

from reached
sort id
",
            column(&[t("A1"), t("A2"), t("A3")]),
            )
        },
        script(
            "a recursive query with `+` follows a chain of transfers",
            "\
create table transfers:
    id: int = field(primary_key=True)
    source: str
    target: str

from [
    {\"id\": 1, \"source\": \"B1\", \"target\": \"B2\"},
    {\"id\": 2, \"source\": \"B2\", \"target\": \"B3\"},
    {\"id\": 3, \"source\": \"C1\", \"target\": \"C2\"},
]
insert into transfers

chain = (
    from [{\"id\": \"B1\", \"depth\": 0}]
) + (
    from transfers
    join chain on source == chain.id
    select id = target, depth = chain.depth + 1
)

from chain
sort id
",
            vec![vec![t("B1"), i(0)], vec![t("B2"), i(1)], vec![t("B3"), i(2)]],
        ),
        combined("`&` keeps the rows in both", "&", column(&[i(2), i(3)])),
        combined("`|` keeps the rows of both, once", "|", column(&[i(1), i(2), i(3), i(4)])),
        combined("`-` keeps the rows of the first that are not in the second", "-", column(&[i(1)])),
        combined(
            "`+` keeps every row of both",
            "+",
            column(&[i(1), i(2), i(2), i(3), i(3), i(4)]),
        ),
        script(
            "a pivot gives a column for each value",
            "\
create table cases:
    id: int = field(primary_key=True)
    operator: str
    band: str

from [
    {\"id\": 1, \"operator\": \"Acme\", \"band\": \"high\"},
    {\"id\": 2, \"operator\": \"Acme\", \"band\": \"low\"},
    {\"id\": 3, \"operator\": \"Beta\", \"band\": \"high\"},
    {\"id\": 4, \"operator\": \"Acme\", \"band\": \"high\"},
]
insert into cases

from cases
pivot band in [\"high\", \"low\"] by operator:
    n = count()
sort operator
",
            vec![vec![t("Acme"), i(2), i(1)], vec![t("Beta"), i(1), i(0)]],
        ),
        script(
            "rows written in the query are read like a table",
            "from [{\"name\": \"b\", \"n\": 2}, {\"name\": \"a\", \"n\": 1}, {\"name\": \"c\", \"n\": 0}]\nwhere n > 0\nsort name\n",
            vec![vec![t("a"), i(1)], vec![t("b"), i(2)]],
        ),
        Script {
            not_on: vec![(Dialect::Sqlite, "SQLite can't keep a query in the database")],
            checks: vec![
                (Dialect::Postgres, "SELECT id FROM rich_accounts(60) ORDER BY id", column(&[t("A1")])),
                (Dialect::DuckDb, "SELECT id FROM rich_accounts(60) ORDER BY id", column(&[t("A1")])),
                (
                    Dialect::ClickHouse,
                    "SELECT id FROM rich_accounts(at_least = 40) ORDER BY id",
                    column(&[t("A1"), t("A2")]),
                ),
            ],
            ..with_accounts(
                "a stored def is kept in the database, and called there",
                "@stored\ndef rich_accounts(at_least: int):\n    from accounts\n    where balance >= at_least\n    select id\n",
                Vec::new(),
            )
        },
        Script {
            not_on: vec![
                (Dialect::Sqlite, "SQLite can't keep a query in the database"),
                (Dialect::DuckDb, "DuckDB can't keep a query that changes rows"),
                (Dialect::ClickHouse, "ClickHouse can't keep a query that changes rows"),
            ],
            checks: vec![(
                Dialect::Postgres,
                "CALL tax(10); SELECT id, balance FROM accounts ORDER BY id",
                vec![vec![t("A1"), i(90)], vec![t("A2"), i(40)], vec![t("A3"), i(-7)]],
            )],
            ..with_accounts(
                "a stored def that changes rows is a procedure on PostgreSQL",
                "@stored\ndef tax(amount: int = 5):\n    from accounts\n    where balance > 0\n    update balance -= amount\n",
                Vec::new(),
            )
        },
    ]
}

/// What a script gives: the last statement's rows, and each check's.
type Ran = Result<(Vec<Vec<Cell>>, Vec<Vec<Vec<Cell>>>), String>;

/// Whether a statement gives rows: a query, or a change with a `select` after it.
fn gives_rows(statement: &buttercupql::Statement) -> bool {
    statement.kind == buttercupql::StatementKind::Query
        || (statement.kind == buttercupql::StatementKind::Change
            && statement.sql.contains("\nRETURNING "))
}

/// Runs a script on SQLite, in a database of its own: the last statement's rows, and the checks'.
fn sqlite_script(statements: &[buttercupql::Statement], checks: &[&str]) -> Ran {
    let connection = rusqlite::Connection::open_in_memory().map_err(|error| error.to_string())?;
    let mut rows = Vec::new();
    for (n, statement) in statements.iter().enumerate() {
        let values = statement.values().map_err(|error| error.to_string())?;
        if n == statements.len() - 1 && gives_rows(statement) {
            rows = run_sqlite(&connection, &statement.sql, &values)?;
        } else {
            let params = values.iter().map(|value| match value {
                Value::Null => rusqlite::types::Value::Null,
                Value::Bool(value) => rusqlite::types::Value::Integer(i64::from(*value)),
                Value::Int(value) => rusqlite::types::Value::Integer(*value),
                Value::Float(value) => rusqlite::types::Value::Real(*value),
                Value::Text(text) => rusqlite::types::Value::Text(text.clone()),
            });
            connection
                .execute(&statement.sql, rusqlite::params_from_iter(params))
                .map_err(|error| format!("{error}\n{}", statement.sql))?;
        }
    }
    let mut checked = Vec::new();
    for check in checks {
        checked.push(run_sqlite(&connection, check, &[])?);
    }
    Ok((rows, checked))
}

/// Runs a script on PostgreSQL, in a schema of its own.
fn postgres_script(
    client: &mut postgres::Client,
    schema: &str,
    setup: &[&str],
    statements: &[buttercupql::Statement],
    checks: &[&str],
) -> Ran {
    let refused = |error: postgres::Error| {
        let detail = error
            .as_db_error()
            .map_or(String::new(), |db| format!(": {}", db.message()));
        format!("{error}{detail}")
    };
    client
        .batch_execute(&format!(
            "SET search_path TO public; DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema}"
        ))
        .map_err(refused)?;
    for sql in setup {
        client.batch_execute(sql).map_err(refused)?;
    }
    let mut rows = Vec::new();
    for (n, statement) in statements.iter().enumerate() {
        let values = statement.values().map_err(|error| error.to_string())?;
        if n == statements.len() - 1 && gives_rows(statement) {
            rows = if statement.kind == buttercupql::StatementKind::Change {
                // A change's rows come back through a query that reads them.
                let wrapped = format!(
                    "WITH _changed AS (\n{}\n) SELECT row_to_json(_changed)::text FROM _changed",
                    statement.sql
                );
                run_postgres_json(client, &wrapped, &values)?
            } else {
                run_postgres(client, &statement.sql, &values)?
            };
            continue;
        }
        if values.is_empty() {
            client
                .batch_execute(&statement.sql)
                .map_err(|error| format!("{}\n{}", refused(error), statement.sql))?;
            continue;
        }
        let boxed: Vec<Box<dyn postgres::types::ToSql + Sync>> = values
            .iter()
            .map(|value| -> Box<dyn postgres::types::ToSql + Sync> {
                match value {
                    Value::Text(text) => Box::new(text.clone()),
                    Value::Int(value) => Box::new(*value),
                    Value::Float(value) => Box::new(*value),
                    Value::Bool(value) => Box::new(*value),
                    Value::Null => Box::new(Option::<String>::None),
                }
            })
            .collect();
        let refs: Vec<&(dyn postgres::types::ToSql + Sync)> =
            boxed.iter().map(|value| value.as_ref()).collect();
        client
            .execute(&statement.sql, &refs)
            .map_err(|error| format!("{}\n{}", refused(error), statement.sql))?;
    }
    let mut checked = Vec::new();
    for check in checks {
        // A check may be several statements; the last one's rows are compared.
        let (before, last) = check.rsplit_once("; ").unwrap_or(("", check));
        if !before.is_empty() {
            client.batch_execute(before).map_err(refused)?;
        }
        checked.push(run_postgres(client, last, &[])?);
    }
    Ok((rows, checked))
}

/// Runs a script on ClickHouse, in a database of its own.
fn clickhouse_script(
    clickhouse: &ClickHouse,
    setup: &[&str],
    statements: &[buttercupql::Statement],
    checks: &[&str],
) -> Ran {
    clickhouse.send(
        &format!("DROP DATABASE IF EXISTS {}", clickhouse.database),
        &[],
        false,
    )?;
    clickhouse.send(
        &format!("CREATE DATABASE {}", clickhouse.database),
        &[],
        false,
    )?;
    for sql in setup {
        clickhouse.send(sql, &[], false)?;
    }
    let mut rows = Vec::new();
    for (n, statement) in statements.iter().enumerate() {
        let values = statement.values().map_err(|error| error.to_string())?;
        let named: Vec<(String, Value)> = statement.names.iter().cloned().zip(values).collect();
        if n == statements.len() - 1 && gives_rows(statement) {
            rows = clickhouse.run(&statement.sql, &named)?;
        } else {
            clickhouse
                .send(&statement.sql, &named, true)
                .map_err(|error| format!("{error}\n{}", statement.sql))?;
        }
    }
    let mut checked = Vec::new();
    for check in checks {
        checked.push(clickhouse.run(check, &[])?);
    }
    Ok((rows, checked))
}

/// A script kept for DuckDB, with what it must give.
struct QueuedScript {
    label: String,
    statements: Vec<buttercupql::Statement>,
    rows: Vec<Vec<Cell>>,
    checks: Vec<(&'static str, Vec<Vec<Cell>>)>,
}

/// Runs the scripts kept for DuckDB, each in a database of its own, and returns what went wrong.
fn duckdb_scripts(python: &str, scripts: &[QueuedScript]) -> Vec<String> {
    let to_json = |value: &Value| match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(value) => serde_json::Value::Bool(*value),
        Value::Int(value) => serde_json::Value::from(*value),
        Value::Float(value) => serde_json::Value::from(*value),
        Value::Text(text) => serde_json::Value::String(text.clone()),
    };
    let job = serde_json::json!({
        "scripts": scripts.iter().map(|script| serde_json::json!({
            "statements": script.statements.iter().map(|statement| serde_json::json!({
                "sql": statement.sql,
                "params": statement.values().unwrap_or_default().iter().map(to_json).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "rows": script.statements.last().is_some_and(gives_rows),
            "checks": script.checks.iter().map(|(sql, _)| *sql).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    });
    let runner = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/duckdb_scripts.py");
    let mut child = Command::new(python)
        .arg(runner)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the Python for DuckDB starts");
    child
        .stdin
        .take()
        .expect("its input")
        .write_all(job.to_string().as_bytes())
        .expect("the scripts are sent to DuckDB");
    let out = child.wait_with_output().expect("DuckDB's scripts finish");
    if !out.status.success() {
        return vec![format!(
            "DuckDB's script runner failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )];
    }
    let results: Vec<serde_json::Value> =
        serde_json::from_slice(&out.stdout).expect("DuckDB's answer is JSON");
    let rows_of = |value: &serde_json::Value| -> Vec<Vec<Cell>> {
        value
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|row| {
                row.as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(json_cell)
                    .collect()
            })
            .collect()
    };
    let mut failures = Vec::new();
    for (script, result) in scripts.iter().zip(results) {
        if let Some(error) = result.get("error").and_then(serde_json::Value::as_str) {
            failures.push(format!("{}: the database refused\n{error}", script.label));
            continue;
        }
        let rows = rows_of(&result["rows"]);
        if rows != script.rows {
            failures.push(format!(
                "{}: wrong rows\ngot:\n{}\nwanted:\n{}",
                script.label,
                shown(&rows),
                shown(&script.rows)
            ));
        }
        for ((sql, wanted), got) in script
            .checks
            .iter()
            .zip(result["checks"].as_array().cloned().unwrap_or_default())
        {
            let got = rows_of(&got);
            if got != *wanted {
                failures.push(format!(
                    "{}: `{sql}` gave\n{}\nwanted:\n{}",
                    script.label,
                    shown(&got),
                    shown(wanted)
                ));
            }
        }
    }
    failures
}

#[test]
fn every_script_changes_the_database_as_it_says() {
    let mut postgres_client = std::env::var("BCQL_POSTGRES_URL").ok().map(|url| {
        postgres::Client::connect(&url, postgres::NoTls).expect("PostgreSQL can be reached")
    });
    let clickhouse = ClickHouse::connect("scripts");
    let duckdb = std::env::var("BCQL_DUCKDB_PYTHON").ok();
    let mut queued = Vec::new();
    let mut failures = Vec::new();
    for (n, case) in scripts().into_iter().enumerate() {
        for dialect in Dialect::ALL {
            let label = format!("{} ({dialect})", case.name);
            let compiled = buttercupql::compile(&case.text, &Options::new(dialect));
            if let Some((_, refusal)) = case.not_on.iter().find(|(not, _)| *not == dialect) {
                match compiled {
                    Ok(_) => failures.push(format!("{label}: compiles, but {dialect} can't do it")),
                    Err(problems) => {
                        if !problems
                            .iter()
                            .any(|problem| problem.message.contains(refusal))
                        {
                            let messages: Vec<String> =
                                problems.iter().map(ToString::to_string).collect();
                            failures.push(format!(
                                "{label}: refused for another reason: {}",
                                messages.join("; ")
                            ));
                        }
                    }
                }
                continue;
            }
            let compiled = match compiled {
                Ok(compiled) => compiled,
                Err(problems) => {
                    let messages: Vec<String> = problems.iter().map(ToString::to_string).collect();
                    failures.push(format!(
                        "{label}: does not compile: {}",
                        messages.join("; ")
                    ));
                    continue;
                }
            };
            let checks: Vec<(&'static str, Vec<Vec<Cell>>)> = case
                .checks
                .iter()
                .filter(|(on, _, _)| *on == dialect)
                .map(|(_, sql, rows)| (*sql, rows.clone()))
                .collect();
            let check_sql: Vec<&str> = checks.iter().map(|(sql, _)| *sql).collect();
            let setup: Vec<&str> = case
                .setup
                .iter()
                .filter(|(on, _)| *on == dialect)
                .map(|(_, sql)| *sql)
                .collect();
            let result = match dialect {
                Dialect::Sqlite => Some(sqlite_script(&compiled.statements, &check_sql)),
                Dialect::Postgres => postgres_client.as_mut().map(|client| {
                    let schema = format!("bcql_script{n}_{}", std::process::id());
                    let result =
                        postgres_script(client, &schema, &setup, &compiled.statements, &check_sql);
                    let _ = client.batch_execute(&format!(
                        "SET search_path TO public; DROP SCHEMA IF EXISTS {schema} CASCADE"
                    ));
                    result
                }),
                Dialect::ClickHouse => clickhouse.as_ref().map(|clickhouse| {
                    clickhouse_script(clickhouse, &setup, &compiled.statements, &check_sql)
                }),
                Dialect::DuckDb => {
                    queued.push(QueuedScript {
                        label: label.clone(),
                        statements: compiled.statements.clone(),
                        rows: case.rows.clone(),
                        checks: checks.clone(),
                    });
                    None
                }
            };
            match result {
                None => {}
                Some(Err(error)) => {
                    failures.push(format!("{label}: the database refused\n{error}"))
                }
                Some(Ok((rows, checked))) => {
                    if rows != case.rows {
                        failures.push(format!(
                            "{label}: wrong rows\n{}\ngot:\n{}\nwanted:\n{}",
                            compiled.sql,
                            shown(&rows),
                            shown(&case.rows)
                        ));
                    }
                    for ((sql, wanted), got) in checks.iter().zip(checked) {
                        if got != *wanted {
                            failures.push(format!(
                                "{label}: `{sql}` gave\n{}\nwanted:\n{}",
                                shown(&got),
                                shown(wanted)
                            ));
                        }
                    }
                }
            }
        }
    }
    if let Some(python) = &duckdb {
        failures.extend(duckdb_scripts(python, &queued));
    }
    if let Some(clickhouse) = &clickhouse {
        let _ = clickhouse.send(
            &format!("DROP DATABASE IF EXISTS {}", clickhouse.database),
            &[],
            false,
        );
    }
    report(failures);
}
