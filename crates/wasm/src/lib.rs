//! ButtercupQL in the browser: the compiler built for WebAssembly, with a small interface for JavaScript.
//!
//! JavaScript asks for memory with [`bcql_alloc`], writes the text, the database's name, the schema and the name of
//! a query to compile into it, and calls [`bcql_compile`]. The answer is a JSON text, which it reads from
//! [`bcql_output`], as long as [`bcql_compile`] said. [`answer`] says what the JSON holds.

use std::cell::RefCell;

use bcql::json::{self, Json, object};
use buttercupql::ast::{ExprKind, Item, StepKind};
use buttercupql::{Diagnostic, Dialect, Options, Param, Schema, Severity, SourceFile};

thread_local! {
    /// The last answer, kept until the next.
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Memory for JavaScript to write `len` bytes into.
#[unsafe(no_mangle)]
pub extern "C" fn bcql_alloc(len: usize) -> *mut u8 {
    let mut memory = Vec::<u8>::with_capacity(len.max(1));
    let pointer = memory.as_mut_ptr();
    std::mem::forget(memory);
    pointer
}

/// Gives back memory from [`bcql_alloc`].
///
/// # Safety
///
/// `pointer` and `len` must be those of one call to [`bcql_alloc`], given back once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bcql_free(pointer: *mut u8, len: usize) {
    // SAFETY: the caller gives back memory that `bcql_alloc` made with this capacity.
    drop(unsafe { Vec::from_raw_parts(pointer, 0, len.max(1)) });
}

/// Compiles a text, and returns the length of the answer, which [`bcql_output`] points to.
///
/// # Safety
///
/// Each pointer and length must be those of memory from [`bcql_alloc`] that holds that many bytes, or a length of 0.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn bcql_compile(
    text: *const u8,
    text_len: usize,
    dialect: *const u8,
    dialect_len: usize,
    schema: *const u8,
    schema_len: usize,
    query: *const u8,
    query_len: usize,
) -> usize {
    // SAFETY: the caller gives memory that holds these many bytes.
    let (text, dialect, schema, query) = unsafe {
        (
            read(text, text_len),
            read(dialect, dialect_len),
            read(schema, schema_len),
            read(query, query_len),
        )
    };
    let answer = answer(&text, &dialect, &schema, &query).to_string();
    OUTPUT.with(|output| {
        let mut output = output.borrow_mut();
        *output = answer.into_bytes();
        output.len()
    })
}

/// Where the last answer is.
#[unsafe(no_mangle)]
pub extern "C" fn bcql_output() -> *const u8 {
    OUTPUT.with(|output| output.borrow().as_ptr())
}

/// A text from JavaScript's memory.
///
/// # Safety
///
/// `pointer` holds `len` bytes, or `len` is 0.
unsafe fn read(pointer: *const u8, len: usize) -> String {
    if len == 0 {
        return String::new();
    }
    // SAFETY: the caller says the memory holds `len` bytes.
    let bytes = unsafe { std::slice::from_raw_parts(pointer, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

/// The answer for a text, as JSON:
///
/// - `ok`: whether it compiled;
/// - `sql`, `params`, `names`, `args`, `statements` and `warnings`, when it did, each statement with its `sql`,
///   `params`, `names` and `kind`: `query`, `change`, `definition`, `begin` or `commit`;
/// - `problems`, when it did not, or `schema_problems` for a schema that can't be read;
/// - `queries`: the named queries and query `def`s that can be compiled in place of the main query;
/// - `version`.
///
/// Each problem has its `severity`, `message`, `label`, `help`, where it is from `line` and `column` to `end_line` and
/// `end_column`, all counted from 1 in characters, and `rendered`, as `bcql` shows it.
pub fn answer(text: &str, dialect: &str, schema: &str, query: &str) -> Json {
    let queries = queries(text);
    let version = Json::from(buttercupql::VERSION);
    let dialect: Dialect = match dialect.parse() {
        Ok(dialect) => dialect,
        Err(problem) => {
            return object([
                ("ok", false.into()),
                ("error", problem.to_string().into()),
                ("queries", queries),
                ("version", version),
            ]);
        }
    };
    let schema = if schema.trim().is_empty() {
        None
    } else {
        match Schema::from_sql(schema) {
            Ok(schema) => Some(schema),
            Err(problems) => {
                return object([
                    ("ok", false.into()),
                    ("schema_problems", shown(&problems, "schema.sql", schema)),
                    ("queries", queries),
                    ("version", version),
                ]);
            }
        }
    };
    let mut options = Options::new(dialect).with_name("query.bcql");
    if let Some(schema) = &schema {
        options = options.with_schema(schema);
    }
    if !query.is_empty() {
        options = options.with_query(query);
    }
    match buttercupql::compile(text, &options) {
        Ok(compiled) => object([
            ("ok", true.into()),
            ("sql", compiled.sql.as_str().into()),
            (
                "params",
                compiled.params.iter().map(param).collect::<Vec<_>>().into(),
            ),
            (
                "names",
                compiled
                    .names
                    .iter()
                    .map(|name| Json::from(name.as_str()))
                    .collect::<Vec<_>>()
                    .into(),
            ),
            (
                "args",
                compiled
                    .args
                    .iter()
                    .map(|arg| {
                        object([
                            ("name", arg.name.as_str().into()),
                            ("type", arg.ty.describe().into()),
                            ("default", arg.default.as_ref().map(json::value).into()),
                        ])
                    })
                    .collect::<Vec<_>>()
                    .into(),
            ),
            (
                "statements",
                compiled
                    .statements
                    .iter()
                    .map(|statement| {
                        object([
                            ("sql", statement.sql.as_str().into()),
                            (
                                "params",
                                statement
                                    .params
                                    .iter()
                                    .map(param)
                                    .collect::<Vec<_>>()
                                    .into(),
                            ),
                            (
                                "names",
                                statement
                                    .names
                                    .iter()
                                    .map(|name| Json::from(name.as_str()))
                                    .collect::<Vec<_>>()
                                    .into(),
                            ),
                            ("kind", statement.kind.name().into()),
                        ])
                    })
                    .collect::<Vec<_>>()
                    .into(),
            ),
            ("warnings", shown(&compiled.warnings, "query.bcql", text)),
            ("queries", queries),
            ("version", version),
        ]),
        Err(problems) => object([
            ("ok", false.into()),
            ("problems", shown(&problems, "query.bcql", text)),
            ("queries", queries),
            ("version", version),
        ]),
    }
}

/// What goes in a placeholder: a value, or `{"arg": name}` for an argument given when the query runs.
fn param(param: &Param) -> Json {
    match param {
        Param::Value(value) => json::value(value),
        Param::Arg(name) => object([("arg", name.as_str().into())]),
    }
}

/// The named queries and query `def`s of a text.
fn queries(text: &str) -> Json {
    let parsed = buttercupql::parse(text);
    parsed
        .file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Let(named) if matches!(named.value.kind, ExprKind::Query(_)) => {
                Some(Json::from(named.name.text.as_str()))
            }
            Item::Def(def)
                if matches!(
                    def.body.first().map(|step| &step.kind),
                    Some(StepKind::From(_))
                ) =>
            {
                Some(Json::from(def.name.text.as_str()))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .into()
}

fn shown(problems: &[Diagnostic], name: &str, text: &str) -> Json {
    let source = SourceFile::new(name, text);
    problems
        .iter()
        .map(|problem| {
            let (line, column) = source.location(problem.span.start);
            let (end_line, end_column) = source.location(problem.span.end);
            object([
                (
                    "severity",
                    match problem.severity {
                        Severity::Error => "error",
                        Severity::Warning => "warning",
                    }
                    .into(),
                ),
                ("message", problem.message.as_str().into()),
                ("label", problem.label.as_str().into()),
                (
                    "help",
                    problem
                        .help
                        .iter()
                        .map(|help| Json::from(help.as_str()))
                        .collect::<Vec<_>>()
                        .into(),
                ),
                ("line", line.into()),
                ("column", column.into()),
                ("end_line", end_line.into()),
                ("end_column", end_column.into()),
                ("rendered", problem.render(&source, false).into()),
            ])
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(text: &str, dialect: &str, schema: &str, query: &str) -> Json {
        let parts: Vec<(*mut u8, usize)> = [text, dialect, schema, query]
            .iter()
            .map(|part| {
                let pointer = bcql_alloc(part.len());
                // SAFETY: the memory holds `part.len()` bytes.
                unsafe { std::ptr::copy_nonoverlapping(part.as_ptr(), pointer, part.len()) };
                (pointer, part.len())
            })
            .collect();
        // SAFETY: each part is memory from `bcql_alloc` that holds its length.
        let len = unsafe {
            bcql_compile(
                parts[0].0, parts[0].1, parts[1].0, parts[1].1, parts[2].0, parts[2].1, parts[3].0,
                parts[3].1,
            )
        };
        // SAFETY: the answer is `len` bytes at `bcql_output()`, until the next call.
        let answer =
            unsafe { std::str::from_utf8(std::slice::from_raw_parts(bcql_output(), len)).unwrap() };
        let answer = Json::parse(answer).expect("the answer is JSON");
        for (pointer, len) in parts {
            // SAFETY: each is given back once.
            unsafe { bcql_free(pointer, len) };
        }
        answer
    }

    #[test]
    fn a_query_compiles_through_the_interface() {
        let answer = call(
            "from cases\nwhere status == \"open\"\ntake 5\n",
            "sqlite",
            "",
            "",
        );
        assert_eq!(answer.get("ok"), &Json::Bool(true));
        assert_eq!(
            answer.get("sql").as_str(),
            Some("SELECT *\nFROM cases\nWHERE status = ?\nLIMIT 5")
        );
        assert_eq!(answer.get("params").to_string(), r#"["open"]"#);
        assert_eq!(answer.get("version").as_str(), Some(buttercupql::VERSION));
    }

    #[test]
    fn problems_say_where_they_are() {
        let answer = call("from cases\nwhere status = \"🌼\"\n", "postgres", "", "");
        assert_eq!(answer.get("ok"), &Json::Bool(false));
        let problem = &answer.get("problems").items()[0];
        assert_eq!(problem.get("message").as_str(), Some("use `==` to compare"));
        assert_eq!(
            (
                problem.get("line").as_u64(),
                problem.get("column").as_u64(),
                problem.get("end_column").as_u64()
            ),
            (Some(2), Some(14), Some(15))
        );
        assert!(
            problem
                .get("rendered")
                .as_str()
                .unwrap()
                .starts_with("error: use `==` to compare\n  ┌─ query.bcql:2:14\n")
        );
    }

    #[test]
    fn a_schema_and_a_query_def_are_used() {
        let schema = "CREATE TABLE cases (key text NOT NULL, status text NOT NULL, risk integer);";
        let text = "def risky(at_least: int = 90):\n    from cases\n    where risk >= at_least\n\nfrom cases\nwhere stauts == \"open\"\n";
        let answer = call(text, "duckdb", schema, "");
        let problem = &answer.get("problems").items()[0];
        assert_eq!(
            problem.get("label").as_str(),
            Some("did you mean `status`?")
        );
        assert_eq!(answer.get("queries").to_string(), r#"["risky"]"#);
        let answer = call(text, "duckdb", schema, "risky");
        assert_eq!(
            answer.get("sql").as_str(),
            Some("SELECT *\nFROM cases\nWHERE risk >= $1")
        );
        assert_eq!(
            answer.get("args").to_string(),
            r#"[{"name":"at_least","type":"a whole number","default":90}]"#
        );
        let script = call(
            "create table archive:\n    key: str\n\nfrom cases\nwhere risk > 90\nselect key\ninsert into archive\n",
            "sqlite",
            schema,
            "",
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
                (
                    "definition",
                    "CREATE TABLE archive (\n  key TEXT NOT NULL\n)"
                ),
                (
                    "change",
                    "INSERT INTO archive (key)\nSELECT key\nFROM cases\nWHERE risk > 90"
                ),
            ]
        );
        let answer = call("from t\n", "oracle", "", "");
        assert_eq!(
            answer.get("error").as_str(),
            Some("unknown dialect `oracle`: use sqlite, postgres, duckdb or clickhouse")
        );
        let answer = call("from t\n", "sqlite", "CREATE TABLE (", "");
        assert!(!answer.get("schema_problems").items().is_empty());
    }
}
