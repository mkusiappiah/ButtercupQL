//! ButtercupQL, a Python-like language for SQL, named after the buttercup flower.
//!
//! A query is written one step per line, in the order the data flows, and compiles to the SQL a database already
//! runs, with every value passed as a parameter. The language is described in `docs/design.md`.
//!
//! ```
//! let parsed = buttercupql::parse(
//!     r#"
//!     from cases
//!     where status == "open" and risk >= 90
//!     sort -risk
//!     take 20
//!     "#,
//! );
//! assert!(parsed.diagnostics.is_empty());
//! assert_eq!(parsed.file.query.unwrap().steps.len(), 4);
//! ```
//!
//! Every problem in a text is found in one reading, each with its place, and shown as a person reads it:
//!
//! ```
//! use buttercupql::SourceFile;
//!
//! let text = "from cases\nwhere status = \"open\"\n";
//! let parsed = buttercupql::parse(text);
//! let shown = parsed.diagnostics[0].render(&SourceFile::new("open.bcql", text), false);
//! assert_eq!(
//!     shown,
//!     "\
//! error: use `==` to compare
//!   ┌─ open.bcql:2:14
//!   │
//! 2 │ where status = \"open\"
//!   │              ^ `=` gives a column a value; `==` compares two values
//! "
//! );
//! ```

pub mod ast;
mod compile;
mod diagnostic;
mod dialect;
mod lexer;
mod parser;
mod schema;
mod span;
mod sql;
mod suggest;

pub use diagnostic::{Diagnostic, Label, Severity, SourceFile};
pub use dialect::{Dialect, UnknownDialect};
pub use schema::{Column, Schema, Table, Type};
pub use span::Span;
pub use sql::{ParamStyle, Value};

/// The version of this crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A ButtercupQL text read into its syntax tree, with every problem found in it.
#[derive(Debug, Clone)]
pub struct Parsed {
    /// The syntax tree. When the text has mistakes, it holds every part that could be read.
    pub file: ast::File,
    /// The problems found, in the order of the text.
    pub diagnostics: Vec<Diagnostic>,
}

impl Parsed {
    /// Whether any of the problems is an error.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }
}

/// Reads a ButtercupQL text into its syntax tree, finding every problem in it.
pub fn parse(text: &str) -> Parsed {
    if u32::try_from(text.len()).is_err() {
        return Parsed {
            file: ast::File::default(),
            diagnostics: vec![Diagnostic::error(
                Span::default(),
                "the text is too long: ButtercupQL reads texts of up to 4 GiB",
            )],
        };
    }
    let (file, diagnostics) = parser::parse(text);
    Parsed { file, diagnostics }
}

/// How to compile: for which database, and with what knowledge of its tables.
#[derive(Debug, Clone, Copy)]
pub struct Options<'s> {
    /// The database the SQL is for.
    pub dialect: Dialect,
    /// The database's tables, to check names against. Without it, names are written as they are and the database
    /// checks them.
    pub schema: Option<&'s Schema>,
    /// How placeholders are written. Without it, as the database's own drivers take them: `$1` for PostgreSQL and
    /// `?` for SQLite.
    pub params: Option<ParamStyle>,
}

impl<'s> Options<'s> {
    /// Options for a database, with no schema.
    pub fn new(dialect: Dialect) -> Options<'s> {
        Options {
            dialect,
            schema: None,
            params: None,
        }
    }

    /// The options, with a schema to check names against.
    #[must_use]
    pub fn with_schema(mut self, schema: &'s Schema) -> Options<'s> {
        self.schema = Some(schema);
        self
    }

    /// The options, with placeholders written in another style.
    #[must_use]
    pub fn with_params(mut self, style: ParamStyle) -> Options<'s> {
        self.params = Some(style);
        self
    }
}

/// A query compiled to SQL.
#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    /// The SQL.
    pub sql: String,
    /// The values for its placeholders, in order.
    pub params: Vec<Value>,
    /// Problems that do not stop the query compiling, but may mean it does not do what was meant.
    pub warnings: Vec<Diagnostic>,
}

/// Compiles a ButtercupQL text to SQL for a database. On failure, every problem found is returned.
///
/// ```
/// use buttercupql::{Dialect, Options, Value};
///
/// let text = "from cases\nwhere status == \"open\"\nsort -risk\ntake 20\n";
/// let compiled = buttercupql::compile(text, &Options::new(Dialect::Postgres)).unwrap();
/// assert_eq!(
///     compiled.sql,
///     "SELECT *\nFROM cases\nWHERE status = $1\nORDER BY risk DESC NULLS LAST\nLIMIT 20"
/// );
/// assert_eq!(compiled.params, [Value::Text("open".to_string())]);
/// ```
pub fn compile(text: &str, options: &Options<'_>) -> Result<Compiled, Vec<Diagnostic>> {
    let parsed = parse(text);
    if parsed.has_errors() {
        return Err(parsed.diagnostics);
    }
    if matches!(options.dialect, Dialect::DuckDb | Dialect::ClickHouse) {
        return Err(vec![
            Diagnostic::error(
                Span::default(),
                format!("compiling for {} is not supported yet", options.dialect),
            )
            .with_help("SQLite and PostgreSQL come first; DuckDB and ClickHouse come next"),
        ]);
    }
    let mut compiler = compile::Compiler::new(&parsed.file, options.dialect, options.schema);
    let select = compiler.main();
    let mut diagnostics = parsed.diagnostics;
    diagnostics.append(&mut compiler.diagnostics);
    diagnostics.sort_by_key(|diagnostic| (diagnostic.span.start, diagnostic.span.end));
    let failed = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error);
    match select {
        Some(select) if !failed => {
            let style = options.params.unwrap_or(ParamStyle::of(options.dialect));
            let written = sql::write(&select, options.dialect, style);
            Ok(Compiled {
                sql: written.sql,
                params: written.params,
                warnings: diagnostics,
            })
        }
        _ => Err(diagnostics),
    }
}
