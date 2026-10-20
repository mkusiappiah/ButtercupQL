//! The SQL ButtercupQL writes: a small syntax tree of the `SELECT` statements it builds, and the writer that turns
//! one into text, with its parameters in the order they appear.

use std::collections::HashMap;

use crate::dialect::Dialect;
use crate::schema::Type;

/// A value passed to the database beside the SQL, for one of its placeholders.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// SQL's `NULL`.
    Null,
    /// `True` or `False`.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A number with a fraction.
    Float(f64),
    /// Text.
    Text(String),
}

/// What goes in one of the SQL's placeholders.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    /// A value written in the query.
    Value(Value),
    /// An argument of the query compiled, given when the query runs. See [`crate::Compiled::bind`].
    Arg(String),
}

/// How placeholders for parameters are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamStyle {
    /// `$1`, `$2`: as PostgreSQL and DuckDB take them, and asyncpg, node-postgres and Rust's sqlx and tokio-postgres.
    Dollar,
    /// `?`: SQLite's, as Python's `sqlite3` takes them.
    Question,
    /// `%s`: as Python's psycopg takes them. A `%` in the SQL is then written `%%`.
    Format,
    /// `:p1`, `:p2`, and an argument by its name: `:account`.
    Colon,
    /// `{p1:String}`, and an argument by its name: `{account:String}`. ClickHouse's own, with each value's type.
    Braces,
}

impl ParamStyle {
    /// The style a database's own drivers take.
    pub fn of(dialect: Dialect) -> ParamStyle {
        match dialect {
            Dialect::Postgres | Dialect::DuckDb => ParamStyle::Dollar,
            Dialect::Sqlite => ParamStyle::Question,
            Dialect::ClickHouse => ParamStyle::Braces,
        }
    }
}

/// A statement, as it is written for a database. A statement lives only while it is written, so its `SELECT`
/// is kept in place rather than boxed.
#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Statement {
    /// A query that gives rows.
    Select(Select),
    /// `UPDATE table SET column = value WHERE ...`.
    Update {
        table: Vec<String>,
        set: Vec<(String, Expr)>,
        filter: Vec<Expr>,
        returning: Vec<Item>,
    },
    /// `DELETE FROM table WHERE ...`.
    Delete {
        table: Vec<String>,
        filter: Vec<Expr>,
        returning: Vec<Item>,
    },
    /// `INSERT INTO table (columns) SELECT ...`, or `... VALUES (...)`.
    Insert {
        table: Vec<String>,
        columns: Vec<String>,
        rows: Rows,
        returning: Vec<Item>,
    },
    /// A statement the compiler wrote out whole, with no placeholders: `CREATE TABLE`, `GRANT`.
    Written(String),
    /// `BEGIN`: the statements after it run together, until `COMMIT`.
    Begin,
    /// `COMMIT`.
    Commit,
}

/// The rows an `INSERT` adds.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Rows {
    /// The rows of a query.
    Query(Box<Select>),
    /// Rows written in the query, each its values in the order of the columns.
    Values(Vec<Vec<Expr>>),
}

/// How the rows of two queries are combined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetOp {
    /// Every row of both: `+`.
    UnionAll,
    /// The rows of both, repeats dropped: `|`.
    Union,
    /// The rows in both: `&`.
    Intersect,
    /// The rows of the first that are not in the second: `-`.
    Except,
}

impl SetOp {
    fn keyword(self, dialect: Dialect) -> &'static str {
        // ClickHouse says which it means, as its own defaults differ.
        match (self, dialect) {
            (SetOp::UnionAll, _) => "UNION ALL",
            (SetOp::Union, Dialect::ClickHouse) => "UNION DISTINCT",
            (SetOp::Union, _) => "UNION",
            (SetOp::Intersect, Dialect::ClickHouse) => "INTERSECT DISTINCT",
            (SetOp::Intersect, _) => "INTERSECT",
            (SetOp::Except, Dialect::ClickHouse) => "EXCEPT DISTINCT",
            (SetOp::Except, _) => "EXCEPT",
        }
    }
}

/// A recursive query: `WITH RECURSIVE name AS (anchor UNION step)`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cte {
    pub(crate) name: String,
    /// The rows it starts with.
    pub(crate) anchor: Box<Select>,
    pub(crate) op: SetOp,
    /// The rows each round adds, from the rows of the round before.
    pub(crate) step: Box<Select>,
}

/// A `SELECT` statement.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Select {
    /// The recursive queries it reads: `WITH RECURSIVE`.
    pub(crate) with: Vec<Cte>,
    pub(crate) distinct: bool,
    /// The columns. None means `*`.
    pub(crate) columns: Vec<Item>,
    pub(crate) from: Option<Source>,
    pub(crate) joins: Vec<Join>,
    /// The conditions of `WHERE`, all of which must hold.
    pub(crate) filter: Vec<Expr>,
    pub(crate) group: Vec<Expr>,
    /// The conditions of `HAVING`.
    pub(crate) having: Vec<Expr>,
    /// The conditions of `QUALIFY`, which hold after window functions are worked out.
    pub(crate) qualify: Vec<Expr>,
    pub(crate) order: Vec<Order>,
    pub(crate) limit: Option<Count>,
    pub(crate) offset: Option<Count>,
    /// ClickHouse's settings for the query: `SETTINGS join_use_nulls = 1`.
    pub(crate) settings: Vec<(&'static str, &'static str)>,
}

impl Select {
    /// Whether the statement's columns are written with their table's name: when it reads more than one table, or,
    /// on ClickHouse, when it gives a column the name of one it reads. ClickHouse reads such a name as the new
    /// column wherever it is written alone, in `WHERE` and in the other columns too, where the old one is meant.
    fn qualified(&self, dialect: Dialect) -> bool {
        !self.joins.is_empty()
            || (dialect == Dialect::ClickHouse
                && self.renamed().any(|name| self.reads_column(name)))
    }

    /// The names of the columns it gives that are not the columns of the same name it reads: `risk` of
    /// `-risk AS risk`.
    fn renamed(&self) -> impl Iterator<Item = &str> {
        self.columns.iter().filter_map(|item| match item {
            Item::Expr {
                expr: Expr::Column { name, .. },
                alias: Some(alias),
            } if name == alias => None,
            Item::Expr {
                alias: Some(alias), ..
            } => Some(alias.as_str()),
            _ => None,
        })
    }

    /// Whether its own clauses read a column of this name from one of its tables.
    fn reads_column(&self, name: &str) -> bool {
        let items = self.columns.iter().any(|item| match item {
            Item::Expr { expr, .. } => expr.reads_column(name),
            Item::Star | Item::TableStar(_) => false,
        });
        items
            || self
                .filter
                .iter()
                .chain(&self.group)
                .chain(&self.having)
                .chain(&self.qualify)
                .chain(self.joins.iter().map(|join| &join.on))
                .any(|expr| expr.reads_column(name))
            || self.order.iter().any(|order| order.expr.reads_column(name))
    }

    /// Whether the statement, or a query inside it, has a `LEFT JOIN`.
    pub(crate) fn has_left_join(&self) -> bool {
        let inside = |source: &Source| match &source.kind {
            SourceKind::Query(select) => select.has_left_join(),
            SourceKind::Compound(left, _, right) => left.has_left_join() || right.has_left_join(),
            SourceKind::Table(_) | SourceKind::Rows(..) => false,
        };
        self.joins
            .iter()
            .any(|join| join.left || inside(&join.source))
            || self.from.as_ref().is_some_and(inside)
    }
}

/// A count of rows for `LIMIT` and `OFFSET`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Count {
    Rows(u64),
    /// A count given when the query runs.
    Param(Param),
}

/// A column of a `SELECT`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Item {
    /// `*`, every column of every table read.
    Star,
    /// `table.*`, every column of one table.
    TableStar(String),
    /// A value, and the name it is given.
    Expr { expr: Expr, alias: Option<String> },
}

/// What a `FROM` or a `JOIN` reads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Source {
    pub(crate) kind: SourceKind,
    pub(crate) alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SourceKind {
    /// A table, maybe with its schema: `analytics.daily`.
    Table(Vec<String>),
    /// A query in brackets.
    Query(Box<Select>),
    /// The rows of two queries, combined: `(SELECT ... UNION ALL SELECT ...)`.
    Compound(Box<Select>, SetOp, Box<Select>),
    /// Rows written in the query, each its values, and the columns' names.
    Rows(Vec<Vec<Expr>>, Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Join {
    pub(crate) left: bool,
    pub(crate) source: Source,
    pub(crate) on: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Order {
    pub(crate) expr: Expr,
    pub(crate) descending: bool,
    /// Whether to write `NULLS LAST`, where the database would otherwise put missing values first.
    pub(crate) nulls_last: bool,
}

/// The rows a window function works over, around each row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Frame {
    /// From the first row to this one: a running total.
    Running,
    /// This row and the ones before it, as many as given in all: a moving total.
    Last(u64),
    /// Every row of the partition.
    Whole,
}

/// An SQL expression.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Expr {
    /// A column, with the table it is read from.
    Column {
        table: Option<String>,
        name: String,
    },
    /// A placeholder, and the type of what goes in it.
    Param(Param, Type),
    /// SQL the compiler wrote itself, such as `TRUE`, `NULL` or `CURRENT_DATE`. Never text from a query.
    Keyword(&'static str),
    /// A number, already checked to be digits.
    Number(String),
    /// A string written into the SQL, its quotes doubled: for `literal("...")` and the compiler's own text.
    Text(String),
    /// A typed string: `DATE '2026-10-20'`, `INTERVAL '7 days'`.
    Typed(&'static str, String),
    /// ClickHouse's length of time: `INTERVAL 7 DAY`.
    Interval(Box<Expr>, &'static str),
    Unary(Unary, Box<Expr>),
    Binary(Binary, Box<Expr>, Box<Expr>),
    /// A function: `lower(name)`.
    Call(String, Vec<Expr>),
    /// A total: `count(*)`, `sum(DISTINCT x) FILTER (WHERE ...)`. No argument means `*`.
    Aggregate {
        name: &'static str,
        arg: Option<Box<Expr>>,
        distinct: bool,
        filter: Option<Box<Expr>>,
    },
    /// A window function: `rank() OVER (PARTITION BY operator ORDER BY risk DESC)`.
    Window {
        function: Box<Expr>,
        partition: Vec<Expr>,
        order: Vec<Order>,
        frame: Option<Frame>,
    },
    /// `x IS NULL`, or `x IS NOT NULL`.
    IsNull(Box<Expr>, bool),
    /// `x IS TRUE`, `x IS NOT FALSE` and the like: the value, the truth compared with, and whether it is negated.
    IsBool(Box<Expr>, bool, bool),
    /// `x IN (...)`, or `x NOT IN (...)`.
    In(Box<Expr>, Vec<Expr>, bool),
    /// `x IN (SELECT ...)`, or `x NOT IN (SELECT ...)`.
    InQuery(Box<Expr>, Box<Select>, bool),
    /// `x BETWEEN low AND high`.
    Between(Box<Expr>, Box<Expr>, Box<Expr>),
    /// `x LIKE pattern ESCAPE '\'`, or SQLite's `x GLOB pattern`.
    Like {
        expr: Box<Expr>,
        pattern: Box<Expr>,
        glob: bool,
    },
    /// `CASE WHEN ... THEN ... ELSE ... END`.
    Case(Vec<(Expr, Expr)>, Option<Box<Expr>>),
    /// `CAST(x AS type)`.
    Cast(Box<Expr>, &'static str),
    /// `EXTRACT(part FROM x)`.
    Extract(&'static str, Box<Expr>),
}

impl Expr {
    /// Whether it reads a column of this name from a table, outside the queries inside it.
    fn reads_column(&self, wanted: &str) -> bool {
        let any = |exprs: &[Expr]| exprs.iter().any(|expr| expr.reads_column(wanted));
        match self {
            Expr::Column { table, name } => table.is_some() && name == wanted,
            Expr::Param(..)
            | Expr::Keyword(_)
            | Expr::Number(_)
            | Expr::Text(_)
            | Expr::Typed(..) => false,
            Expr::Interval(inner, _)
            | Expr::Unary(_, inner)
            | Expr::IsNull(inner, _)
            | Expr::IsBool(inner, ..)
            | Expr::InQuery(inner, ..)
            | Expr::Cast(inner, _)
            | Expr::Extract(_, inner) => inner.reads_column(wanted),
            Expr::Binary(_, left, right) => left.reads_column(wanted) || right.reads_column(wanted),
            Expr::Call(_, args) => any(args),
            Expr::Aggregate { arg, filter, .. } => [arg, filter]
                .into_iter()
                .flatten()
                .any(|expr| expr.reads_column(wanted)),
            Expr::Window {
                function,
                partition,
                order,
                ..
            } => {
                function.reads_column(wanted)
                    || any(partition)
                    || order.iter().any(|order| order.expr.reads_column(wanted))
            }
            Expr::In(value, list, _) => value.reads_column(wanted) || any(list),
            Expr::Between(value, low, high) => {
                value.reads_column(wanted) || low.reads_column(wanted) || high.reads_column(wanted)
            }
            Expr::Like { expr, pattern, .. } => {
                expr.reads_column(wanted) || pattern.reads_column(wanted)
            }
            Expr::Case(arms, otherwise) => {
                arms.iter()
                    .any(|(when, then)| when.reads_column(wanted) || then.reads_column(wanted))
                    || otherwise
                        .as_ref()
                        .is_some_and(|expr| expr.reads_column(wanted))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unary {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Binary {
    Or,
    And,
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Concat,
}

impl Binary {
    fn symbol(self) -> &'static str {
        match self {
            Binary::Or => "OR",
            Binary::And => "AND",
            Binary::Eq => "=",
            Binary::NotEq => "<>",
            Binary::Lt => "<",
            Binary::LtEq => "<=",
            Binary::Gt => ">",
            Binary::GtEq => ">=",
            Binary::Add => "+",
            Binary::Sub => "-",
            Binary::Mul => "*",
            Binary::Div => "/",
            Binary::Mod => "%",
            Binary::Concat => "||",
        }
    }

    fn precedence(self) -> u8 {
        match self {
            Binary::Or => OR,
            Binary::And => AND,
            Binary::Eq | Binary::NotEq | Binary::Lt | Binary::LtEq | Binary::Gt | Binary::GtEq => {
                COMPARE
            }
            Binary::Concat => CONCAT,
            Binary::Add | Binary::Sub => SUM,
            Binary::Mul | Binary::Div | Binary::Mod => PRODUCT,
        }
    }
}

// How tightly each kind of expression holds together, the same for every database ButtercupQL writes for. Where
// databases differ, as SQLite's `||` binds tighter than `*` and PostgreSQL's looser, the writer adds brackets.
const OR: u8 = 1;
const AND: u8 = 2;
const NOT: u8 = 3;
const COMPARE: u8 = 4;
const CONCAT: u8 = 5;
const SUM: u8 = 6;
const PRODUCT: u8 = 7;
const NEG: u8 = 8;
const ATOM: u8 = 9;

impl Expr {
    pub(crate) fn column(table: Option<&str>, name: &str) -> Expr {
        Expr::Column {
            table: table.map(str::to_string),
            name: name.to_string(),
        }
    }

    pub(crate) fn binary(op: Binary, left: Expr, right: Expr) -> Expr {
        Expr::Binary(op, Box::new(left), Box::new(right))
    }

    pub(crate) fn call(name: &str, args: Vec<Expr>) -> Expr {
        Expr::Call(name.to_string(), args)
    }

    pub(crate) fn not(expr: Expr) -> Expr {
        Expr::Unary(Unary::Not, Box::new(expr))
    }

    pub(crate) fn text(value: &str) -> Expr {
        Expr::Param(Param::Value(Value::Text(value.to_string())), Type::Text)
    }

    pub(crate) fn number(digits: &str) -> Expr {
        Expr::Number(digits.to_string())
    }

    fn precedence(&self) -> u8 {
        match self {
            Expr::Binary(op, _, _) => op.precedence(),
            Expr::Unary(Unary::Not, _) => NOT,
            Expr::Unary(Unary::Neg, _) => NEG,
            Expr::IsNull(..)
            | Expr::IsBool(..)
            | Expr::In(..)
            | Expr::InQuery(..)
            | Expr::Between(..)
            | Expr::Like { .. } => COMPARE,
            Expr::Number(digits) if digits.starts_with('-') => NEG,
            _ => ATOM,
        }
    }
}

/// SQL text and its parameters.
pub(crate) struct Written {
    pub(crate) sql: String,
    /// What goes in each placeholder, in order.
    pub(crate) params: Vec<Param>,
    /// Each placeholder's name: `p1`, `p2` for values, and an argument's own name.
    pub(crate) names: Vec<String>,
}

/// Writes a query as text for a database.
#[cfg(test)]
pub(crate) fn write(select: &Select, dialect: Dialect, style: ParamStyle) -> Written {
    write_statement(&Statement::Select(select.clone()), dialect, style)
}

/// Writes a statement as text for a database.
pub(crate) fn write_statement(
    statement: &Statement,
    dialect: Dialect,
    style: ParamStyle,
) -> Written {
    let mut writer = Writer::new(dialect, style);
    writer.statement(statement);
    Written {
        sql: writer.out,
        params: writer.params,
        names: writer.names,
    }
}

/// How the arguments of a query `def` are written.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Args {
    /// As placeholders, given when the query runs.
    Placeholders,
    /// By name, after the name of the function they belong to: PostgreSQL's `cases_for.account`.
    Qualified(String),
    /// By name alone: DuckDB's macros.
    Bare,
}

/// Writes a statement with every value written into it, as the body of a stored function or the default of a
/// column is: neither can have placeholders.
pub(crate) fn write_inline(statement: &Statement, dialect: Dialect, args: Args) -> String {
    let mut writer = Writer::new(dialect, ParamStyle::of(dialect));
    writer.inline = true;
    writer.args = args;
    writer.statement(statement);
    writer.out
}

/// Writes a value with everything in it written in, for a column's default or an index's condition.
pub(crate) fn write_expr_inline(expr: &Expr, dialect: Dialect) -> String {
    let mut writer = Writer::new(dialect, ParamStyle::of(dialect));
    writer.inline = true;
    writer.expr(expr, 0, 0);
    writer.out
}

/// A name, quoted if it must be.
pub(crate) fn ident(name: &str) -> String {
    if needs_quotes(name) {
        format!("\"{}\"", name.replace('"', "\"\""))
    } else {
        name.to_string()
    }
}

/// A table's name and its schema's: `analytics.daily`.
pub(crate) fn path(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| ident(part))
        .collect::<Vec<_>>()
        .join(".")
}

/// A string written into SQL, its quotes doubled.
pub(crate) fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

struct Writer {
    dialect: Dialect,
    style: ParamStyle,
    out: String,
    params: Vec<Param>,
    names: Vec<String>,
    /// How many values have been given placeholders.
    values: usize,
    /// The number of each argument's placeholder, for `$1` placeholders, where an argument used twice is written once.
    numbered: HashMap<String, usize>,
    /// Whether the statement being written reads more than one table.
    qualified: bool,
    /// While its `ORDER BY` is written, the names of the columns the statement gives in place of columns it reads.
    /// Alone in `ORDER BY`, such a name means the new column on every database, so the old one is written with its
    /// table's name.
    renamed: Vec<String>,
    /// Whether values are written into the SQL, rather than as placeholders.
    inline: bool,
    /// How a `def`'s arguments are written.
    args: Args,
}

impl Writer {
    fn new(dialect: Dialect, style: ParamStyle) -> Writer {
        Writer {
            dialect,
            style,
            out: String::new(),
            params: Vec::new(),
            names: Vec::new(),
            values: 0,
            numbered: HashMap::new(),
            qualified: false,
            renamed: Vec::new(),
            inline: false,
            args: Args::Placeholders,
        }
    }

    fn statement(&mut self, statement: &Statement) {
        match statement {
            Statement::Select(select) => {
                self.select(select, 0);
                self.settings(&select.settings);
            }
            Statement::Update {
                table,
                set,
                filter,
                returning,
            } => {
                if self.dialect == Dialect::ClickHouse {
                    // ClickHouse changes rows with a mutation of the table, which it finishes before it answers.
                    self.push("ALTER TABLE ");
                    self.push(&path(table));
                    self.push(" UPDATE ");
                    self.assignments(set);
                    self.line(0);
                    self.push("WHERE ");
                    if filter.is_empty() {
                        self.push("true");
                    } else {
                        self.conditions(filter, 0);
                    }
                    self.settings(&[("mutations_sync", "1")]);
                    return;
                }
                self.push("UPDATE ");
                self.push(&path(table));
                self.push(" SET ");
                self.assignments(set);
                if !filter.is_empty() {
                    self.line(0);
                    self.push("WHERE ");
                    self.conditions(filter, 0);
                }
                self.returning(returning);
            }
            Statement::Delete {
                table,
                filter,
                returning,
            } => {
                self.push("DELETE FROM ");
                self.push(&path(table));
                if !filter.is_empty() {
                    self.line(0);
                    self.push("WHERE ");
                    self.conditions(filter, 0);
                } else if self.dialect == Dialect::ClickHouse {
                    // ClickHouse deletes only the rows a condition chooses.
                    self.line(0);
                    self.push("WHERE true");
                }
                self.returning(returning);
            }
            Statement::Insert {
                table,
                columns,
                rows,
                returning,
            } => {
                self.push("INSERT INTO ");
                self.push(&path(table));
                if !columns.is_empty() {
                    let names: Vec<String> = columns.iter().map(|name| ident(name)).collect();
                    self.push(&format!(" ({})", names.join(", ")));
                }
                self.line(0);
                match rows {
                    Rows::Query(select) => self.select(select, 0),
                    Rows::Values(values) => {
                        self.push("VALUES ");
                        for (n, row) in values.iter().enumerate() {
                            if n > 0 {
                                self.push(", ");
                            }
                            self.push("(");
                            for (m, value) in row.iter().enumerate() {
                                if m > 0 {
                                    self.push(", ");
                                }
                                self.expr(value, 0, 0);
                            }
                            self.push(")");
                        }
                    }
                }
                self.returning(returning);
            }
            Statement::Written(sql) => self.out.push_str(sql),
            Statement::Begin => self.push("BEGIN"),
            Statement::Commit => self.push("COMMIT"),
        }
    }

    fn assignments(&mut self, set: &[(String, Expr)]) {
        for (n, (column, value)) in set.iter().enumerate() {
            if n > 0 {
                self.push(", ");
            }
            self.ident(column);
            self.push(" = ");
            self.expr(value, 0, 0);
        }
    }

    fn returning(&mut self, returning: &[Item]) {
        if returning.is_empty() {
            return;
        }
        self.line(0);
        self.push("RETURNING ");
        self.items(returning, 0);
    }

    fn settings(&mut self, settings: &[(&str, &str)]) {
        if settings.is_empty() {
            return;
        }
        let settings: Vec<String> = settings
            .iter()
            .map(|(name, value)| format!("{name} = {value}"))
            .collect();
        self.line(0);
        self.push(&format!("SETTINGS {}", settings.join(", ")));
    }

    /// The recursive queries a `SELECT` reads, before it.
    fn with(&mut self, with: &[Cte], depth: usize) {
        if with.is_empty() {
            return;
        }
        self.push("WITH RECURSIVE ");
        for (n, cte) in with.iter().enumerate() {
            if n > 0 {
                self.push(", ");
            }
            self.ident(&cte.name);
            self.push(" AS (");
            self.line(depth + 1);
            self.select(&cte.anchor, depth + 1);
            self.line(depth + 1);
            self.push(cte.op.keyword(self.dialect));
            self.line(depth + 1);
            self.select(&cte.step, depth + 1);
            self.line(depth);
            self.push(")");
        }
        self.line(depth);
    }

    /// Writes SQL the compiler made. With `%s` placeholders, a `%` is doubled so that it is not taken for one.
    fn push(&mut self, text: &str) {
        if self.style == ParamStyle::Format && text.contains('%') {
            self.out.push_str(&text.replace('%', "%%"));
        } else {
            self.out.push_str(text);
        }
    }

    fn line(&mut self, depth: usize) {
        self.out.push('\n');
        for _ in 0..depth {
            self.out.push_str("  ");
        }
    }

    fn select(&mut self, select: &Select, depth: usize) {
        self.with(&select.with, depth);
        let outer = std::mem::replace(&mut self.qualified, select.qualified(self.dialect));
        let outer_renamed = std::mem::take(&mut self.renamed);
        self.push(if select.distinct {
            "SELECT DISTINCT "
        } else {
            "SELECT "
        });
        if select.columns.is_empty() {
            self.push("*");
        }
        self.items(&select.columns, depth);
        self.clauses(select, depth);
        self.qualified = outer;
        self.renamed = outer_renamed;
    }

    fn items(&mut self, items: &[Item], depth: usize) {
        for (n, item) in items.iter().enumerate() {
            if n > 0 {
                self.push(", ");
            }
            match item {
                Item::Star => self.push("*"),
                Item::TableStar(table) => {
                    self.ident(table);
                    self.push(".*");
                }
                Item::Expr { expr, alias } => {
                    self.expr(expr, 0, depth);
                    if let Some(alias) = alias {
                        let named_already =
                            matches!(expr, Expr::Column { name, .. } if name == alias);
                        if !named_already {
                            self.push(" AS ");
                            self.ident(alias);
                        }
                    }
                }
            }
        }
    }

    /// The clauses of a `SELECT` after its columns.
    fn clauses(&mut self, select: &Select, depth: usize) {
        if let Some(from) = &select.from {
            self.line(depth);
            self.push("FROM ");
            self.source(from, depth);
        }
        for join in &select.joins {
            self.line(depth);
            self.push(if join.left { "LEFT JOIN " } else { "JOIN " });
            self.source(&join.source, depth);
            self.push(" ON ");
            self.expr(&join.on, 0, depth);
        }
        if !select.filter.is_empty() {
            self.line(depth);
            self.push("WHERE ");
            self.conditions(&select.filter, depth);
        }
        if !select.group.is_empty() {
            self.line(depth);
            self.push("GROUP BY ");
            for (n, expr) in select.group.iter().enumerate() {
                if n > 0 {
                    self.push(", ");
                }
                self.expr(expr, 0, depth);
            }
        }
        if !select.having.is_empty() {
            self.line(depth);
            self.push("HAVING ");
            self.conditions(&select.having, depth);
        }
        if !select.qualify.is_empty() {
            self.line(depth);
            self.push("QUALIFY ");
            self.conditions(&select.qualify, depth);
        }
        if !select.order.is_empty() {
            self.line(depth);
            self.push("ORDER BY ");
            self.renamed = select.renamed().map(str::to_string).collect();
            self.order(&select.order, depth);
            self.renamed.clear();
        }
        match (&select.limit, &select.offset) {
            (Some(limit), None) => {
                self.line(depth);
                self.push("LIMIT ");
                self.count(limit);
            }
            (Some(limit), Some(offset)) => {
                self.line(depth);
                self.push("LIMIT ");
                self.count(limit);
                self.push(" OFFSET ");
                self.count(offset);
            }
            (None, Some(offset)) => {
                self.line(depth);
                if self.dialect == Dialect::Sqlite {
                    // SQLite takes an offset only after a limit, where -1 is no limit.
                    self.push("LIMIT -1 ");
                }
                self.push("OFFSET ");
                self.count(offset);
            }
            (None, None) => {}
        }
    }

    fn order(&mut self, order: &[Order], depth: usize) {
        for (n, key) in order.iter().enumerate() {
            if n > 0 {
                self.push(", ");
            }
            self.expr(&key.expr, 0, depth);
            if key.descending {
                self.push(" DESC");
            }
            if key.nulls_last {
                self.push(" NULLS LAST");
            }
        }
    }

    fn count(&mut self, count: &Count) {
        match count {
            Count::Rows(rows) => self.push(&rows.to_string()),
            Count::Param(param) => self.param(param, Type::Int),
        }
    }

    fn conditions(&mut self, conditions: &[Expr], depth: usize) {
        // A condition alone needs no brackets; joined with others by `AND`, an `OR` does.
        let context = if conditions.len() == 1 { 0 } else { AND };
        for (n, condition) in conditions.iter().enumerate() {
            if n > 0 {
                self.push(" AND ");
            }
            self.expr(condition, context, depth);
        }
    }

    fn source(&mut self, source: &Source, depth: usize) {
        match &source.kind {
            SourceKind::Table(path) => {
                for (n, part) in path.iter().enumerate() {
                    if n > 0 {
                        self.push(".");
                    }
                    self.ident(part);
                }
            }
            SourceKind::Query(select) => {
                self.push("(");
                self.line(depth + 1);
                self.select(select, depth + 1);
                self.line(depth);
                self.push(")");
            }
            SourceKind::Compound(left, op, right) => {
                self.push("(");
                self.line(depth + 1);
                self.select(left, depth + 1);
                self.line(depth + 1);
                self.push(op.keyword(self.dialect));
                self.line(depth + 1);
                self.select(right, depth + 1);
                self.line(depth);
                self.push(")");
            }
            SourceKind::Rows(rows, columns) => {
                // `SELECT ... UNION ALL SELECT ...` names the columns the same way on every database.
                self.push("(");
                for (n, row) in rows.iter().enumerate() {
                    self.line(depth + 1);
                    if n > 0 {
                        self.push("UNION ALL ");
                    }
                    self.push("SELECT ");
                    for (m, (value, column)) in row.iter().zip(columns).enumerate() {
                        if m > 0 {
                            self.push(", ");
                        }
                        self.expr(value, 0, depth + 1);
                        self.push(" AS ");
                        self.ident(column);
                    }
                }
                self.line(depth);
                self.push(")");
            }
        }
        if let Some(alias) = &source.alias {
            let same =
                matches!(&source.kind, SourceKind::Table(path) if path.last() == Some(alias));
            if !same {
                self.push(" AS ");
                self.ident(alias);
            }
        }
    }

    fn ident(&mut self, name: &str) {
        if needs_quotes(name) {
            let quoted = format!("\"{}\"", name.replace('"', "\"\""));
            self.push(&quoted);
        } else {
            self.push(name);
        }
    }

    fn text(&mut self, value: &str) {
        let quoted = format!("'{}'", value.replace('\'', "''"));
        self.push(&quoted);
    }

    fn param(&mut self, param: &Param, ty: Type) {
        if self.inline {
            match param {
                Param::Value(value) => {
                    let written = match value {
                        Value::Null => "NULL".to_string(),
                        Value::Bool(true) => "TRUE".to_string(),
                        Value::Bool(false) => "FALSE".to_string(),
                        Value::Int(number) => number.to_string(),
                        Value::Float(number) => format!("{number:?}"),
                        Value::Text(text) => literal(text),
                    };
                    self.push(&written);
                    return;
                }
                Param::Arg(name) => match self.args.clone() {
                    Args::Qualified(function) => {
                        self.ident(&function);
                        self.push(".");
                        self.ident(name);
                        return;
                    }
                    Args::Bare => {
                        self.ident(name);
                        return;
                    }
                    // ClickHouse's stored queries take their arguments as its own placeholders.
                    Args::Placeholders => {}
                },
            }
        }
        let name = match param {
            Param::Value(_) => {
                self.values += 1;
                format!("p{}", self.values)
            }
            Param::Arg(name) => name.clone(),
        };
        match self.style {
            ParamStyle::Dollar => {
                // An argument used twice is one placeholder; each value is one of its own.
                let reused = match param {
                    Param::Arg(name) => self.numbered.get(name).copied(),
                    Param::Value(_) => None,
                };
                let number = match reused {
                    Some(number) => number,
                    None => {
                        self.params.push(param.clone());
                        self.names.push(name.clone());
                        let number = self.params.len();
                        if let Param::Arg(arg) = param {
                            self.numbered.insert(arg.clone(), number);
                        }
                        number
                    }
                };
                self.out.push_str(&format!("${number}"));
                return;
            }
            ParamStyle::Question => self.out.push('?'),
            ParamStyle::Format => self.out.push_str("%s"),
            ParamStyle::Colon => self.out.push_str(&format!(":{name}")),
            ParamStyle::Braces => {
                let ty = match param {
                    Param::Value(value) => value_type(value),
                    Param::Arg(_) => clickhouse_type(ty),
                };
                self.out.push_str(&format!("{{{name}:{ty}}}"));
            }
        }
        self.params.push(param.clone());
        self.names.push(name);
    }

    /// Writes an expression where it must hold together at least as tightly as `context`, in brackets if it does
    /// not.
    fn expr(&mut self, expr: &Expr, context: u8, depth: usize) {
        let bracket = expr.precedence() < context;
        if bracket {
            self.push("(");
        }
        self.bare(expr, depth);
        if bracket {
            self.push(")");
        }
    }

    fn bare(&mut self, expr: &Expr, depth: usize) {
        match expr {
            Expr::Column { table, name } => {
                let qualified = self.qualified || self.renamed.contains(name);
                if let (true, Some(table)) = (qualified, table) {
                    self.ident(table);
                    self.push(".");
                }
                self.ident(name);
            }
            Expr::Param(param, ty) => self.param(param, *ty),
            Expr::Keyword(keyword) => self.push(keyword),
            Expr::Number(digits) => self.push(digits),
            Expr::Text(value) => self.text(value),
            Expr::Typed(keyword, value) => {
                self.push(keyword);
                self.push(" ");
                self.text(value);
            }
            Expr::Interval(amount, unit) => {
                self.push("INTERVAL ");
                self.expr(amount, ATOM, depth);
                self.push(" ");
                self.push(unit);
            }
            Expr::Unary(Unary::Neg, operand) => {
                self.push("-");
                // `--` would start a comment, so a negative operand goes in brackets.
                let context = if operand.precedence() == NEG {
                    ATOM
                } else {
                    NEG
                };
                self.expr(operand, context, depth);
            }
            Expr::Unary(Unary::Not, operand) => {
                self.push("NOT ");
                let context = if operand.precedence() == COMPARE {
                    ATOM
                } else {
                    NOT
                };
                self.expr(operand, context, depth);
            }
            Expr::Binary(op, left, right) => {
                let precedence = op.precedence();
                let (left_context, right_context) = match op {
                    // Each side of `||` is a single value or another `||`: SQLite's `||` binds tighter than
                    // arithmetic, PostgreSQL's looser.
                    Binary::Concat => {
                        let left_context = match left.as_ref() {
                            Expr::Binary(Binary::Concat, _, _) => CONCAT,
                            _ => ATOM,
                        };
                        (left_context, ATOM)
                    }
                    Binary::And | Binary::Or => (precedence, precedence),
                    _ if precedence == COMPARE => (COMPARE + 1, COMPARE + 1),
                    _ => (precedence, precedence + 1),
                };
                self.expr(left, left_context, depth);
                self.push(" ");
                self.push(op.symbol());
                self.push(" ");
                self.expr(right, right_context, depth);
            }
            Expr::Call(name, args) => {
                self.push(name);
                self.push("(");
                for (n, arg) in args.iter().enumerate() {
                    if n > 0 {
                        self.push(", ");
                    }
                    self.expr(arg, 0, depth);
                }
                self.push(")");
            }
            Expr::Aggregate {
                name,
                arg,
                distinct,
                filter,
            } => {
                self.push(name);
                self.push("(");
                if *distinct {
                    self.push("DISTINCT ");
                }
                match arg {
                    Some(arg) => self.expr(arg, 0, depth),
                    None => self.push("*"),
                }
                self.push(")");
                if let Some(filter) = filter {
                    self.push(" FILTER (WHERE ");
                    self.expr(filter, 0, depth);
                    self.push(")");
                }
            }
            Expr::Window {
                function,
                partition,
                order,
                frame,
            } => {
                self.expr(function, ATOM, depth);
                self.push(" OVER (");
                let mut parts = 0;
                if !partition.is_empty() {
                    self.push("PARTITION BY ");
                    for (n, key) in partition.iter().enumerate() {
                        if n > 0 {
                            self.push(", ");
                        }
                        self.expr(key, 0, depth);
                    }
                    parts += 1;
                }
                if !order.is_empty() {
                    if parts > 0 {
                        self.push(" ");
                    }
                    self.push("ORDER BY ");
                    self.order(order, depth);
                    parts += 1;
                }
                if let Some(frame) = frame {
                    if parts > 0 {
                        self.push(" ");
                    }
                    match frame {
                        Frame::Running => {
                            self.push("ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW")
                        }
                        Frame::Last(rows) => self.push(&format!(
                            "ROWS BETWEEN {} PRECEDING AND CURRENT ROW",
                            rows.saturating_sub(1)
                        )),
                        Frame::Whole => {
                            self.push("ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING")
                        }
                    }
                }
                self.push(")");
            }
            Expr::IsNull(operand, negated) => {
                self.expr(operand, COMPARE + 1, depth);
                self.push(if *negated { " IS NOT NULL" } else { " IS NULL" });
            }
            Expr::IsBool(operand, truth, negated) => {
                self.expr(operand, COMPARE + 1, depth);
                self.push(if *negated { " IS NOT " } else { " IS " });
                self.push(if *truth { "TRUE" } else { "FALSE" });
            }
            Expr::In(operand, list, negated) => {
                self.expr(operand, COMPARE + 1, depth);
                self.push(if *negated { " NOT IN (" } else { " IN (" });
                for (n, item) in list.iter().enumerate() {
                    if n > 0 {
                        self.push(", ");
                    }
                    self.expr(item, 0, depth);
                }
                self.push(")");
            }
            Expr::InQuery(operand, select, negated) => {
                self.expr(operand, COMPARE + 1, depth);
                self.push(if *negated { " NOT IN (" } else { " IN (" });
                self.line(depth + 1);
                self.select(select, depth + 1);
                self.line(depth);
                self.push(")");
            }
            Expr::Between(operand, low, high) => {
                self.expr(operand, COMPARE + 1, depth);
                self.push(" BETWEEN ");
                self.expr(low, COMPARE + 1, depth);
                self.push(" AND ");
                self.expr(high, COMPARE + 1, depth);
            }
            Expr::Like {
                expr,
                pattern,
                glob,
            } => {
                self.expr(expr, COMPARE + 1, depth);
                if *glob {
                    self.push(" GLOB ");
                    self.expr(pattern, COMPARE + 1, depth);
                } else {
                    self.push(" LIKE ");
                    self.expr(pattern, COMPARE + 1, depth);
                    self.push(" ESCAPE '\\'");
                }
            }
            Expr::Case(whens, otherwise) => {
                self.push("CASE");
                for (test, then) in whens {
                    self.push(" WHEN ");
                    self.expr(test, 0, depth);
                    self.push(" THEN ");
                    self.expr(then, 0, depth);
                }
                if let Some(otherwise) = otherwise {
                    self.push(" ELSE ");
                    self.expr(otherwise, 0, depth);
                }
                self.push(" END");
            }
            Expr::Cast(operand, ty) => {
                self.push("CAST(");
                self.expr(operand, 0, depth);
                self.push(&format!(" AS {ty})"));
            }
            Expr::Extract(part, operand) => {
                self.push(&format!("EXTRACT({part} FROM "));
                self.expr(operand, 0, depth);
                self.push(")");
            }
        }
    }
}

/// ClickHouse's type for a value given as a parameter.
fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "Nullable(String)",
        Value::Bool(_) => "Bool",
        Value::Int(_) => "Int64",
        Value::Float(_) => "Float64",
        Value::Text(_) => "String",
    }
}

/// ClickHouse's type for an argument, from its annotation.
fn clickhouse_type(ty: Type) -> &'static str {
    match ty {
        Type::Bool => "Bool",
        Type::Int => "Int64",
        Type::Float | Type::Decimal => "Float64",
        Type::Date => "Date",
        Type::Timestamp => "DateTime",
        Type::Text | Type::Unknown | Type::Duration => "String",
    }
}

/// Whether a name must be quoted: when it is not lowercase letters, digits and `_`, starting with a letter or `_`,
/// or when it is a word SQL keeps for itself.
pub(crate) fn needs_quotes(name: &str) -> bool {
    let mut chars = name.chars();
    let plain = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    !plain
        || RESERVED
            .binary_search(&name.to_ascii_uppercase().as_str())
            .is_ok()
}

/// The words PostgreSQL, SQLite, DuckDB or ClickHouse keep for themselves, which a name must be quoted to use.
/// Sorted, to be searched.
const RESERVED: [&str; 112] = [
    "ABORT",
    "ALL",
    "ALTER",
    "ANALYSE",
    "ANALYZE",
    "AND",
    "ANY",
    "ARRAY",
    "AS",
    "ASC",
    "ASYMMETRIC",
    "AUTHORIZATION",
    "BETWEEN",
    "BINARY",
    "BOTH",
    "BY",
    "CASE",
    "CAST",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "CONCURRENTLY",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "CURRENT_CATALOG",
    "CURRENT_DATE",
    "CURRENT_ROLE",
    "CURRENT_SCHEMA",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "DEFAULT",
    "DEFERRABLE",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DO",
    "DROP",
    "ELSE",
    "END",
    "ESCAPE",
    "EXCEPT",
    "EXISTS",
    "FALSE",
    "FETCH",
    "FILTER",
    "FINAL",
    "FOR",
    "FOREIGN",
    "FREEZE",
    "FROM",
    "FULL",
    "GLOB",
    "GRANT",
    "GROUP",
    "HAVING",
    "ILIKE",
    "IN",
    "INDEX",
    "INITIALLY",
    "INNER",
    "INSERT",
    "INTERSECT",
    "INTO",
    "IS",
    "ISNULL",
    "JOIN",
    "LATERAL",
    "LEADING",
    "LEFT",
    "LIKE",
    "LIMIT",
    "LOCALTIME",
    "LOCALTIMESTAMP",
    "NATURAL",
    "NOT",
    "NOTNULL",
    "NULL",
    "OFFSET",
    "ON",
    "ONLY",
    "OR",
    "ORDER",
    "OUTER",
    "OVER",
    "OVERLAPS",
    "PLACING",
    "PREWHERE",
    "PRIMARY",
    "QUALIFY",
    "REFERENCES",
    "RETURNING",
    "RIGHT",
    "SELECT",
    "SESSION_USER",
    "SET",
    "SIMILAR",
    "SOME",
    "SYMMETRIC",
    "TABLE",
    "TABLESAMPLE",
    "THEN",
    "TO",
    "TRAILING",
    "TRUE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "USER",
    "USING",
    "VALUES",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str) -> Expr {
        Expr::column(None, name)
    }

    fn text(value: &str) -> Value {
        Value::Text(value.to_string())
    }

    fn param(value: &str) -> Expr {
        Expr::text(value)
    }

    fn written(select: &Select, style: ParamStyle) -> (String, Vec<Param>) {
        let written = write(select, Dialect::Postgres, style);
        (written.sql, written.params)
    }

    fn values(params: Vec<Param>) -> Vec<Value> {
        params
            .into_iter()
            .map(|param| match param {
                Param::Value(value) => value,
                Param::Arg(name) => panic!("an argument, {name}"),
            })
            .collect()
    }

    #[test]
    fn the_reserved_words_are_sorted() {
        let mut sorted = RESERVED;
        sorted.sort_unstable();
        assert_eq!(sorted, RESERVED);
    }

    #[test]
    fn names_are_quoted_only_when_they_must_be() {
        assert!(!needs_quotes("risk"));
        assert!(!needs_quotes("_raw2"));
        assert!(needs_quotes("Risk"));
        assert!(needs_quotes("order date"));
        assert!(needs_quotes("user"));
        assert!(needs_quotes("from"));
        assert!(needs_quotes("2fa"));
        assert!(needs_quotes(""));
    }

    #[test]
    fn a_select_is_written_one_clause_per_line() {
        let select = Select {
            columns: vec![
                Item::Expr {
                    expr: column("key"),
                    alias: Some("key".to_string()),
                },
                Item::Expr {
                    expr: Expr::binary(Binary::Mul, column("risk"), Expr::number("2")),
                    alias: Some("double risk".to_string()),
                },
            ],
            from: Some(Source {
                kind: SourceKind::Table(vec!["cases".to_string()]),
                alias: None,
            }),
            filter: vec![
                Expr::binary(Binary::Eq, column("status"), param("open")),
                Expr::binary(
                    Binary::Or,
                    Expr::binary(Binary::Gt, column("risk"), Expr::number("90")),
                    Expr::IsNull(Box::new(column("user")), false),
                ),
            ],
            order: vec![Order {
                expr: column("risk"),
                descending: true,
                nulls_last: true,
            }],
            limit: Some(Count::Rows(20)),
            ..Select::default()
        };
        let (sql, params) = written(&select, ParamStyle::Dollar);
        assert_eq!(
            sql,
            "\
SELECT key, risk * 2 AS \"double risk\"
FROM cases
WHERE status = $1 AND (risk > 90 OR \"user\" IS NULL)
ORDER BY risk DESC NULLS LAST
LIMIT 20"
        );
        assert_eq!(values(params), [text("open")]);
        assert!(
            written(&select, ParamStyle::Question)
                .0
                .contains("status = ?")
        );
    }

    #[test]
    fn brackets_keep_the_meaning_on_every_database() {
        let select = |expr: Expr| Select {
            columns: vec![Item::Expr { expr, alias: None }],
            ..Select::default()
        };
        let (a, b, c) = (column("a"), column("b"), column("c"));
        let cases = [
            (
                Expr::binary(
                    Binary::Sub,
                    a.clone(),
                    Expr::binary(Binary::Sub, b.clone(), c.clone()),
                ),
                "a - (b - c)",
            ),
            (
                Expr::binary(
                    Binary::Sub,
                    Expr::binary(Binary::Sub, a.clone(), b.clone()),
                    c.clone(),
                ),
                "a - b - c",
            ),
            (
                Expr::binary(
                    Binary::Mul,
                    Expr::binary(Binary::Add, a.clone(), b.clone()),
                    c.clone(),
                ),
                "(a + b) * c",
            ),
            (
                Expr::binary(
                    Binary::Concat,
                    a.clone(),
                    Expr::binary(Binary::Mul, b.clone(), c.clone()),
                ),
                "a || (b * c)",
            ),
            (
                Expr::binary(
                    Binary::Concat,
                    Expr::binary(Binary::Concat, a.clone(), b.clone()),
                    c.clone(),
                ),
                "a || b || c",
            ),
            (
                Expr::Unary(Unary::Neg, Box::new(Expr::number("-5"))),
                "-(-5)",
            ),
            (
                Expr::Unary(
                    Unary::Neg,
                    Box::new(Expr::Unary(Unary::Neg, Box::new(a.clone()))),
                ),
                "-(-a)",
            ),
            (
                Expr::not(Expr::binary(Binary::Eq, a.clone(), b.clone())),
                "NOT (a = b)",
            ),
            (
                Expr::binary(
                    Binary::And,
                    Expr::binary(Binary::Or, a.clone(), b.clone()),
                    c.clone(),
                ),
                "(a OR b) AND c",
            ),
        ];
        for (expr, expected) in cases {
            assert_eq!(
                written(&select(expr), ParamStyle::Dollar).0,
                format!("SELECT {expected}")
            );
        }
    }

    #[test]
    fn parameters_are_numbered_in_the_order_they_are_written() {
        let inner = Select {
            from: Some(Source {
                kind: SourceKind::Table(vec!["cases".to_string()]),
                alias: None,
            }),
            filter: vec![Expr::binary(Binary::Eq, column("band"), param("high"))],
            ..Select::default()
        };
        let outer = Select {
            from: Some(Source {
                kind: SourceKind::Query(Box::new(inner)),
                alias: Some("_q1".to_string()),
            }),
            filter: vec![Expr::binary(Binary::Eq, column("status"), param("open"))],
            ..Select::default()
        };
        let (sql, params) = written(&outer, ParamStyle::Dollar);
        assert_eq!(
            sql,
            "\
SELECT *
FROM (
  SELECT *
  FROM cases
  WHERE band = $1
) AS _q1
WHERE status = $2"
        );
        assert_eq!(values(params), [text("high"), text("open")]);
        let (format, _) = written(
            &Select {
                columns: vec![Item::Expr {
                    expr: Expr::binary(
                        Binary::Mod,
                        column("a"),
                        Expr::Param(Param::Value(Value::Int(2)), Type::Int),
                    ),
                    alias: None,
                }],
                ..Select::default()
            },
            ParamStyle::Format,
        );
        assert_eq!(format, "SELECT a %% %s");
    }

    #[test]
    fn an_argument_is_one_placeholder_however_often_it_is_used() {
        let account = Expr::Param(Param::Arg("account".to_string()), Type::Text);
        let select = Select {
            from: Some(Source {
                kind: SourceKind::Table(vec!["cases".to_string()]),
                alias: None,
            }),
            filter: vec![
                Expr::binary(Binary::Eq, column("payer"), account.clone()),
                Expr::binary(Binary::Eq, column("status"), param("open")),
                Expr::binary(Binary::Eq, column("payee"), account),
            ],
            limit: Some(Count::Param(Param::Arg("rows".to_string()))),
            ..Select::default()
        };
        let dollar = write(&select, Dialect::Postgres, ParamStyle::Dollar);
        assert_eq!(
            dollar.sql,
            "SELECT *\nFROM cases\nWHERE payer = $1 AND status = $2 AND payee = $1\nLIMIT $3"
        );
        assert_eq!(dollar.names, ["account", "p1", "rows"]);
        let braces = write(&select, Dialect::ClickHouse, ParamStyle::Braces);
        assert_eq!(
            braces.sql,
            "SELECT *\nFROM cases\nWHERE payer = {account:String} AND status = {p1:String} AND payee = {account:String}\nLIMIT {rows:Int64}"
        );
        assert_eq!(braces.names, ["account", "p1", "account", "rows"]);
        let question = write(&select, Dialect::Sqlite, ParamStyle::Question);
        assert_eq!(question.params.len(), 4);
    }

    #[test]
    fn windows_qualify_and_settings() {
        let window = Expr::Window {
            function: Box::new(Expr::call("rank", Vec::new())),
            partition: vec![column("operator")],
            order: vec![Order {
                expr: column("risk"),
                descending: true,
                nulls_last: false,
            }],
            frame: None,
        };
        let running = Expr::Window {
            function: Box::new(Expr::Aggregate {
                name: "sum",
                arg: Some(Box::new(column("amount"))),
                distinct: false,
                filter: None,
            }),
            partition: Vec::new(),
            order: vec![Order {
                expr: column("time"),
                descending: false,
                nulls_last: false,
            }],
            frame: Some(Frame::Running),
        };
        let select = Select {
            columns: vec![
                Item::TableStar("cases".to_string()),
                Item::Expr {
                    expr: window.clone(),
                    alias: Some("place".to_string()),
                },
                Item::Expr {
                    expr: running,
                    alias: Some("so_far".to_string()),
                },
            ],
            from: Some(Source {
                kind: SourceKind::Table(vec!["cases".to_string()]),
                alias: None,
            }),
            qualify: vec![Expr::binary(Binary::LtEq, window, Expr::number("3"))],
            settings: vec![("join_use_nulls", "1")],
            ..Select::default()
        };
        assert_eq!(
            write(&select, Dialect::DuckDb, ParamStyle::Dollar).sql,
            "\
SELECT cases.*, rank() OVER (PARTITION BY operator ORDER BY risk DESC) AS place, sum(amount) OVER (ORDER BY time ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS so_far
FROM cases
QUALIFY rank() OVER (PARTITION BY operator ORDER BY risk DESC) <= 3
SETTINGS join_use_nulls = 1"
        );
    }

    #[test]
    fn strings_written_into_the_sql_have_their_quotes_doubled() {
        let select = Select {
            columns: vec![Item::Expr {
                expr: Expr::Text("it's".to_string()),
                alias: None,
            }],
            ..Select::default()
        };
        assert_eq!(written(&select, ParamStyle::Dollar).0, "SELECT 'it''s'");
    }

    #[test]
    fn an_offset_alone_is_written_as_each_database_takes_it() {
        let select = Select {
            from: Some(Source {
                kind: SourceKind::Table(vec!["t".to_string()]),
                alias: None,
            }),
            offset: Some(Count::Rows(5)),
            ..Select::default()
        };
        assert_eq!(
            write(&select, Dialect::Sqlite, ParamStyle::Question).sql,
            "SELECT *\nFROM t\nLIMIT -1 OFFSET 5"
        );
        assert_eq!(
            write(&select, Dialect::Postgres, ParamStyle::Dollar).sql,
            "SELECT *\nFROM t\nOFFSET 5"
        );
    }
}
