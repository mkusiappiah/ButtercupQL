//! Every example in `examples/` reads without a problem.

use std::fs;
use std::path::Path;

#[test]
fn every_example_reads_without_a_problem() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut read = 0;
    for entry in fs::read_dir(&folder).expect("the examples folder can be read") {
        let path = entry.expect("an entry of the examples folder").path();
        if path.extension().is_none_or(|extension| extension != "bcql") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("an example can be read");
        let parsed = buttercupql::parse(&text);
        assert!(
            parsed.diagnostics.is_empty(),
            "{}: {:#?}",
            path.display(),
            parsed.diagnostics
        );
        assert!(
            parsed.file.query.is_some(),
            "{} has no query",
            path.display()
        );
        read += 1;
    }
    assert!(read >= 4, "only {read} examples were found");
}
