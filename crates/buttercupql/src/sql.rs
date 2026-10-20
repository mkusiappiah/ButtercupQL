//! The SQL ButtercupQL writes: a small syntax tree of the `SELECT` statements it builds, and the writer that turns
//! one into text, with its parameters in the order they appear.

use std::fmt::Write;

use crate::dialect::Dialect;

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

/// How placeholders for parameters are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParamStyle {
    /// `$1`, `$2`: PostgreSQL's own, as asyncpg, node-postgres and Rust's sqlx and tokio-postgres take them.
    Dollar,
    /// `?`: SQLite's, as Python's `sqlite3` takes them.
    Question,
    /// `%s`: as Python's psycopg takes them. A `%` in the SQL is then written `%%`.
    Format,
    /// `:p1`, `:p2`: named.
    Colon,
}

impl ParamStyle {
    /// The style a database's own drivers take.
    pub fn of(dialect: Dialect) -> ParamStyle {
        match dialect {
            Dialect::Postgres => ParamStyle::Dollar,
            _ => ParamStyle::Question,
        }
    }
}

/// A `SELECT` statement.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Select {
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
    pub(crate) order: Vec<Order>,
    pub(crate) limit: Option<u64>,
    pub(crate) offset: u64,
}

impl Select {
    /// Whether the statement reads more than one table, so that its columns are written with their table's name.
    fn qualified(&self) -> bool {
        !self.joins.is_empty()
    }
}

/// A column of a `SELECT`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Item {
    /// `*`, every column of every table read.
    Star,
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

/// An SQL expression.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Expr {
    /// A column, with the table it is read from.
    Column {
        table: Option<String>,
        name: String,
    },
    /// A value passed as a parameter.
    Param(Value),
    /// SQL the compiler wrote itself, such as `TRUE`, `NULL` or `CURRENT_DATE`. Never text from a query.
    Keyword(&'static str),
    /// A number, already checked to be digits.
    Number(String),
    /// A string written into the SQL, its quotes doubled: for `literal("...")` and the compiler's own text.
    Text(String),
    /// A typed string: `DATE '2026-10-20'`, `INTERVAL '7 days'`.
    Typed(&'static str, String),
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
    pub(crate) params: Vec<Value>,
}

/// Writes a statement as text for a database.
pub(crate) fn write(select: &Select, dialect: Dialect, style: ParamStyle) -> Written {
    let mut writer = Writer {
        dialect,
        style,
        out: String::new(),
        params: Vec::new(),
        qualified: false,
    };
    writer.select(select, 0);
    Written {
        sql: writer.out,
        params: writer.params,
    }
}

struct Writer {
    dialect: Dialect,
    style: ParamStyle,
    out: String,
    params: Vec<Value>,
    /// Whether the statement being written reads more than one table.
    qualified: bool,
}

impl Writer {
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
        let outer = std::mem::replace(&mut self.qualified, select.qualified());
        self.push(if select.distinct {
            "SELECT DISTINCT "
        } else {
            "SELECT "
        });
        if select.columns.is_empty() {
            self.push("*");
        }
        for (n, item) in select.columns.iter().enumerate() {
            if n > 0 {
                self.push(", ");
            }
            match item {
                Item::Star => self.push("*"),
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
        if !select.order.is_empty() {
            self.line(depth);
            self.push("ORDER BY ");
            for (n, order) in select.order.iter().enumerate() {
                if n > 0 {
                    self.push(", ");
                }
                self.expr(&order.expr, 0, depth);
                if order.descending {
                    self.push(" DESC");
                }
                if order.nulls_last {
                    self.push(" NULLS LAST");
                }
            }
        }
        match (select.limit, select.offset) {
            (Some(limit), 0) => {
                self.line(depth);
                self.push(&format!("LIMIT {limit}"));
            }
            (Some(limit), offset) => {
                self.line(depth);
                self.push(&format!("LIMIT {limit} OFFSET {offset}"));
            }
            (None, 0) => {}
            (None, offset) => {
                self.line(depth);
                if self.dialect == Dialect::Sqlite {
                    // SQLite takes an offset only after a limit, where -1 is no limit.
                    self.push(&format!("LIMIT -1 OFFSET {offset}"));
                } else {
                    self.push(&format!("OFFSET {offset}"));
                }
            }
        }
        self.qualified = outer;
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

    fn param(&mut self, value: &Value) {
        self.params.push(value.clone());
        let n = self.params.len();
        match self.style {
            ParamStyle::Dollar => self.out.push_str(&format!("${n}")),
            ParamStyle::Question => self.out.push('?'),
            ParamStyle::Format => self.out.push_str("%s"),
            ParamStyle::Colon => self.out.push_str(&format!(":p{n}")),
        }
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
                if let (true, Some(table)) = (self.qualified, table) {
                    self.ident(table);
                    self.push(".");
                }
                self.ident(name);
            }
            Expr::Param(value) => self.param(value),
            Expr::Keyword(keyword) => self.push(keyword),
            Expr::Number(digits) => self.push(digits),
            Expr::Text(value) => self.text(value),
            Expr::Typed(keyword, value) => {
                self.push(keyword);
                self.push(" ");
                self.text(value);
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
                let _ = write!(self.out, " AS {ty})");
            }
            Expr::Extract(part, operand) => {
                let _ = write!(self.out, "EXTRACT({part} FROM ");
                self.expr(operand, 0, depth);
                self.push(")");
            }
        }
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

/// The words PostgreSQL or SQLite keep for themselves, which a name must be quoted to use. Sorted, to be searched.
const RESERVED: [&str; 109] = [
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
    "PRIMARY",
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

    fn written(select: &Select, style: ParamStyle) -> (String, Vec<Value>) {
        let written = write(select, Dialect::Postgres, style);
        (written.sql, written.params)
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
                    expr: Expr::binary(Binary::Mul, column("risk"), Expr::Number("2".to_string())),
                    alias: Some("double risk".to_string()),
                },
            ],
            from: Some(Source {
                kind: SourceKind::Table(vec!["cases".to_string()]),
                alias: None,
            }),
            filter: vec![
                Expr::binary(
                    Binary::Eq,
                    column("status"),
                    Expr::Param(Value::Text("open".to_string())),
                ),
                Expr::binary(
                    Binary::Or,
                    Expr::binary(Binary::Gt, column("risk"), Expr::Number("90".to_string())),
                    Expr::IsNull(Box::new(column("user")), false),
                ),
            ],
            order: vec![Order {
                expr: column("risk"),
                descending: true,
                nulls_last: true,
            }],
            limit: Some(20),
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
        assert_eq!(params, [Value::Text("open".to_string())]);
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
                Expr::Unary(Unary::Neg, Box::new(Expr::Number("-5".to_string()))),
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
            filter: vec![Expr::binary(
                Binary::Eq,
                column("band"),
                Expr::Param(Value::Text("high".to_string())),
            )],
            ..Select::default()
        };
        let outer = Select {
            from: Some(Source {
                kind: SourceKind::Query(Box::new(inner)),
                alias: Some("_q1".to_string()),
            }),
            filter: vec![Expr::binary(
                Binary::Eq,
                column("status"),
                Expr::Param(Value::Text("open".to_string())),
            )],
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
        assert_eq!(
            params,
            [
                Value::Text("high".to_string()),
                Value::Text("open".to_string())
            ]
        );
        let (format, _) = written(
            &Select {
                columns: vec![Item::Expr {
                    expr: Expr::binary(Binary::Mod, column("a"), Expr::Param(Value::Int(2))),
                    alias: None,
                }],
                ..Select::default()
            },
            ParamStyle::Format,
        );
        assert_eq!(format, "SELECT a %% %s");
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
}
