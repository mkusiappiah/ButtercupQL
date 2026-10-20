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
pub mod builtins;
mod compile;
mod diagnostic;
mod dialect;
mod lexer;
mod module;
mod parser;
mod schema;
mod span;
mod sql;
mod suggest;

use std::fmt;

pub use diagnostic::{Diagnostic, Label, Severity, SourceFile, SourceText};
pub use dialect::{Dialect, UnknownDialect};
pub use module::{FileLoader, Loader, ModuleName};
pub use schema::{Column, Reference, Schema, Table, Type};
pub use span::Span;
pub use sql::{Param, ParamStyle, Value};

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

/// How to compile: for which database, with what knowledge of its tables, and where modules come from.
#[derive(Clone, Copy)]
pub struct Options<'s> {
    /// The database the SQL is for.
    pub dialect: Dialect,
    /// The database's tables, to check names against. Without it, names are written as they are and the database
    /// checks them.
    pub schema: Option<&'s Schema>,
    /// How placeholders are written. Without it, as the database's own drivers take them: `$1` for PostgreSQL and
    /// DuckDB, `?` for SQLite and `{p1:String}` for ClickHouse.
    pub params: Option<ParamStyle>,
    /// The name of the text compiled, such as its file's path: messages show it, and a [`FileLoader`] finds
    /// relative imports from it.
    pub name: &'s str,
    /// Where modules come from, for a text that imports them.
    pub loader: Option<&'s dyn Loader>,
    /// A named query or a query `def` of the text to compile, in place of its main query. A `def`'s parameters
    /// become placeholders, given when the query runs: see [`Compiled::bind`].
    pub query: Option<&'s str>,
}

impl fmt::Debug for Options<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Options")
            .field("dialect", &self.dialect)
            .field("schema", &self.schema)
            .field("params", &self.params)
            .field("name", &self.name)
            .field("loader", &self.loader.map(|_| "a loader"))
            .field("query", &self.query)
            .finish()
    }
}

impl<'s> Options<'s> {
    /// Options for a database, with no schema.
    pub fn new(dialect: Dialect) -> Options<'s> {
        Options {
            dialect,
            schema: None,
            params: None,
            name: "<query>",
            loader: None,
            query: None,
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

    /// The options, with the name of the text compiled, such as its file's path.
    #[must_use]
    pub fn with_name(mut self, name: &'s str) -> Options<'s> {
        self.name = name;
        self
    }

    /// The options, with a loader for the modules the text imports.
    #[must_use]
    pub fn with_loader(mut self, loader: &'s dyn Loader) -> Options<'s> {
        self.loader = Some(loader);
        self
    }

    /// The options, compiling a named query or a query `def` in place of the text's main query.
    #[must_use]
    pub fn with_query(mut self, query: &'s str) -> Options<'s> {
        self.query = Some(query);
        self
    }
}

/// An argument of a query `def` compiled with its parameters open: given when the query runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    /// The parameter's name.
    pub name: String,
    /// The type its annotation names, if it has one.
    pub ty: Type,
    /// Its value when none is given.
    pub default: Option<Value>,
}

/// A query compiled to SQL.
#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    /// The SQL.
    pub sql: String,
    /// What goes in each placeholder, in order: a value written in the query, or an argument given when it runs.
    pub params: Vec<Param>,
    /// Each placeholder's name, in order: `p1`, `p2` for values, and each argument's own name. The SQL uses them
    /// with the `:name` and `{name:Type}` styles.
    pub names: Vec<String>,
    /// The arguments of the query, for a `def` compiled with [`Options::with_query`].
    pub args: Vec<Arg>,
    /// Problems that do not stop the query compiling, but may mean it does not do what was meant.
    pub warnings: Vec<Diagnostic>,
}

/// An argument that was not given, or given but unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindError(pub String);

impl fmt::Display for BindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BindError {}

impl Compiled {
    /// The values for the placeholders, in order, with the query's arguments given by name. An argument not given
    /// takes its default.
    ///
    /// ```
    /// use buttercupql::{Dialect, Options, Value};
    ///
    /// let text = "def for_account(account: str, rows: int = 10):\n    from cases\n    where account_id == account\n    take rows\n";
    /// let compiled = buttercupql::compile(text, &Options::new(Dialect::Postgres).with_query("for_account")).unwrap();
    /// assert_eq!(compiled.sql, "SELECT *\nFROM cases\nWHERE account_id = $1\nLIMIT $2");
    /// let values = compiled.bind(&[("account", Value::Text("A1".to_string()))]).unwrap();
    /// assert_eq!(values, [Value::Text("A1".to_string()), Value::Int(10)]);
    /// ```
    pub fn bind(&self, args: &[(&str, Value)]) -> Result<Vec<Value>, BindError> {
        for (name, _) in args {
            if !self.args.iter().any(|arg| arg.name == *name) {
                let known: Vec<&str> = self.args.iter().map(|arg| arg.name.as_str()).collect();
                return Err(BindError(if known.is_empty() {
                    format!("the query takes no arguments, but `{name}` was given")
                } else {
                    format!(
                        "the query has no argument `{name}`: it takes {}",
                        known.join(", ")
                    )
                }));
            }
        }
        self.params
            .iter()
            .map(|param| match param {
                Param::Value(value) => Ok(value.clone()),
                Param::Arg(name) => args
                    .iter()
                    .find(|(given, _)| given == name)
                    .map(|(_, value)| value.clone())
                    .or_else(|| {
                        self.args
                            .iter()
                            .find(|arg| &arg.name == name)
                            .and_then(|arg| arg.default.clone())
                    })
                    .ok_or_else(|| BindError(format!("the query needs a value for `{name}`"))),
            })
            .collect()
    }

    /// The values for the placeholders, in order, for a query with no arguments to give, or with defaults for all.
    pub fn values(&self) -> Result<Vec<Value>, BindError> {
        self.bind(&[])
    }

    /// The values for the placeholders by name, each name once: for the `:name` and `{name:Type}` styles.
    pub fn bind_named(&self, args: &[(&str, Value)]) -> Result<Vec<(String, Value)>, BindError> {
        let values = self.bind(args)?;
        let mut named: Vec<(String, Value)> = Vec::new();
        for (name, value) in self.names.iter().zip(values) {
            if !named.iter().any(|(seen, _)| seen == name) {
                named.push((name.clone(), value));
            }
        }
        Ok(named)
    }
}

/// Compiles a ButtercupQL text to SQL for a database. On failure, every problem found is returned; a problem in an
/// imported module says so in [`Diagnostic::file`].
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
/// assert_eq!(compiled.values().unwrap(), [Value::Text("open".to_string())]);
/// ```
pub fn compile(text: &str, options: &Options<'_>) -> Result<Compiled, Vec<Diagnostic>> {
    let parsed = parse(text);
    if parsed.has_errors() {
        return Err(parsed.diagnostics);
    }
    let mut diagnostics = parsed.diagnostics;
    let main = module::Loaded::new(
        SourceText {
            name: options.name.to_string(),
            text: text.to_string(),
        },
        parsed.file,
    );
    let modules = module::load_all(main, options.loader, &mut diagnostics);
    let mut compiler = compile::Compiler::new(&modules, options.dialect, options.schema);
    let compiled = match options.query {
        Some(name) => compiler.named(name),
        None => compiler.main().map(|select| (select, Vec::new())),
    };
    diagnostics.append(&mut compiler.diagnostics);
    diagnostics.sort_by_key(|diagnostic| {
        (
            diagnostic.file.is_some(),
            diagnostic.span.start,
            diagnostic.span.end,
        )
    });
    let failed = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error);
    match compiled {
        Some((select, args)) if !failed => {
            let style = options.params.unwrap_or(ParamStyle::of(options.dialect));
            let written = sql::write(&select, options.dialect, style);
            Ok(Compiled {
                sql: written.sql,
                params: written.params,
                names: written.names,
                args,
                warnings: diagnostics,
            })
        }
        _ => Err(diagnostics),
    }
}
