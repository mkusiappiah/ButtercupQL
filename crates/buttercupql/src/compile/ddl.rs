//! Changes to the database itself: tables, indexes and permissions, and `def`s stored in it. Each is written for its
//! database, with every value written into the SQL, as these statements take no placeholders.

use super::*;
use crate::schema::{Column, Table};
use crate::sql::{self as written, Args, Statement};

/// A column's SQL type on a database, for its Python type, and the type ButtercupQL knows it by.
fn column_type(ty: &ast::Type, dialect: Dialect) -> Option<(&'static str, Type)> {
    let name: Vec<&str> = ty
        .name
        .parts
        .iter()
        .map(|part| part.text.as_str())
        .collect();
    let (postgres, sqlite, duckdb, clickhouse, known) = match name.join(".").as_str() {
        "int" => ("BIGINT", "INTEGER", "BIGINT", "Int64", Type::Int),
        "float" => ("DOUBLE PRECISION", "REAL", "DOUBLE", "Float64", Type::Float),
        "str" => ("TEXT", "TEXT", "VARCHAR", "String", Type::Text),
        "bool" => ("BOOLEAN", "BOOLEAN", "BOOLEAN", "Bool", Type::Bool),
        "date" => ("DATE", "TEXT", "DATE", "Date", Type::Date),
        "datetime" => (
            "TIMESTAMP",
            "TEXT",
            "TIMESTAMP",
            "DateTime",
            Type::Timestamp,
        ),
        "Decimal" | "decimal" => (
            "NUMERIC",
            "NUMERIC",
            "DECIMAL(38, 10)",
            "Decimal(38, 10)",
            Type::Decimal,
        ),
        "bytes" => ("BYTEA", "BLOB", "BLOB", "String", Type::Unknown),
        _ => return None,
    };
    let sql = match dialect {
        Dialect::Postgres => postgres,
        Dialect::Sqlite => sqlite,
        Dialect::DuckDb => duckdb,
        Dialect::ClickHouse => clickhouse,
    };
    Some((sql, known))
}

/// PostgreSQL's type for a value ButtercupQL knows the type of.
fn postgres_type(ty: Type) -> Option<&'static str> {
    Some(match ty {
        Type::Int => "BIGINT",
        Type::Float => "DOUBLE PRECISION",
        Type::Decimal => "NUMERIC",
        Type::Text => "TEXT",
        Type::Bool => "BOOLEAN",
        Type::Date => "DATE",
        Type::Timestamp => "TIMESTAMP",
        Type::Unknown | Type::Duration => return None,
    })
}

/// The names of a dotted name.
fn parts(path: &ast::Path) -> Vec<String> {
    path.parts.iter().map(|part| part.text.clone()).collect()
}

/// A value written into SQL.
fn literal_sql(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(true) => "TRUE".to_string(),
        Value::Bool(false) => "FALSE".to_string(),
        Value::Int(number) => number.to_string(),
        Value::Float(number) => format!("{number:?}"),
        Value::Text(text) => written::literal(text),
    }
}

impl<'a> Compiler<'a> {
    /// The options given after a comma, each checked to be one the statement takes.
    fn options(
        &mut self,
        options: &'a [ast::Arg],
        allowed: &[&str],
        what: &str,
    ) -> Option<Vec<(&'a str, &'a ast::Expr)>> {
        let mut found = Vec::new();
        let mut ok = true;
        for option in options {
            let Some(name) = &option.name else {
                continue;
            };
            if !allowed.contains(&name.text.as_str()) {
                let mut problem =
                    self.error(name.span, format!("{what} has no option `{}`", name.text));
                problem = match suggest::closest(&name.text, allowed.iter().copied()) {
                    Some(best) => problem.with_label(format!("did you mean `{best}`?")),
                    None => problem.with_help(format!("its options are {}", list(allowed))),
                };
                self.report(problem);
                ok = false;
                continue;
            }
            found.push((name.text.as_str(), &option.value));
        }
        ok.then_some(found)
    }

    /// A `True` or `False` written in the query.
    fn flag(&mut self, value: &ast::Expr) -> Option<bool> {
        match value.kind {
            ExprKind::Bool(flag) => Some(flag),
            _ => {
                let problem = self.error(value.span, "expected `True` or `False`");
                self.report(problem);
                None
            }
        }
    }

    fn flag_option(&mut self, options: &[(&str, &'a ast::Expr)], name: &str) -> Option<bool> {
        match options.iter().find(|(option, _)| *option == name) {
            Some((_, value)) => self.flag(value),
            None => Some(false),
        }
    }

    /// A value written into SQL whole, for a column's default.
    fn inline_value(&mut self, value: &'a ast::Expr, frame: &Frame) -> Option<(String, Typed)> {
        let empty = Bindings::new();
        let typed = self.expr(value, Cx::of(frame, &empty))?;
        let sql = written::write_expr_inline(&typed.sql, self.dialect);
        let simple = matches!(
            typed.sql,
            Sql::Param(..) | Sql::Number(_) | Sql::Keyword(_) | Sql::Text(_)
        );
        // An expression in a default goes in brackets, as SQLite asks.
        let sql = if simple { sql } else { format!("({sql})") };
        Some((sql, typed))
    }

    /// `create table name:`: the statement, and the table as the schema of the statements after it knows it.
    pub(crate) fn create_table(
        &mut self,
        table: &'a ast::CreateTable,
    ) -> Option<(Vec<Statement>, Table)> {
        let options = self.options(&table.options, &["exist_ok"], "`create table`")?;
        let exist_ok = self.flag_option(&options, "exist_ok")?;
        let name = parts(&table.name);
        let mut ok = true;
        let mut lines = Vec::new();
        let mut columns = Vec::new();
        let mut primary: Vec<String> = Vec::new();
        let mut uniques: Vec<Vec<String>> = Vec::new();
        let mut dropped: Vec<&'static str> = Vec::new();
        for column in &table.columns {
            let Some(line) = self.column_line(column, &mut primary, &mut uniques, &mut dropped)
            else {
                ok = false;
                continue;
            };
            lines.push(line.0);
            columns.push(line.1);
        }
        for key in &table.keys {
            let names: Vec<String> = key.columns.iter().map(|name| name.text.clone()).collect();
            for column in &key.columns {
                if !table
                    .columns
                    .iter()
                    .any(|defined| defined.name.text == column.text)
                {
                    let problem = self.error(
                        column.span,
                        format!("there is no column `{}` in this table", column.text),
                    );
                    self.report(problem);
                    ok = false;
                }
            }
            if key.primary {
                if !primary.is_empty() {
                    let problem = self.error(key.span, "a table has one primary key");
                    self.report(problem);
                    ok = false;
                }
                primary = names;
            } else {
                uniques.push(names);
            }
        }
        if !ok {
            return None;
        }
        for column in &mut columns {
            if primary.contains(&column.name) {
                column.nullable = false;
            }
        }
        let exists = if exist_ok { "IF NOT EXISTS " } else { "" };
        let mut sql = format!("CREATE TABLE {exists}{} (", written::path(&name));
        let keys = |names: &[String]| {
            names
                .iter()
                .map(|name| written::ident(name))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let mut parts_sql = lines;
        if self.dialect == Dialect::ClickHouse {
            if !dropped.is_empty() || !uniques.is_empty() {
                let warning = Diagnostic::warning(
                    table.name.span,
                    "ClickHouse keeps neither unique keys nor references, so they are left out",
                );
                self.report(warning);
            }
        } else {
            if !primary.is_empty() {
                parts_sql.push(format!("PRIMARY KEY ({})", keys(&primary)));
            }
            for unique in &uniques {
                parts_sql.push(format!("UNIQUE ({})", keys(unique)));
            }
        }
        for (n, part) in parts_sql.iter().enumerate() {
            sql.push_str(if n == 0 { "\n  " } else { ",\n  " });
            sql.push_str(part);
        }
        sql.push_str("\n)");
        if self.dialect == Dialect::ClickHouse {
            // ClickHouse keeps a table's rows sorted by its key.
            let order = if primary.is_empty() {
                "tuple()".to_string()
            } else {
                format!("({})", keys(&primary))
            };
            sql.push_str(&format!("\nENGINE = MergeTree\nORDER BY {order}"));
        }
        Some((
            vec![Statement::Written(sql)],
            Table {
                name: name.join("."),
                columns,
                primary_key: primary,
            },
        ))
    }

    /// A column of `create table` as SQL, and as the schema knows it.
    fn column_line(
        &mut self,
        column: &'a ast::ColumnDef,
        primary: &mut Vec<String>,
        uniques: &mut Vec<Vec<String>>,
        dropped: &mut Vec<&'static str>,
    ) -> Option<(String, Column)> {
        let Some((sql_type, known)) = column_type(&column.ty, self.dialect) else {
            let written = &column.ty.name.parts;
            let name: Vec<&str> = written.iter().map(|part| part.text.as_str()).collect();
            let types = [
                "int", "float", "str", "bool", "date", "datetime", "Decimal", "bytes",
            ];
            let mut problem = self.error(
                column.ty.span,
                format!("there is no column type `{}`", name.join(".")),
            );
            problem = match suggest::closest(&name.join("."), types.iter().copied()) {
                Some(best) => problem.with_label(format!("did you mean `{best}`?")),
                None => problem.with_help(format!("a column's type is one of {}", list(&types))),
            };
            self.report(problem);
            return None;
        };
        let mut references = None;
        for arg in &column.field {
            let Some(name) = &arg.name else {
                let problem = self
                    .error(arg.value.span, "the arguments of `field()` are named")
                    .with_help("for example: `field(primary_key=True)`");
                self.report(problem);
                return None;
            };
            match name.text.as_str() {
                "primary_key" => {
                    if self.flag(&arg.value)? {
                        primary.push(column.name.text.clone());
                    }
                }
                "unique" => {
                    if self.flag(&arg.value)? {
                        uniques.push(vec![column.name.text.clone()]);
                    }
                }
                "references" => {
                    let (table, key) = match &arg.value.kind {
                        ExprKind::Attribute(base, key) => {
                            (path_of(base).join("."), key.text.clone())
                        }
                        ExprKind::Name(table) => {
                            let key = self
                                .schema
                                .and_then(|schema| schema.table(table))
                                .and_then(|found| match found.primary_key.as_slice() {
                                    [key] => Some(key.clone()),
                                    _ => None,
                                })
                                .unwrap_or_else(|| "id".to_string());
                            (table.clone(), key)
                        }
                        _ => {
                            let problem = self
                                .error(arg.value.span, "`references=` takes a table's column")
                                .with_help("for example: `field(references=accounts.id)`");
                            self.report(problem);
                            return None;
                        }
                    };
                    references = Some(Reference { table, column: key });
                }
                other => {
                    let names = ["primary_key", "unique", "references", "default"];
                    let mut problem =
                        self.error(name.span, format!("`field()` has no argument `{other}`"));
                    if let Some(best) = suggest::closest(other, names.iter().copied()) {
                        problem = problem.with_label(format!("did you mean `{best}`?"));
                    }
                    self.report(problem);
                    return None;
                }
            }
        }
        let default = match &column.default {
            Some(value) => {
                let (sql, typed) = self.inline_value(value, &Frame::default())?;
                let fits = typed.ty == Type::Unknown
                    || typed.ty == known
                    || (known.is_number() && typed.ty.is_number())
                    || (known.is_time() && typed.ty.is_time());
                if !fits {
                    let problem = self.error(
                        value.span,
                        format!(
                            "`{}` is {}, and this default is {}",
                            column.name.text,
                            known.describe(),
                            typed.ty.describe()
                        ),
                    );
                    self.report(problem);
                    return None;
                }
                Some(sql)
            }
            None => None,
        };
        let name = written::ident(&column.name.text);
        let mut line = if self.dialect == Dialect::ClickHouse {
            if column.nullable {
                format!("{name} Nullable({sql_type})")
            } else {
                format!("{name} {sql_type}")
            }
        } else {
            let not_null = if column.nullable { "" } else { " NOT NULL" };
            format!("{name} {sql_type}{not_null}")
        };
        if let Some(default) = default {
            line.push_str(&format!(" DEFAULT {default}"));
        }
        if let Some(reference) = &references {
            if self.dialect == Dialect::ClickHouse {
                dropped.push("references");
            } else {
                let table: Vec<String> = reference.table.split('.').map(str::to_string).collect();
                line.push_str(&format!(
                    " REFERENCES {} ({})",
                    written::path(&table),
                    written::ident(&reference.column)
                ));
            }
        }
        Some((
            line,
            Column {
                name: column.name.text.clone(),
                ty: known,
                nullable: column.nullable,
                references,
            },
        ))
    }

    /// `create index on cases(-risk, raised)`.
    pub(crate) fn create_index(&mut self, index: &'a ast::CreateIndex) -> Option<Vec<Statement>> {
        if self.dialect == Dialect::ClickHouse {
            let problem = self
                .error(index.span, "ClickHouse has no indexes like this")
                .with_help("ClickHouse keeps a table's rows sorted by its primary key: choose it in `create table`, with `field(primary_key=True)`");
            self.report(problem);
            return None;
        }
        let options = self.options(&index.options, &["where", "exist_ok"], "`create index`")?;
        let exist_ok = self.flag_option(&options, "exist_ok")?;
        let table = parts(&index.table);
        let source = ast::Source {
            value: path_expr(&index.table),
            alias: None,
            span: index.table.span,
        };
        let rel = self.table(&source);
        let empty = Bindings::new();
        let mut keys = Vec::new();
        let mut names = Vec::new();
        for key in &index.keys {
            let typed = self.expr(&key.value, Cx::of(&rel.frame, &empty))?;
            if let ExprKind::Name(name) = &key.value.kind {
                names.push(name.clone());
            }
            let mut written_key = written::write_expr_inline(&typed.sql, self.dialect);
            if key.descending && self.dialect != Dialect::DuckDb {
                written_key.push_str(" DESC");
            }
            // The index keeps missing values where `sort` puts them, so it serves the sort.
            if self.dialect == Dialect::Postgres && self.nulls_last(key.descending, &typed) {
                written_key.push_str(" NULLS LAST");
            }
            keys.push(written_key);
        }
        let condition = match options.iter().find(|(name, _)| *name == "where") {
            Some((_, condition)) => {
                if self.dialect == Dialect::DuckDb {
                    let problem = self.error(condition.span, "DuckDB's indexes take no condition");
                    self.report(problem);
                    return None;
                }
                let typed = self.expr(condition, Cx::of(&rel.frame, &empty))?;
                let truth = self.truth(typed);
                Some(written::write_expr_inline(&truth, self.dialect))
            }
            None => None,
        };
        let name = match &index.name {
            Some(name) => name.text.clone(),
            None => {
                let mut name = table.last().cloned().unwrap_or_default();
                for column in &names {
                    name.push('_');
                    name.push_str(column);
                }
                name.push_str("_idx");
                name.to_lowercase()
            }
        };
        let unique = if index.unique { "UNIQUE " } else { "" };
        let exists = if exist_ok { "IF NOT EXISTS " } else { "" };
        let mut sql = format!(
            "CREATE {unique}INDEX {exists}{} ON {} ({})",
            written::ident(&name),
            written::path(&table),
            keys.join(", ")
        );
        if let Some(condition) = condition {
            sql.push_str(&format!(" WHERE {condition}"));
        }
        Some(vec![Statement::Written(sql)])
    }

    /// `alter table name:`: a statement for each change, and the table as it is after them.
    pub(crate) fn alter_table(
        &mut self,
        alter: &'a ast::AlterTable,
    ) -> Option<(Vec<Statement>, Option<Table>)> {
        let table = parts(&alter.table);
        let mut known = self
            .schema
            .and_then(|schema| schema.table(&table.join(".")))
            .cloned();
        let mut statements = Vec::new();
        let prefix = format!("ALTER TABLE {}", written::path(&table));
        for change in &alter.changes {
            match change {
                ast::Change::Add(column) => {
                    if column.field.iter().any(|arg| {
                        arg.name.as_ref().is_some_and(|name| {
                            matches!(name.text.as_str(), "primary_key" | "unique")
                        })
                    }) {
                        let problem = self.error(
                            column.span,
                            "a column added to a table can't be one of its keys",
                        );
                        self.report(problem);
                        return None;
                    }
                    let (mut primary, mut uniques, mut dropped) =
                        (Vec::new(), Vec::new(), Vec::new());
                    let (line, added) =
                        self.column_line(column, &mut primary, &mut uniques, &mut dropped)?;
                    if self.dialect == Dialect::DuckDb {
                        // DuckDB adds a column with no constraints, and then makes it never missing.
                        if added.references.is_some() {
                            let problem = self.error(
                                column.span,
                                "DuckDB can't add a column that references another table",
                            );
                            self.report(problem);
                            return None;
                        }
                        statements.push(Statement::Written(format!(
                            "{prefix} ADD COLUMN {}",
                            line.replacen(" NOT NULL", "", 1)
                        )));
                        if !added.nullable {
                            statements.push(Statement::Written(format!(
                                "{prefix} ALTER COLUMN {} SET NOT NULL",
                                written::ident(&added.name)
                            )));
                        }
                    } else {
                        statements.push(Statement::Written(format!("{prefix} ADD COLUMN {line}")));
                    }
                    if let Some(known) = &mut known {
                        known.columns.push(added);
                    }
                }
                ast::Change::Drop(name) => {
                    statements.push(Statement::Written(format!(
                        "{prefix} DROP COLUMN {}",
                        written::ident(&name.text)
                    )));
                    if let Some(known) = &mut known {
                        known.columns.retain(|column| column.name != name.text);
                    }
                }
                ast::Change::Rename { from, to } => {
                    statements.push(Statement::Written(format!(
                        "{prefix} RENAME COLUMN {} TO {}",
                        written::ident(&from.text),
                        written::ident(&to.text)
                    )));
                    if let Some(known) = &mut known {
                        for column in &mut known.columns {
                            if column.name == from.text {
                                column.name = to.text.clone();
                            }
                        }
                    }
                }
            }
        }
        Some((statements, known))
    }

    /// `drop table name` or `drop index name`.
    pub(crate) fn drop(&mut self, drop: &'a ast::Drop) -> Option<Vec<Statement>> {
        let options = self.options(&drop.options, &["missing_ok"], "`drop`")?;
        let missing_ok = self.flag_option(&options, "missing_ok")?;
        if drop.index && self.dialect == Dialect::ClickHouse {
            let problem = self.error(drop.span, "ClickHouse has no indexes like this to drop");
            self.report(problem);
            return None;
        }
        let what = if drop.index { "INDEX" } else { "TABLE" };
        let exists = if missing_ok { "IF EXISTS " } else { "" };
        Some(vec![Statement::Written(format!(
            "DROP {what} {exists}{}",
            written::path(&parts(&drop.name))
        ))])
    }

    /// `grant read on cases to analyst`, or `revoke ...`.
    pub(crate) fn grant(&mut self, grant: &'a ast::Grant) -> Option<Vec<Statement>> {
        let word = if grant.revoke { "revoke" } else { "grant" };
        if matches!(self.dialect, Dialect::Sqlite | Dialect::DuckDb) {
            let database = if self.dialect == Dialect::Sqlite {
                "SQLite"
            } else {
                "DuckDB"
            };
            let problem = self
                .error(
                    grant.span,
                    format!("{database} has no users or permissions"),
                )
                .with_help(format!("`{word}` is for PostgreSQL and ClickHouse"));
            self.report(problem);
            return None;
        }
        let clickhouse = self.dialect == Dialect::ClickHouse;
        let mut privileges: Vec<&'static str> = Vec::new();
        let add = |list: &mut Vec<&'static str>, privilege: &'static str| {
            if !list.contains(&privilege) {
                list.push(privilege);
            }
        };
        for privilege in &grant.privileges {
            let wanted: &[&'static str] = match (privilege.text.as_str(), clickhouse) {
                ("read", _) | ("select", _) => &["SELECT"],
                ("write", false) => &["INSERT", "UPDATE", "DELETE"],
                ("write", true) => &["INSERT", "ALTER UPDATE", "ALTER DELETE"],
                ("insert", _) => &["INSERT"],
                ("update", false) => &["UPDATE"],
                ("update", true) => &["ALTER UPDATE"],
                ("delete", false) => &["DELETE"],
                ("delete", true) => &["ALTER DELETE"],
                ("all", false) => &["ALL PRIVILEGES"],
                ("all", true) => &["ALL"],
                (other, _) => {
                    let names = ["read", "write", "insert", "update", "delete", "all"];
                    let mut problem =
                        self.error(privilege.span, format!("there is no permission `{other}`"));
                    problem = match suggest::closest(other, names.iter().copied()) {
                        Some(best) => problem.with_label(format!("did you mean `{best}`?")),
                        None => problem.with_help(format!("the permissions are {}", list(&names))),
                    };
                    self.report(problem);
                    return None;
                }
            };
            for privilege in wanted {
                add(&mut privileges, privilege);
            }
        }
        let roles: Vec<String> = grant
            .roles
            .iter()
            .map(|role| written::ident(&role.text))
            .collect();
        let (verb, to) = if grant.revoke {
            ("REVOKE", "FROM")
        } else {
            ("GRANT", "TO")
        };
        let tables: Vec<String> = grant
            .tables
            .iter()
            .map(|table| written::path(&parts(table)))
            .collect();
        if clickhouse {
            // ClickHouse grants on one table at a time.
            return Some(
                tables
                    .iter()
                    .map(|table| {
                        Statement::Written(format!(
                            "{verb} {} ON {table} {to} {}",
                            privileges.join(", "),
                            roles.join(", ")
                        ))
                    })
                    .collect(),
            );
        }
        Some(vec![Statement::Written(format!(
            "{verb} {} ON TABLE {} {to} {}",
            privileges.join(", "),
            tables.join(", "),
            roles.join(", ")
        ))])
    }

    /// A `def` called on a line of its own: its query, run with the arguments given.
    pub(crate) fn call_statement(&mut self, call: &'a ast::Expr) -> Option<Statement> {
        let ExprKind::Call(function, args) = &call.kind else {
            return None;
        };
        let name = path_of(function).join(".");
        let Some((module, def)) = self.def(function) else {
            let mut problem =
                self.error(function.span, format!("there is no `def` named `{name}`"));
            let names = self.def_names();
            if let Some(best) = suggest::closest(&name, names.iter().map(String::as_str)) {
                problem = problem.with_label(format!("did you mean `{best}`?"));
            }
            self.report(problem);
            return None;
        };
        if !def.is_query() {
            let problem = self
                .error(call.span, format!("`{name}` holds steps, not a query"))
                .with_help("a `def` called on a line of its own runs its query, whose first step is `from`; to put its steps in place, call it inside a query");
            self.report(problem);
            return None;
        }
        let frame = Frame::default();
        let empty = Bindings::new();
        let bound = self.bind(module, def, args, call.span, Cx::of(&frame, &empty))?;
        self.enter(module, &name, call.span)?;
        let statement = self.within(module, |compiler| compiler.statement(&def.body, &bound));
        self.stack.pop();
        statement
    }

    /// A `@stored` def, put in the database: a function, a macro or a view that reads rows, or on PostgreSQL a
    /// procedure that changes them.
    pub(crate) fn store(&mut self, name: &'a ast::Name) -> Option<Vec<Statement>> {
        let Some(Found::Def(module, def)) = self.find(0, &name.text) else {
            return None;
        };
        if !def.is_query() {
            let problem = self
                .error(name.span, "`@stored` keeps a query `def`, one whose first step is `from`")
                .with_help("a `def` of steps is put in place where it is called, so there is nothing to store");
            self.report(problem);
            return None;
        }
        if self.dialect == Dialect::Sqlite {
            let problem = self
                .error(name.span, "SQLite can't keep a query in the database")
                .with_help("compile it with `--query` and run it from your program, which keeps it instead");
            self.report(problem);
            return None;
        }
        let (bindings, args) = self.within(module, |compiler| compiler.open_args(def))?;
        let function = def.name.text.clone();
        if def.changes() {
            if self.dialect != Dialect::Postgres {
                let database = if self.dialect == Dialect::DuckDb {
                    "DuckDB"
                } else {
                    "ClickHouse"
                };
                let problem = self
                    .error(
                        name.span,
                        format!("{database} can't keep a query that changes rows"),
                    )
                    .with_help("PostgreSQL keeps one, as a procedure");
                self.report(problem);
                return None;
            }
            let params = self.postgres_params(def, &args)?;
            let statement =
                self.within(module, |compiler| compiler.statement(&def.body, &bindings))?;
            let body =
                written::write_inline(&statement, self.dialect, Args::Qualified(function.clone()));
            return Some(vec![Statement::Written(format!(
                "CREATE OR REPLACE PROCEDURE {}({params})\nLANGUAGE sql\nBEGIN ATOMIC\n{body};\nEND",
                written::ident(&function)
            ))]);
        }
        let rel = self.within(module, |compiler| compiler.query(&def.body, &bindings))?;
        let whole_table = match &rel.select.from {
            Some(Source {
                kind: SourceKind::Table(path),
                ..
            }) if rel.frame.star
                && rel.select.joins.is_empty()
                && !rel.grouped
                && rel.frame.cols.iter().all(|col| col.base) =>
            {
                Some(path.clone())
            }
            _ => None,
        };
        let columns: Vec<(String, Type)> = rel
            .frame
            .visible()
            .map(|col| (col.name.clone(), col.value.ty))
            .collect();
        let select = self.finish(rel);
        match self.dialect {
            Dialect::Postgres => {
                let params = self.postgres_params(def, &args)?;
                let returns = match whole_table {
                    Some(table) => format!("SETOF {}", written::path(&table)),
                    None => {
                        let mut typed = Vec::new();
                        for (column, ty) in &columns {
                            let Some(sql) = postgres_type(*ty) else {
                                let problem = self
                                    .error(name.span, format!("ButtercupQL does not know the type of `{column}`, which PostgreSQL needs to keep the query"))
                                    .with_help("give the schema of the tables the query reads");
                                self.report(problem);
                                return None;
                            };
                            typed.push(format!("{} {sql}", written::ident(column)));
                        }
                        format!("TABLE ({})", typed.join(", "))
                    }
                };
                let body = written::write_inline(
                    &Statement::Select(select),
                    self.dialect,
                    Args::Qualified(function.clone()),
                );
                Some(vec![Statement::Written(format!(
                    "CREATE OR REPLACE FUNCTION {}({params})\nRETURNS {returns}\nLANGUAGE sql STABLE\nAS $bcql$\n{body}\n$bcql$",
                    written::ident(&function)
                ))])
            }
            Dialect::DuckDb => {
                let params: Vec<String> = args
                    .iter()
                    .map(|arg| match &arg.default {
                        Some(value) => {
                            format!("{} := {}", written::ident(&arg.name), literal_sql(value))
                        }
                        None => written::ident(&arg.name),
                    })
                    .collect();
                let body =
                    written::write_inline(&Statement::Select(select), self.dialect, Args::Bare);
                Some(vec![Statement::Written(format!(
                    "CREATE OR REPLACE MACRO {}({}) AS TABLE\n{body}",
                    written::ident(&function),
                    params.join(", ")
                ))])
            }
            _ => {
                if let Some(param) = def.params.iter().find(|param| param.default.is_some()) {
                    let warning = Diagnostic::warning(
                        param.span,
                        "ClickHouse's stored queries take no defaults",
                    )
                    .with_label("this argument has to be given each time");
                    self.report(warning);
                }
                let body = written::write_inline(
                    &Statement::Select(select),
                    self.dialect,
                    Args::Placeholders,
                );
                Some(vec![Statement::Written(format!(
                    "CREATE OR REPLACE VIEW {} AS\n{body}",
                    written::ident(&function)
                ))])
            }
        }
    }

    /// A stored def's parameters on PostgreSQL, each with its type and its default.
    fn postgres_params(&mut self, def: &'a ast::Def, args: &[crate::Arg]) -> Option<String> {
        let mut params = Vec::new();
        for (param, arg) in def.params.iter().zip(args) {
            let ty = match &param.annotation {
                Some(annotation) => column_type(annotation, Dialect::Postgres).map(|(sql, _)| sql),
                None => None,
            };
            let Some(ty) = ty else {
                let problem = self
                    .error(
                        param.span,
                        "on PostgreSQL, a stored def's parameters say their types",
                    )
                    .with_help(format!("for example: `{}: str`", param.name.text));
                self.report(problem);
                return None;
            };
            let default = arg.default.as_ref().map_or_else(String::new, |value| {
                format!(" DEFAULT {}", literal_sql(value))
            });
            params.push(format!(
                "{} {ty}{default}",
                written::ident(&param.name.text)
            ));
        }
        Some(params.join(", "))
    }
}

/// A dotted name as an expression, as a source names a table.
fn path_expr(path: &ast::Path) -> ast::Expr {
    let mut parts = path.parts.iter();
    let first = parts.next().expect("a path has a name");
    let mut expr = ast::Expr {
        kind: ExprKind::Name(first.text.clone()),
        span: first.span,
    };
    for part in parts {
        expr = ast::Expr {
            span: expr.span.to(part.span),
            kind: ExprKind::Attribute(Box::new(expr), part.clone()),
        };
    }
    expr
}
