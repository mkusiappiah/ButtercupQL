//! The language server, driven as an editor drives it.

use std::io::Cursor;

use super::*;

fn request(id: u32, method: &str, params: Json) -> Json {
    object([
        ("jsonrpc", "2.0".into()),
        ("id", id.into()),
        ("method", method.into()),
        ("params", params),
    ])
}

fn notify(method: &str, params: Json) -> Json {
    object([
        ("jsonrpc", "2.0".into()),
        ("method", method.into()),
        ("params", params),
    ])
}

fn started(options: Json) -> Server {
    let mut server = Server::default();
    let replies = server.handle(&request(
        1,
        "initialize",
        object([
            ("capabilities", object([])),
            ("initializationOptions", options),
        ]),
    ));
    assert!(
        replies
            .iter()
            .any(|reply| reply.get("id").as_u64() == Some(1)),
        "{replies:?}"
    );
    assert!(server.handle(&notify("initialized", object([]))).is_empty());
    server
}

fn open(server: &mut Server, uri: &str, text: &str) -> Vec<Json> {
    server.handle(&notify(
        "textDocument/didOpen",
        object([(
            "textDocument",
            object([
                ("uri", uri.into()),
                ("languageId", "buttercupql".into()),
                ("version", 1u32.into()),
                ("text", text.into()),
            ]),
        )]),
    ))
}

fn position(line: u32, character: u32) -> Json {
    object([("line", line.into()), ("character", character.into())])
}

fn at(uri: &str, line: u32, character: u32) -> Json {
    object([
        ("textDocument", object([("uri", uri.into())])),
        ("position", position(line, character)),
    ])
}

/// The problems sent for a file, from the messages a call gave.
fn published<'m>(messages: &'m [Json], uri: &str) -> Option<&'m [Json]> {
    messages
        .iter()
        .find(|message| {
            message.get("method").as_str() == Some("textDocument/publishDiagnostics")
                && message.get("params").get("uri").as_str() == Some(uri)
        })
        .map(|message| message.get("params").get("diagnostics").items())
}

fn result(server: &mut Server, method: &str, params: Json) -> Json {
    let replies = server.handle(&request(9, method, params));
    let reply = replies
        .iter()
        .find(|reply| reply.get("id").as_u64() == Some(9))
        .expect("a reply");
    reply.get("result").clone()
}

fn labels(items: &Json) -> Vec<&str> {
    items
        .items()
        .iter()
        .filter_map(|item| item.get("label").as_str())
        .collect()
}

/// A folder of its own for a test, empty.
fn folder(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("bcql-lsp-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).expect("a folder for the test");
    folder
}

const SCHEMA: &str = "
CREATE TABLE accounts (id text PRIMARY KEY, operator text NOT NULL);
CREATE TABLE cases (
    key text NOT NULL,
    status text NOT NULL,
    risk integer,
    account_id text REFERENCES accounts
);
";

fn with_schema(name: &str) -> (Server, PathBuf) {
    let folder = folder(name);
    let schema = folder.join("schema.sql");
    std::fs::write(&schema, SCHEMA).expect("the schema is written");
    let server = started(object([
        ("dialect", "sqlite".into()),
        ("schema", schema.display().to_string().into()),
    ]));
    (server, folder)
}

#[test]
fn messages_are_framed_with_their_length() {
    let mut written = Vec::new();
    write_message(&mut written, r#"{"a":"é"}"#).unwrap();
    assert_eq!(written, b"Content-Length: 10\r\n\r\n{\"a\":\"\xc3\xa9\"}");
    let mut input = Cursor::new([written.clone(), written].concat());
    assert_eq!(
        read_message(&mut input).unwrap().as_deref(),
        Some(r#"{"a":"é"}"#)
    );
    assert_eq!(
        read_message(&mut input).unwrap().as_deref(),
        Some(r#"{"a":"é"}"#)
    );
    assert_eq!(read_message(&mut input).unwrap(), None);
    let mut other_headers = Cursor::new(
        b"Content-Type: application/vscode-jsonrpc; charset=utf-8\r\ncontent-length: 2\r\n\r\n{}"
            .to_vec(),
    );
    assert_eq!(
        read_message(&mut other_headers).unwrap().as_deref(),
        Some("{}")
    );
    let mut wrong = Cursor::new(b"Content-Length: lots\r\n\r\n".to_vec());
    assert!(read_message(&mut wrong).is_err());
}

#[test]
fn the_server_says_what_it_can_do() {
    let mut server = Server::default();
    let replies = server.handle(&request(
        1,
        "initialize",
        object([("capabilities", object([]))]),
    ));
    let capabilities = replies[0].get("result").get("capabilities");
    assert_eq!(
        capabilities.get("positionEncoding").as_str(),
        Some("utf-16")
    );
    assert_eq!(
        capabilities.get("textDocumentSync").get("change").as_u64(),
        Some(2)
    );
    assert_eq!(capabilities.get("hoverProvider"), &Json::Bool(true));
    assert_eq!(capabilities.get("definitionProvider"), &Json::Bool(true));
    assert_eq!(
        replies[0]
            .get("result")
            .get("serverInfo")
            .get("name")
            .as_str(),
        Some("bcql")
    );

    // An editor that counts in UTF-8 gets its way.
    let mut server = Server::default();
    let offered = object([(
        "capabilities",
        object([(
            "general",
            object([(
                "positionEncodings",
                vec!["utf-8".into(), "utf-16".into()].into(),
            )]),
        )]),
    )]);
    let replies = server.handle(&request(1, "initialize", offered));
    assert_eq!(
        replies[0]
            .get("result")
            .get("capabilities")
            .get("positionEncoding")
            .as_str(),
        Some("utf-8")
    );
}

#[test]
fn the_server_starts_and_stops_as_the_protocol_says() {
    let mut server = Server::default();
    let early = server.handle(&request(1, "textDocument/hover", object([])));
    assert_eq!(
        early[0].get("error").get("code"),
        &Json::from(SERVER_NOT_INITIALIZED)
    );
    assert!(
        server
            .handle(&notify("textDocument/didOpen", object([])))
            .is_empty()
    );

    let mut server = started(Json::Null);
    let unknown = server.handle(&request(2, "textDocument/rename", object([])));
    assert_eq!(
        unknown[0].get("error").get("code"),
        &Json::from(METHOD_NOT_FOUND)
    );
    assert!(
        server
            .handle(&notify("$/cancelRequest", object([("id", 2u32.into())])))
            .is_empty()
    );
    let done = server.handle(&request(3, "shutdown", Json::Null));
    assert!(done[0].get("result").is_null() && done[0].get("error").is_null());
    let after = server.handle(&request(4, "textDocument/hover", object([])));
    assert_eq!(
        after[0].get("error").get("code"),
        &Json::from(INVALID_REQUEST)
    );
    server.handle(&notify("exit", Json::Null));
    assert_eq!(server.exit, Some(0));

    let mut abrupt = started(Json::Null);
    abrupt.handle(&notify("exit", Json::Null));
    assert_eq!(abrupt.exit, Some(1));
}

#[test]
fn problems_are_shown_as_the_file_is_edited() {
    let mut server = started(Json::Null);
    let uri = "file:///work/open.bcql";
    let sent = open(&mut server, uri, "from cases\nwhere status = \"open\"\n");
    let problems = published(&sent, uri).expect("the problems are sent");
    assert_eq!(problems.len(), 1);
    let problem = &problems[0];
    assert_eq!(problem.get("severity").as_u64(), Some(1));
    assert_eq!(problem.get("source").as_str(), Some("bcql"));
    assert_eq!(
        problem.get("message").as_str(),
        Some("use `==` to compare\n`=` gives a column a value; `==` compares two values")
    );
    assert_eq!(problem.get("range").get("start"), &position(1, 13));
    assert_eq!(problem.get("range").get("end"), &position(1, 14));

    // The mistake is mended: `=` becomes `==`.
    let change = |text: &str, line: u32, from: u32, to: u32| {
        notify(
            "textDocument/didChange",
            object([
                (
                    "textDocument",
                    object([("uri", uri.into()), ("version", 2u32.into())]),
                ),
                (
                    "contentChanges",
                    vec![object([
                        (
                            "range",
                            object([("start", position(line, from)), ("end", position(line, to))]),
                        ),
                        ("text", text.into()),
                    ])]
                    .into(),
                ),
            ]),
        )
    };
    let sent = server.handle(&change("==", 1, 13, 14));
    assert_eq!(published(&sent, uri), Some(&[][..]));
    assert_eq!(
        server.documents[uri].text,
        "from cases\nwhere status == \"open\"\n"
    );
    // A change that leaves the problems as they were sends nothing.
    let sent = server.handle(&change("# open ones\n", 1, 0, 0));
    assert_eq!(published(&sent, uri), None);
    // A step from another language gets its hint.
    let sent = server.handle(&change("limit 5\n", 3, 0, 0));
    let problems = published(&sent, uri).expect("the problems are sent");
    assert!(
        problems[0]
            .get("message")
            .as_str()
            .unwrap()
            .contains("help: for example: `take 20`"),
        "{problems:?}"
    );

    // Closed, its problems are taken away.
    let sent = server.handle(&notify(
        "textDocument/didClose",
        object([("textDocument", object([("uri", uri.into())]))]),
    ));
    assert_eq!(published(&sent, uri), Some(&[][..]));
    assert!(server.documents.is_empty());
}

#[test]
fn every_query_of_a_file_is_checked() {
    let mut server = started(Json::Null);
    let uri = "file:///work/defs.bcql";
    // No main query: the named query and the query `def` are compiled. A `def` of steps is only read, as what its
    // names mean depends on where it is called.
    let text = "busy = (\n    from cases\n    where risk > maxx(1, 2)\n)\n\ndef for_account(account):\n    from cases\n    where lenn(account_id) > 3\n\ndef recent(n=7):\n    where raised > now() - dayz(n)\n";
    let sent = open(&mut server, uri, text);
    let problems = published(&sent, uri).expect("the problems are sent");
    let messages: Vec<&str> = problems
        .iter()
        .map(|problem| problem.get("message").as_str().unwrap())
        .collect();
    assert_eq!(
        messages,
        [
            "there is no function `maxx`\ndid you mean `max`?",
            "there is no function `lenn`\ndid you mean `len`?"
        ],
        "{problems:?}"
    );
}

#[test]
fn places_are_counted_in_utf16_units_unless_the_editor_counts_bytes() {
    let text = "a 🌼 é\nbc";
    let lines = Lines::new(text, false);
    // The flower is two UTF-16 units and four bytes; `é` one unit and two bytes.
    assert_eq!(lines.position(text.find('é').unwrap()), position(0, 5));
    assert_eq!(lines.offset(&position(0, 5)), text.find('é').unwrap());
    assert_eq!(lines.offset(&position(0, 99)), text.find('\n').unwrap());
    assert_eq!(lines.offset(&position(1, 1)), text.len() - 1);
    assert_eq!(lines.offset(&position(7, 0)), text.len());
    let bytes = Lines::new(text, true);
    assert_eq!(bytes.position(text.find('é').unwrap()), position(0, 7));
    assert_eq!(bytes.offset(&position(0, 7)), text.find('é').unwrap());
    // Inside a character, a place falls back to its start.
    assert_eq!(bytes.offset(&position(0, 3)), 2);

    let mut server = started(Json::Null);
    let uri = "file:///work/flower.bcql";
    let sent = open(&mut server, uri, "from t\nwhere name == \"🌼\" and x = 1\n");
    let problem = &published(&sent, uri).unwrap()[0];
    // 25 UTF-16 units come before the `=`, and 27 bytes.
    assert_eq!(problem.get("range").get("start"), &position(1, 25));
}

#[test]
fn a_problem_in_a_module_is_shown_on_its_import() {
    let mut server = started(Json::Null);
    let folder = folder("modules");
    let shared = uri_of(&folder.join("shared.bcql"));
    let report = uri_of(&folder.join("report.bcql"));
    // The module is open and not saved: its text in the editor is the one read.
    open(
        &mut server,
        &shared,
        "def recent(days_back=7):\n    where raised > now() - dayz(days_back)\n",
    );
    let sent = open(
        &mut server,
        &report,
        "# the newest\nfrom shared import recent\nfrom cases\nrecent(30)\n",
    );
    let problems = published(&sent, &report).expect("the problems are sent");
    assert_eq!(problems.len(), 1, "{problems:?}");
    let problem = &problems[0];
    let message = problem.get("message").as_str().unwrap();
    assert!(
        message.starts_with("in ") && message.contains("shared.bcql: "),
        "{message}"
    );
    assert!(message.contains("did you mean `days`?"), "{message}");
    assert_eq!(problem.get("range").get("start"), &position(1, 0));
    let related = &problem.get("relatedInformation").items()[0];
    assert_eq!(
        related.get("location").get("uri").as_str(),
        Some(shared.as_str())
    );
    assert_eq!(
        related.get("location").get("range").get("start"),
        &position(1, 27)
    );

    // Mended in the module, the report's problem goes too.
    let sent = server.handle(&notify(
        "textDocument/didChange",
        object([
            ("textDocument", object([("uri", shared.as_str().into())])),
            (
                "contentChanges",
                vec![object([(
                    "text",
                    "def recent(days_back=7):\n    where raised > now() - days(days_back)\n".into(),
                )])]
                .into(),
            ),
        ]),
    ));
    assert_eq!(published(&sent, &report), Some(&[][..]));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn names_are_completed_from_the_schema_and_the_file() {
    let (mut server, folder) = with_schema("completion");
    let uri = uri_of(&folder.join("q.bcql"));
    let text =
        "def recent(n=7):\n    where raised > now() - days(n)\n\nfrom cases\nwhere \nsort \n";
    open(&mut server, &uri, text);

    let steps = result(&mut server, "textDocument/completion", at(&uri, 6, 0));
    let steps = labels(&steps);
    for word in [
        "where",
        "sort",
        "group",
        "summarize",
        "take",
        "recent",
        "def",
    ] {
        assert!(steps.contains(&word), "{word} is not in {steps:?}");
    }

    let tables_items = result(&mut server, "textDocument/completion", at(&uri, 6, 0));

    let tables = labels(&tables_items);
    assert!(!tables.contains(&"accounts"));
    server.handle(&notify(
        "textDocument/didChange",
        object([
            ("textDocument", object([("uri", uri.as_str().into())])),
            (
                "contentChanges",
                vec![object([("text", format!("{text}join ").into())])].into(),
            ),
        ]),
    ));
    let tables_items = result(&mut server, "textDocument/completion", at(&uri, 6, 5));
    let tables = labels(&tables_items);
    assert_eq!(tables, ["accounts", "cases"]);

    let names = result(&mut server, "textDocument/completion", at(&uri, 4, 6));
    let found = labels(&names);
    for name in [
        "status",
        "risk",
        "account_id",
        "account",
        "sum",
        "row_number",
        "None",
        "and",
    ] {
        assert!(found.contains(&name), "{name} is not in {found:?}");
    }
    let risk = names
        .items()
        .iter()
        .find(|item| item.get("label").as_str() == Some("risk"))
        .unwrap();
    assert_eq!(
        risk.get("detail").as_str(),
        Some("a whole number, can be missing")
    );
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_dot_completes_a_links_columns_or_the_methods() {
    let (mut server, folder) = with_schema("dots");
    let uri = uri_of(&folder.join("q.bcql"));
    open(
        &mut server,
        &uri,
        "from cases\nwhere account.\nwhere status.\nwhere accounts.\n",
    );
    let linked_items = result(&mut server, "textDocument/completion", at(&uri, 1, 14));
    let linked = labels(&linked_items);
    assert_eq!(linked, ["id", "operator"]);
    let methods_items = result(&mut server, "textDocument/completion", at(&uri, 2, 13));
    let methods = labels(&methods_items);
    assert!(
        methods.contains(&"startswith") && methods.contains(&"year"),
        "{methods:?}"
    );
    let table_items = result(&mut server, "textDocument/completion", at(&uri, 3, 15));
    let table = labels(&table_items);
    assert_eq!(table, ["id", "operator"]);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_module_imported_whole_completes_its_names() {
    let mut server = started(Json::Null);
    let folder = folder("module-names");
    let shared = uri_of(&folder.join("shared.bcql"));
    let report = uri_of(&folder.join("report.bcql"));
    open(
        &mut server,
        &shared,
        "cutoff = 90\n\ndef recent(n=7):\n    where raised > now() - days(n)\n",
    );
    open(&mut server, &report, "import shared\nfrom cases\nshared.\n");
    let names = result(&mut server, "textDocument/completion", at(&report, 2, 7));
    assert_eq!(labels(&names), ["cutoff", "recent"]);
    let recent = &names.items()[1];
    assert_eq!(recent.get("detail").as_str(), Some("def recent(n=7):"));
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_name_is_described_where_the_pointer_rests() {
    let (mut server, folder) = with_schema("hover");
    let uri = uri_of(&folder.join("q.bcql"));
    let text = "def recent(n=7):\n    \"\"\"The rows of the last `n` days.\"\"\"\n    where raised > now() - days(n)\n\nfrom cases\nwhere risk > 90\nrecent(30)\ngroup by account.operator:\n    total = sum(risk)\n";
    open(&mut server, &uri, text);
    let hover = |server: &mut Server, line, character| {
        let shown = result(server, "textDocument/hover", at(&uri, line, character));
        shown
            .get("contents")
            .get("value")
            .as_str()
            .map(str::to_string)
    };
    let step = hover(&mut server, 5, 2).unwrap();
    assert!(
        step.contains("where condition") && step.contains("Keep the rows"),
        "{step}"
    );
    let column = hover(&mut server, 5, 7).unwrap();
    assert_eq!(
        column,
        "```python\ncases.risk\n```\n\na whole number, can be missing"
    );
    let def = hover(&mut server, 6, 2).unwrap();
    assert_eq!(
        def,
        "```python\ndef recent(n=7)\n```\n\nThe rows of the last `n` days."
    );
    let link = hover(&mut server, 7, 19).unwrap();
    assert!(
        link.contains("accounts.operator") && link.contains("never missing"),
        "{link}"
    );
    let total = hover(&mut server, 8, 13).unwrap();
    assert!(
        total.contains("sum(x, where=condition)") && total.contains("0, as in Python"),
        "{total}"
    );
    let days = hover(&mut server, 2, 28).unwrap();
    assert!(days.starts_with("```python\ndays(n)\n```"), "{days}");
    let parameter = hover(&mut server, 2, 32).unwrap();
    assert!(
        parameter.ends_with("a parameter of `recent`"),
        "{parameter}"
    );
    assert_eq!(hover(&mut server, 3, 0), None);
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn a_name_leads_to_where_it_is_defined() {
    let mut server = started(Json::Null);
    let folder = folder("definition");
    let shared = uri_of(&folder.join("shared.bcql"));
    let report = uri_of(&folder.join("report.bcql"));
    open(
        &mut server,
        &shared,
        "cutoff = 90\n\ndef recent(n=7):\n    where raised > now() - days(n)\n",
    );
    open(
        &mut server,
        &report,
        "from shared import recent\nimport shared\nbusy = (\n    from cases\n    where risk > shared.cutoff\n)\n\nfrom busy\nrecent(3)\n",
    );
    let found = result(&mut server, "textDocument/definition", at(&report, 8, 2));
    assert_eq!(found.get("uri").as_str(), Some(shared.as_str()));
    assert_eq!(found.get("range").get("start"), &position(2, 4));
    let local = result(&mut server, "textDocument/definition", at(&report, 7, 6));
    assert_eq!(local.get("uri").as_str(), Some(report.as_str()));
    assert_eq!(local.get("range").get("start"), &position(2, 0));
    let dotted = result(&mut server, "textDocument/definition", at(&report, 4, 26));
    assert_eq!(dotted.get("uri").as_str(), Some(shared.as_str()));
    assert_eq!(dotted.get("range").get("start"), &position(0, 0));
    let module = result(&mut server, "textDocument/definition", at(&report, 1, 9));
    assert_eq!(module.get("uri").as_str(), Some(shared.as_str()));
    assert!(result(&mut server, "textDocument/definition", at(&report, 7, 0)).is_null());
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn the_files_definitions_are_listed() {
    let mut server = started(Json::Null);
    let uri = "file:///work/list.bcql";
    open(
        &mut server,
        uri,
        "cutoff = 90\nbusy = (\n    from cases\n    where risk > cutoff\n)\n\ndef recent(n=7):\n    where raised > now() - days(n)\n\nfrom busy\nrecent(3)\n",
    );
    let symbols = result(
        &mut server,
        "textDocument/documentSymbol",
        object([("textDocument", object([("uri", uri.into())]))]),
    );
    let listed: Vec<(&str, &str)> = symbols
        .items()
        .iter()
        .map(|symbol| {
            (
                symbol.get("name").as_str().unwrap(),
                symbol.get("detail").as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("cutoff", "a value"),
            ("busy", "a named query"),
            ("recent", "steps"),
            ("query", "the file's query")
        ]
    );
}

const SCRIPT: &str = "create table archive:\n    key: str = field(primary_key=True)\n    risk: int | None\n\n@stored\ndef risky(at_least: int):\n    from cases\n    where risk >= at_least\n\nfrom cases\nwhere status == \"closed\"\ninsert into archive\n";

#[test]
fn statements_are_completed_described_and_listed() {
    let (mut server, folder) = with_schema("statements");
    let uri = uri_of(&folder.join("script.bcql"));
    open(&mut server, &uri, SCRIPT);

    let words_items = result(&mut server, "textDocument/completion", at(&uri, 12, 0));
    let words = labels(&words_items);
    for word in [
        "create", "alter", "drop", "grant", "revoke", "with", "update", "delete", "insert", "pivot",
    ] {
        assert!(words.contains(&word), "{word} is not in {words:?}");
    }

    let decorated = uri_of(&folder.join("decorated.bcql"));
    open(&mut server, &decorated, &format!("{SCRIPT}@"));
    let decorators = result(
        &mut server,
        "textDocument/completion",
        at(&decorated, 12, 1),
    );
    assert_eq!(labels(&decorators), ["stored"]);

    let inserting = uri_of(&folder.join("inserting.bcql"));
    open(&mut server, &inserting, &format!("{SCRIPT}grant read on "));
    let tables_items = result(
        &mut server,
        "textDocument/completion",
        at(&inserting, 12, 14),
    );
    assert_eq!(labels(&tables_items), ["accounts", "cases", "archive"]);

    let hover = |server: &mut Server, line, character| {
        let shown = result(server, "textDocument/hover", at(&uri, line, character));
        shown
            .get("contents")
            .get("value")
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    let create = hover(&mut server, 0, 2);
    assert!(
        create.contains("create table cases:") && create.contains("Python annotations"),
        "{create}"
    );
    let stored = hover(&mut server, 4, 3);
    assert!(
        stored.contains("@stored") && stored.contains("Keep the def in the database"),
        "{stored}"
    );
    let insert = hover(&mut server, 11, 2);
    assert!(insert.contains("insert into archive"), "{insert}");

    let symbols = result(
        &mut server,
        "textDocument/documentSymbol",
        object([("textDocument", object([("uri", uri.as_str().into())]))]),
    );
    let listed: Vec<(&str, &str)> = symbols
        .items()
        .iter()
        .map(|symbol| {
            (
                symbol.get("name").as_str().unwrap(),
                symbol.get("detail").as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("archive", "a table"),
            ("risky", "a query"),
            ("insert into archive", "changes rows")
        ]
    );
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn the_sql_is_given_for_an_editor_to_show() {
    let (mut server, folder) = with_schema("compile");
    let uri = uri_of(&folder.join("q.bcql"));
    open(
        &mut server,
        &uri,
        "def for_operator(operator: str, rows: int = 10):\n    from cases\n    where account.operator == operator\n    take rows\n\nfrom cases\nwhere status == \"open\"\n",
    );
    let compiled = result(
        &mut server,
        "buttercupql/compile",
        object([("textDocument", object([("uri", uri.as_str().into())]))]),
    );
    assert_eq!(compiled.get("dialect").as_str(), Some("sqlite"));
    assert_eq!(
        compiled.get("sql").as_str(),
        Some("SELECT *\nFROM cases\nWHERE status = ?")
    );
    assert_eq!(compiled.get("params").items(), [Json::from("open")]);
    let query = result(
        &mut server,
        "buttercupql/compile",
        object([
            ("textDocument", object([("uri", uri.as_str().into())])),
            ("query", "for_operator".into()),
        ]),
    );
    assert_eq!(
        query.get("params").to_string(),
        r#"[{"arg":"operator"},{"arg":"rows"}]"#
    );
    assert_eq!(
        query.get("args").to_string(),
        r#"[{"name":"operator","type":"text","default":null},{"name":"rows","type":"a whole number","default":10}]"#
    );

    server.handle(&notify(
        "textDocument/didChange",
        object([
            ("textDocument", object([("uri", uri.as_str().into())])),
            (
                "contentChanges",
                vec![object([("text", "from cases\nwhere stauts == 1\n".into())])].into(),
            ),
        ]),
    ));
    let failed = result(
        &mut server,
        "buttercupql/compile",
        object([("textDocument", object([("uri", uri.as_str().into())]))]),
    );
    let shown = failed.get("problems").items()[0].as_str().unwrap();
    assert!(shown.contains("did you mean `status`?"), "{shown}");

    server.handle(&notify(
        "textDocument/didChange",
        object([
            ("textDocument", object([("uri", uri.as_str().into())])),
            (
                "contentChanges",
                vec![object([(
                    "text",
                    "with transaction:\n    from cases\n    where risk > 90\n    update status = \"urgent\"\n\nfrom cases\nwhere status == \"urgent\"\n".into(),
                )])]
                .into(),
            ),
        ]),
    ));
    let script = result(
        &mut server,
        "buttercupql/compile",
        object([("textDocument", object([("uri", uri.as_str().into())]))]),
    );
    let statements: Vec<(&str, &str)> = script
        .get("statements")
        .items()
        .iter()
        .map(|statement| {
            (
                statement.get("kind").as_str().unwrap(),
                statement.get("sql").as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        statements,
        [
            ("begin", "BEGIN"),
            ("change", "UPDATE cases SET status = ?\nWHERE risk > 90"),
            ("commit", "COMMIT"),
            ("query", "SELECT *\nFROM cases\nWHERE status = ?"),
        ]
    );
    assert_eq!(
        script.get("statements").items()[1]
            .get("params")
            .to_string(),
        r#"["urgent"]"#
    );
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn settings_change_the_database_and_a_bad_schema_is_reported() {
    let mut server = started(object([("dialect", "oracle".into())]));
    assert_eq!(server.dialect, None);
    let uri = "file:///work/s.bcql";
    let sent = open(&mut server, uri, "from t\nwhere a == \"x\"\n");
    assert!(published(&sent, uri).is_some());
    let shown = |server: &mut Server| {
        result(
            server,
            "buttercupql/compile",
            object([("textDocument", object([("uri", uri.into())]))]),
        )
        .get("sql")
        .as_str()
        .map(str::to_string)
    };
    assert_eq!(
        shown(&mut server).as_deref(),
        Some("SELECT *\nFROM t\nWHERE a = $1")
    );

    let folder = folder("settings");
    let schema = folder.join("schema.sql");
    std::fs::write(&schema, "CREATE TABLE t (a text").unwrap();
    let sent = server.handle(&notify(
        "workspace/didChangeConfiguration",
        object([(
            "settings",
            object([(
                "buttercupql",
                object([
                    ("dialect", "clickhouse".into()),
                    ("schema", schema.display().to_string().into()),
                ]),
            )]),
        )]),
    ));
    let warning = sent
        .iter()
        .find(|message| message.get("method").as_str() == Some("window/showMessage"))
        .expect("the editor is told");
    assert_eq!(warning.get("params").get("type").as_u64(), Some(2));
    assert!(
        warning
            .get("params")
            .get("message")
            .as_str()
            .unwrap()
            .contains("can't be read"),
        "{warning:?}"
    );
    assert_eq!(server.dialect, Some(Dialect::ClickHouse));
    assert_eq!(
        shown(&mut server).as_deref(),
        Some("SELECT *\nFROM t\nWHERE a = {p1:String}")
    );
    // Told once, not at every keystroke.
    let again = server.handle(&notify("textDocument/didSave", object([])));
    assert!(
        again
            .iter()
            .all(|message| message.get("method").as_str() != Some("window/showMessage"))
    );
    let _ = std::fs::remove_dir_all(folder);
}

#[test]
fn file_uris_and_paths_turn_into_each_other() {
    assert_eq!(
        path_of("file:///Users/me/My%20Queries/open.bcql"),
        Some(PathBuf::from("/Users/me/My Queries/open.bcql"))
    );
    assert_eq!(
        path_of("file:///c%3A/Users/me/open.bcql"),
        Some(PathBuf::from("c:/Users/me/open.bcql"))
    );
    assert_eq!(path_of("untitled:Untitled-1"), None);
    assert_eq!(
        uri_of(Path::new("/Users/me/My Queries/é.bcql")),
        "file:///Users/me/My%20Queries/%C3%A9.bcql"
    );
    assert_eq!(
        uri_of(Path::new("c:\\Users\\me\\a.bcql")),
        "file:///c:/Users/me/a.bcql"
    );
}
