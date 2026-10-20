//! The documentation's examples, run with the `bcql` command: every ButtercupQL query in the book, the README and the
//! design reads without a problem; every query shown with its SQL compiles to exactly that SQL; and every mistake
//! shown with its message gets exactly that message.
//!
//! In the Markdown, a ```` ```python ```` block is ButtercupQL, as Python's colours suit it, and a ```` ```py ````
//! block is Python. A ```` ```sql ```` block after a ButtercupQL block is what `bcql compile` writes for it: for
//! PostgreSQL, unless the words after `sql` name another database (`sqlite`, `duckdb` or `clickhouse`), with `schema`
//! to compile with the playground's schema and `query=NAME` to compile a named query or a query `def`. A block that
//! leaves out the `-- parameters:` line is compared without it. A ```` ```python mistake ```` block is ButtercupQL
//! with mistakes, and a ```` ```text ```` block after it is what `bcql check` shows for it, with the playground's
//! schema for ```` ```text schema ````. A
//! ```` ```python file=shared.bcql ```` block is a module, written beside the queries that import it.
//!
//! `BCQL_BLESS=1 cargo test -p bcql --test book` writes what the command gives into the Markdown. Read the changes
//! before committing them.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// A fenced block of a Markdown file.
#[derive(Debug, Clone)]
struct Block {
    /// The words after the opening fence.
    info: Vec<String>,
    text: String,
    /// The lines of its text, counted from 0: from the line after the opening fence to the closing fence.
    lines: (usize, usize),
}

fn blocks(markdown: &str) -> Vec<Block> {
    let mut found = Vec::new();
    let mut open: Option<(Vec<String>, usize, String)> = None;
    for (number, line) in markdown.lines().enumerate() {
        match &mut open {
            None => {
                if let Some(info) = line.strip_prefix("```") {
                    let info = info.split_whitespace().map(str::to_string).collect();
                    open = Some((info, number + 1, String::new()));
                }
            }
            Some((info, start, text)) => {
                if line == "```" {
                    found.push(Block {
                        info: std::mem::take(info),
                        text: std::mem::take(text),
                        lines: (*start, number),
                    });
                    open = None;
                } else {
                    text.push_str(line);
                    text.push('\n');
                }
            }
        }
    }
    found
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A text file, with its lines ending as they do in the repository: a Windows checkout ends them with `\r\n`.
fn read(path: &Path) -> String {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("can't read {}: {error}", path.display()))
        .replace("\r\n", "\n")
}

/// The Markdown files whose examples are checked.
fn documents() -> Vec<PathBuf> {
    let mut files = vec![root().join("README.md"), root().join("docs/design.md")];
    let book = root().join("docs/book/src");
    let mut pages: Vec<PathBuf> = fs::read_dir(&book)
        .expect("the book can be read")
        .map(|entry| entry.expect("a page of the book").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .collect();
    pages.sort();
    files.extend(pages);
    files
}

/// Runs `bcql` in a folder of its own, with the query written to `query.bcql` there.
fn bcql(folder: &Path, query: &str, args: &[&str]) -> (bool, String, String) {
    fs::write(folder.join("query.bcql"), query).expect("the query is written");
    let out = Command::new(env!("CARGO_BIN_EXE_bcql"))
        .args(args)
        .current_dir(folder)
        .env_remove("NO_COLOR")
        .output()
        .expect("bcql runs");
    let text = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    (out.status.success(), text(&out.stdout), text(&out.stderr))
}

#[test]
fn the_documentation_shows_what_the_compiler_does() {
    let bless = std::env::var_os("BCQL_BLESS").is_some();
    let folder = std::env::temp_dir().join(format!("bcql-book-{}", std::process::id()));
    fs::create_dir_all(&folder).expect("a folder for the queries");
    let schema = root().join("playground/schema.sql");
    let schema = schema.to_str().expect("the schema's path is text");
    let mut problems = Vec::new();
    let (mut queries, mut compiled, mut mistakes) = (0, 0, 0);
    for file in documents() {
        let markdown = read(&file);
        let blocks = blocks(&markdown);
        let mut written: Vec<(usize, usize, String)> = Vec::new();
        let name = file
            .strip_prefix(root())
            .unwrap_or(&file)
            .display()
            .to_string();
        for (index, block) in blocks.iter().enumerate() {
            let place = format!("{name}:{}", block.lines.0);
            let kind = block.info.first().map(String::as_str);
            let mistake = block.info.iter().any(|word| word == "mistake");
            if let Some(module) = block
                .info
                .iter()
                .find_map(|word| word.strip_prefix("file="))
            {
                fs::write(folder.join(module), &block.text).expect("the module is written");
            }
            if kind == Some("python") && !mistake {
                let parsed = buttercupql::parse(&block.text);
                if !parsed.diagnostics.is_empty() {
                    problems.push(format!(
                        "{place}: the query has problems: {:?}",
                        parsed.diagnostics
                    ));
                }
                queries += 1;
            }
            let Some(before) = index.checked_sub(1).map(|before| &blocks[before]) else {
                continue;
            };
            if before.info.first().map(String::as_str) != Some("python") {
                continue;
            }
            let before_mistake = before.info.iter().any(|word| word == "mistake");
            let actual = match (kind, before_mistake) {
                (Some("sql"), false) => {
                    let mut args = vec!["compile", "query.bcql", "--color", "never"];
                    for word in &block.info[1..] {
                        match word.as_str() {
                            "sqlite" | "postgres" | "duckdb" | "clickhouse" => {
                                args.extend(["--dialect", word]);
                            }
                            "schema" => args.extend(["--schema", schema]),
                            other => match other.strip_prefix("query=") {
                                Some(query) => args.extend(["--query", query]),
                                None => {
                                    problems.push(format!("{place}: an unknown word `{other}`"))
                                }
                            },
                        }
                    }
                    let (ok, out, err) = bcql(&folder, &before.text, &args);
                    if !ok {
                        problems.push(format!("{place}: the query does not compile:\n{err}"));
                        continue;
                    }
                    compiled += 1;
                    // A block without the parameters is compared without them.
                    let shows_comments = block.text.lines().any(|line| line.starts_with("-- "));
                    if shows_comments || bless {
                        out
                    } else {
                        let kept: Vec<&str> = out
                            .lines()
                            .take_while(|line| !line.starts_with("-- "))
                            .collect();
                        kept.join("\n") + "\n"
                    }
                }
                (Some("text"), true) => {
                    let mut args = vec!["check", "query.bcql", "--color", "never"];
                    if block.info.iter().any(|word| word == "schema") {
                        args.extend(["--schema", schema]);
                    }
                    let (ok, _, err) = bcql(&folder, &before.text, &args);
                    // A mistake is an error, or a warning that the query likely does not do what was meant.
                    if ok && !err.contains("warning: ") {
                        problems.push(format!("{place}: the mistake is not found"));
                        continue;
                    }
                    mistakes += 1;
                    err
                }
                _ => continue,
            };
            if actual != block.text {
                if bless {
                    written.push((block.lines.0, block.lines.1, actual));
                } else {
                    problems.push(format!(
                        "{place}: the documentation shows\n{}\nbut bcql gives\n{actual}",
                        block.text
                    ));
                }
            }
        }
        if bless && !written.is_empty() {
            let mut lines: Vec<String> = markdown.lines().map(str::to_string).collect();
            for (start, end, text) in written.into_iter().rev() {
                lines.splice(start..end, text.lines().map(str::to_string));
            }
            let mut markdown = lines.join("\n");
            markdown.push('\n');
            fs::write(&file, markdown).expect("the page is written");
        }
    }
    let _ = fs::remove_dir_all(&folder);
    assert!(problems.is_empty(), "{}", problems.join("\n\n"));
    assert!(queries >= 40, "only {queries} queries were read");
    assert!(compiled >= 30, "only {compiled} queries were compiled");
    assert!(mistakes >= 5, "only {mistakes} mistakes were checked");
}

#[test]
fn the_reference_describes_every_function_and_method() {
    let reference = read(&root().join("docs/book/src/functions.md"));
    let builtins = buttercupql::builtins::FUNCTIONS
        .iter()
        .chain(buttercupql::builtins::METHODS)
        .chain(buttercupql::builtins::DATE_PARTS)
        .chain(buttercupql::builtins::STEPS)
        .chain(buttercupql::builtins::STATEMENTS)
        .chain(buttercupql::builtins::DECORATORS);
    for builtin in builtins {
        let shown = format!("`{}`", builtin.signature);
        assert!(
            reference.contains(&shown),
            "the reference does not show {shown}"
        );
    }
}

#[test]
fn the_schema_the_book_shows_is_the_one_its_examples_use() {
    let schema = read(&root().join("playground/schema.sql"));
    let statements: Vec<&str> = schema
        .lines()
        .filter(|line| !line.starts_with("--"))
        .collect();
    let statements = statements.join("\n");
    let page = read(&root().join("docs/book/src/schemas.md"));
    assert!(
        page.contains(statements.trim()),
        "the schema in schemas.md is not playground/schema.sql"
    );
}

#[test]
fn the_command_page_shows_the_help() {
    let out = Command::new(env!("CARGO_BIN_EXE_bcql"))
        .arg("--help")
        .output()
        .expect("bcql runs");
    let help = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    let page = read(&root().join("docs/book/src/command.md"));
    assert!(
        page.contains(&format!("```text\n{help}```")),
        "command.md does not show `bcql --help` as it is:\n{help}"
    );
}
