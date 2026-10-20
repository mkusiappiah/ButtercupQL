//! The SQL ButtercupQL writes, run on real databases.
//!
//! Each query must give the rows written beside it, which are the rows Python's meaning gives, on SQLite and on
//! PostgreSQL alike, with and without a schema. SQLite runs in memory every time; PostgreSQL runs when
//! `BCQL_POSTGRES_URL` names a database to use, as on GitHub.

use std::fmt;

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

/// The tables the queries read, created for a database.
fn fixture(dialect: Dialect) -> String {
    let moment = if dialect == Dialect::Sqlite {
        "TEXT"
    } else {
        "TIMESTAMP"
    };
    let ago = |days: i64| match dialect {
        Dialect::Sqlite => format!("datetime('now', '-{days} days')"),
        _ => format!("now() - interval '{days} days'"),
    };
    let cases = [
        "1, 'C1', 1, 'open', 'critical', 95, 1, NULL, '50% off', 'A1', 'Acme'",
        "2, 'C2', 1, 'open', 'high', 90, 2, 'ana', 'a_b', 'A1', 'Acme'",
        "3, 'C3', 2, 'open', 'low', 40, 3, NULL, 'A_B', 'A2', 'Beta'",
        "4, 'C4', 2, 'closed', 'critical', NULL, 4, 'ben', NULL, 'A2', 'Beta'",
        "5, 'C5', 3, 'open', NULL, -5, 10, '', 'plain', 'A3', 'acme'",
        "6, 'C6', NULL, 'open', 'high', 70, 20, NULL, '5000 off', NULL, NULL",
    ];
    let transactions = [
        "1, 1, 'Acme', 2000000, 1",
        "2, 1, 'Acme', 500, 2",
        "3, 2, 'Beta', 1500000, 3",
        "4, 2, 'Beta', 300, 40",
        "5, 3, 'Gamma', 100, 5",
    ];
    let mut sql = format!(
        "\
CREATE TABLE accounts (id INTEGER PRIMARY KEY, operator TEXT NOT NULL, balance NUMERIC);
CREATE TABLE cases (
    id INTEGER PRIMARY KEY,
    key TEXT NOT NULL,
    account_id INTEGER,
    status TEXT NOT NULL,
    band TEXT,
    risk INTEGER,
    raised {moment} NOT NULL,
    assignee TEXT,
    notes TEXT,
    account TEXT,
    operator TEXT
);
CREATE TABLE transactions (id INTEGER PRIMARY KEY, account_id INTEGER, operator TEXT NOT NULL, amount INTEGER NOT NULL, time {moment} NOT NULL);
INSERT INTO accounts VALUES (1, 'Acme', 120.50), (2, 'Beta', NULL), (3, 'acme', 0);
"
    );
    for row in cases {
        let fields: Vec<&str> = row.split(", ").collect();
        let days: i64 = fields[6].parse().expect("days ago");
        let mut values: Vec<String> = fields.iter().map(|field| field.to_string()).collect();
        values[6] = ago(days);
        sql.push_str(&format!(
            "INSERT INTO cases VALUES ({});\n",
            values.join(", ")
        ));
    }
    for row in transactions {
        let fields: Vec<&str> = row.split(", ").collect();
        let days: i64 = fields[4].parse().expect("days ago");
        let mut values: Vec<String> = fields.iter().map(|field| field.to_string()).collect();
        values[4] = ago(days);
        sql.push_str(&format!(
            "INSERT INTO transactions VALUES ({});\n",
            values.join(", ")
        ));
    }
    sql
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
    ]
}

/// The cases that need a schema, which gives the kinds of the values.
fn schema_cases() -> Vec<Case> {
    vec![Case {
        name: "Python's truth: text that is not empty",
        query: "from cases\nwhere assignee\nsort key\nselect key",
        rows: column(&[t("C2"), t("C4")]),
    }]
}

// ---- Running SQL

fn sqlite() -> rusqlite::Connection {
    let connection = rusqlite::Connection::open_in_memory().expect("an SQLite database in memory");
    connection
        .execute_batch(&fixture(Dialect::Sqlite))
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
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema};\n{}",
            fixture(Dialect::Postgres)
        ))
        .expect("the tables are created in PostgreSQL");
    Some(client)
}

fn run_postgres(
    client: &mut postgres::Client,
    sql: &str,
    params: &[Value],
) -> Result<Vec<Vec<Cell>>, String> {
    // Each row comes back as JSON, so that every kind of value reads the same way.
    let wrapped = format!("SELECT row_to_json(_r)::text FROM (\n{sql}\n) AS _r");
    let texts: Vec<String> = params
        .iter()
        .map(|param| match param {
            Value::Text(text) => Ok(text.clone()),
            other => Err(format!("only text parameters are expected, not {other:?}")),
        })
        .collect::<Result<_, _>>()?;
    let refs: Vec<&(dyn postgres::types::ToSql + Sync)> = texts
        .iter()
        .map(|text| text as &(dyn postgres::types::ToSql + Sync))
        .collect();
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
        out.push(
            object
                .into_values()
                .map(|value| match value {
                    serde_json::Value::Null => Cell::Null,
                    serde_json::Value::Bool(value) => Cell::Int(i64::from(value)),
                    serde_json::Value::Number(number) => match number.as_i64() {
                        Some(n) => Cell::Int(n),
                        None => Cell::Float(number.as_f64().unwrap_or(f64::NAN)),
                    },
                    serde_json::Value::String(text) => Cell::Text(text),
                    other => Cell::Text(other.to_string()),
                })
                .collect(),
        );
    }
    Ok(out)
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

/// Runs each case on each database, with and without the schema, and returns what went wrong.
fn check(cases: &[Case], with_schema: bool, without_schema: bool, test: &str) -> Vec<String> {
    let schema = Schema::from_sql(&fixture(Dialect::Postgres)).expect("the fixture's schema reads");
    let lite = sqlite();
    let mut pg = postgres(test);
    let mut failures = Vec::new();
    for case in cases {
        for schema in [
            without_schema.then_some(None),
            with_schema.then_some(Some(&schema)),
        ]
        .into_iter()
        .flatten()
        {
            for dialect in [Dialect::Sqlite, Dialect::Postgres] {
                let mut options = Options::new(dialect);
                if let Some(schema) = schema {
                    options = options.with_schema(schema);
                }
                let label = format!(
                    "{} ({dialect}, {})",
                    case.name,
                    if schema.is_some() {
                        "with the schema"
                    } else {
                        "without a schema"
                    }
                );
                let compiled = match buttercupql::compile(case.query, &options) {
                    Ok(compiled) => compiled,
                    Err(problems) => {
                        let messages: Vec<String> =
                            problems.iter().map(ToString::to_string).collect();
                        failures.push(format!(
                            "{label}: does not compile: {}",
                            messages.join("; ")
                        ));
                        continue;
                    }
                };
                let result = match dialect {
                    Dialect::Sqlite => Some(run_sqlite(&lite, &compiled.sql, &compiled.params)),
                    _ => pg
                        .as_mut()
                        .map(|client| run_postgres(client, &compiled.sql, &compiled.params)),
                };
                match result {
                    None => {}
                    Some(Err(error)) => failures.push(format!(
                        "{label}: the database refused\n{}\n{error}",
                        compiled.sql
                    )),
                    Some(Ok(rows)) if rows != case.rows => failures.push(format!(
                        "{label}: wrong rows\n{}\ngot:\n{}\nwanted:\n{}",
                        compiled.sql,
                        shown(&rows),
                        shown(&case.rows)
                    )),
                    Some(Ok(_)) => {}
                }
            }
        }
    }
    if pg.is_none() {
        eprintln!("PostgreSQL was not tested: set BCQL_POSTGRES_URL to test it");
    }
    failures
}

#[test]
fn every_query_gives_pythons_rows_on_every_database() {
    let failures = check(&cases(), true, true, "every");
    assert!(
        failures.is_empty(),
        "{} failures:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn the_schema_gives_the_kinds_of_values() {
    let failures = check(&schema_cases(), true, false, "schema");
    assert!(
        failures.is_empty(),
        "{} failures:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn every_example_runs() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let lite = sqlite();
    let mut pg = postgres("examples");
    for entry in std::fs::read_dir(folder).expect("the examples folder can be read") {
        let path = entry.expect("an entry").path();
        if path.extension().is_none_or(|extension| extension != "bcql") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("an example can be read");
        let lite_sql =
            buttercupql::compile(&text, &Options::new(Dialect::Sqlite)).expect("it compiles");
        run_sqlite(&lite, &lite_sql.sql, &lite_sql.params).unwrap_or_else(|error| {
            panic!("{} on SQLite:\n{}\n{error}", path.display(), lite_sql.sql)
        });
        if let Some(client) = pg.as_mut() {
            let pg_sql =
                buttercupql::compile(&text, &Options::new(Dialect::Postgres)).expect("it compiles");
            run_postgres(client, &pg_sql.sql, &pg_sql.params).unwrap_or_else(|error| {
                panic!("{} on PostgreSQL:\n{}\n{error}", path.display(), pg_sql.sql)
            });
        }
    }
}
