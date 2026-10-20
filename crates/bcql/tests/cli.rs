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

fn example(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
        .display()
        .to_string()
}

#[test]
fn compile_writes_the_sql_and_its_parameters() {
    let out = bcql(&["compile", &example("open_critical.bcql")], "");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "\
SELECT key, account, risk
FROM cases
WHERE status = $1 AND band IN ($2, $3) AND raised > now() - INTERVAL '7 days'
ORDER BY risk DESC NULLS LAST
LIMIT 20
-- parameters: [\"open\", \"critical\", \"high\"]
"
    );
    let lite = bcql(
        &["compile", "--dialect", "sqlite", "-"],
        "from t\nwhere a == \"x\"\n",
    );
    assert_eq!(
        text(&lite.stdout),
        "SELECT *\nFROM t\nWHERE a = ?\n-- parameters: [\"x\"]\n"
    );
    let json = bcql(
        &["compile", "--json", "--params=format", "-"],
        "from t\nwhere a == \"x\\\"y\"\n",
    );
    assert_eq!(
        text(&json.stdout),
        "{\"sql\": \"SELECT *\\nFROM t\\nWHERE a = %s\", \"params\": [\"x\\\"y\"], \"args\": []}\n"
    );
}

#[test]
fn a_schema_file_lets_names_be_checked() {
    let schema = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bcql-schema.sql");
    std::fs::write(
        &schema,
        "CREATE TABLE cases (key text NOT NULL, status text NOT NULL, risk integer);\n",
    )
    .expect("the schema is written");
    let schema = schema.display().to_string();
    let out = bcql(
        &["compile", "--schema", &schema, "-"],
        "from cases\nwhere stauts == \"open\"\n",
    );
    assert_eq!(out.status.code(), Some(1));
    let shown = text(&out.stderr);
    assert!(
        shown.contains("error: there is no column `stauts` in `cases`"),
        "{shown}"
    );
    assert!(shown.contains("^^^^^^ did you mean `status`?"), "{shown}");
    let check = bcql(
        &["check", "--schema", &schema, "-"],
        "from cases\nsort -key\n",
    );
    assert!(check.status.success(), "{}", text(&check.stderr));
}

#[test]
fn compile_shows_every_mistake_at_its_place() {
    let out = bcql(&["compile", "-"], "from t\nn = count()\n");
    assert_eq!(out.status.code(), Some(1));
    let shown = text(&out.stderr);
    assert!(
        shown.starts_with("error: `count()` is a total\n"),
        "{shown}"
    );
    assert!(shown.ends_with("bcql: 1 error in <stdin>\n"), "{shown}");
    assert!(text(&out.stdout).is_empty());
    assert_eq!(
        bcql(&["compile", "a.bcql", "b.bcql"], "").status.code(),
        Some(2)
    );
    assert_eq!(
        bcql(&["compile", "--dialect", "oracle", "-"], "")
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn compile_reads_modules_and_leaves_a_defs_arguments_open() {
    let folder = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("bcql-modules");
    std::fs::create_dir_all(&folder).expect("a folder for the files");
    std::fs::write(
        folder.join("shared.bcql"),
        "def recent(days_back=7):\n    where raised > now() - days(days_back)\n",
    )
    .expect("the module is written");
    std::fs::write(
        folder.join("report.bcql"),
        "from shared import recent\n\ndef open_for(account: str):\n    from cases\n    where account_id == account\n    recent(30)\n",
    )
    .expect("the report is written");
    let report = folder.join("report.bcql").display().to_string();
    let out = bcql(
        &[
            "compile",
            &report,
            "--query",
            "open_for",
            "--dialect",
            "sqlite",
        ],
        "",
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        text(&out.stdout),
        "SELECT *\nFROM cases\nWHERE account_id = ? AND raised > datetime('now', '-30 days')\n-- parameters: [{\"arg\": \"account\"}]\n-- arguments: account\n"
    );
    std::fs::write(
        folder.join("broken.bcql"),
        "from shared import recnt\nfrom cases\n",
    )
    .expect("written");
    let broken = folder.join("broken.bcql").display().to_string();
    let out = bcql(&["compile", &broken], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out.stderr).contains("did you mean `recent`?"),
        "{}",
        text(&out.stderr)
    );
}

/// A message as the language server protocol frames it.
fn framed(message: &str) -> String {
    format!("Content-Length: {}\r\n\r\n{message}", message.len())
}

/// The messages in what the language server wrote.
fn messages(output: &[u8]) -> Vec<String> {
    let output = String::from_utf8(output.to_vec()).expect("the server writes UTF-8");
    let mut rest = output.as_str();
    let mut found = Vec::new();
    while let Some(start) = rest.find("\r\n\r\n") {
        let length: usize = rest[..start]
            .trim()
            .strip_prefix("Content-Length: ")
            .expect("each message starts with its length")
            .parse()
            .expect("the length is a number");
        let body = &rest[start + 4..start + 4 + length];
        found.push(body.to_string());
        rest = &rest[start + 4 + length..];
    }
    assert!(
        rest.is_empty(),
        "the output ends inside a message: {rest:?}"
    );
    found
}

#[test]
fn the_language_server_speaks_to_an_editor() {
    let session = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{},"initializationOptions":{"dialect":"sqlite"}}}"#,
        r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#,
        r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///work/open.bcql","languageId":"buttercupql","version":1,"text":"from cases\nwhere status = \"open\"\n"}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"buttercupql/compile","params":{"textDocument":{"uri":"file:///work/open.bcql"}}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"shutdown"}"#,
        r#"{"jsonrpc":"2.0","method":"exit"}"#,
    ]
    .map(framed)
    .concat();
    let out = bcql(&["lsp", "--stdio"], &session);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let replies = messages(&out.stdout);
    assert_eq!(replies.len(), 4, "{replies:#?}");
    assert!(
        replies[0].starts_with(r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":"#),
        "{}",
        replies[0]
    );
    assert!(
        replies[1].starts_with(r#"{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///work/open.bcql","diagnostics":[{"range":{"start":{"line":1,"character":13},"end":{"line":1,"character":14}},"severity":1,"source":"bcql","message":"use `==` to compare"#),
        "{}",
        replies[1]
    );
    assert!(
        replies[2].contains(r#""dialect":"sqlite","problems":["error: use `==` to compare"#),
        "{}",
        replies[2]
    );
    assert_eq!(replies[3], r#"{"jsonrpc":"2.0","id":3,"result":null}"#);
}

#[test]
fn the_language_server_stops_with_its_input() {
    // An editor that goes away without shutting the server down: it stops, with an exit code saying so.
    let out = bcql(&["lsp"], "");
    assert_eq!(out.status.code(), Some(1));
    let out = bcql(&["lsp", "--port", "9"], "");
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).starts_with("bcql: `bcql lsp` takes no `--port`"),
        "{}",
        text(&out.stderr)
    );
}
