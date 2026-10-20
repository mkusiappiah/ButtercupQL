//! Every example compiles to the SQL kept beside it, for each database: `examples/<name>.<database>.sql`.
//!
//! When the compiler changes what it writes on purpose, run the tests with `BCQL_BLESS=1` to write the new SQL, and
//! read the changes before committing them.

use std::fs;
use std::path::Path;

use buttercupql::{Dialect, Options, Value};

/// The SQL and its parameters, as kept in a snapshot file.
fn snapshot(sql: &str, params: &[Value]) -> String {
    let mut out = format!("{sql}\n");
    for (n, param) in params.iter().enumerate() {
        let shown = match param {
            Value::Text(text) => format!("{text:?}"),
            other => format!("{other:?}"),
        };
        out.push_str(&format!("-- parameter {}: {shown}\n", n + 1));
    }
    out
}

#[test]
fn every_example_compiles_to_its_snapshot() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let bless = std::env::var_os("BCQL_BLESS").is_some();
    let mut checked = 0;
    let mut stale = Vec::new();
    for entry in fs::read_dir(&folder).expect("the examples folder can be read") {
        let path = entry.expect("an entry of the examples folder").path();
        if path.extension().is_none_or(|extension| extension != "bcql") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("an example can be read");
        for dialect in [Dialect::Sqlite, Dialect::Postgres] {
            let compiled =
                buttercupql::compile(&text, &Options::new(dialect)).unwrap_or_else(|problems| {
                    panic!(
                        "{} does not compile for {dialect}: {problems:#?}",
                        path.display()
                    )
                });
            let got = snapshot(&compiled.sql, &compiled.params);
            let file = path.with_extension(format!("{dialect}.sql"));
            let kept = fs::read_to_string(&file)
                .unwrap_or_default()
                .replace("\r\n", "\n");
            if bless {
                fs::write(&file, &got).expect("the snapshot can be written");
            } else if kept != got {
                stale.push(format!(
                    "{}:\n--- kept\n{kept}--- now\n{got}",
                    file.display()
                ));
            }
            checked += 1;
        }
    }
    assert!(checked >= 8, "only {checked} snapshots were checked");
    assert!(
        stale.is_empty(),
        "snapshots differ (run with BCQL_BLESS=1 to update them):\n\n{}",
        stale.join("\n")
    );
}
