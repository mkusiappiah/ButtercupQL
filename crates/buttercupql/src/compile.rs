//! Compiles a ButtercupQL query to SQL.
//!
//! Each step is folded into the `SELECT` being built, in the clause that does its work, and a subquery is started
//! only where the meaning needs one: a `where` after a `take`, or a second grouping. Names are resolved as they are
//! met, so a mistake is reported at its place with the name that was most likely meant.

use std::collections::HashMap;

use crate::ast::{self, BinaryOp, CompareOp, ExprKind, FPart, StepKind, UnaryOp};
use crate::diagnostic::Diagnostic;
use crate::dialect::Dialect;
use crate::schema::{Schema, Type};
use crate::span::Span;
use crate::sql::{
    Binary, Expr as Sql, Item, Join, Order, Select, Source, SourceKind, Unary, Value,
};
use crate::suggest;

#[cfg(test)]
mod tests;

/// A value of a query, compiled: its SQL and what is known of it.
#[derive(Debug, Clone)]
struct Typed {
    sql: Sql,
    ty: Type,
    nullable: bool,
    /// The value holds a total, such as `count()`.
    total: bool,
    /// The value reads a column.
    reads: bool,
    /// For a length of time: its parts, such as 7 days and 6 hours. SQLite has no type for one, so a length of time
    /// is kept in parts until it meets the moment it moves.
    parts: Vec<(Sql, Unit)>,
}

impl Typed {
    fn new(sql: Sql, ty: Type) -> Typed {
        Typed {
            sql,
            ty,
            nullable: false,
            total: false,
            reads: false,
            parts: Vec::new(),
        }
    }

    fn nullable(mut self, nullable: bool) -> Typed {
        self.nullable = nullable;
        self
    }

    /// A value made from others: missing if any of them is, a total if any of them is.
    fn from(sql: Sql, ty: Type, inputs: &[&Typed]) -> Typed {
        Typed {
            sql,
            ty,
            nullable: inputs.iter().any(|input| input.nullable),
            total: inputs.iter().any(|input| input.total),
            reads: inputs.iter().any(|input| input.reads),
            parts: Vec::new(),
        }
    }
}

/// A unit of time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Days,
    Hours,
    Minutes,
    Seconds,
}

impl Unit {
    fn name(self) -> &'static str {
        match self {
            Unit::Days => "days",
            Unit::Hours => "hours",
            Unit::Minutes => "minutes",
            Unit::Seconds => "seconds",
        }
    }

    fn one(self) -> &'static str {
        match self {
            Unit::Days => "1 day",
            Unit::Hours => "1 hour",
            Unit::Minutes => "1 minute",
            Unit::Seconds => "1 second",
        }
    }
}

/// A column the query can use at a point.
#[derive(Debug, Clone)]
struct Col {
    name: String,
    value: Typed,
    /// The table the column is read from, by the name it is known by in the query.
    table: Option<String>,
    /// The column is one of a table's own, so `*` includes it.
    base: bool,
}

/// A table read by the `SELECT` being built.
#[derive(Debug, Clone)]
struct Scope {
    alias: String,
    /// Whether all its columns are known, from the schema or from the query.
    known: bool,
}

/// What the query has at a point: its columns, and the tables they come from.
#[derive(Debug, Clone, Default)]
struct Frame {
    cols: Vec<Col>,
    tables: Vec<Scope>,
    /// The output is `*` and the columns it does not include, rather than exactly `cols`.
    star: bool,
}

impl Frame {
    /// The tables whose columns are not all known: a name not among the columns may be one of theirs.
    fn open(&self) -> Vec<&Scope> {
        self.tables.iter().filter(|scope| !scope.known).collect()
    }
}

/// The `SELECT` being built, and what its steps have done so far.
#[derive(Debug, Clone)]
struct Rel {
    select: Select,
    frame: Frame,
    grouped: bool,
    distinct: bool,
    limited: bool,
    projected: bool,
    /// Where the current order was asked for, to warn when a grouping undoes it.
    sorted_at: Option<Span>,
}

/// The names a `def`'s body can use: its parameters.
type Bindings = HashMap<String, Typed>;

/// A call's arguments, compiled: those given by place, and those given by name with where they are written.
type Arguments = (Vec<Typed>, HashMap<String, (Typed, Span)>);

/// Where an expression is compiled.
#[derive(Clone, Copy)]
struct Cx<'f> {
    frame: &'f Frame,
    bindings: &'f Bindings,
    /// Totals such as `count()` can be used: in the block of a `group by` or a `summarize`.
    totals: bool,
    /// The expression is a total's argument, where another total can't be.
    in_total: bool,
}

/// The functions ButtercupQL knows, for suggestions.
const FUNCTIONS: [&str; 22] = [
    "count",
    "count_distinct",
    "sum",
    "mean",
    "avg",
    "min",
    "max",
    "any",
    "all",
    "len",
    "abs",
    "round",
    "str",
    "int",
    "float",
    "now",
    "today",
    "date",
    "datetime",
    "days",
    "hours",
    "literal",
];

pub(crate) struct Compiler<'a> {
    dialect: Dialect,
    schema: Option<&'a Schema>,
    file: &'a ast::File,
    lets: HashMap<&'a str, &'a ast::Let>,
    defs: HashMap<&'a str, &'a ast::Def>,
    values: HashMap<String, Typed>,
    /// The names being compiled, to find one that uses itself.
    stack: Vec<String>,
    aliases: usize,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// A step has failed, so a name it should have made is not reported as missing.
    poisoned: bool,
}

impl<'a> Compiler<'a> {
    pub(crate) fn new(
        file: &'a ast::File,
        dialect: Dialect,
        schema: Option<&'a Schema>,
    ) -> Compiler<'a> {
        let mut lets = HashMap::new();
        let mut defs = HashMap::new();
        for item in &file.items {
            match item {
                ast::Item::Let(value) => {
                    lets.insert(value.name.text.as_str(), value);
                }
                ast::Item::Def(def) => {
                    defs.insert(def.name.text.as_str(), def);
                }
                ast::Item::Import(_) => {}
            }
        }
        Compiler {
            dialect,
            schema,
            file,
            lets,
            defs,
            values: HashMap::new(),
            stack: Vec::new(),
            aliases: 0,
            diagnostics: Vec::new(),
            poisoned: false,
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(span, message)
    }

    fn report(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// The file's query, compiled.
    pub(crate) fn main(&mut self) -> Option<Select> {
        for item in &self.file.items {
            if let ast::Item::Import(import) = item {
                let problem = self
                    .error(import.span, "imports are not supported yet")
                    .with_help("they come in the next version; until then, put what the query uses in its own file");
                self.report(problem);
            }
        }
        let Some(query) = &self.file.query else {
            let problem = Diagnostic::error(Span::default(), "this file has no query to compile")
                .with_help("a query starts with `from` and a table, after the file's definitions");
            self.report(problem);
            return None;
        };
        let rel = self.query(&query.steps, &Bindings::new())?;
        Some(self.output(rel))
    }

    // ---- Queries and steps

    fn query(&mut self, steps: &'a [ast::Step], bindings: &Bindings) -> Option<Rel> {
        let (first, rest) = steps.split_first()?;
        let StepKind::From(source) = &first.kind else {
            return None;
        };
        let mut rel = self.from(source, bindings)?;
        for step in rest {
            rel = self.step(rel, step, bindings);
        }
        Some(rel)
    }

    fn step(&mut self, rel: Rel, step: &'a ast::Step, bindings: &Bindings) -> Rel {
        match &step.kind {
            // The parser has reported a second `from`.
            StepKind::From(_) | StepKind::Pass => rel,
            StepKind::Where(condition) => self.filter(rel, condition, bindings),
            StepKind::Column { name, value } => self.column(rel, name, value, bindings),
            StepKind::Select(fields) => self.select(rel, fields, bindings),
            StepKind::Del(columns) => self.del(rel, columns, bindings),
            StepKind::Sort { keys, reverse } => {
                self.sort(rel, keys, reverse.as_ref(), step.span, bindings)
            }
            StepKind::Take(count) => self.take(rel, count, false, bindings),
            StepKind::Skip(count) => self.take(rel, count, true, bindings),
            StepKind::Distinct => self.distinct(rel),
            StepKind::Group { by, totals } => self.group(rel, Some(by), totals, bindings),
            StepKind::Summarize(totals) => self.group(rel, None, totals, bindings),
            StepKind::Join { left, source, on } => self.join(rel, *left, source, on, bindings),
            StepKind::Call(call) => self.call_steps(rel, call, bindings),
        }
    }

    fn alias(&mut self) -> String {
        self.aliases += 1;
        format!("_q{}", self.aliases)
    }

    /// The `SELECT` with its columns written out.
    fn output(&self, rel: Rel) -> Select {
        let mut select = rel.select;
        select.columns = if rel.frame.star {
            let extra: Vec<Item> = rel
                .frame
                .cols
                .iter()
                .filter(|col| !col.base)
                .map(|col| Item::Expr {
                    expr: col.value.sql.clone(),
                    alias: Some(col.name.clone()),
                })
                .collect();
            if extra.is_empty() {
                Vec::new()
            } else {
                std::iter::once(Item::Star).chain(extra).collect()
            }
        } else {
            rel.frame
                .cols
                .iter()
                .map(|col| Item::Expr {
                    expr: col.value.sql.clone(),
                    alias: Some(col.name.clone()),
                })
                .collect()
        };
        // A sort by a value that comes out as a column is written with the column's name: `ORDER BY money DESC`.
        for order in &mut select.order {
            if matches!(order.expr, Sql::Column { .. }) {
                continue;
            }
            let named = select.columns.iter().find_map(|item| match item {
                Item::Expr {
                    expr,
                    alias: Some(alias),
                } if *expr == order.expr => Some(alias.clone()),
                _ => None,
            });
            if let Some(alias) = named {
                order.expr = Sql::column(None, &alias);
            }
        }
        select
    }

    /// Starts a new `SELECT` that reads the one built so far, for a step that must come after all of its clauses.
    fn wrap(&mut self, rel: Rel) -> Rel {
        let alias = self.alias();
        let known = rel.frame.open().is_empty() || !rel.frame.star;
        let cols = rel.frame.cols.clone();
        let outer_cols: Vec<Col> = cols
            .iter()
            .map(|col| Col {
                name: col.name.clone(),
                value: Typed {
                    sql: Sql::column(Some(&alias), &col.name),
                    ty: col.value.ty,
                    nullable: col.value.nullable,
                    total: false,
                    reads: true,
                    parts: Vec::new(),
                },
                table: Some(alias.clone()),
                base: true,
            })
            .collect();
        // The order is kept: inside, where a limit needs it, and outside for the rows that come out.
        let mut order = Vec::new();
        for key in &rel.select.order {
            let name = match cols.iter().find(|col| col.value.sql == key.expr) {
                Some(col) => Some(col.name.clone()),
                // A column of a table whose columns are not all known comes out through `*`, under its own name.
                None => match &key.expr {
                    Sql::Column { name, .. } if rel.frame.star => Some(name.clone()),
                    _ => None,
                },
            };
            if let Some(name) = name {
                order.push(Order {
                    expr: Sql::column(Some(&alias), &name),
                    descending: key.descending,
                    nulls_last: key.nulls_last,
                });
            }
        }
        let limited = rel.limited;
        let sorted_at = rel.sorted_at;
        let mut inner = self.output(rel);
        if !limited {
            inner.order.clear();
        }
        Rel {
            select: Select {
                from: Some(Source {
                    kind: SourceKind::Query(Box::new(inner)),
                    alias: Some(alias.clone()),
                }),
                order,
                ..Select::default()
            },
            frame: Frame {
                cols: outer_cols,
                tables: vec![Scope { alias, known }],
                star: true,
            },
            grouped: false,
            distinct: false,
            limited: false,
            projected: false,
            sorted_at,
        }
    }

    // ---- Sources

    /// The relation a `from` starts with: a table, a named query, a query `def`, or a query in brackets.
    fn from(&mut self, source: &'a ast::Source, bindings: &Bindings) -> Option<Rel> {
        if !self.is_query(&source.value, bindings) {
            return Some(self.table(source));
        }
        let rel = self.relation(&source.value, bindings)?;
        match &source.alias {
            Some(alias) => Some(self.rename(rel, &alias.text)),
            None => Some(rel),
        }
    }

    /// Whether a source is a query rather than a table: a named query, a query `def`, or a query in brackets.
    fn is_query(&self, value: &ast::Expr, bindings: &Bindings) -> bool {
        match &value.kind {
            ExprKind::Query(_) | ExprKind::Call(..) => true,
            ExprKind::Name(name) => {
                !bindings.contains_key(name)
                    && (self.named_query(name).is_some() || self.defs.contains_key(name.as_str()))
            }
            _ => false,
        }
    }

    /// The relation of a query source.
    fn relation(&mut self, value: &'a ast::Expr, bindings: &Bindings) -> Option<Rel> {
        match &value.kind {
            ExprKind::Query(query) => self.query(&query.steps, bindings),
            ExprKind::Name(name) if self.named_query(name).is_some() => {
                let query = self.named_query(name)?;
                self.enter(name, value.span)?;
                let rel = self.query(&query.steps, &Bindings::new());
                self.stack.pop();
                rel
            }
            ExprKind::Name(name) => self.query_def(name, &[], value.span, bindings),
            ExprKind::Call(function, args) => {
                let ExprKind::Name(name) = &function.kind else {
                    let problem = self.error(function.span, "imports are not supported yet");
                    self.report(problem);
                    return None;
                };
                if !self.defs.contains_key(name.as_str()) {
                    let names: Vec<&str> = self.defs.keys().copied().collect();
                    let mut problem = self.error(
                        function.span,
                        format!("there is no query `def` named `{name}`"),
                    );
                    if let Some(best) = suggest::closest(name, names) {
                        problem = problem.with_label(format!("did you mean `{best}`?"));
                    }
                    self.report(problem);
                    return None;
                }
                self.query_def(name, args, value.span, bindings)
            }
            _ => None,
        }
    }

    /// The relation, read under a name of its own: `from (from cases) as c`.
    fn rename(&mut self, rel: Rel, alias: &str) -> Rel {
        let mut rel = self.wrap(rel);
        let old = rel.frame.tables[0].alias.clone();
        let SourceKind::Query(_) = &rel
            .select
            .from
            .as_ref()
            .expect("a wrapped query reads one")
            .kind
        else {
            return rel;
        };
        if let Some(from) = rel.select.from.as_mut() {
            from.alias = Some(alias.to_string());
        }
        rel.frame.tables[0].alias = alias.to_string();
        for col in &mut rel.frame.cols {
            col.table = Some(alias.to_string());
            col.value.sql = Sql::column(Some(alias), &col.name);
        }
        for order in &mut rel.select.order {
            if let Sql::Column { table, .. } = &mut order.expr {
                if table.as_deref() == Some(old.as_str()) {
                    *table = Some(alias.to_string());
                }
            }
        }
        rel
    }

    fn named_query(&self, name: &str) -> Option<&'a ast::Query> {
        match &self.lets.get(name)?.value.kind {
            ExprKind::Query(query) => Some(query),
            _ => None,
        }
    }

    /// Marks a named query or a `def` as being compiled, refusing one that uses itself.
    fn enter(&mut self, name: &str, span: Span) -> Option<()> {
        if self.stack.iter().any(|entered| entered == name) {
            let problem = self
                .error(span, format!("`{name}` uses itself"))
                .with_help("a named query or a `def` can't use itself, directly or through others");
            self.report(problem);
            return None;
        }
        self.stack.push(name.to_string());
        Some(())
    }

    /// A table: what the schema knows of it, or nothing.
    fn table(&mut self, source: &ast::Source) -> Rel {
        let path = path_of(&source.value);
        let written = path.join(".");
        let (path, cols, known) = match self.schema {
            Some(schema) => match schema.table(&written) {
                Some(table) => {
                    // The column's table and SQL are set below, once the table's name in the query is known.
                    let cols = table
                        .columns
                        .iter()
                        .map(|column| Col {
                            name: column.name.clone(),
                            value: Typed {
                                sql: Sql::Keyword("NULL"),
                                ty: column.ty,
                                nullable: column.nullable,
                                total: false,
                                reads: true,
                                parts: Vec::new(),
                            },
                            table: None,
                            base: true,
                        })
                        .collect();
                    let path = table.name.split('.').map(str::to_string).collect();
                    (path, cols, true)
                }
                None => {
                    let names: Vec<String> = schema
                        .tables()
                        .iter()
                        .map(|table| table.name.clone())
                        .collect();
                    let mut problem =
                        self.error(source.value.span, format!("there is no table `{written}`"));
                    if let Some(best) = suggest::closest(&written, names.iter().map(String::as_str))
                    {
                        problem = problem.with_label(format!("did you mean `{best}`?"));
                    }
                    self.report(problem);
                    self.poisoned = true;
                    (path, Vec::new(), false)
                }
            },
            None => (path, Vec::new(), false),
        };
        let alias = source.alias.as_ref().map_or_else(
            || path.last().cloned().unwrap_or_default(),
            |alias| alias.text.clone(),
        );
        let cols = cols
            .into_iter()
            .map(|mut col: Col| {
                col.table = Some(alias.clone());
                col.value.sql = Sql::column(Some(&alias), &col.name);
                col
            })
            .collect();
        Rel {
            select: Select {
                from: Some(Source {
                    kind: SourceKind::Table(path),
                    alias: source.alias.as_ref().map(|alias| alias.text.clone()),
                }),
                ..Select::default()
            },
            frame: Frame {
                cols,
                tables: vec![Scope { alias, known }],
                star: true,
            },
            grouped: false,
            distinct: false,
            limited: false,
            projected: false,
            sorted_at: None,
        }
    }

    // ---- The steps

    fn filter(&mut self, mut rel: Rel, condition: &'a ast::Expr, bindings: &Bindings) -> Rel {
        if rel.limited || (rel.grouped && rel.select.group.is_empty()) {
            rel = self.wrap(rel);
        }
        let cx = Cx {
            frame: &rel.frame,
            bindings,
            totals: false,
            in_total: false,
        };
        let Some(value) = self.expr(condition, cx) else {
            self.poisoned = true;
            return rel;
        };
        let sql = self.truth(value);
        if rel.grouped {
            rel.select.having.push(sql);
        } else {
            rel.select.filter.push(sql);
        }
        rel
    }

    /// `name = value`: a new column, or a new value for one.
    fn column(
        &mut self,
        mut rel: Rel,
        name: &'a ast::Name,
        value: &'a ast::Expr,
        bindings: &Bindings,
    ) -> Rel {
        if rel.distinct {
            rel = self.wrap(rel);
        }
        let cx = Cx {
            frame: &rel.frame,
            bindings,
            totals: false,
            in_total: false,
        };
        let Some(typed) = self.expr(value, cx) else {
            self.poisoned = true;
            rel.frame.cols.push(Col {
                name: name.text.clone(),
                value: Typed::new(Sql::Keyword("NULL"), Type::Unknown).nullable(true),
                table: None,
                base: false,
            });
            return rel;
        };
        if !self.storable(&typed, value.span) {
            return rel;
        }
        let col = Col {
            name: name.text.clone(),
            value: typed,
            table: None,
            base: false,
        };
        match rel.frame.cols.iter().position(|old| old.name == name.text) {
            Some(at) => {
                if rel.frame.cols[at].base && rel.frame.star {
                    if !rel.frame.open().is_empty() {
                        self.needs_columns(
                            name.span,
                            &format!("a new value for the column `{}`", name.text),
                        );
                        return rel;
                    }
                    rel.frame.star = false;
                }
                rel.frame.cols[at] = col;
            }
            None => {
                if mentions(value, &name.text) && !rel.frame.open().is_empty() {
                    // `risk = risk + 1`: the table has the column, but its other columns are not known.
                    self.needs_columns(
                        name.span,
                        &format!("a new value for the column `{}`", name.text),
                    );
                    return rel;
                }
                rel.frame.cols.push(col);
            }
        }
        rel
    }

    fn needs_columns(&mut self, span: Span, what: &str) {
        let problem = self
            .error(span, format!("{what} needs the table's columns"))
            .with_help(
                "give a schema, so that ButtercupQL knows the table's other columns; or give the new value a name of its own",
            );
        self.report(problem);
        self.poisoned = true;
    }

    /// Whether a value can be a column's: a length of time can't be, on SQLite.
    fn storable(&mut self, typed: &Typed, span: Span) -> bool {
        if typed.ty == Type::Duration && self.dialect == Dialect::Sqlite {
            let problem = self
                .error(span, "SQLite has no type for a length of time")
                .with_help("add it to a date or a time instead, as in `raised + days(7)`");
            self.report(problem);
            self.poisoned = true;
            return false;
        }
        true
    }

    fn select(&mut self, mut rel: Rel, fields: &'a [ast::Field], bindings: &Bindings) -> Rel {
        if rel.distinct {
            rel = self.wrap(rel);
        }
        let mut cols: Vec<Col> = Vec::new();
        for field in fields {
            let cx = Cx {
                frame: &rel.frame,
                bindings,
                totals: false,
                in_total: false,
            };
            let Some(value) = self.expr(&field.value, cx) else {
                self.poisoned = true;
                continue;
            };
            let Some(name) = self.field_name(field, "column") else {
                continue;
            };
            if !self.storable(&value, field.value.span) {
                continue;
            }
            if cols.iter().any(|col| col.name == name) {
                let span = field
                    .name
                    .as_ref()
                    .map_or(field.value.span, |name| name.span);
                let problem = self.error(span, format!("there are two columns named `{name}`"));
                self.report(problem);
                continue;
            }
            let table = match &value.sql {
                Sql::Column { table, .. } => table.clone(),
                _ => None,
            };
            cols.push(Col {
                name,
                value,
                table,
                base: false,
            });
        }
        rel.frame.cols = cols;
        rel.frame.star = false;
        for scope in &mut rel.frame.tables {
            scope.known = true;
        }
        rel.projected = true;
        rel
    }

    /// The name of a column of a `select` or a `group by`: the one given with `=`, or the column's own.
    fn field_name(&mut self, field: &ast::Field, what: &str) -> Option<String> {
        if let Some(name) = &field.name {
            return Some(name.text.clone());
        }
        match &field.value.kind {
            ExprKind::Name(name) => Some(name.clone()),
            ExprKind::Attribute(_, name) => Some(name.text.clone()),
            _ => {
                let written = "name = ...";
                let problem = self
                    .error(field.value.span, format!("this {what} needs a name"))
                    .with_label(format!("give it a name, as in `{written}`"));
                self.report(problem);
                None
            }
        }
    }

    fn del(&mut self, mut rel: Rel, columns: &'a [ast::Expr], bindings: &Bindings) -> Rel {
        if rel.distinct {
            rel = self.wrap(rel);
        }
        if rel.frame.star && !rel.frame.open().is_empty() {
            let span = columns.first().map_or(Span::default(), |first| {
                first.span.to(columns[columns.len() - 1].span)
            });
            let problem = self
                .error(span, "`del` needs the table's columns")
                .with_help("give a schema, so that ButtercupQL knows the columns to keep; or name them with `select`");
            self.report(problem);
            self.poisoned = true;
            return rel;
        }
        for column in columns {
            let cx = Cx {
                frame: &rel.frame,
                bindings,
                totals: false,
                in_total: false,
            };
            let Some(value) = self.expr(column, cx) else {
                continue;
            };
            let before = rel.frame.cols.len();
            rel.frame.cols.retain(|col| col.value.sql != value.sql);
            if rel.frame.cols.len() == before {
                let problem = self.error(column.span, "this is not one of the query's columns");
                self.report(problem);
            }
        }
        rel.frame.star = false;
        for scope in &mut rel.frame.tables {
            scope.known = true;
        }
        rel.projected = true;
        rel
    }

    fn sort(
        &mut self,
        mut rel: Rel,
        keys: &'a [ast::SortKey],
        reverse: Option<&'a ast::Expr>,
        span: Span,
        bindings: &Bindings,
    ) -> Rel {
        if rel.limited {
            rel = self.wrap(rel);
        }
        let reverse = match reverse.map(|value| &value.kind) {
            None | Some(ExprKind::Bool(false)) => false,
            Some(ExprKind::Bool(true)) => true,
            Some(_) => {
                let span = reverse.map_or(span, |value| value.span);
                let problem = self.error(span, "`reverse=` takes `True` or `False`");
                self.report(problem);
                false
            }
        };
        let mut order = Vec::new();
        for key in keys {
            let cx = Cx {
                frame: &rel.frame,
                bindings,
                totals: false,
                in_total: false,
            };
            let Some(value) = self.expr(&key.value, cx) else {
                self.poisoned = true;
                continue;
            };
            let descending = key.descending != reverse;
            order.push(Order {
                nulls_last: self.nulls_last(descending, &value),
                expr: value.sql,
                descending,
            });
        }
        // Python's sort is stable: rows that tie keep the order they had, so the old order follows the new one.
        let old = std::mem::take(&mut rel.select.order);
        for key in old {
            if !order.iter().any(|new| new.expr == key.expr) {
                order.push(key);
            }
        }
        if rel.distinct
            && order
                .iter()
                .any(|key| !rel.frame.cols.iter().any(|col| col.value.sql == key.expr))
        {
            // With `DISTINCT`, PostgreSQL sorts only by the columns that come out.
            let problem = self
                .error(
                    span,
                    "after `distinct`, a sort can only use the query's columns",
                )
                .with_help("name the value as a column before `distinct`, then sort by it");
            self.report(problem);
            return rel;
        }
        rel.select.order = order;
        rel.sorted_at = Some(span);
        rel
    }

    /// Whether `NULLS LAST` must be written: missing values sort last whichever way, on every database.
    fn nulls_last(&self, descending: bool, value: &Typed) -> bool {
        value.nullable
            && match self.dialect {
                Dialect::Postgres => descending,
                _ => !descending,
            }
    }

    fn take(&mut self, mut rel: Rel, count: &'a ast::Expr, skip: bool, bindings: &Bindings) -> Rel {
        let Some(count) = self.whole_number(count, bindings) else {
            return rel;
        };
        if skip {
            rel.select.offset += count;
            if let Some(limit) = rel.select.limit {
                rel.select.limit = Some(limit.saturating_sub(count));
            }
        } else {
            rel.select.limit = Some(rel.select.limit.map_or(count, |limit| limit.min(count)));
        }
        rel.limited = true;
        rel
    }

    /// A count of rows, for `take` and `skip`: a whole number, written or named.
    fn whole_number(&mut self, value: &'a ast::Expr, bindings: &Bindings) -> Option<u64> {
        let frame = Frame::default();
        let cx = Cx {
            frame: &frame,
            bindings,
            totals: false,
            in_total: false,
        };
        let poisoned = std::mem::replace(&mut self.poisoned, false);
        let typed = self.expr(value, cx);
        self.poisoned = poisoned;
        if let Some(Typed {
            sql: Sql::Number(digits),
            ty: Type::Int,
            ..
        }) = &typed
        {
            if let Ok(count) = digits.parse::<u64>() {
                return Some(count);
            }
        }
        if typed.is_some() {
            let problem = self
                .error(value.span, "expected a whole number of rows, 0 or more")
                .with_help("for example: `take 20`");
            self.report(problem);
        }
        None
    }

    fn distinct(&mut self, mut rel: Rel) -> Rel {
        if rel.limited {
            rel = self.wrap(rel);
        }
        if rel
            .select
            .order
            .iter()
            .any(|key| !rel.frame.cols.iter().any(|col| col.value.sql == key.expr))
            && !rel.frame.star
        {
            rel = self.wrap(rel);
        }
        rel.select.distinct = true;
        rel.distinct = true;
        rel
    }

    fn group(
        &mut self,
        mut rel: Rel,
        by: Option<&'a [ast::Field]>,
        totals: &'a [ast::Field],
        bindings: &Bindings,
    ) -> Rel {
        if rel.grouped || rel.distinct || rel.limited {
            rel = self.wrap(rel);
        }
        if let Some(sorted_at) = rel.sorted_at.take() {
            let warning = Diagnostic::warning(sorted_at, "this sort has no effect")
                .with_label("grouping does not keep the rows' order")
                .with_help("to order the groups, sort after the grouping");
            self.report(warning);
        }
        rel.select.order.clear();

        let mut cols: Vec<Col> = Vec::new();
        for field in by.unwrap_or(&[]) {
            let cx = Cx {
                frame: &rel.frame,
                bindings,
                totals: false,
                in_total: false,
            };
            let Some(value) = self.expr(&field.value, cx) else {
                self.poisoned = true;
                continue;
            };
            let Some(name) = self.field_name(field, "group") else {
                continue;
            };
            let table = match &value.sql {
                Sql::Column { table, .. } => table.clone(),
                _ => None,
            };
            rel.select.group.push(value.sql.clone());
            cols.push(Col {
                name,
                value,
                table,
                base: false,
            });
        }
        let keys = cols.len();
        for total in totals {
            // A total can use the totals before it: `rate = big / n`.
            let mut frame = rel.frame.clone();
            frame.cols.extend(cols[keys..].iter().cloned());
            let cx = Cx {
                frame: &frame,
                bindings,
                totals: true,
                in_total: false,
            };
            let Some(value) = self.expr(&total.value, cx) else {
                self.poisoned = true;
                continue;
            };
            let name = total
                .name
                .as_ref()
                .map_or_else(String::new, |name| name.text.clone());
            if !value.total && value.reads {
                let problem = self
                    .error(
                        total.value.span,
                        "a total is worked out from the rows of each group",
                    )
                    .with_label("this is a value of each row, not of the group")
                    .with_help(
                        "use a total such as `count()`, `sum(x)`, `mean(x)`, `min(x)` or `max(x)`",
                    );
                self.report(problem);
                continue;
            }
            if !self.storable(&value, total.value.span) {
                continue;
            }
            cols.push(Col {
                name,
                value,
                table: None,
                base: false,
            });
        }
        // The keys are values of the group, not of its rows: they are never totals themselves.
        rel.frame = Frame {
            cols,
            tables: rel
                .frame
                .tables
                .into_iter()
                .map(|scope| Scope {
                    known: true,
                    ..scope
                })
                .collect(),
            star: false,
        };
        rel.grouped = true;
        rel.projected = true;
        rel
    }

    fn join(
        &mut self,
        mut rel: Rel,
        left: bool,
        source: &'a ast::Source,
        on: &'a ast::Expr,
        bindings: &Bindings,
    ) -> Rel {
        if rel.grouped || rel.distinct || rel.limited || rel.projected {
            rel = self.wrap(rel);
        }
        let Some((joined, scope, cols)) = self.joined(source, bindings) else {
            self.poisoned = true;
            return rel;
        };
        if rel
            .frame
            .tables
            .iter()
            .any(|other| other.alias == scope.alias)
        {
            let span = source
                .alias
                .as_ref()
                .map_or(source.value.span, |alias| alias.span);
            let problem = self
                .error(
                    span,
                    format!("two tables are called `{}` here", scope.alias),
                )
                .with_help("give one of them another name with `as`, as in `join accounts as a`");
            self.report(problem);
            self.poisoned = true;
            return rel;
        }
        rel.frame.tables.push(scope);
        rel.frame.cols.extend(cols.into_iter().map(|mut col| {
            // After a `left join`, the joined table's columns are missing on rows that had no match.
            col.value.nullable |= left;
            col
        }));
        let cx = Cx {
            frame: &rel.frame,
            bindings,
            totals: false,
            in_total: false,
        };
        let Some(condition) = self.expr(on, cx) else {
            self.poisoned = true;
            return rel;
        };
        let condition = self.truth(condition);
        rel.select.joins.push(Join {
            left,
            source: joined,
            on: condition,
        });
        rel
    }

    /// What a `join` reads: a table, or a query, which is read as a query in brackets under its name.
    fn joined(
        &mut self,
        source: &'a ast::Source,
        bindings: &Bindings,
    ) -> Option<(Source, Scope, Vec<Col>)> {
        if !self.is_query(&source.value, bindings) {
            let rel = self.table(source);
            let scope = rel.frame.tables[0].clone();
            return Some((
                rel.select.from.expect("a table is read"),
                scope,
                rel.frame.cols,
            ));
        }
        let alias = match (&source.alias, &source.value.kind) {
            (Some(alias), _) => alias.text.clone(),
            (None, ExprKind::Name(name)) => name.clone(),
            (None, ExprKind::Call(function, _)) => path_of(function).join("_"),
            (None, _) => {
                let problem = self
                    .error(
                        source.value.span,
                        "a query in brackets that is joined needs a name",
                    )
                    .with_help("name it with `as`, as in `join (from accounts) as a on ...`");
                self.report(problem);
                return None;
            }
        };
        let rel = self.relation(&source.value, bindings)?;
        let known = rel.frame.open().is_empty() || !rel.frame.star;
        let cols: Vec<Col> = rel
            .frame
            .cols
            .iter()
            .map(|col| Col {
                name: col.name.clone(),
                value: Typed {
                    sql: Sql::column(Some(&alias), &col.name),
                    ty: col.value.ty,
                    nullable: col.value.nullable,
                    total: false,
                    reads: true,
                    parts: Vec::new(),
                },
                table: Some(alias.clone()),
                base: true,
            })
            .collect();
        let select = self.output(rel);
        let joined = Source {
            kind: SourceKind::Query(Box::new(select)),
            alias: Some(alias.clone()),
        };
        Some((joined, Scope { alias, known }, cols))
    }

    // ---- Defs

    /// `recent(30)`: the steps of a `def`, put in place.
    fn call_steps(&mut self, mut rel: Rel, call: &'a ast::Expr, bindings: &Bindings) -> Rel {
        let ExprKind::Call(function, args) = &call.kind else {
            return rel;
        };
        let ExprKind::Name(name) = &function.kind else {
            let problem = self.error(function.span, "imports are not supported yet");
            self.report(problem);
            return rel;
        };
        let Some(def) = self.defs.get(name.as_str()).copied() else {
            let names: Vec<&str> = self.defs.keys().copied().collect();
            let mut problem =
                self.error(function.span, format!("there is no `def` named `{name}`"));
            if let Some(best) = suggest::closest(name, names) {
                problem = problem.with_label(format!("did you mean `{best}`?"));
            }
            self.report(problem);
            self.poisoned = true;
            return rel;
        };
        if def.is_query() {
            let problem = self
                .error(call.span, format!("`{name}` is a query, not steps"))
                .with_help(format!("use it with `from` or `join`: `from {name}(...)`"));
            self.report(problem);
            return rel;
        }
        let cx = Cx {
            frame: &rel.frame,
            bindings,
            totals: false,
            in_total: false,
        };
        let Some(bound) = self.bind(def, args, call.span, cx) else {
            self.poisoned = true;
            return rel;
        };
        if self.enter(name, call.span).is_none() {
            return rel;
        }
        for step in &def.body {
            rel = self.step(rel, step, &bound);
        }
        self.stack.pop();
        rel
    }

    /// A query `def`, called with its arguments: `from cases_for("A1")`.
    fn query_def(
        &mut self,
        name: &str,
        args: &'a [ast::Arg],
        span: Span,
        bindings: &Bindings,
    ) -> Option<Rel> {
        let def = self.defs.get(name).copied()?;
        if !def.is_query() {
            let problem = self
                .error(span, format!("`{name}` holds steps, not a query"))
                .with_help(format!(
                    "put its steps in place by calling it on a line of its own: `{name}()`"
                ));
            self.report(problem);
            return None;
        }
        let frame = Frame::default();
        let cx = Cx {
            frame: &frame,
            bindings,
            totals: false,
            in_total: false,
        };
        let bound = self.bind(def, args, span, cx)?;
        self.enter(name, span)?;
        let rel = self.query(&def.body, &bound);
        self.stack.pop();
        rel
    }

    /// The values of a `def`'s parameters for one call.
    fn bind(
        &mut self,
        def: &'a ast::Def,
        args: &'a [ast::Arg],
        span: Span,
        cx: Cx<'_>,
    ) -> Option<Bindings> {
        let name = &def.name.text;
        let mut bound = Bindings::new();
        let positional: Vec<&ast::Arg> = args.iter().filter(|arg| arg.name.is_none()).collect();
        if positional.len() > def.params.len() {
            let problem = self.error(
                positional[def.params.len()].value.span,
                format!(
                    "`{name}` takes {} argument{}, but {} were given",
                    def.params.len(),
                    if def.params.len() == 1 { "" } else { "s" },
                    positional.len()
                ),
            );
            self.report(problem);
            return None;
        }
        let mut ok = true;
        // The parameters given a value, whether or not the value was right.
        let mut given: Vec<&str> = Vec::new();
        for (param, arg) in def.params.iter().zip(&positional) {
            given.push(&param.name.text);
            match self.argument(param, &arg.value, cx) {
                Some(value) => {
                    bound.insert(param.name.text.clone(), value);
                }
                None => ok = false,
            }
        }
        for arg in args.iter().filter(|arg| arg.name.is_some()) {
            let arg_name = arg.name.as_ref().expect("a named argument");
            let Some(param) = def
                .params
                .iter()
                .find(|param| param.name.text == arg_name.text)
            else {
                let names = def.params.iter().map(|param| param.name.text.as_str());
                let mut problem = self.error(
                    arg_name.span,
                    format!("`{name}` has no parameter `{}`", arg_name.text),
                );
                if let Some(best) = suggest::closest(&arg_name.text, names) {
                    problem = problem.with_label(format!("did you mean `{best}`?"));
                }
                self.report(problem);
                ok = false;
                continue;
            };
            if given.contains(&param.name.text.as_str()) {
                let problem =
                    self.error(arg_name.span, format!("`{}` is given twice", arg_name.text));
                self.report(problem);
                ok = false;
                continue;
            }
            given.push(&param.name.text);
            match self.argument(param, &arg.value, cx) {
                Some(value) => {
                    bound.insert(param.name.text.clone(), value);
                }
                None => ok = false,
            }
        }
        for param in &def.params {
            if given.contains(&param.name.text.as_str()) {
                continue;
            }
            match &param.default {
                Some(default) => {
                    let frame = Frame::default();
                    let empty = Bindings::new();
                    let default_cx = Cx {
                        frame: &frame,
                        bindings: &empty,
                        totals: false,
                        in_total: false,
                    };
                    match self.argument(param, default, default_cx) {
                        Some(value) => {
                            bound.insert(param.name.text.clone(), value);
                        }
                        None => ok = false,
                    }
                }
                None => {
                    let problem = self
                        .error(
                            span,
                            format!("`{name}` needs a value for `{}`", param.name.text),
                        )
                        .with_related(param.span, "the parameter");
                    self.report(problem);
                    ok = false;
                }
            }
        }
        ok.then_some(bound)
    }

    /// An argument's value, checked against its parameter's type.
    fn argument(&mut self, param: &ast::Param, value: &'a ast::Expr, cx: Cx<'_>) -> Option<Typed> {
        let typed = self.expr(value, cx)?;
        let wanted = param.annotation.as_ref().map(|annotation| {
            let name = annotation
                .name
                .parts
                .last()
                .map_or("", |part| part.text.as_str());
            match name {
                "str" => Type::Text,
                "int" => Type::Int,
                "float" => Type::Float,
                "bool" => Type::Bool,
                "date" => Type::Date,
                "datetime" => Type::Timestamp,
                _ => Type::Unknown,
            }
        });
        if let Some(wanted) = wanted {
            let fits = wanted == Type::Unknown
                || typed.ty == Type::Unknown
                || typed.ty == wanted
                || (wanted == Type::Float && typed.ty.is_number());
            if !fits {
                let problem = self.error(
                    value.span,
                    format!(
                        "`{}` takes {}, but this is {}",
                        param.name.text,
                        wanted.describe(),
                        typed.ty.describe()
                    ),
                );
                self.report(problem);
                return None;
            }
        }
        Some(typed)
    }

    // ---- Names

    fn value(&mut self, name: &str, span: Span) -> Option<Option<Typed>> {
        if let Some(value) = self.values.get(name) {
            return Some(Some(value.clone()));
        }
        let declared = *self.lets.get(name)?;
        if matches!(declared.value.kind, ExprKind::Query(_)) {
            return None;
        }
        if self.enter(name, span).is_none() {
            return Some(None);
        }
        let frame = Frame::default();
        let bindings = Bindings::new();
        let cx = Cx {
            frame: &frame,
            bindings: &bindings,
            totals: false,
            in_total: false,
        };
        let typed = self.expr(&declared.value, cx);
        self.stack.pop();
        if let Some(typed) = &typed {
            self.values.insert(name.to_string(), typed.clone());
        }
        Some(typed)
    }

    /// A name: a parameter, a named value, or a column.
    fn name(&mut self, name: &str, span: Span, cx: Cx<'_>) -> Option<Typed> {
        if let Some(value) = cx.bindings.get(name) {
            return Some(value.clone());
        }
        if let Some(value) = self.value(name, span) {
            return value;
        }
        let exact: Vec<&Col> = cx
            .frame
            .cols
            .iter()
            .rev()
            .filter(|col| col.name == name)
            .collect();
        let found: Vec<&Col> = if exact.is_empty() {
            cx.frame
                .cols
                .iter()
                .rev()
                .filter(|col| col.name.eq_ignore_ascii_case(name))
                .collect()
        } else {
            exact
        };
        match found.as_slice() {
            [col] => return Some(col.value.clone()),
            [latest, ..] if !latest.base => return Some(latest.value.clone()),
            [first, second, ..] => {
                let (a, b) = (
                    first.table.clone().unwrap_or_default(),
                    second.table.clone().unwrap_or_default(),
                );
                let problem = self
                    .error(
                        span,
                        format!("`{name}` is a column of both `{b}` and `{a}`"),
                    )
                    .with_help(format!("say which: `{b}.{name}` or `{a}.{name}`"));
                self.report(problem);
                return None;
            }
            [] => {}
        }
        if self.named_query(name).is_some() || self.defs.contains_key(name) {
            let problem = self
                .error(span, format!("`{name}` is a query, not a value"))
                .with_help(format!(
                    "read it with `from {name}` or `join {name}`, or test values with `in {name}`"
                ));
            self.report(problem);
            return None;
        }
        let open = cx.frame.open();
        if !open.is_empty() || self.poisoned {
            let table = match open.as_slice() {
                [only] => Some(only.alias.as_str()),
                _ => None,
            };
            let mut typed = Typed::new(Sql::column(table, name), Type::Unknown).nullable(true);
            typed.reads = true;
            return Some(typed);
        }
        let names: Vec<&str> = cx.frame.cols.iter().map(|col| col.name.as_str()).collect();
        let tables: Vec<&str> = cx
            .frame
            .tables
            .iter()
            .map(|scope| scope.alias.as_str())
            .collect();
        let place = match tables.as_slice() {
            [only] if !only.starts_with("_q") => format!(" in `{only}`"),
            _ => String::new(),
        };
        let mut problem = self.error(span, format!("there is no column `{name}`{place}"));
        if let Some(best) = suggest::closest(name, names.iter().copied()) {
            problem = problem.with_label(format!("did you mean `{best}`?"));
        } else if !names.is_empty() && names.len() <= 12 {
            problem = problem.with_help(format!("the columns here are {}", list(&names)));
        }
        self.report(problem);
        None
    }

    /// `table.column`, or a part of a date: `raised.year`.
    fn attribute(
        &mut self,
        base: &'a ast::Expr,
        field: &'a ast::Name,
        span: Span,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        if let ExprKind::Name(table) = &base.kind {
            let is_table = cx.frame.tables.iter().any(|scope| &scope.alias == table)
                && !cx.bindings.contains_key(table)
                && !cx.frame.cols.iter().any(|col| &col.name == table);
            if is_table {
                let scope = cx
                    .frame
                    .tables
                    .iter()
                    .find(|scope| &scope.alias == table)
                    .expect("the table is in scope");
                let found = cx
                    .frame
                    .cols
                    .iter()
                    .rev()
                    .find(|col| col.table.as_ref() == Some(table) && col.name == field.text)
                    .or_else(|| {
                        cx.frame.cols.iter().rev().find(|col| {
                            col.table.as_ref() == Some(table)
                                && col.name.eq_ignore_ascii_case(&field.text)
                        })
                    });
                if let Some(col) = found {
                    return Some(col.value.clone());
                }
                if !scope.known || self.poisoned {
                    let mut typed =
                        Typed::new(Sql::column(Some(table), &field.text), Type::Unknown)
                            .nullable(true);
                    typed.reads = true;
                    return Some(typed);
                }
                let names: Vec<&str> = cx
                    .frame
                    .cols
                    .iter()
                    .filter(|col| col.table.as_ref() == Some(table))
                    .map(|col| col.name.as_str())
                    .collect();
                let mut problem = self.error(
                    field.span,
                    format!("there is no column `{}` in `{table}`", field.text),
                );
                if let Some(best) = suggest::closest(&field.text, names) {
                    problem = problem.with_label(format!("did you mean `{best}`?"));
                }
                self.report(problem);
                return None;
            }
        }
        let value = self.expr(base, cx)?;
        let part = match field.text.as_str() {
            "year" => "YEAR",
            "month" => "MONTH",
            "day" => "DAY",
            "hour" => "HOUR",
            "minute" => "MINUTE",
            "second" => "SECOND",
            _ => {
                let problem = match &base.kind {
                    ExprKind::Name(name) if cx.frame.tables.is_empty() || !value.reads => self
                        .error(span, format!("`{name}` has no `{}`", field.text)),
                    ExprKind::Name(name) => self
                        .error(base.span, format!("there is no table `{name}` here"))
                        .with_help("a dot after a table's name reads one of its columns: `accounts.operator`"),
                    _ => self.error(field.span, format!("there is no `.{}` for this value", field.text)),
                };
                self.report(problem);
                return None;
            }
        };
        if !(value.ty.is_time() || value.ty == Type::Unknown) {
            let problem = self.error(
                base.span,
                format!(
                    "`.{}` is a part of a date, and this is {}",
                    field.text,
                    value.ty.describe()
                ),
            );
            self.report(problem);
            return None;
        }
        Some(self.date_part(part, value))
    }

    fn date_part(&self, part: &'static str, value: Typed) -> Typed {
        let sql = match self.dialect {
            Dialect::Sqlite => {
                let format = match part {
                    "YEAR" => "%Y",
                    "MONTH" => "%m",
                    "DAY" => "%d",
                    "HOUR" => "%H",
                    "MINUTE" => "%M",
                    _ => "%S",
                };
                Sql::Cast(
                    Box::new(Sql::call(
                        "strftime",
                        vec![Sql::Text(format.to_string()), value.sql.clone()],
                    )),
                    "INTEGER",
                )
            }
            _ => Sql::Cast(
                Box::new(Sql::Extract(part, Box::new(value.sql.clone()))),
                "INTEGER",
            ),
        };
        Typed::from(sql, Type::Int, &[&value])
    }

    // ---- Expressions

    fn expr(&mut self, expr: &'a ast::Expr, cx: Cx<'_>) -> Option<Typed> {
        match &expr.kind {
            ExprKind::Name(name) => self.name(name, expr.span, cx),
            ExprKind::Attribute(base, field) => self.attribute(base, field, expr.span, cx),
            ExprKind::Call(function, args) => self.call(function, args, expr.span, cx),
            ExprKind::Number(number) => Some(Typed::new(
                Sql::Number(number.text.clone()),
                if number.float { Type::Float } else { Type::Int },
            )),
            ExprKind::Str(text) => Some(Typed::new(
                Sql::Param(Value::Text(text.clone())),
                Type::Text,
            )),
            ExprKind::FString(parts) => self.fstring(parts, cx),
            ExprKind::Bool(value) => Some(Typed::new(
                Sql::Keyword(if *value { "TRUE" } else { "FALSE" }),
                Type::Bool,
            )),
            ExprKind::None => Some(Typed::new(Sql::Keyword("NULL"), Type::Unknown).nullable(true)),
            ExprKind::Unary(op, operand) => self.unary(*op, operand, expr.span, cx),
            ExprKind::Binary(op, left, right) => self.binary(*op, left, right, expr.span, cx),
            ExprKind::Compare(first, rest) => self.compare(first, rest, cx),
            ExprKind::IfElse {
                test,
                then,
                otherwise,
            } => {
                let test = self.expr(test, cx)?;
                let then = self.expr(then, cx)?;
                let otherwise = self.expr(otherwise, cx)?;
                let ty = if then.ty == Type::Unknown {
                    otherwise.ty
                } else {
                    then.ty
                };
                let condition = self.truth(test.clone());
                let sql = Sql::Case(
                    vec![(condition, then.sql.clone())],
                    Some(Box::new(otherwise.sql.clone())),
                );
                Some(Typed::from(sql, ty, &[&test, &then, &otherwise]))
            }
            ExprKind::List(_) | ExprKind::Tuple(_) | ExprKind::Set(_) => {
                let problem = self
                    .error(expr.span, "a list can only be used after `in`")
                    .with_help("for example: `band in [\"critical\", \"high\"]`");
                self.report(problem);
                None
            }
            ExprKind::Query(_) => {
                let problem = self
                    .error(
                        expr.span,
                        "a query used as a value can only be used after `in` for now",
                    )
                    .with_help("for example: `account_id in (from blocked; select account_id)`");
                self.report(problem);
                None
            }
            ExprKind::Error => None,
        }
    }

    /// A condition, as `where` reads it: Python's truth, in which `0`, `""` and `None` are false.
    fn truth(&self, value: Typed) -> Sql {
        match value.ty {
            Type::Int | Type::Float | Type::Decimal => {
                Sql::binary(Binary::NotEq, value.sql, Sql::Number("0".into()))
            }
            Type::Text => Sql::binary(
                Binary::NotEq,
                value.sql,
                Sql::Param(Value::Text(String::new())),
            ),
            _ => value.sql,
        }
    }

    fn fstring(&mut self, parts: &'a [FPart], cx: Cx<'_>) -> Option<Typed> {
        let mut pieces: Vec<Typed> = Vec::new();
        for part in parts {
            match part {
                FPart::Text(text) if text.is_empty() => {}
                FPart::Text(text) => pieces.push(Typed::new(
                    Sql::Param(Value::Text(text.clone())),
                    Type::Text,
                )),
                FPart::Value(value) => {
                    let typed = self.expr(value, cx)?;
                    let sql = if typed.ty == Type::Text {
                        typed.sql.clone()
                    } else {
                        Sql::Cast(Box::new(typed.sql.clone()), "TEXT")
                    };
                    pieces.push(Typed::from(sql, Type::Text, &[&typed]));
                }
            }
        }
        let Some(first) = pieces.first().cloned() else {
            return Some(Typed::new(
                Sql::Param(Value::Text(String::new())),
                Type::Text,
            ));
        };
        Some(pieces[1..].iter().fold(first, |joined, piece| {
            Typed::from(
                Sql::binary(Binary::Concat, joined.sql.clone(), piece.sql.clone()),
                Type::Text,
                &[&joined, piece],
            )
        }))
    }

    fn unary(
        &mut self,
        op: UnaryOp,
        operand: &'a ast::Expr,
        span: Span,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        let value = self.expr(operand, cx)?;
        match op {
            UnaryOp::Pos => Some(value),
            UnaryOp::Not => {
                let condition = self.truth(value.clone());
                Some(Typed::from(Sql::not(condition), Type::Bool, &[&value]))
            }
            UnaryOp::Neg if value.ty == Type::Duration => {
                let mut negated = value;
                negated.parts = negated
                    .parts
                    .into_iter()
                    .map(|(amount, unit)| (negate(amount), unit))
                    .collect();
                negated.sql = self.interval(&negated.parts);
                Some(negated)
            }
            UnaryOp::Neg => {
                if !(value.ty.is_number() || value.ty == Type::Unknown) {
                    let problem = self.error(
                        span,
                        format!(
                            "`-` makes a number negative, and this is {}",
                            value.ty.describe()
                        ),
                    );
                    self.report(problem);
                    return None;
                }
                let sql = negate(value.sql.clone());
                Some(Typed::from(sql, value.ty, &[&value]))
            }
        }
    }

    fn binary(
        &mut self,
        op: BinaryOp,
        left: &'a ast::Expr,
        right: &'a ast::Expr,
        span: Span,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        let a = self.expr(left, cx);
        let b = self.expr(right, cx);
        let (a, b) = (a?, b?);
        match op {
            BinaryOp::And | BinaryOp::Or => {
                let sql_op = if op == BinaryOp::And {
                    Binary::And
                } else {
                    Binary::Or
                };
                let sql = Sql::binary(sql_op, self.truth(a.clone()), self.truth(b.clone()));
                Some(Typed::from(sql, Type::Bool, &[&a, &b]))
            }
            BinaryOp::Add | BinaryOp::Sub => self.add(op == BinaryOp::Sub, a, b, span),
            BinaryOp::Mul => {
                if a.ty == Type::Duration || b.ty == Type::Duration {
                    let (duration, times) = if a.ty == Type::Duration {
                        (a, b)
                    } else {
                        (b, a)
                    };
                    let mut scaled = duration.clone();
                    scaled.parts = duration
                        .parts
                        .iter()
                        .map(|(amount, unit)| {
                            (
                                Sql::binary(Binary::Mul, amount.clone(), times.sql.clone()),
                                *unit,
                            )
                        })
                        .collect();
                    scaled.sql = self.interval(&scaled.parts);
                    return Some(scaled);
                }
                self.numbers("*", &a, &b, span)?;
                let ty = number_type(a.ty, b.ty);
                Some(Typed::from(
                    Sql::binary(Binary::Mul, a.sql.clone(), b.sql.clone()),
                    ty,
                    &[&a, &b],
                ))
            }
            BinaryOp::Div => {
                self.numbers("/", &a, &b, span)?;
                // `/` divides exactly, as in Python, even where the database would drop the fraction of two whole
                // numbers.
                let exact = matches!(a.ty, Type::Float | Type::Decimal)
                    || matches!(b.ty, Type::Float | Type::Decimal);
                let left = if exact {
                    a.sql.clone()
                } else {
                    Sql::binary(Binary::Mul, a.sql.clone(), Sql::Number("1.0".into()))
                };
                let ty = if a.ty == Type::Decimal || b.ty == Type::Decimal {
                    Type::Decimal
                } else {
                    Type::Float
                };
                let mut typed =
                    Typed::from(Sql::binary(Binary::Div, left, b.sql.clone()), ty, &[&a, &b]);
                typed.nullable = true;
                Some(typed)
            }
            BinaryOp::FloorDiv => {
                self.numbers("//", &a, &b, span)?;
                Some(self.floor_div(a, b))
            }
            BinaryOp::Mod => {
                self.numbers("%", &a, &b, span)?;
                let fraction = matches!(a.ty, Type::Float | Type::Decimal)
                    || matches!(b.ty, Type::Float | Type::Decimal);
                let sql = if fraction && self.dialect == Dialect::Sqlite {
                    // SQLite's `%` drops fractions, so Python's is worked out: a - b * floor(a / b).
                    let quotient = Sql::binary(
                        Binary::Div,
                        Sql::binary(Binary::Mul, a.sql.clone(), Sql::Number("1.0".into())),
                        b.sql.clone(),
                    );
                    Sql::binary(
                        Binary::Sub,
                        a.sql.clone(),
                        Sql::binary(Binary::Mul, b.sql.clone(), self.floor(quotient)),
                    )
                } else {
                    // Python's `%` takes the sign of the divisor; SQL's takes the dividend's.
                    python_mod(a.sql.clone(), b.sql.clone())
                };
                Some(Typed::from(sql, number_type(a.ty, b.ty), &[&a, &b]))
            }
            BinaryOp::Pow => {
                self.numbers("**", &a, &b, span)?;
                if let Sql::Number(digits) = &b.sql {
                    if let Ok(power @ 1..=4) = digits.parse::<u32>() {
                        let product = (1..power).fold(a.sql.clone(), |product, _| {
                            Sql::binary(Binary::Mul, product, a.sql.clone())
                        });
                        return Some(Typed::from(product, a.ty, &[&a, &b]));
                    }
                }
                let sql = Sql::call("power", vec![a.sql.clone(), b.sql.clone()]);
                Some(Typed::from(sql, Type::Float, &[&a, &b]))
            }
        }
    }

    /// Checks that both sides of an operator are numbers, as far as is known.
    fn numbers(&mut self, symbol: &str, a: &Typed, b: &Typed, span: Span) -> Option<()> {
        for side in [a, b] {
            if !(side.ty.is_number() || side.ty == Type::Unknown) {
                let problem = self.error(
                    span,
                    format!(
                        "`{symbol}` works on numbers, and one side is {}",
                        side.ty.describe()
                    ),
                );
                self.report(problem);
                return None;
            }
        }
        Some(())
    }

    fn add(&mut self, subtract: bool, a: Typed, b: Typed, span: Span) -> Option<Typed> {
        let symbol = if subtract { "-" } else { "+" };
        // A moment moved by a length of time.
        if a.ty.is_time() || (a.ty == Type::Unknown && b.ty == Type::Duration) {
            if b.ty == Type::Duration {
                return Some(self.shift(a, &b.parts, subtract));
            }
            if b.ty.is_time() {
                let problem = self
                    .error(
                        span,
                        "subtracting one date from another is not supported yet",
                    )
                    .with_help("compare them instead, as in `closed > raised + days(7)`");
                self.report(problem);
                return None;
            }
        }
        if b.ty.is_time() && a.ty == Type::Duration && !subtract {
            return Some(self.shift(b, &a.parts, false));
        }
        if a.ty == Type::Duration && b.ty == Type::Duration {
            let mut parts = a.parts.clone();
            parts.extend(b.parts.iter().map(|(amount, unit)| {
                let amount = if subtract {
                    negate(amount.clone())
                } else {
                    amount.clone()
                };
                (amount, *unit)
            }));
            let mut typed = Typed::from(self.interval(&parts), Type::Duration, &[&a, &b]);
            typed.parts = parts;
            return Some(typed);
        }
        // Text joined with `+`, as in Python.
        if !subtract && (a.ty == Type::Text || b.ty == Type::Text) {
            let other = if a.ty == Type::Text { &b } else { &a };
            if other.ty != Type::Text && other.ty != Type::Unknown {
                let problem = self
                    .error(
                        span,
                        format!("`+` can't join text and {}", other.ty.describe()),
                    )
                    .with_help(
                        "to put a value in text, use an f-string, as in `f\"{name} {risk}\"`",
                    );
                self.report(problem);
                return None;
            }
            return Some(Typed::from(
                Sql::binary(Binary::Concat, a.sql.clone(), b.sql.clone()),
                Type::Text,
                &[&a, &b],
            ));
        }
        self.numbers(symbol, &a, &b, span)?;
        let op = if subtract { Binary::Sub } else { Binary::Add };
        Some(Typed::from(
            Sql::binary(op, a.sql.clone(), b.sql.clone()),
            number_type(a.ty, b.ty),
            &[&a, &b],
        ))
    }

    /// A moment, moved by the parts of a length of time.
    fn shift(&self, moment: Typed, parts: &[(Sql, Unit)], subtract: bool) -> Typed {
        let parts: Vec<(Sql, Unit)> = parts
            .iter()
            .map(|(amount, unit)| {
                let amount = if subtract {
                    negate(amount.clone())
                } else {
                    amount.clone()
                };
                (amount, *unit)
            })
            .collect();
        let date = moment.ty == Type::Date && parts.iter().all(|(_, unit)| *unit == Unit::Days);
        let ty = if date { Type::Date } else { Type::Timestamp };
        let sql = match self.dialect {
            Dialect::Sqlite => {
                // SQLite moves a moment with its date functions: datetime(x, '+7 days').
                let mut args = vec![moment.sql.clone()];
                for (amount, unit) in &parts {
                    args.push(match amount {
                        Sql::Number(digits) => {
                            let sign = if digits.starts_with('-') { "" } else { "+" };
                            Sql::Text(format!("{sign}{digits} {}", unit.name()))
                        }
                        amount => Sql::binary(
                            Binary::Concat,
                            Sql::Cast(Box::new(amount.clone()), "TEXT"),
                            Sql::Text(format!(" {}", unit.name())),
                        ),
                    });
                }
                let function = if date { "date" } else { "datetime" };
                // datetime(datetime('now'), '-7 days') is datetime('now', '-7 days'): the modifiers apply in order.
                if let Sql::Call(inner, inner_args) = &moment.sql {
                    if inner == function {
                        let mut flat = inner_args.clone();
                        flat.extend(args.into_iter().skip(1));
                        args = flat;
                        return Typed::from(Sql::call(function, args), ty, &[&moment]);
                    }
                }
                Sql::call(function, args)
            }
            _ if date => {
                // A date moved by whole days stays a date: date + integer.
                parts
                    .iter()
                    .fold(moment.sql.clone(), |moved, (amount, _)| match amount {
                        Sql::Number(digits) if digits.starts_with('-') => {
                            Sql::binary(Binary::Sub, moved, Sql::Number(digits[1..].to_string()))
                        }
                        Sql::Unary(Unary::Neg, inner) => {
                            Sql::binary(Binary::Sub, moved, (**inner).clone())
                        }
                        amount => Sql::binary(Binary::Add, moved, amount.clone()),
                    })
            }
            _ => {
                let (op, interval) = match (subtract, self.interval(&parts)) {
                    // Written as `now() - INTERVAL '7 days'` rather than adding a negative length.
                    (true, _) => (
                        Binary::Sub,
                        self.interval(
                            &parts
                                .iter()
                                .map(|(amount, unit)| (negate(amount.clone()), *unit))
                                .collect::<Vec<_>>(),
                        ),
                    ),
                    (false, interval) => (Binary::Add, interval),
                };
                Sql::binary(op, moment.sql.clone(), interval)
            }
        };
        let mut typed = Typed::from(sql, ty, &[&moment]);
        typed.parts = Vec::new();
        typed
    }

    /// A length of time as PostgreSQL writes it: `INTERVAL '7 days'`, or `n * INTERVAL '1 day'`.
    fn interval(&self, parts: &[(Sql, Unit)]) -> Sql {
        let literal: Option<Vec<String>> = parts
            .iter()
            .map(|(amount, unit)| match amount {
                Sql::Number(digits) => Some(format!("{digits} {}", unit.name())),
                _ => None,
            })
            .collect();
        if let Some(literal) = literal {
            return Sql::Typed("INTERVAL", literal.join(" "));
        }
        let mut terms = parts.iter().map(|(amount, unit)| {
            Sql::binary(
                Binary::Mul,
                amount.clone(),
                Sql::Typed("INTERVAL", unit.one().to_string()),
            )
        });
        let first = terms
            .next()
            .unwrap_or(Sql::Typed("INTERVAL", "0 days".to_string()));
        terms.fold(first, |sum, term| Sql::binary(Binary::Add, sum, term))
    }

    fn floor_div(&self, a: Typed, b: Typed) -> Typed {
        let both_whole = a.ty == Type::Int && b.ty == Type::Int;
        let sql = if both_whole {
            // Whole numbers: SQL's `/` rounds towards zero, Python's `//` down; one less when the signs differ and
            // the division is not exact.
            let quotient = Sql::binary(Binary::Div, a.sql.clone(), b.sql.clone());
            let signs_differ = Sql::binary(
                Binary::NotEq,
                Sql::binary(Binary::Lt, a.sql.clone(), Sql::Number("0".into())),
                Sql::binary(Binary::Lt, b.sql.clone(), Sql::Number("0".into())),
            );
            let inexact = Sql::binary(
                Binary::NotEq,
                Sql::binary(Binary::Mod, a.sql.clone(), b.sql.clone()),
                Sql::Number("0".into()),
            );
            let adjust = Sql::Case(
                vec![(
                    Sql::binary(Binary::And, inexact, signs_differ),
                    Sql::Number("1".into()),
                )],
                Some(Box::new(Sql::Number("0".into()))),
            );
            Sql::binary(Binary::Sub, quotient, adjust)
        } else {
            let quotient = Sql::binary(
                Binary::Div,
                Sql::binary(Binary::Mul, a.sql.clone(), Sql::Number("1.0".into())),
                b.sql.clone(),
            );
            self.floor(quotient)
        };
        let ty = if both_whole { Type::Int } else { Type::Float };
        let mut typed = Typed::from(sql, ty, &[&a, &b]);
        typed.nullable = true;
        typed
    }

    /// A number rounded down. Many builds of SQLite leave out `floor()`, so there it is worked out from a cast,
    /// which rounds towards zero: one less for a negative number with a fraction.
    fn floor(&self, value: Sql) -> Sql {
        match self.dialect {
            Dialect::Sqlite => {
                let truncated = Sql::Cast(Box::new(value.clone()), "INTEGER");
                Sql::binary(
                    Binary::Sub,
                    truncated.clone(),
                    Sql::binary(Binary::Lt, value, truncated),
                )
            }
            _ => Sql::call("floor", vec![value]),
        }
    }

    fn compare(
        &mut self,
        first: &'a ast::Expr,
        rest: &'a [(CompareOp, ast::Expr)],
        cx: Cx<'_>,
    ) -> Option<Typed> {
        // `low <= x <= high` is SQL's `BETWEEN`.
        if let [(CompareOp::LtEq, middle), (CompareOp::LtEq, high)] = rest {
            let low = self.expr(first, cx)?;
            let middle = self.expr(middle, cx)?;
            let high = self.expr(high, cx)?;
            let sql = Sql::Between(
                Box::new(middle.sql.clone()),
                Box::new(low.sql.clone()),
                Box::new(high.sql.clone()),
            );
            return Some(Typed::from(sql, Type::Bool, &[&low, &middle, &high]));
        }
        let mut left = first;
        let mut conditions = Vec::new();
        for (op, right) in rest {
            conditions.push(self.comparison(*op, left, right, cx)?);
            left = right;
        }
        let mut all = conditions.remove(0);
        for next in conditions {
            all = Typed::from(
                Sql::binary(Binary::And, all.sql.clone(), next.sql.clone()),
                Type::Bool,
                &[&all, &next],
            );
        }
        Some(all)
    }

    fn comparison(
        &mut self,
        op: CompareOp,
        left: &'a ast::Expr,
        right: &'a ast::Expr,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        match op {
            CompareOp::In | CompareOp::NotIn => {
                return self.membership(left, right, op == CompareOp::NotIn, cx);
            }
            CompareOp::Is | CompareOp::IsNot => {
                let negated = op == CompareOp::IsNot;
                let value = self.expr(left, cx)?;
                let sql = match &right.kind {
                    ExprKind::None => Sql::IsNull(Box::new(value.sql.clone()), negated),
                    ExprKind::Bool(truth) => {
                        Sql::IsBool(Box::new(value.sql.clone()), *truth, negated)
                    }
                    _ => {
                        let problem = self
                            .error(right.span, "`is` compares with `None`, `True` or `False`")
                            .with_help("to compare two values, use `==`");
                        self.report(problem);
                        return None;
                    }
                };
                return Some(Typed::from(sql, Type::Bool, &[&value]).nullable(false));
            }
            _ => {}
        }
        // `x == None` means what it says: x is missing.
        let missing = |expr: &ast::Expr| matches!(expr.kind, ExprKind::None);
        if matches!(op, CompareOp::Eq | CompareOp::NotEq) && (missing(left) || missing(right)) {
            let other = if missing(left) { right } else { left };
            let value = self.expr(other, cx)?;
            let sql = Sql::IsNull(Box::new(value.sql.clone()), op == CompareOp::NotEq);
            return Some(Typed::from(sql, Type::Bool, &[&value]).nullable(false));
        }
        let a = self.expr(left, cx);
        let b = self.expr(right, cx);
        let (a, b) = (a?, b?);
        let comparable = a.ty == b.ty
            || a.ty == Type::Unknown
            || b.ty == Type::Unknown
            || (a.ty.is_number() && b.ty.is_number())
            || (a.ty.is_time() && b.ty.is_time())
            || (a.ty.is_time() && b.ty == Type::Text)
            || (a.ty == Type::Text && b.ty.is_time());
        if !comparable {
            let problem = self.error(
                left.span.to(right.span),
                format!(
                    "{} can't be compared with {}",
                    capitalised(a.ty.describe()),
                    b.ty.describe()
                ),
            );
            self.report(problem);
            return None;
        }
        let sql_op = match op {
            CompareOp::Eq => Binary::Eq,
            CompareOp::NotEq => Binary::NotEq,
            CompareOp::Lt => Binary::Lt,
            CompareOp::LtEq => Binary::LtEq,
            CompareOp::Gt => Binary::Gt,
            _ => Binary::GtEq,
        };
        Some(Typed::from(
            Sql::binary(sql_op, a.sql.clone(), b.sql.clone()),
            Type::Bool,
            &[&a, &b],
        ))
    }

    /// `x in [...]`, `x in (from ...)`, or text in text: `"Acme" in operator`.
    fn membership(
        &mut self,
        left: &'a ast::Expr,
        right: &'a ast::Expr,
        negated: bool,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        let value = self.expr(left, cx)?;
        match &right.kind {
            ExprKind::List(items) | ExprKind::Tuple(items) | ExprKind::Set(items) => {
                let mut list = Vec::new();
                let mut inputs = vec![value.clone()];
                let mut with_none = false;
                for item in items {
                    if matches!(item.kind, ExprKind::None) {
                        with_none = true;
                        continue;
                    }
                    let typed = self.expr(item, cx)?;
                    list.push(typed.sql.clone());
                    inputs.push(typed);
                }
                // Each case means what Python means, missing values included: SQL's `x NOT IN (1, NULL)` is never
                // true, a silent mistake ButtercupQL does not make.
                let sql = match (list.is_empty(), with_none) {
                    (true, false) => Sql::Keyword(if negated { "TRUE" } else { "FALSE" }),
                    (true, true) => Sql::IsNull(Box::new(value.sql.clone()), negated),
                    (false, false) => Sql::In(Box::new(value.sql.clone()), list, negated),
                    (false, true) => {
                        let inside = Sql::In(Box::new(value.sql.clone()), list, negated);
                        let missing = Sql::IsNull(Box::new(value.sql.clone()), negated);
                        Sql::binary(
                            if negated { Binary::And } else { Binary::Or },
                            inside,
                            missing,
                        )
                    }
                };
                let refs: Vec<&Typed> = inputs.iter().collect();
                Some(Typed::from(sql, Type::Bool, &refs))
            }
            ExprKind::Query(query) => {
                let select = self.subquery(&query.steps, right.span, cx.bindings)?;
                let sql = Sql::InQuery(Box::new(value.sql.clone()), Box::new(select), negated);
                Some(Typed::from(sql, Type::Bool, &[&value]))
            }
            ExprKind::Name(name)
                if self.named_query(name).is_some() && !cx.bindings.contains_key(name) =>
            {
                let query = self.named_query(name)?;
                self.enter(name, right.span)?;
                let select = self.subquery(&query.steps, right.span, &Bindings::new());
                self.stack.pop();
                let sql = Sql::InQuery(Box::new(value.sql.clone()), Box::new(select?), negated);
                Some(Typed::from(sql, Type::Bool, &[&value]))
            }
            _ => {
                let text = self.expr(right, cx)?;
                if !(text.ty == Type::Text || text.ty == Type::Unknown)
                    || !(value.ty == Type::Text || value.ty == Type::Unknown)
                {
                    let problem = self
                        .error(right.span, "`in` needs a list, a query, or text to look in")
                        .with_help("for example: `band in [\"critical\", \"high\"]`, or `\"Acme\" in operator`");
                    self.report(problem);
                    return None;
                }
                let position = match self.dialect {
                    Dialect::Sqlite => {
                        Sql::call("instr", vec![text.sql.clone(), value.sql.clone()])
                    }
                    _ => Sql::call("strpos", vec![text.sql.clone(), value.sql.clone()]),
                };
                let op = if negated { Binary::Eq } else { Binary::Gt };
                let sql = Sql::binary(op, position, Sql::Number("0".into()));
                Some(Typed::from(sql, Type::Bool, &[&value, &text]))
            }
        }
    }

    /// A query in brackets used as a list of values: it must give one column.
    fn subquery(
        &mut self,
        steps: &'a [ast::Step],
        span: Span,
        bindings: &Bindings,
    ) -> Option<Select> {
        let rel = self.query(steps, bindings)?;
        let columns = if rel.frame.star && !rel.frame.open().is_empty() {
            None
        } else {
            Some(rel.frame.cols.len())
        };
        if columns != Some(1) {
            let problem = self
                .error(span, "a query after `in` must give one column")
                .with_help("end it with `select` and the column, as in `(from blocked; select account_id)`");
            self.report(problem);
            return None;
        }
        Some(self.output(rel))
    }

    // ---- Functions

    fn call(
        &mut self,
        function: &'a ast::Expr,
        args: &'a [ast::Arg],
        span: Span,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        match &function.kind {
            ExprKind::Name(name) => self.function(name, function.span, args, span, cx),
            ExprKind::Attribute(base, method) => {
                if let ExprKind::Name(module) = &base.kind {
                    let is_value = cx.bindings.contains_key(module)
                        || self.lets.contains_key(module.as_str())
                        || cx.frame.cols.iter().any(|col| &col.name == module)
                        || !cx.frame.open().is_empty();
                    if !is_value && cx.frame.tables.iter().all(|scope| &scope.alias != module) {
                        let problem = self.error(base.span, format!("there is no `{module}` here"));
                        self.report(problem);
                        return None;
                    }
                }
                let value = self.expr(base, cx)?;
                self.method(value, method, args, span, cx)
            }
            _ => {
                let problem = self.error(function.span, "this can't be called");
                self.report(problem);
                None
            }
        }
    }

    /// The arguments of a call, compiled, with the argument given as `where=` kept apart.
    fn args(
        &mut self,
        name: &str,
        args: &'a [ast::Arg],
        allowed: &[&str],
        cx: Cx<'_>,
    ) -> Option<Arguments> {
        let mut positional = Vec::new();
        let mut named = HashMap::new();
        let mut ok = true;
        for arg in args {
            let inner = if let Some(arg_name) = &arg.name {
                if !allowed.contains(&arg_name.text.as_str()) {
                    let problem = self.error(
                        arg_name.span,
                        format!("`{name}` has no argument `{}`", arg_name.text),
                    );
                    self.report(problem);
                    ok = false;
                    continue;
                }
                // A total's condition is about each row: it can't hold a total.
                Cx {
                    totals: false,
                    ..cx
                }
            } else {
                cx
            };
            match self.expr(&arg.value, inner) {
                Some(value) => match &arg.name {
                    Some(arg_name) => {
                        named.insert(arg_name.text.clone(), (value, arg.value.span));
                    }
                    None => positional.push(value),
                },
                None => ok = false,
            }
        }
        ok.then_some((positional, named))
    }

    fn arity(
        &mut self,
        name: &str,
        given: usize,
        low: usize,
        high: usize,
        span: Span,
    ) -> Option<()> {
        if given < low || given > high {
            let wanted = match (low, high) {
                (0, 0) => "no arguments".to_string(),
                (low, high) if low == high => {
                    format!("{low} argument{}", if low == 1 { "" } else { "s" })
                }
                (low, high) => format!("{low} to {high} arguments"),
            };
            let problem = self.error(
                span,
                format!("`{name}()` takes {wanted}, but {given} were given"),
            );
            self.report(problem);
            return None;
        }
        Some(())
    }

    fn function(
        &mut self,
        name: &str,
        name_span: Span,
        args: &'a [ast::Arg],
        span: Span,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        let total = matches!(
            name,
            "count" | "count_distinct" | "sum" | "mean" | "avg" | "any" | "all"
        ) || (matches!(name, "min" | "max")
            && args.iter().filter(|arg| arg.name.is_none()).count() == 1);
        if total {
            if !cx.totals {
                let problem = self
                    .error(span, format!("`{name}()` is a total"))
                    .with_label("a total is worked out from many rows")
                    .with_help("it goes in the block of a `group by` or a `summarize`:\n\n    summarize:\n        n = count()");
                self.report(problem);
                return None;
            }
            if cx.in_total {
                let problem = self.error(span, "a total can't be worked out from another total");
                self.report(problem);
                return None;
            }
            return self.total(name, args, span, cx);
        }
        if self.defs.contains_key(name) && !cx.bindings.contains_key(name) {
            let problem = self
                .error(
                    span,
                    format!("`{name}` is a `def` of steps, not a function"),
                )
                .with_help(format!(
                    "call it on a line of its own, where its steps go: `{name}(...)`"
                ));
            self.report(problem);
            return None;
        }
        let (values, _) = self.args(name, args, &[], cx)?;
        let given = values.len();
        let dialect = self.dialect;
        match name {
            "len" => {
                self.arity(name, given, 1, 1, span)?;
                let value = &values[0];
                Some(Typed::from(
                    Sql::call("length", vec![value.sql.clone()]),
                    Type::Int,
                    &[value],
                ))
            }
            "abs" => {
                self.arity(name, given, 1, 1, span)?;
                let value = &values[0];
                Some(Typed::from(
                    Sql::call("abs", vec![value.sql.clone()]),
                    value.ty,
                    &[value],
                ))
            }
            "round" => {
                self.arity(name, given, 1, 2, span)?;
                let value = &values[0];
                let mut target = value.sql.clone();
                if dialect == Dialect::Postgres && value.ty != Type::Decimal {
                    // PostgreSQL rounds to places only a `numeric`.
                    target = Sql::Cast(Box::new(target), "NUMERIC");
                }
                let mut call = vec![target];
                if let Some(places) = values.get(1) {
                    call.push(places.sql.clone());
                }
                let ty = if given == 1 { Type::Int } else { Type::Decimal };
                let inputs: Vec<&Typed> = values.iter().collect();
                Some(Typed::from(Sql::call("round", call), ty, &inputs))
            }
            "min" | "max" => {
                if given < 2 {
                    self.arity(name, given, 2, usize::MAX, span)?;
                }
                let function = match (dialect, name) {
                    (Dialect::Postgres, "min") => "least",
                    (Dialect::Postgres, _) => "greatest",
                    (_, name) => name,
                };
                let ty = values
                    .iter()
                    .map(|value| value.ty)
                    .find(|ty| *ty != Type::Unknown)
                    .unwrap_or(Type::Unknown);
                let inputs: Vec<&Typed> = values.iter().collect();
                Some(Typed::from(
                    Sql::call(
                        function,
                        values.iter().map(|value| value.sql.clone()).collect(),
                    ),
                    ty,
                    &inputs,
                ))
            }
            "str" | "int" | "float" => {
                self.arity(name, given, 1, 1, span)?;
                let value = &values[0];
                let (sql, ty) = match name {
                    "str" => (Sql::Cast(Box::new(value.sql.clone()), "TEXT"), Type::Text),
                    "int" if dialect == Dialect::Postgres => {
                        // Python's int() drops the fraction; PostgreSQL's cast would round it.
                        let whole = if matches!(value.ty, Type::Float | Type::Decimal) {
                            Sql::call("trunc", vec![value.sql.clone()])
                        } else {
                            value.sql.clone()
                        };
                        (Sql::Cast(Box::new(whole), "BIGINT"), Type::Int)
                    }
                    "int" => (Sql::Cast(Box::new(value.sql.clone()), "INTEGER"), Type::Int),
                    _ if dialect == Dialect::Postgres => (
                        Sql::Cast(Box::new(value.sql.clone()), "DOUBLE PRECISION"),
                        Type::Float,
                    ),
                    _ => (Sql::Cast(Box::new(value.sql.clone()), "REAL"), Type::Float),
                };
                Some(Typed::from(sql, ty, &[value]))
            }
            "now" => {
                self.arity(name, given, 0, 0, span)?;
                let sql = match dialect {
                    Dialect::Sqlite => Sql::call("datetime", vec![Sql::Text("now".into())]),
                    _ => Sql::call("now", Vec::new()),
                };
                Some(Typed::new(sql, Type::Timestamp))
            }
            "today" => {
                self.arity(name, given, 0, 0, span)?;
                let sql = match dialect {
                    Dialect::Sqlite => Sql::call("date", vec![Sql::Text("now".into())]),
                    _ => Sql::Keyword("CURRENT_DATE"),
                };
                Some(Typed::new(sql, Type::Date))
            }
            "date" | "datetime" => {
                self.arity(name, given, 1, 1, span)?;
                let text = match &args[0].value.kind {
                    ExprKind::Str(text) => text,
                    _ => {
                        let problem = self
                            .error(
                                args[0].value.span,
                                format!("`{name}()` takes the date written as text"),
                            )
                            .with_help(if name == "date" {
                                "for example: `date(\"2026-10-20\")`"
                            } else {
                                "for example: `datetime(\"2026-10-20 14:30\")`"
                            });
                        self.report(problem);
                        return None;
                    }
                };
                let Some(normal) = (if name == "date" {
                    valid_date(text)
                } else {
                    valid_datetime(text)
                }) else {
                    let problem = self
                        .error(
                            args[0].value.span,
                            format!(
                                "`{text}` is not a valid {}",
                                if name == "date" {
                                    "date"
                                } else {
                                    "date and time"
                                }
                            ),
                        )
                        .with_help(if name == "date" {
                            "write it as year, month and day: `2026-10-20`"
                        } else {
                            "write it as `2026-10-20 14:30` or `2026-10-20 14:30:00`"
                        });
                    self.report(problem);
                    return None;
                };
                let (keyword, ty) = if name == "date" {
                    ("DATE", Type::Date)
                } else {
                    ("TIMESTAMP", Type::Timestamp)
                };
                let sql = match dialect {
                    Dialect::Sqlite => Sql::Text(normal),
                    _ => Sql::Typed(keyword, normal),
                };
                Some(Typed::new(sql, ty))
            }
            "days" | "hours" | "minutes" | "seconds" | "weeks" => {
                self.arity(name, given, 1, 1, span)?;
                let amount = &values[0];
                if !(amount.ty.is_number() || amount.ty == Type::Unknown) {
                    let problem =
                        self.error(args[0].value.span, format!("`{name}()` takes a number"));
                    self.report(problem);
                    return None;
                }
                let (amount_sql, unit) = match name {
                    "weeks" => (
                        match &amount.sql {
                            Sql::Number(digits) => match digits.parse::<i64>() {
                                Ok(weeks) => Sql::Number((weeks * 7).to_string()),
                                Err(_) => Sql::binary(
                                    Binary::Mul,
                                    amount.sql.clone(),
                                    Sql::Number("7".into()),
                                ),
                            },
                            sql => Sql::binary(Binary::Mul, sql.clone(), Sql::Number("7".into())),
                        },
                        Unit::Days,
                    ),
                    "days" => (amount.sql.clone(), Unit::Days),
                    "hours" => (amount.sql.clone(), Unit::Hours),
                    "minutes" => (amount.sql.clone(), Unit::Minutes),
                    _ => (amount.sql.clone(), Unit::Seconds),
                };
                let parts = vec![(amount_sql, unit)];
                let mut typed = Typed::from(self.interval(&parts), Type::Duration, &[amount]);
                typed.parts = parts;
                Some(typed)
            }
            "literal" => {
                self.arity(name, given, 1, 1, span)?;
                match &args[0].value.kind {
                    ExprKind::Str(text) => Some(Typed::new(Sql::Text(text.clone()), Type::Text)),
                    ExprKind::Number(_) | ExprKind::Bool(_) | ExprKind::None => {
                        Some(values[0].clone())
                    }
                    _ => {
                        let problem = self
                            .error(
                                args[0].value.span,
                                "`literal()` takes a value written in the query",
                            )
                            .with_label(
                                "this value is worked out, so it can't be written into the SQL",
                            );
                        self.report(problem);
                        None
                    }
                }
            }
            _ => {
                let mut problem = self.error(name_span, format!("there is no function `{name}`"));
                let known = FUNCTIONS.into_iter().chain(["minutes", "seconds", "weeks"]);
                if let Some(best) = suggest::closest(name, known) {
                    problem = problem.with_label(format!("did you mean `{best}`?"));
                }
                self.report(problem);
                None
            }
        }
    }

    /// A total: `count()`, `sum(amount)`, `count(where=amount > 1_000_000)`.
    fn total(&mut self, name: &str, args: &'a [ast::Arg], span: Span, cx: Cx<'_>) -> Option<Typed> {
        let inner = Cx {
            in_total: true,
            ..cx
        };
        let (values, named) = self.args(name, args, &["where"], inner)?;
        let filter = named.get("where").map(|(condition, _)| {
            if condition.total {
                None
            } else {
                Some(self.truth(condition.clone()))
            }
        });
        let filter = match filter {
            Some(None) => {
                let problem = self.error(
                    span,
                    "a total's `where=` is about each row, and can't hold a total",
                );
                self.report(problem);
                return None;
            }
            Some(Some(filter)) => Some(Box::new(filter)),
            None => None,
        };
        let one = |compiler: &mut Self| compiler.arity(name, values.len(), 1, 1, span);
        let aggregate = |name: &'static str, arg: Option<&Typed>, distinct: bool| Sql::Aggregate {
            name,
            arg: arg.map(|value| Box::new(value.sql.clone())),
            distinct,
            filter: filter.clone(),
        };
        let mut typed = match name {
            "count" => {
                self.arity(name, values.len(), 0, 1, span)?;
                Typed::new(aggregate("count", values.first(), false), Type::Int)
            }
            "count_distinct" => {
                one(self)?;
                Typed::new(aggregate("count", values.first(), true), Type::Int)
            }
            "sum" => {
                one(self)?;
                // The sum of no rows is 0, as in Python, not SQL's NULL.
                let sql = Sql::call(
                    "coalesce",
                    vec![
                        aggregate("sum", values.first(), false),
                        Sql::Number("0".into()),
                    ],
                );
                Typed::new(sql, values[0].ty)
            }
            "mean" | "avg" => {
                one(self)?;
                let ty = if values[0].ty == Type::Decimal {
                    Type::Decimal
                } else {
                    Type::Float
                };
                Typed::new(aggregate("avg", values.first(), false), ty).nullable(true)
            }
            "min" | "max" => {
                let function = if name == "min" { "min" } else { "max" };
                Typed::new(aggregate(function, values.first(), false), values[0].ty).nullable(true)
            }
            _ => {
                one(self)?;
                // any() of no rows is False and all() is True, as in Python.
                let (function, empty) = match (self.dialect, name) {
                    (Dialect::Postgres, "any") => ("bool_or", "FALSE"),
                    (Dialect::Postgres, _) => ("bool_and", "TRUE"),
                    (_, "any") => ("max", "FALSE"),
                    (_, _) => ("min", "TRUE"),
                };
                let sql = Sql::call(
                    "coalesce",
                    vec![
                        aggregate(function, values.first(), false),
                        Sql::Keyword(empty),
                    ],
                );
                Typed::new(sql, Type::Bool)
            }
        };
        typed.total = true;
        typed.reads = true;
        Some(typed)
    }

    /// A method of a value: `name.lower()`, `name.startswith("A")`, `raised.date()`.
    fn method(
        &mut self,
        value: Typed,
        method: &'a ast::Name,
        args: &'a [ast::Arg],
        span: Span,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        let name = method.text.as_str();
        let (values, _) = self.args(name, args, &[], cx)?;
        let given = values.len();
        let dialect = self.dialect;
        let text_method = matches!(
            name,
            "lower"
                | "upper"
                | "strip"
                | "lstrip"
                | "rstrip"
                | "startswith"
                | "endswith"
                | "replace"
        );
        if text_method && !(value.ty == Type::Text || value.ty == Type::Unknown) {
            let problem = self.error(
                method.span,
                format!(
                    "`.{name}()` works on text, and this is {}",
                    value.ty.describe()
                ),
            );
            self.report(problem);
            return None;
        }
        let mut inputs: Vec<&Typed> = vec![&value];
        inputs.extend(values.iter());
        match name {
            "lower" | "upper" => {
                self.arity(name, given, 0, 0, span)?;
                Some(Typed::from(
                    Sql::call(name, vec![value.sql.clone()]),
                    Type::Text,
                    &inputs,
                ))
            }
            "strip" | "lstrip" | "rstrip" => {
                self.arity(name, given, 0, 1, span)?;
                let function = match (name, given, dialect) {
                    ("strip", 1, Dialect::Postgres) => "btrim",
                    ("strip", _, _) => "trim",
                    ("lstrip", _, _) => "ltrim",
                    _ => "rtrim",
                };
                let mut call = vec![value.sql.clone()];
                call.extend(values.iter().map(|value| value.sql.clone()));
                Some(Typed::from(Sql::call(function, call), Type::Text, &inputs))
            }
            "replace" => {
                self.arity(name, given, 2, 2, span)?;
                let call = vec![
                    value.sql.clone(),
                    values[0].sql.clone(),
                    values[1].sql.clone(),
                ];
                Some(Typed::from(Sql::call("replace", call), Type::Text, &inputs))
            }
            "startswith" | "endswith" => {
                self.arity(name, given, 1, 1, span)?;
                let start = name == "startswith";
                let sql = match &values[0].sql {
                    // A prefix written in the query becomes a pattern, which an index can serve. Its wildcards are
                    // escaped, and the match is exact about case on every database.
                    Sql::Param(Value::Text(fix)) => {
                        let (pattern, glob) = match dialect {
                            Dialect::Sqlite => {
                                let fix = glob_escape(fix);
                                (
                                    if start {
                                        format!("{fix}*")
                                    } else {
                                        format!("*{fix}")
                                    },
                                    true,
                                )
                            }
                            _ => {
                                let fix = like_escape(fix);
                                (
                                    if start {
                                        format!("{fix}%")
                                    } else {
                                        format!("%{fix}")
                                    },
                                    false,
                                )
                            }
                        };
                        Sql::Like {
                            expr: Box::new(value.sql.clone()),
                            pattern: Box::new(Sql::Param(Value::Text(pattern))),
                            glob,
                        }
                    }
                    fix => {
                        let length = Sql::call("length", vec![fix.clone()]);
                        let part = match (start, dialect) {
                            (true, _) => Sql::call(
                                "substr",
                                vec![value.sql.clone(), Sql::Number("1".into()), length],
                            ),
                            (false, Dialect::Sqlite) => {
                                Sql::call("substr", vec![value.sql.clone(), negate(length)])
                            }
                            (false, _) => Sql::call("right", vec![value.sql.clone(), length]),
                        };
                        Sql::binary(Binary::Eq, part, fix.clone())
                    }
                };
                Some(Typed::from(sql, Type::Bool, &inputs))
            }
            "date" => {
                self.arity(name, given, 0, 0, span)?;
                if !(value.ty == Type::Timestamp || value.ty == Type::Unknown) {
                    let problem = self.error(
                        method.span,
                        format!(
                            "`.date()` takes the day of a date and time, and this is {}",
                            value.ty.describe()
                        ),
                    );
                    self.report(problem);
                    return None;
                }
                let sql = match dialect {
                    Dialect::Sqlite => Sql::call("date", vec![value.sql.clone()]),
                    _ => Sql::Cast(Box::new(value.sql.clone()), "DATE"),
                };
                Some(Typed::from(sql, Type::Date, &inputs))
            }
            _ => {
                let known = [
                    "lower",
                    "upper",
                    "strip",
                    "lstrip",
                    "rstrip",
                    "startswith",
                    "endswith",
                    "replace",
                    "date",
                ];
                let mut problem =
                    self.error(method.span, format!("there is no method `.{name}()`"));
                if let Some(best) = suggest::closest(name, known) {
                    problem = problem.with_label(format!("did you mean `.{best}()`?"));
                }
                self.report(problem);
                None
            }
        }
    }
}

/// The names of a dotted path: `analytics.daily` is `analytics` and `daily`.
fn path_of(expr: &ast::Expr) -> Vec<String> {
    match &expr.kind {
        ExprKind::Name(name) => vec![name.clone()],
        ExprKind::Attribute(base, name) => {
            let mut path = path_of(base);
            path.push(name.text.clone());
            path
        }
        ExprKind::Call(function, _) => path_of(function),
        _ => Vec::new(),
    }
}

/// Whether an expression uses a name.
fn mentions(expr: &ast::Expr, name: &str) -> bool {
    match &expr.kind {
        ExprKind::Name(found) => found == name,
        ExprKind::Attribute(base, _) => mentions(base, name),
        ExprKind::Call(function, args) => {
            mentions(function, name) || args.iter().any(|arg| mentions(&arg.value, name))
        }
        ExprKind::FString(parts) => parts
            .iter()
            .any(|part| matches!(part, FPart::Value(value) if mentions(value, name))),
        ExprKind::Unary(_, operand) => mentions(operand, name),
        ExprKind::Binary(_, left, right) => mentions(left, name) || mentions(right, name),
        ExprKind::Compare(first, rest) => {
            mentions(first, name) || rest.iter().any(|(_, value)| mentions(value, name))
        }
        ExprKind::IfElse {
            test,
            then,
            otherwise,
        } => mentions(test, name) || mentions(then, name) || mentions(otherwise, name),
        ExprKind::List(items) | ExprKind::Tuple(items) | ExprKind::Set(items) => {
            items.iter().any(|item| mentions(item, name))
        }
        _ => false,
    }
}

fn negate(sql: Sql) -> Sql {
    match sql {
        Sql::Number(digits) => match digits.strip_prefix('-') {
            Some(positive) => Sql::Number(positive.to_string()),
            None => Sql::Number(format!("-{digits}")),
        },
        Sql::Unary(Unary::Neg, inner) => *inner,
        sql => Sql::Unary(Unary::Neg, Box::new(sql)),
    }
}

/// Python's `a % b`, whose result takes the sign of `b`: `((a % b) + b) % b`.
fn python_mod(a: Sql, b: Sql) -> Sql {
    Sql::binary(
        Binary::Mod,
        Sql::binary(
            Binary::Add,
            Sql::binary(Binary::Mod, a, b.clone()),
            b.clone(),
        ),
        b,
    )
}

fn number_type(a: Type, b: Type) -> Type {
    match (a, b) {
        (Type::Int, Type::Int) => Type::Int,
        (Type::Decimal, _) | (_, Type::Decimal) => Type::Decimal,
        (Type::Float, _) | (_, Type::Float) => Type::Float,
        _ => Type::Unknown,
    }
}

/// A prefix or suffix for `LIKE`, with its wildcards made plain.
fn like_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// A prefix or suffix for SQLite's `GLOB`, with its wildcards made plain.
fn glob_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '*' | '?' | '[' => {
                out.push('[');
                out.push(c);
                out.push(']');
            }
            c => out.push(c),
        }
    }
    out
}

/// A date written `2026-10-20`, if it is one.
fn valid_date(text: &str) -> Option<String> {
    let parts: Vec<&str> = text.trim().split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return None;
    };
    if year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    let (year, month, day): (u32, u32, u32) =
        (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (1..=days)
        .contains(&day)
        .then(|| format!("{year:04}-{month:02}-{day:02}"))
}

/// A date and time written `2026-10-20 14:30` or `2026-10-20 14:30:00` (or with a `T`), if it is one. It is
/// returned in the form SQLite's date functions write: `2026-10-20 14:30:00`.
fn valid_datetime(text: &str) -> Option<String> {
    let text = text.trim();
    let (date, time) = text.split_once([' ', 'T'])?;
    let date = valid_date(date)?;
    let parts: Vec<&str> = time.split(':').collect();
    let numbers: Option<Vec<u32>> = parts
        .iter()
        .map(|part| (part.len() == 2).then(|| part.parse().ok()).flatten())
        .collect();
    let (hour, minute, second) = match numbers?.as_slice() {
        [hour, minute] => (*hour, *minute, 0),
        [hour, minute, second] => (*hour, *minute, *second),
        _ => return None,
    };
    (hour < 24 && minute < 60 && second < 60)
        .then(|| format!("{date} {hour:02}:{minute:02}:{second:02}"))
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Names in a list: "`a`, `b` and `c`".
fn list(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}
