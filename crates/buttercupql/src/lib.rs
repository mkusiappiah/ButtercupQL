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
mod diagnostic;
mod dialect;
mod lexer;
mod parser;
mod span;
mod suggest;

pub use diagnostic::{Diagnostic, Label, Severity, SourceFile};
pub use dialect::{Dialect, UnknownDialect};
pub use span::Span;

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
