//! Every example the playground offers compiles with the playground's schema, for every database.

use std::fs;
use std::path::Path;

use bcql::json::Json;

#[test]
fn every_example_in_the_playground_compiles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let schema =
        fs::read_to_string(root.join("playground/schema.sql")).expect("the schema can be read");
    let mut checked = 0;
    for folder in ["examples", "playground/examples"] {
        for entry in fs::read_dir(root.join(folder)).expect("the folder can be read") {
            let path = entry.expect("an entry of the folder").path();
            if path.extension().is_none_or(|extension| extension != "bcql") {
                continue;
            }
            let text = fs::read_to_string(&path).expect("an example can be read");
            // The main query, and each query the playground offers to compile on its own.
            let queries = buttercupql_wasm::answer(&text, "sqlite", &schema, "")
                .get("queries")
                .clone();
            let mut names = vec![""];
            names.extend(queries.items().iter().filter_map(Json::as_str));
            for dialect in ["sqlite", "postgres", "duckdb", "clickhouse"] {
                for name in &names {
                    let answer = buttercupql_wasm::answer(&text, dialect, &schema, name);
                    assert_eq!(
                        answer.get("ok"),
                        &Json::Bool(true),
                        "{} ({name}) on {dialect}: {answer}",
                        path.display()
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked >= 32, "only {checked} were compiled");
}
