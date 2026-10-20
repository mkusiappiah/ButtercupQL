//! The `bcql` command, run as a person runs it.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// Runs `bcql` with these arguments, giving it `input` on standard input.
fn bcql(args: &[&str], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bcql"))
        .args(args)
        .env_remove("NO_COLOR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bcql starts");
    let mut stdin = child.stdin.take().expect("bcql's standard input");
    stdin
        .write_all(input.as_bytes())
        .expect("bcql reads its input");
    drop(stdin);
    child.wait_with_output().expect("bcql finishes")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

fn examples() -> Vec<String> {
    let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut files: Vec<String> = std::fs::read_dir(folder)
        .expect("the examples folder can be read")
        .map(|entry| entry.expect("an entry of the examples folder").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "bcql")
        })
        .map(|path| path.display().to_string())
        .collect();
    files.sort();
    files
}

#[test]
fn the_examples_have_no_problems() {
    let files = examples();
    let mut args = vec!["check"];
    args.extend(files.iter().map(String::as_str));
    let out = bcql(&args, "");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stderr),
        format!("checked {} files: no problems found\n", files.len())
    );
}

#[test]
fn a_mistake_is_shown_at_its_place() {
    let out = bcql(&["check", "-"], "from cases\nwhere status = \"open\"\n");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        text(&out.stderr),
        "\
error: use `==` to compare
  ┌─ <stdin>:2:14
  │
2 │ where status = \"open\"
  │              ^ `=` gives a column a value; `==` compares two values

checked 1 file: 1 error
"
    );
}

#[test]
fn colour_is_used_only_when_asked_for() {
    let mistake = "from t\nwhere a = 1\n";
    // Standard error is not a terminal here, so `auto` means no colour.
    assert!(!text(&bcql(&["check", "-"], mistake).stderr).contains('\x1b'));
    let coloured = bcql(&["check", "--color", "always", "-"], mistake);
    assert!(text(&coloured.stderr).contains("\x1b[31m"));
    let plain = bcql(&["check", "--color=never", "-"], mistake);
    assert!(!text(&plain.stderr).contains('\x1b'));
}

#[test]
fn the_version_and_the_help() {
    let version = bcql(&["--version"], "");
    assert!(version.status.success());
    assert_eq!(
        text(&version.stdout),
        format!("bcql {}\n", env!("CARGO_PKG_VERSION"))
    );
    let help = bcql(&["--help"], "");
    assert!(help.status.success());
    assert!(text(&help.stdout).contains("bcql check"));
}

#[test]
fn a_wrong_command_exits_with_2() {
    let wrong: [&[&str]; 5] = [
        &[],
        &["frobnicate"],
        &["check"],
        &["check", "--colour=sometimes", "x.bcql"],
        &["check", "--nope", "x.bcql"],
    ];
    for args in wrong {
        let out = bcql(args, "");
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
}

#[test]
fn a_missing_file_is_reported() {
    let out = bcql(&["check", "no-such-file.bcql"], "");
    assert_eq!(out.status.code(), Some(1));
    let shown = text(&out.stderr);
    assert!(
        shown.starts_with("bcql: can't read no-such-file.bcql:"),
        "{shown}"
    );
    assert!(
        shown.ends_with("checked 1 file: 1 file could not be read\n"),
        "{shown}"
    );
}
