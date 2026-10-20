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
    let rows = client.query(&wrapped, &refs).map_err(|error| {
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
