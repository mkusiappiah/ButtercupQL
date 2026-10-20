//! What the compiler knows of a database: its tables, their columns, and the kind of value each column holds.
//!
//! A schema is optional. Without one, ButtercupQL compiles names as they are written and leaves the database to check
//! them; with one, it checks every name, suggests the one that was meant, and writes SQL that depends on the kinds of
//! values, such as joining text with `+`.

use crate::diagnostic::Diagnostic;
use crate::span::Span;

/// The kind of value a column or an expression holds, as far as ButtercupQL can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Type {
    /// Not known: there is no schema, or it does not say.
    Unknown,
    /// `True` or `False`.
    Bool,
    /// A whole number.
    Int,
    /// A number with a fraction, kept to a binary precision: SQL's `real` or `double precision`.
    Float,
    /// A number with a fraction, kept exactly: SQL's `numeric` or `decimal`.
    Decimal,
    /// Text.
    Text,
    /// A day.
    Date,
    /// A moment: a day and a time of day.
    Timestamp,
    /// A length of time, such as `days(7)`.
    Duration,
}

impl Type {
    /// Whether the type is a number.
    pub fn is_number(self) -> bool {
        matches!(self, Type::Int | Type::Float | Type::Decimal)
    }

    /// Whether the type is a day or a moment.
    pub fn is_time(self) -> bool {
        matches!(self, Type::Date | Type::Timestamp)
    }

    /// The type, as a message names it.
    pub fn describe(self) -> &'static str {
        match self {
            Type::Unknown => "a value",
            Type::Bool => "`True` or `False`",
            Type::Int => "a whole number",
            Type::Float | Type::Decimal => "a number",
            Type::Text => "text",
            Type::Date => "a date",
            Type::Timestamp => "a date and time",
            Type::Duration => "a length of time",
        }
    }

    /// The type an SQL type name stands for: `varchar(20)` is text, `timestamp with time zone` is a moment. A name
    /// ButtercupQL does not know is [`Type::Unknown`].
    pub fn from_sql(name: &str) -> Type {
        let name = name.trim().to_ascii_lowercase();
        let base = name.split('(').next().unwrap_or("").trim();
        let exact = match base {
            "int" | "integer" | "bigint" | "smallint" | "tinyint" | "mediumint" | "int2"
            | "int4" | "int8" | "serial" | "bigserial" | "smallserial" | "serial4" | "serial8"
            | "oid" => Some(Type::Int),
            "real" | "float" | "float4" | "float8" | "double" | "double precision" => {
                Some(Type::Float)
            }
            "numeric" | "decimal" | "money" | "number" => Some(Type::Decimal),
            "text" | "varchar" | "char" | "character" | "character varying" | "nchar"
            | "nvarchar" | "varchar2" | "nvarchar2" | "clob" | "string" | "citext" | "bpchar"
            | "name" | "tinytext" | "mediumtext" | "longtext" => Some(Type::Text),
            "bool" | "boolean" => Some(Type::Bool),
            "date" => Some(Type::Date),
            "timestamp"
            | "timestamptz"
            | "datetime"
            | "timestamp with time zone"
            | "timestamp without time zone" => Some(Type::Timestamp),
            "interval" => Some(Type::Duration),
            _ => None,
        };
        if let Some(found) = exact {
            return found;
        }
        // The rules SQLite uses to read a type it does not know.
        if base.contains("int") {
            Type::Int
        } else if base.contains("char") || base.contains("clob") || base.contains("text") {
            Type::Text
        } else if base.contains("real") || base.contains("floa") || base.contains("doub") {
            Type::Float
        } else if base.starts_with("timestamp") {
            Type::Timestamp
        } else {
            Type::Unknown
        }
    }
}

/// A column of a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// The column's name.
    pub name: String,
    /// The kind of value it holds.
    pub ty: Type,
    /// Whether it can be missing (SQL's `NULL`).
    pub nullable: bool,
    /// The row of another table the column points to, when it holds that row's key: SQL's `REFERENCES`.
    pub references: Option<Reference>,
}

/// A column of another table that a column points to: `accounts.id` for `account_id REFERENCES accounts (id)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// The table pointed to.
    pub table: String,
    /// Its column that holds the key.
    pub column: String,
}

impl Column {
    /// A column that can be missing.
    pub fn new(name: impl Into<String>, ty: Type) -> Column {
        Column {
            name: name.into(),
            ty,
            nullable: true,
            references: None,
        }
    }

    /// The column, marked as never missing: SQL's `NOT NULL`.
    #[must_use]
    pub fn not_null(mut self) -> Column {
        self.nullable = false;
        self
    }

    /// The column, marked as holding the key of another table's rows. A query follows it with a dot: a column
    /// `account_id` that points to `accounts` makes `account.operator` read the account's operator.
    #[must_use]
    pub fn references(mut self, table: impl Into<String>, column: impl Into<String>) -> Column {
        self.references = Some(Reference {
            table: table.into(),
            column: column.into(),
        });
        self
    }

    /// The name a query follows the column's reference by: the column's name without `_id`, so `account_id` is
    /// followed as `account`.
    pub fn link(&self) -> Option<&str> {
        self.references.as_ref()?;
        Some(
            self.name
                .strip_suffix("_id")
                .filter(|stem| !stem.is_empty())
                .unwrap_or(&self.name),
        )
    }
}

/// A table and its columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// The table's name, as the database knows it: `cases`, or `analytics.daily` with its schema.
    pub name: String,
    /// Its columns, in order.
    pub columns: Vec<Column>,
}

impl Table {
    /// The column with this name. A name matches with or without regard to case, as SQL names do.
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns
            .iter()
            .find(|column| column.name == name)
            .or_else(|| {
                self.columns
                    .iter()
                    .find(|column| column.name.eq_ignore_ascii_case(name))
            })
    }
}

/// The tables of a database.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Schema {
    tables: Vec<Table>,
}

impl Schema {
    /// A schema with no tables yet.
    pub fn new() -> Schema {
        Schema::default()
    }

    /// The schema with one more table. A table of the same name is replaced.
    #[must_use]
    pub fn with_table(
        mut self,
        name: impl Into<String>,
        columns: impl IntoIterator<Item = Column>,
    ) -> Schema {
        self.add_table(Table {
            name: name.into(),
            columns: columns.into_iter().collect(),
        });
        self
    }

    /// Adds a table, replacing one of the same name.
    pub fn add_table(&mut self, table: Table) {
        self.tables
            .retain(|other| !other.name.eq_ignore_ascii_case(&table.name));
        self.tables.push(table);
    }

    /// The tables, in the order they were added.
    pub fn tables(&self) -> &[Table] {
        &self.tables
    }

    /// The table with this name: `cases`, or `analytics.daily`. A name matches with or without regard to case, and a
    /// table in a database schema also matches by its own name alone.
    pub fn table(&self, name: &str) -> Option<&Table> {
        let own_name = |table: &&Table| {
            table
                .name
                .rsplit('.')
                .next()
                .unwrap_or(&table.name)
                .to_string()
        };
        self.tables
            .iter()
            .find(|table| table.name == name)
            .or_else(|| {
                self.tables
                    .iter()
                    .find(|table| table.name.eq_ignore_ascii_case(name))
            })
            .or_else(|| {
                self.tables
                    .iter()
                    .find(|table| !name.contains('.') && own_name(table).eq_ignore_ascii_case(name))
            })
    }

    /// Reads the tables from SQL `CREATE TABLE` statements, such as a database's own schema script. Every other
    /// statement is passed over, so a whole migration file can be read.
    pub fn from_sql(text: &str) -> Result<Schema, Vec<Diagnostic>> {
        let tokens = sql_tokens(text);
        let mut schema = Schema::new();
        let mut problems = Vec::new();
        let mut keys: Vec<(String, Vec<String>)> = Vec::new();
        let mut at = 0;
        while at < tokens.len() {
            let end = statement_end(&tokens, at);
            if let Some((table, primary)) = create_table(text, &tokens[at..end], &mut problems) {
                keys.push((table.name.clone(), primary));
                schema.add_table(table);
            }
            at = end + 1;
        }
        // `REFERENCES accounts` with no column points to the table's key.
        for table in &mut schema.tables {
            for column in &mut table.columns {
                if let Some(reference) = &mut column.references {
                    if reference.column.is_empty() {
                        let key = keys
                            .iter()
                            .find(|(name, _)| name.eq_ignore_ascii_case(&reference.table))
                            .and_then(|(_, primary)| {
                                (primary.len() == 1).then(|| primary[0].clone())
                            });
                        reference.column = key.unwrap_or_else(|| "id".to_string());
                    }
                }
            }
        }
        if problems.is_empty() {
            Ok(schema)
        } else {
            Err(problems)
        }
    }
}

// ---- Reading CREATE TABLE statements

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Word,
    Quoted,
    Punct,
    Other,
}

#[derive(Debug, Clone)]
struct SqlToken {
    kind: Kind,
    /// A word's text, a quoted name without its quotes, or a punctuation mark.
    text: String,
    span: Span,
}

impl SqlToken {
    fn is(&self, word: &str) -> bool {
        self.kind == Kind::Word && self.text.eq_ignore_ascii_case(word)
    }

    fn is_punct(&self, mark: &str) -> bool {
        self.kind == Kind::Punct && self.text == mark
    }
}

/// Splits SQL into words, quoted names and punctuation, leaving out comments and strings.
fn sql_tokens(text: &str) -> Vec<SqlToken> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let b = bytes[at];
        let start = at;
        if b.is_ascii_whitespace() {
            at += 1;
        } else if b == b'-' && bytes.get(at + 1) == Some(&b'-') {
            while at < bytes.len() && bytes[at] != b'\n' {
                at += 1;
            }
        } else if b == b'/' && bytes.get(at + 1) == Some(&b'*') {
            at = text[at + 2..]
                .find("*/")
                .map_or(bytes.len(), |end| at + 2 + end + 2);
        } else if b == b'\'' {
            at += 1;
            while at < bytes.len() {
                if bytes[at] == b'\'' {
                    if bytes.get(at + 1) == Some(&b'\'') {
                        at += 2;
                        continue;
                    }
                    at += 1;
                    break;
                }
                at += 1;
            }
            tokens.push(SqlToken {
                kind: Kind::Other,
                text: String::new(),
                span: Span::new(start, at),
            });
        } else if matches!(b, b'"' | b'`' | b'[') {
            let close = if b == b'[' { b']' } else { b };
            at += 1;
            let mut name = String::new();
            while at < bytes.len() {
                if bytes[at] == close {
                    if close != b']' && bytes.get(at + 1) == Some(&close) {
                        name.push(close as char);
                        at += 2;
                        continue;
                    }
                    at += 1;
                    break;
                }
                let c = text[at..].chars().next().unwrap_or(' ');
                name.push(c);
                at += c.len_utf8();
            }
            tokens.push(SqlToken {
                kind: Kind::Quoted,
                text: name,
                span: Span::new(start, at),
            });
        } else if b == b'$' {
            // A dollar-quoted string, as PostgreSQL writes function bodies: $$ ... $$ or $tag$ ... $tag$.
            let tag_end = text[at + 1..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map(|end| at + 1 + end);
            match tag_end {
                Some(end) if bytes[end] == b'$' => {
                    let tag = &text[at..=end];
                    at = text[end + 1..]
                        .find(tag)
                        .map_or(bytes.len(), |close| end + 1 + close + tag.len());
                }
                _ => at += 1,
            }
            tokens.push(SqlToken {
                kind: Kind::Other,
                text: String::new(),
                span: Span::new(start, at),
            });
        } else if b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80 {
            while at < bytes.len() {
                let c = text[at..].chars().next().unwrap_or(' ');
                if !(c.is_alphanumeric() || c == '_' || c == '$') {
                    break;
                }
                at += c.len_utf8();
            }
            tokens.push(SqlToken {
                kind: Kind::Word,
                text: text[start..at].to_string(),
                span: Span::new(start, at),
            });
        } else {
            at += 1;
            tokens.push(SqlToken {
                kind: Kind::Punct,
                text: (b as char).to_string(),
                span: Span::new(start, at),
            });
        }
    }
    tokens
}

/// Where the statement starting at `at` ends: at its `;`, or at the end of the text.
fn statement_end(tokens: &[SqlToken], mut at: usize) -> usize {
    let mut depth = 0usize;
    while at < tokens.len() {
        let token = &tokens[at];
        if token.is_punct("(") {
            depth += 1;
        } else if token.is_punct(")") {
            depth = depth.saturating_sub(1);
        } else if token.is_punct(";") && depth == 0 {
            return at;
        }
        at += 1;
    }
    at
}

/// What follows `REFERENCES`: the table, and the column if one is given. An empty column is the table's key.
fn reference(tokens: &[SqlToken]) -> Option<Reference> {
    let mut at = tokens.iter().position(|token| token.is("references"))? + 1;
    let mut table = Vec::new();
    while let Some(token) = tokens.get(at) {
        if !matches!(token.kind, Kind::Word | Kind::Quoted) {
            break;
        }
        table.push(token.text.clone());
        at += 1;
        if !tokens.get(at).is_some_and(|token| token.is_punct(".")) {
            break;
        }
        at += 1;
    }
    if table.is_empty() {
        return None;
    }
    let column = match (tokens.get(at), tokens.get(at + 1)) {
        (Some(open), Some(column))
            if open.is_punct("(") && matches!(column.kind, Kind::Word | Kind::Quoted) =>
        {
            column.text.clone()
        }
        _ => String::new(),
    };
    Some(Reference {
        table: table.join("."),
        column,
    })
}

/// The names in the first brackets of a list of tokens: `(a, b)`.
fn bracketed_names(tokens: &[SqlToken]) -> Vec<String> {
    let Some(open) = tokens.iter().position(|token| token.is_punct("(")) else {
        return Vec::new();
    };
    tokens[open + 1..]
        .iter()
        .take_while(|token| !token.is_punct(")"))
        .filter(|token| matches!(token.kind, Kind::Word | Kind::Quoted))
        .map(|token| token.text.clone())
        .collect()
}

/// The table a statement creates, if it is a `CREATE TABLE` with a list of columns, and its key's columns.
fn create_table(
    text: &str,
    tokens: &[SqlToken],
    problems: &mut Vec<Diagnostic>,
) -> Option<(Table, Vec<String>)> {
    let mut at = 0;
    let word = |at: usize, word: &str| tokens.get(at).is_some_and(|token| token.is(word));
    if !word(at, "create") {
        return None;
    }
    at += 1;
    if word(at, "or") && word(at + 1, "replace") {
        at += 2;
    }
    while [
        "global",
        "local",
        "temp",
        "temporary",
        "unlogged",
        "virtual",
    ]
    .iter()
    .any(|w| word(at, w))
    {
        at += 1;
    }
    if !word(at, "table") {
        return None;
    }
    at += 1;
    if word(at, "if") && word(at + 1, "not") && word(at + 2, "exists") {
        at += 3;
    }
    // The table's name, maybe with its schema: `analytics.daily`.
    let mut name = Vec::new();
    while let Some(token) = tokens.get(at) {
        if !matches!(token.kind, Kind::Word | Kind::Quoted) {
            break;
        }
        name.push(token.text.clone());
        at += 1;
        if !tokens.get(at).is_some_and(|token| token.is_punct(".")) {
            break;
        }
        at += 1;
    }
    if name.is_empty() {
        let span = tokens
            .get(at)
            .map_or(Span::empty(text.len()), |token| token.span);
        problems.push(Diagnostic::error(
            span,
            "expected the table's name after `CREATE TABLE`",
        ));
        return None;
    }
    // `CREATE TABLE name AS SELECT ...` and `CREATE VIRTUAL TABLE ... USING` give no list of columns to read.
    if !tokens.get(at).is_some_and(|token| token.is_punct("(")) {
        return None;
    }
    let open = at;
    let mut depth = 0usize;
    let mut close = None;
    for (offset, token) in tokens[open..].iter().enumerate() {
        if token.is_punct("(") {
            depth += 1;
        } else if token.is_punct(")") {
            depth -= 1;
            if depth == 0 {
                close = Some(open + offset);
                break;
            }
        }
    }
    let Some(close) = close else {
        problems.push(Diagnostic::error(
            tokens[open].span,
            "this `(` is never closed",
        ));
        return None;
    };

    let mut columns: Vec<Column> = Vec::new();
    let mut primary: Vec<String> = Vec::new();
    let mut foreign_keys: Vec<(String, Reference)> = Vec::new();
    for part in split_commas(&tokens[open + 1..close]) {
        let Some(first) = part.first() else {
            continue;
        };
        let constraint = [
            "constraint",
            "primary",
            "unique",
            "check",
            "foreign",
            "exclude",
            "like",
        ]
        .iter()
        .any(|w| first.is(w));
        if constraint {
            // `PRIMARY KEY (a, b)` makes its columns never missing.
            if let Some(key) = part.iter().position(|token| token.is("primary")) {
                primary.extend(bracketed_names(&part[key..]));
            }
            // `FOREIGN KEY (account_id) REFERENCES accounts (id)` makes the column point to the other table.
            if let Some(foreign) = part.iter().position(|token| token.is("foreign")) {
                let from = bracketed_names(&part[foreign..]);
                if let ([column], Some(target)) = (from.as_slice(), reference(&part[foreign..])) {
                    foreign_keys.push((column.clone(), target));
                }
            }
            continue;
        }
        if !matches!(first.kind, Kind::Word | Kind::Quoted) {
            problems.push(Diagnostic::error(first.span, "expected a column's name"));
            continue;
        }
        let stops = [
            "not",
            "null",
            "primary",
            "unique",
            "check",
            "default",
            "references",
            "constraint",
            "collate",
            "generated",
            "as",
            "autoincrement",
            "identity",
        ];
        let type_end = part
            .iter()
            .skip(1)
            .position(|token| token.kind == Kind::Word && stops.iter().any(|w| token.is(w)))
            .map_or(part.len(), |end| end + 1);
        let type_name = match (part.get(1), part.get(type_end.saturating_sub(1))) {
            (Some(start), Some(end)) if type_end > 1 => {
                &text[start.span.start as usize..end.span.end as usize]
            }
            _ => "",
        };
        let rest = &part[type_end..];
        let not_null = rest
            .windows(2)
            .any(|pair| pair[0].is("not") && pair[1].is("null"))
            || rest.iter().any(|token| token.is("primary"));
        if rest.iter().any(|token| token.is("primary")) {
            primary.push(first.text.clone());
        }
        columns.push(Column {
            name: first.text.clone(),
            ty: Type::from_sql(type_name),
            nullable: !not_null,
            references: reference(rest),
        });
    }
    for (name, target) in foreign_keys {
        if let Some(column) = columns
            .iter_mut()
            .find(|column| column.name.eq_ignore_ascii_case(&name))
        {
            column.references = Some(target);
        }
    }
    for column in &mut columns {
        if primary
            .iter()
            .any(|key| key.eq_ignore_ascii_case(&column.name))
        {
            column.nullable = false;
        }
    }
    Some((
        Table {
            name: name.join("."),
            columns,
        },
        primary,
    ))
}

/// The parts of a list, split at its commas outside brackets.
fn split_commas(tokens: &[SqlToken]) -> Vec<&[SqlToken]> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, token) in tokens.iter().enumerate() {
        if token.is_punct("(") {
            depth += 1;
        } else if token.is_punct(")") {
            depth = depth.saturating_sub(1);
        } else if token.is_punct(",") && depth == 0 {
            parts.push(&tokens[start..at]);
            start = at + 1;
        }
    }
    parts.push(&tokens[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_type_names_are_read() {
        assert_eq!(Type::from_sql("VARCHAR(20)"), Type::Text);
        assert_eq!(Type::from_sql("timestamp with time zone"), Type::Timestamp);
        assert_eq!(Type::from_sql("BIGINT"), Type::Int);
        assert_eq!(Type::from_sql("numeric(12, 2)"), Type::Decimal);
        assert_eq!(Type::from_sql("double precision"), Type::Float);
        assert_eq!(Type::from_sql("UNSIGNED BIG INT"), Type::Int);
        assert_eq!(Type::from_sql("jsonb"), Type::Unknown);
    }

    #[test]
    fn create_table_statements_are_read() {
        let schema = Schema::from_sql(
            r#"
            -- The cases
            CREATE TABLE IF NOT EXISTS cases (
                id         bigserial PRIMARY KEY,
                "key"      text NOT NULL UNIQUE,
                account_id bigint REFERENCES accounts (id),
                risk       numeric(5, 2) DEFAULT 0 CHECK (risk >= 0),
                status     varchar(20) NOT NULL DEFAULT 'open',
                raised     timestamptz NOT NULL DEFAULT now(),
                notes      text,
                CONSTRAINT one_key UNIQUE (key, status)
            );
            CREATE INDEX cases_risk ON cases (risk DESC);
            CREATE TABLE analytics.daily (day date, total integer, PRIMARY KEY (day));
            CREATE FUNCTION touch() RETURNS trigger AS $$ BEGIN CREATE TABLE x (y int); END $$ LANGUAGE plpgsql;
            CREATE TABLE copy AS SELECT * FROM cases;
            "#,
        )
        .expect("the schema reads");
        assert_eq!(schema.tables().len(), 2);
        let cases = schema.table("cases").expect("the cases table");
        let columns: Vec<(&str, Type, bool)> = cases
            .columns
            .iter()
            .map(|column| (column.name.as_str(), column.ty, column.nullable))
            .collect();
        assert_eq!(
            columns,
            [
                ("id", Type::Int, false),
                ("key", Type::Text, false),
                ("account_id", Type::Int, true),
                ("risk", Type::Decimal, true),
                ("status", Type::Text, false),
                ("raised", Type::Timestamp, false),
                ("notes", Type::Text, true),
            ]
        );
        let daily = schema
            .table("daily")
            .expect("a table is found by its own name too");
        assert_eq!(daily.name, "analytics.daily");
        assert!(
            !daily
                .column("DAY")
                .expect("names match without regard to case")
                .nullable
        );
    }

    #[test]
    fn references_are_read_from_columns_and_constraints() {
        let schema = Schema::from_sql(
            "\
CREATE TABLE banks (code text PRIMARY KEY, name text);
CREATE TABLE accounts (id bigint PRIMARY KEY, bank text REFERENCES banks, operator text);
CREATE TABLE cases (
    id bigint PRIMARY KEY,
    account_id bigint,
    owner_id bigint REFERENCES people (person),
    CONSTRAINT to_account FOREIGN KEY (account_id) REFERENCES accounts (id)
);",
        )
        .expect("the schema reads");
        let cases = schema.table("cases").expect("cases");
        let account = cases.column("account_id").expect("account_id");
        assert_eq!(
            account.references,
            Some(Reference {
                table: "accounts".to_string(),
                column: "id".to_string()
            })
        );
        assert_eq!(account.link(), Some("account"));
        assert_eq!(
            cases.column("owner_id").and_then(Column::link),
            Some("owner")
        );
        let bank = schema
            .table("accounts")
            .and_then(|table| table.column("bank"))
            .expect("bank");
        assert_eq!(
            bank.references.as_ref().map(|r| r.column.as_str()),
            Some("code")
        );
        assert_eq!(bank.link(), Some("bank"));
        assert_eq!(cases.column("id").and_then(Column::link), None);
    }

    #[test]
    fn a_schema_can_be_built_in_code() {
        let schema = Schema::new().with_table(
            "cases",
            [
                Column::new("key", Type::Text).not_null(),
                Column::new("risk", Type::Int),
            ],
        );
        assert_eq!(schema.table("Cases").map(|t| t.columns.len()), Some(2));
        assert!(schema.table("accounts").is_none());
    }

    #[test]
    fn a_broken_create_table_is_reported() {
        let problems = Schema::from_sql("CREATE TABLE cases (id int").unwrap_err();
        assert_eq!(problems[0].message, "this `(` is never closed");
    }
}
