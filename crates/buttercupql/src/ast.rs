//! The syntax tree of a ButtercupQL file, as the parser reads it.
//!
//! Every part keeps its [`Span`], the place in the text it was read from, so that later stages can point at it.

use crate::span::Span;

/// A name: of a table, a column, a function or a module.
#[derive(Debug, Clone, PartialEq)]
pub struct Name {
    /// The name, with any backticks taken away.
    pub text: String,
    /// Where it is written.
    pub span: Span,
}

/// A dotted name, such as `shared.recent`.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// The names between the dots.
    pub parts: Vec<Name>,
    /// Where it is written.
    pub span: Span,
}

/// A ButtercupQL file: definitions first, then the query.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct File {
    /// The string on the file's first line, if there is one.
    pub docstring: Option<Text>,
    /// Imports, `def`s and named values, in order.
    pub items: Vec<Item>,
    /// The query, when the file has one. A file of definitions alone is a module, for others to import.
    pub query: Option<Query>,
}

/// A string written alone on its line, as the first line of a file or a `def`.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    /// The string's value.
    pub value: String,
    /// Where it is written.
    pub span: Span,
}

/// Something a file defines before its query.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// `import shared` or `from shared import recent`.
    Import(Import),
    /// `def name(parameters):` and its steps.
    Def(Def),
    /// `name = value`, or a named query: `name = (from ...)`.
    Let(Let),
}

/// An import line, or one module of an `import a, b` line.
#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    /// What is imported.
    pub kind: ImportKind,
    /// Where it is written.
    pub span: Span,
}

/// The two forms of Python's imports.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportKind {
    /// `import shared`, or `import shared as s`.
    Module {
        /// The module.
        path: Path,
        /// The name it is used by, when it is not its own.
        alias: Option<Name>,
    },
    /// `from shared import recent, busy as b`.
    Names {
        /// How many dots come before the module's name: `from .shared import x` has 1, for a module beside this
        /// file.
        level: u32,
        /// The module, unless the line is `from . import x`.
        module: Option<Path>,
        /// The names imported from it.
        names: Vec<ImportName>,
    },
}

/// A name in a `from ... import` line.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportName {
    /// The name in the module.
    pub name: Name,
    /// The name it is used by here, when it is not its own.
    pub alias: Option<Name>,
}

/// `def name(parameters):` and its body.
#[derive(Debug, Clone, PartialEq)]
pub struct Def {
    /// The `def`'s name.
    pub name: Name,
    /// Its parameters, in order.
    pub params: Vec<Param>,
    /// The type after `->`, if one is written.
    pub returns: Option<Type>,
    /// The string on the body's first line, if there is one.
    pub docstring: Option<Text>,
    /// The steps. A body that starts with `from` makes the `def` a query; otherwise its steps are put in place
    /// where it is called.
    pub body: Vec<Step>,
    /// Where it is written, from `def` to the end of its body.
    pub span: Span,
}

impl Def {
    /// Whether the `def` is a query, its body starting with `from`, rather than steps to put in place.
    pub fn is_query(&self) -> bool {
        matches!(
            self.body.first(),
            Some(Step {
                kind: StepKind::From(_),
                ..
            })
        )
    }
}

/// A parameter of a `def`: `days_back`, `account: str`, `days_back: int = 30`.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    /// The parameter's name.
    pub name: Name,
    /// Its type, if one is written.
    pub annotation: Option<Type>,
    /// Its value when a call leaves it out.
    pub default: Option<Expr>,
    /// Where it is written.
    pub span: Span,
}

/// A type: `str`, `int`, `list[str]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Type {
    /// The type's name.
    pub name: Path,
    /// The types in its brackets, as in `list[str]`.
    pub args: Vec<Type>,
    /// Where it is written.
    pub span: Span,
}

/// `name = value` before the query.
#[derive(Debug, Clone, PartialEq)]
pub struct Let {
    /// The name given.
    pub name: Name,
    /// The value: a query in brackets makes a named query.
    pub value: Expr,
    /// Where it is written.
    pub span: Span,
}

/// A query: its steps, the first of them a `from`.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// The steps, one per line.
    pub steps: Vec<Step>,
    /// Where it is written.
    pub span: Span,
}

/// A step of a query.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// What the step does.
    pub kind: StepKind,
    /// Where it is written.
    pub span: Span,
}

/// What a step does.
#[derive(Debug, Clone, PartialEq)]
pub enum StepKind {
    /// `from cases`: start from a table, a named query or a query `def`.
    From(Source),
    /// `where condition`: keep the rows where the condition is true.
    Where(Expr),
    /// `name = value`: add a column, or replace one. `risk += 1` is read as `risk = risk + 1`.
    Column {
        /// The column's name.
        name: Name,
        /// Its value on each row.
        value: Expr,
    },
    /// `select a, b, total = x + y`: keep these columns, in this order.
    Select(Vec<Field>),
    /// `del a, b`: drop these columns.
    Del(Vec<Expr>),
    /// `sort -risk, raised`: order the rows.
    Sort {
        /// The columns to sort by, the first of them first.
        keys: Vec<SortKey>,
        /// The value of `reverse=`, which turns the whole order around.
        reverse: Option<Expr>,
    },
    /// `take 20`: keep the first rows.
    Take(Expr),
    /// `skip 40`: leave out the first rows.
    Skip(Expr),
    /// `distinct`: drop repeated rows.
    Distinct,
    /// `group by a, b:` and its block: one row per group, with the block's totals.
    Group {
        /// The columns that make the groups.
        by: Vec<Field>,
        /// The totals worked out for each group.
        totals: Vec<Field>,
    },
    /// `summarize:` and its block: one row of totals for all the rows.
    Summarize(Vec<Field>),
    /// `join accounts on condition`, or `left join`, which keeps the rows that have no match.
    Join {
        /// Whether rows with no match are kept.
        left: bool,
        /// The table joined.
        source: Source,
        /// The condition that pairs rows.
        on: Expr,
    },
    /// `recent(30)`: the steps of a `def`, put in place.
    Call(Expr),
    /// `pass`: nothing.
    Pass,
}

/// What a `from` or a `join` reads from: a table, a named query, a query `def` called with its arguments, or a
/// query in brackets.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// The table: a name, a dotted name, a call or a query.
    pub value: Expr,
    /// The name given with `as`.
    pub alias: Option<Name>,
    /// Where it is written.
    pub span: Span,
}

/// A column of a `select` or a `group by`, or a total: `name = value`, or a value that names itself, such as `risk`.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    /// The name given with `=`.
    pub name: Option<Name>,
    /// The value.
    pub value: Expr,
    /// Where it is written.
    pub span: Span,
}

/// A column to sort by.
#[derive(Debug, Clone, PartialEq)]
pub struct SortKey {
    /// The value sorted by.
    pub value: Expr,
    /// Whether it sorts high to low, written with `-`.
    pub descending: bool,
    /// Where it is written.
    pub span: Span,
}

/// An expression.
#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    /// What the expression is.
    pub kind: ExprKind,
    /// Where it is written.
    pub span: Span,
}

/// What an expression is.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// A name: a column, a parameter, a named value.
    Name(String),
    /// `base.name`: a table's column, a module's `def`, a method.
    Attribute(Box<Expr>, Name),
    /// `function(arguments)`.
    Call(Box<Expr>, Vec<Arg>),
    /// A number.
    Number(Number),
    /// A string.
    Str(String),
    /// An f-string: its text and its values, in order.
    FString(Vec<FPart>),
    /// `True` or `False`.
    Bool(bool),
    /// `None`.
    None,
    /// `-x`, `+x`, `not x`.
    Unary(UnaryOp, Box<Expr>),
    /// `a + b`, `a and b`, and the other operators with two sides.
    Binary(BinaryOp, Box<Expr>, Box<Expr>),
    /// Comparisons, chained as Python chains them: `1 <= risk <= 100` is the first value and two comparisons.
    Compare(Box<Expr>, Vec<(CompareOp, Expr)>),
    /// `then if test else otherwise`.
    IfElse {
        /// The condition.
        test: Box<Expr>,
        /// The value when it is true.
        then: Box<Expr>,
        /// The value when it is not.
        otherwise: Box<Expr>,
    },
    /// `[a, b]`.
    List(Vec<Expr>),
    /// `(a, b)`.
    Tuple(Vec<Expr>),
    /// `{a, b}`.
    Set(Vec<Expr>),
    /// A query in brackets, used as a value.
    Query(Box<Query>),
    /// A part that could not be read. Its problem has been reported.
    Error,
}

/// A number, as SQL writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Number {
    /// The digits, in base 10, without underscores: `1_000_000` is `1000000` and `0xff` is `255`.
    pub text: String,
    /// Whether the number has a fraction or an exponent.
    pub float: bool,
}

/// An argument of a call: a value, or `name=value`.
#[derive(Debug, Clone, PartialEq)]
pub struct Arg {
    /// The name, for an argument given by name.
    pub name: Option<Name>,
    /// The value.
    pub value: Expr,
}

/// A part of an f-string.
#[derive(Debug, Clone, PartialEq)]
pub enum FPart {
    /// Text, as it is.
    Text(String),
    /// A value in braces.
    Value(Expr),
}

/// An operator with one side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnaryOp {
    /// `-x`
    Neg,
    /// `+x`
    Pos,
    /// `not x`
    Not,
}

/// An operator with two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    /// `or`
    Or,
    /// `and`
    And,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`, which always divides exactly, as in Python.
    Div,
    /// `//`, which rounds down.
    FloorDiv,
    /// `%`
    Mod,
    /// `**`
    Pow,
}

/// A comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompareOp {
    /// `==`
    Eq,
    /// `!=`
    NotEq,
    /// `<`
    Lt,
    /// `<=`
    LtEq,
    /// `>`
    Gt,
    /// `>=`
    GtEq,
    /// `in`
    In,
    /// `not in`
    NotIn,
    /// `is`
    Is,
    /// `is not`
    IsNot,
}

impl UnaryOp {
    /// The operator as it is written.
    pub fn symbol(self) -> &'static str {
        match self {
            UnaryOp::Neg => "-",
            UnaryOp::Pos => "+",
            UnaryOp::Not => "not",
        }
    }
}

impl BinaryOp {
    /// The operator as it is written.
    pub fn symbol(self) -> &'static str {
        match self {
            BinaryOp::Or => "or",
            BinaryOp::And => "and",
            BinaryOp::Add => "+",
            BinaryOp::Sub => "-",
            BinaryOp::Mul => "*",
            BinaryOp::Div => "/",
            BinaryOp::FloorDiv => "//",
            BinaryOp::Mod => "%",
            BinaryOp::Pow => "**",
        }
    }
}

impl CompareOp {
    /// The operator as it is written.
    pub fn symbol(self) -> &'static str {
        match self {
            CompareOp::Eq => "==",
            CompareOp::NotEq => "!=",
            CompareOp::Lt => "<",
            CompareOp::LtEq => "<=",
            CompareOp::Gt => ">",
            CompareOp::GtEq => ">=",
            CompareOp::In => "in",
            CompareOp::NotIn => "not in",
            CompareOp::Is => "is",
            CompareOp::IsNot => "is not",
        }
    }
}
