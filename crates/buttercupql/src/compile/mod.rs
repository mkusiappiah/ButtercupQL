//! Compiles a ButtercupQL query to SQL.
//!
//! Each step is folded into the `SELECT` being built, in the clause that does its work, and a subquery is started
//! only where the meaning needs one: a `where` after a `take`, or a second grouping. Names are resolved as they are
//! met, so a mistake is reported at its place with the name that was most likely meant.
//!
//! The expressions are compiled in [`expr`], and functions, totals and window functions in [`call`].

mod call;
mod change;
mod ddl;
mod expr;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use call::FUNCTIONS;

use std::collections::HashMap;

use crate::Arg;
use crate::ast::{self, ExprKind, ImportKind, StepKind};
use crate::diagnostic::Diagnostic;
use crate::dialect::Dialect;
use crate::module::Loaded;
use crate::schema::{Reference, Schema, Type};
use crate::span::Span;
use crate::sql::{
    Binary, Count, Expr as Sql, Item, Join, Order, Param, Select, Source, SourceKind, Unary, Value,
};
use crate::suggest;

/// A value of a query, compiled: its SQL and what is known of it.
#[derive(Debug, Clone)]
struct Typed {
    sql: Sql,
    ty: Type,
    nullable: bool,
    /// The value holds a total, such as `count()`.
    total: bool,
    /// The value holds a window function, such as `rank()`.
    window: bool,
    /// The value holds a window function written in the step being compiled, rather than read from a column made
    /// before: it must count only the rows the steps before kept.
    fresh: bool,
    /// The value reads a column.
    reads: bool,
    /// The row of another table the value points to, when it is a column that holds that row's key.
    reference: Option<Reference>,
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
            window: false,
            fresh: false,
            reads: false,
            reference: None,
            parts: Vec::new(),
        }
    }

    fn nullable(mut self, nullable: bool) -> Typed {
        self.nullable = nullable;
        self
    }

    /// A column's value, read from a table.
    fn column(table: Option<&str>, name: &str, ty: Type, nullable: bool) -> Typed {
        let mut typed = Typed::new(Sql::column(table, name), ty).nullable(nullable);
        typed.reads = true;
        typed
    }

    /// A value made from others: missing if any of them is, a total if any of them is.
    fn from(sql: Sql, ty: Type, inputs: &[&Typed]) -> Typed {
        Typed {
            sql,
            ty,
            nullable: inputs.iter().any(|input| input.nullable),
            total: inputs.iter().any(|input| input.total),
            window: inputs.iter().any(|input| input.window),
            fresh: inputs.iter().any(|input| input.fresh),
            reads: inputs.iter().any(|input| input.reads),
            reference: None,
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

    /// ClickHouse's word for the unit, as in `INTERVAL 7 DAY`.
    fn clickhouse(self) -> &'static str {
        match self {
            Unit::Days => "DAY",
            Unit::Hours => "HOUR",
            Unit::Minutes => "MINUTE",
            Unit::Seconds => "SECOND",
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
    /// The column belongs to a table joined to follow a link: it is read only through the link, as in
    /// `account.operator`, and does not come out of the query.
    hidden: bool,
}

/// A table read by the `SELECT` being built.
#[derive(Debug, Clone)]
struct Scope {
    alias: String,
    /// Whether all its columns are known, from the schema or from the query.
    known: bool,
    /// For a table joined to follow a link: the names that lead to it, such as `account` or `account.bank`.
    link: Option<String>,
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
        self.tables
            .iter()
            .filter(|scope| !scope.known && scope.link.is_none())
            .collect()
    }

    /// The columns a name can reach without its table's name.
    fn visible(&self) -> impl Iterator<Item = &Col> {
        self.cols.iter().filter(|col| !col.hidden)
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
    /// The query has window values, worked out after its `WHERE` and its grouping.
    windowed: bool,
    /// Where the current order was asked for, to warn when a grouping undoes it.
    sorted_at: Option<Span>,
}

impl Rel {
    /// Whether another table can be joined in this `SELECT`, rather than in a new one that reads it.
    fn joinable(&self) -> bool {
        !(self.grouped || self.distinct || self.limited || self.projected || self.windowed)
    }
}

/// The names a `def`'s body can use: its parameters.
type Bindings = HashMap<String, Typed>;

/// The columns a query gives: their names, types, and whether each can be missing.
type Columns = Vec<(String, Type, bool)>;

/// Where an expression is compiled.
#[derive(Clone, Copy)]
struct Cx<'f> {
    frame: &'f Frame,
    bindings: &'f Bindings,
    /// Totals such as `count()` can be used: in the block of a `group by` or a `summarize`.
    totals: bool,
    /// The expression is a total's argument, where another total can't be.
    in_total: bool,
    /// The expression is a condition that keeps the rows where it holds: a missing value and a false one keep the
    /// same rows there, so their truth is written plainly.
    condition: bool,
}

impl<'f> Cx<'f> {
    fn of(frame: &'f Frame, bindings: &'f Bindings) -> Cx<'f> {
        Cx {
            frame,
            bindings,
            totals: false,
            in_total: false,
            condition: false,
        }
    }

    /// The same place, for a condition.
    fn condition(self) -> Cx<'f> {
        Cx {
            condition: true,
            ..self
        }
    }
}

/// The names a module defines and imports.
#[derive(Default)]
struct Namespace<'a> {
    lets: HashMap<&'a str, &'a ast::Let>,
    defs: HashMap<&'a str, &'a ast::Def>,
    /// Modules imported whole, by the name they are used by: `shared`, `shared.dates`, or an `as` name.
    modules: HashMap<String, usize>,
    /// Names imported from a module, by the name they are used by: the module, the name there, and where it is
    /// imported.
    items: HashMap<String, (usize, String, Span)>,
}

/// What a name means in a module.
#[derive(Clone, Copy)]
enum Found<'a> {
    Let(usize, &'a ast::Let),
    Def(usize, &'a ast::Def),
    Module(usize),
}

/// A join to follow a link: from a column holding another table's key to that table.
struct LinkJoin {
    /// The column that holds the key.
    from: Sql,
    /// The table it points to, as the schema has it, and its column that holds the key.
    target: crate::schema::Table,
    key: String,
    /// The names that lead to the table, which name its scope.
    path: String,
}

pub(crate) struct Compiler<'a> {
    dialect: Dialect,
    schema: Option<&'a Schema>,
    modules: &'a [Loaded],
    spaces: Vec<Namespace<'a>>,
    /// The module whose code is being compiled.
    current: usize,
    /// Named values already compiled, by module and name.
    values: HashMap<(usize, String), Typed>,
    /// The names being compiled, to find one that uses itself.
    stack: Vec<(usize, String)>,
    aliases: usize,
    pub(crate) diagnostics: Vec<Diagnostic>,
    /// A step has failed, so a name it should have made is not reported as missing.
    poisoned: bool,
    /// The recursive queries being compiled, by name, with their columns: in its second part, a recursive query
    /// reads its own rows as a table.
    ctes: Vec<(String, Columns)>,
    /// Whether the schema knows every table, as one given does, rather than only the tables the file creates.
    complete: bool,
    /// The tables the file has dropped before the statement being compiled.
    gone: Vec<String>,
}

impl<'a> Compiler<'a> {
    pub(crate) fn new(
        modules: &'a [Loaded],
        dialect: Dialect,
        schema: Option<&'a Schema>,
    ) -> Compiler<'a> {
        let mut spaces = Vec::new();
        for module in modules {
            let mut space = Namespace::default();
            let mut imports = module.imports.iter();
            for item in &module.file.items {
                match item {
                    ast::Item::Let(value) => {
                        space.lets.insert(value.name.text.as_str(), value);
                    }
                    ast::Item::Def(def) => {
                        space.defs.insert(def.name.text.as_str(), def);
                    }
                    ast::Item::Import(import) => {
                        let read = imports.next().cloned().unwrap_or_default();
                        match &import.kind {
                            ImportKind::Module { path, alias } => {
                                if let Some(Some(index)) = read.first() {
                                    let name = alias.as_ref().map_or_else(
                                        || {
                                            path.parts
                                                .iter()
                                                .map(|part| part.text.as_str())
                                                .collect::<Vec<_>>()
                                                .join(".")
                                        },
                                        |alias| alias.text.clone(),
                                    );
                                    space.modules.insert(name, *index);
                                }
                            }
                            ImportKind::Names {
                                module: Some(_),
                                names,
                                ..
                            } => {
                                if let Some(Some(index)) = read.first() {
                                    for name in names {
                                        let used = name.alias.as_ref().unwrap_or(&name.name);
                                        space.items.insert(
                                            used.text.clone(),
                                            (*index, name.name.text.clone(), name.name.span),
                                        );
                                    }
                                }
                            }
                            ImportKind::Names {
                                module: None,
                                names,
                                ..
                            } => {
                                for (name, index) in names.iter().zip(&read) {
                                    if let Some(index) = index {
                                        let used = name.alias.as_ref().unwrap_or(&name.name);
                                        space.modules.insert(used.text.clone(), *index);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            spaces.push(space);
        }
        let mut compiler = Compiler {
            dialect,
            schema,
            modules,
            spaces,
            current: 0,
            values: HashMap::new(),
            stack: Vec::new(),
            aliases: 0,
            diagnostics: Vec::new(),
            poisoned: false,
            ctes: Vec::new(),
            complete: true,
            gone: Vec::new(),
        };
        compiler.check_imports();
        compiler
    }

    /// Reports each name imported from a module that does not define it.
    /// Knows that the schema holds only the tables the file creates, so a table it does not hold may still be.
    pub(crate) fn partial(mut self) -> Self {
        self.complete = false;
        self
    }

    /// Knows the tables the file has dropped, which statements after it can't read.
    pub(crate) fn without(mut self, gone: &[String]) -> Self {
        self.gone = gone.to_vec();
        self
    }

    fn check_imports(&mut self) {
        for module in 0..self.spaces.len() {
            let mut items: Vec<(usize, String, Span)> =
                self.spaces[module].items.values().cloned().collect();
            items.sort_by_key(|(_, _, span)| span.start);
            for (from, name, span) in items {
                if self.find(from, &name).is_some() {
                    continue;
                }
                let space = &self.spaces[from];
                let names = space
                    .lets
                    .keys()
                    .chain(space.defs.keys())
                    .copied()
                    .collect::<Vec<_>>();
                let mut problem = Diagnostic::error(
                    span,
                    format!("`{}` has no `{name}`", self.modules[from].source.name),
                );
                if let Some(best) = suggest::closest(&name, names) {
                    problem = problem.with_label(format!("did you mean `{best}`?"));
                }
                self.within(module, |compiler| compiler.report(problem));
            }
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(span, message)
    }

    /// Reports a problem in the module being compiled.
    fn report(&mut self, mut diagnostic: Diagnostic) {
        if self.current != 0 && diagnostic.file.is_none() {
            diagnostic.file = Some(self.modules[self.current].source.clone());
        }
        self.diagnostics.push(diagnostic);
    }

    /// Compiles `work` as code of `module`, where its names mean what that module says.
    fn within<T>(&mut self, module: usize, work: impl FnOnce(&mut Self) -> T) -> T {
        let outer = std::mem::replace(&mut self.current, module);
        let result = work(self);
        self.current = outer;
        result
    }

    /// What a name means in a module: a named value or query, a `def`, or a module imported whole.
    fn find(&self, module: usize, name: &str) -> Option<Found<'a>> {
        self.find_in(module, name, 0)
    }

    fn find_in(&self, module: usize, name: &str, depth: usize) -> Option<Found<'a>> {
        let space = &self.spaces[module];
        if let Some(value) = space.lets.get(name) {
            return Some(Found::Let(module, value));
        }
        if let Some(def) = space.defs.get(name) {
            return Some(Found::Def(module, def));
        }
        if let Some(index) = space.modules.get(name) {
            return Some(Found::Module(*index));
        }
        match space.items.get(name) {
            Some((from, original, _)) if depth < 32 => self.find_in(*from, original, depth + 1),
            _ => None,
        }
    }

    /// What a name or a dotted name means in the module being compiled: `recent`, `shared.recent`.
    fn find_path(&self, expr: &ast::Expr) -> Option<Found<'a>> {
        let parts = names_of(expr)?;
        for split in (1..=parts.len()).rev() {
            let Some(mut found) = self.find(self.current, &parts[..split].join(".")) else {
                continue;
            };
            for part in &parts[split..] {
                found = match found {
                    Found::Module(index) => self.find(index, part)?,
                    _ => return None,
                };
            }
            return Some(found);
        }
        None
    }

    /// The named query a source names: `open_cases`, or `shared.open_cases`.
    fn named_query(&self, expr: &ast::Expr) -> Option<(usize, &'a ast::Query)> {
        match self.find_path(expr)? {
            Found::Let(module, value) => match &value.value.kind {
                ExprKind::Query(query) => Some((module, query)),
                _ => None,
            },
            _ => None,
        }
    }

    /// The named queries combined that a source names: `reached = (from ...) | (from ...)`.
    fn named_compound(&self, expr: &ast::Expr) -> Option<(usize, &'a ast::Let)> {
        match self.find_path(expr)? {
            Found::Let(module, value) if change::is_compound(&value.value) => Some((module, value)),
            _ => None,
        }
    }

    /// The `def` a call names: `recent`, or `shared.recent`.
    fn def(&self, expr: &ast::Expr) -> Option<(usize, &'a ast::Def)> {
        match self.find_path(expr)? {
            Found::Def(module, def) => Some((module, def)),
            _ => None,
        }
    }

    /// The names of the `def`s the module being compiled can call, for suggestions.
    fn def_names(&self) -> Vec<String> {
        let space = &self.spaces[self.current];
        let mut names: Vec<String> = space.defs.keys().map(|name| name.to_string()).collect();
        names.extend(
            space
                .items
                .keys()
                .filter(|name| matches!(self.find(self.current, name), Some(Found::Def(..))))
                .cloned(),
        );
        names
    }

    // ---- What is compiled

    /// A named query or a query `def` of the file, compiled. A `def`'s parameters become placeholders, filled when
    /// the query runs.
    pub(crate) fn named(&mut self, name: &str) -> Option<(crate::sql::Statement, Vec<Arg>)> {
        let found = self.find(0, name);
        let problem = match found {
            Some(Found::Let(module, value)) => match &value.value.kind {
                ExprKind::Query(query) => {
                    let statement = self.within(module, |compiler| {
                        compiler.statement(&query.steps, &Bindings::new())
                    })?;
                    return Some((statement, Vec::new()));
                }
                _ if change::is_compound(&value.value) => {
                    let own = value.name.text.as_str();
                    let rel = self.within(module, |compiler| {
                        compiler.compound(&value.value, Some(own), &Bindings::new())
                    })?;
                    return Some((crate::sql::Statement::Select(self.finish(rel)), Vec::new()));
                }
                _ => {
                    Diagnostic::error(value.name.span, format!("`{name}` is a value, not a query"))
                }
            },
            Some(Found::Def(module, def)) if def.is_query() => {
                let (bindings, args) = self.within(module, |compiler| compiler.open_args(def))?;
                let statement =
                    self.within(module, |compiler| compiler.statement(&def.body, &bindings))?;
                return Some((statement, args));
            }
            Some(Found::Def(_, def)) => {
                Diagnostic::error(def.name.span, format!("`{name}` holds steps, not a query"))
                    .with_help("a `def` is a query when its first step is `from`")
            }
            Some(Found::Module(_)) => Diagnostic::error(
                Span::default(),
                format!("`{name}` is a module, not a query"),
            ),
            None => {
                let space = &self.spaces[0];
                let queries: Vec<&str> = space
                    .lets
                    .iter()
                    .filter(|(_, value)| matches!(value.value.kind, ExprKind::Query(_)))
                    .map(|(name, _)| *name)
                    .chain(
                        space
                            .defs
                            .iter()
                            .filter(|(_, def)| def.is_query())
                            .map(|(name, _)| *name),
                    )
                    .collect();
                let mut problem =
                    Diagnostic::error(Span::default(), format!("there is no query named `{name}`"));
                if let Some(best) = suggest::closest(name, queries.iter().copied()) {
                    problem = problem.with_label(format!("did you mean `{best}`?"));
                } else if !queries.is_empty() {
                    problem = problem.with_help(format!("the queries here are {}", list(&queries)));
                }
                problem
            }
        };
        self.report(problem);
        None
    }

    /// The parameters of a query `def` left open, as placeholders, with what is known of each.
    fn open_args(&mut self, def: &'a ast::Def) -> Option<(Bindings, Vec<Arg>)> {
        let mut bindings = Bindings::new();
        let mut args = Vec::new();
        let mut ok = true;
        for param in &def.params {
            let ty = annotation_type(param);
            let default = match &param.default {
                None => None,
                Some(default) => {
                    match literal_value(default) {
                        Some(value) => Some(value),
                        None => {
                            let problem = self
                            .error(default.span, "a default is written as a value")
                            .with_help("for example: `days_back: int = 30`, or `status: str = \"open\"`");
                            self.report(problem);
                            ok = false;
                            continue;
                        }
                    }
                }
            };
            let mut value =
                Typed::new(Sql::Param(Param::Arg(param.name.text.clone()), ty), ty).nullable(true);
            value.reads = false;
            bindings.insert(param.name.text.clone(), value);
            args.push(Arg {
                name: param.name.text.clone(),
                ty,
                default,
            });
        }
        ok.then_some((bindings, args))
    }

    /// The finished statement: its columns written out, and ClickHouse's settings for what it needs.
    fn finish(&self, rel: Rel) -> Select {
        let mut select = self.output(rel);
        if self.dialect == Dialect::ClickHouse && select.has_left_join() {
            // ClickHouse fills a row with no match with empty values, not NULL, unless told to.
            select.settings.push(("join_use_nulls", "1"));
        }
        select
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
        let rel = self.links(rel, step, bindings);
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
            StepKind::Pivot { by, expanded, .. } => {
                let by = (!by.is_empty()).then_some(by.as_slice());
                self.group(rel, by, expanded, bindings)
            }
            StepKind::Update(_) | StepKind::Delete | StepKind::Insert(_) => {
                let problem = self
                    .error(step.span, "a query that changes rows can't be read as rows")
                    .with_help("a `def` that changes rows is called on a line of its own: `close_stale(90)`");
                self.report(problem);
                self.poisoned = true;
                rel
            }
        }
    }

    fn alias(&mut self) -> String {
        self.aliases += 1;
        format!("_q{}", self.aliases)
    }

    /// The `SELECT` with its columns written out.
    fn output(&self, rel: Rel) -> Select {
        let mut select = rel.select;
        let hidden = rel.frame.tables.iter().any(|scope| scope.link.is_some());
        select.columns = if rel.frame.star {
            let extra: Vec<Item> = rel
                .frame
                .cols
                .iter()
                .filter(|col| !col.base && !col.hidden)
                .map(|col| Item::Expr {
                    expr: col.value.sql.clone(),
                    alias: Some(col.name.clone()),
                })
                .collect();
            // With tables joined to follow links, `*` would bring their columns out too.
            let stars: Vec<Item> = if hidden {
                rel.frame
                    .tables
                    .iter()
                    .filter(|scope| scope.link.is_none())
                    .map(|scope| Item::TableStar(scope.alias.clone()))
                    .collect()
            } else {
                vec![Item::Star]
            };
            if extra.is_empty() && !hidden {
                Vec::new()
            } else {
                stars.into_iter().chain(extra).collect()
            }
        } else {
            rel.frame
                .visible()
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
        let cols: Vec<Col> = rel.frame.visible().cloned().collect();
        let outer_cols: Vec<Col> = cols
            .iter()
            .map(|col| {
                let mut value =
                    Typed::column(Some(&alias), &col.name, col.value.ty, col.value.nullable);
                value.reference = col.value.reference.clone();
                Col {
                    name: col.name.clone(),
                    value,
                    table: Some(alias.clone()),
                    base: true,
                    hidden: false,
                }
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
                tables: vec![Scope {
                    alias,
                    known,
                    link: None,
                }],
                star: true,
            },
            grouped: false,
            distinct: false,
            limited: false,
            projected: false,
            windowed: false,
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
            ExprKind::Query(_) | ExprKind::Call(..) | ExprKind::List(_) => true,
            ExprKind::Name(name) if bindings.contains_key(name) => false,
            ExprKind::Name(name) if self.ctes.iter().any(|(cte, _)| cte == name) => false,
            ExprKind::Name(_) | ExprKind::Attribute(..) => {
                self.named_query(value).is_some()
                    || self.named_compound(value).is_some()
                    || self.def(value).is_some()
            }
            _ => false,
        }
    }

    /// The relation of a query source.
    fn relation(&mut self, value: &'a ast::Expr, bindings: &Bindings) -> Option<Rel> {
        if let ExprKind::Query(query) = &value.kind {
            return self.query(&query.steps, bindings);
        }
        if let ExprKind::List(rows) = &value.kind {
            return self.rows(rows, value.span);
        }
        if let Some((module, named)) = self.named_compound(value) {
            let name = path_of(value).join(".");
            self.enter(module, &name, value.span)?;
            let own = named.name.text.as_str();
            let rel = self.within(module, |compiler| {
                compiler.compound(&named.value, Some(own), &Bindings::new())
            });
            self.stack.pop();
            return rel;
        }
        if let Some((module, query)) = self.named_query(value) {
            let name = path_of(value).join(".");
            self.enter(module, &name, value.span)?;
            let rel = self.within(module, |compiler| {
                compiler.query(&query.steps, &Bindings::new())
            });
            self.stack.pop();
            return rel;
        }
        let (function, args): (&'a ast::Expr, &'a [ast::Arg]) = match &value.kind {
            ExprKind::Call(function, args) => (function, args),
            _ => (value, &[]),
        };
        let Some((module, def)) = self.def(function) else {
            let name = path_of(function).join(".");
            let mut problem = self.error(
                function.span,
                format!("there is no query `def` named `{name}`"),
            );
            let names = self.def_names();
            if let Some(best) = suggest::closest(&name, names.iter().map(String::as_str)) {
                problem = problem.with_label(format!("did you mean `{best}`?"));
            }
            self.report(problem);
            return None;
        };
        self.query_def(module, def, args, value.span, bindings)
    }

    /// The relation, read under a name of its own: `from (from cases) as c`.
    fn rename(&mut self, rel: Rel, alias: &str) -> Rel {
        let mut rel = self.wrap(rel);
        let old = rel.frame.tables[0].alias.clone();
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

    /// Marks a named query or a `def` as being compiled, refusing one that uses itself.
    fn enter(&mut self, module: usize, name: &str, span: Span) -> Option<()> {
        if self
            .stack
            .iter()
            .any(|(entered, entered_name)| *entered == module && entered_name == name)
        {
            let problem = self
                .error(span, format!("`{name}` uses itself"))
                .with_help("a named query or a `def` can't use itself, directly or through others");
            self.report(problem);
            return None;
        }
        self.stack.push((module, name.to_string()));
        Some(())
    }

    /// A table: what the schema knows of it, or nothing.
    fn table(&mut self, source: &ast::Source) -> Rel {
        if let ExprKind::Name(name) = &source.value.kind {
            if let Some(rel) = self.cte_table(name, source) {
                return rel;
            }
        }
        let path = path_of(&source.value);
        let written = path.join(".");
        let (path, table) = match self.schema {
            Some(schema) => match schema.table(&written) {
                Some(table) => (
                    table.name.split('.').map(str::to_string).collect(),
                    Some(table),
                ),
                None if !self.complete
                    && !self
                        .gone
                        .iter()
                        .any(|gone| gone.eq_ignore_ascii_case(&written)) =>
                {
                    (path, None)
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
                    (path, None)
                }
            },
            None => (path, None),
        };
        let alias = source.alias.as_ref().map_or_else(
            || path.last().cloned().unwrap_or_default(),
            |alias| alias.text.clone(),
        );
        let cols = table.map_or_else(Vec::new, |table| table_cols(table, &alias, false));
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
                tables: vec![Scope {
                    alias,
                    known: table.is_some(),
                    link: None,
                }],
                star: true,
            },
            grouped: false,
            distinct: false,
            limited: false,
            projected: false,
            windowed: false,
            sorted_at: None,
        }
    }

    // ---- Links

    /// Joins the tables a step's links lead to, before the step reads them: `account.operator` reads the account a
    /// case's `account_id` points to.
    fn links(&mut self, mut rel: Rel, step: &'a ast::Step, bindings: &Bindings) -> Rel {
        if self.schema.is_none() {
            return rel;
        }
        let mut bases = Vec::new();
        for expr in step_exprs(step) {
            attribute_bases(expr, &mut bases);
        }
        if bases.is_empty() {
            return rel;
        }
        let planned = |compiler: &Self, rel: &Rel| {
            let mut joins: Vec<LinkJoin> = Vec::new();
            for base in &bases {
                for join in compiler.link_plan(&rel.frame, base, bindings) {
                    let known = joins.iter().any(|other| other.path == join.path)
                        || rel
                            .frame
                            .tables
                            .iter()
                            .any(|scope| scope.link.as_deref() == Some(&join.path));
                    if !known {
                        joins.push(join);
                    }
                }
            }
            joins
        };
        let mut joins = planned(self, &rel);
        if joins.is_empty() {
            return rel;
        }
        if !rel.joinable() {
            rel = self.wrap(rel);
            joins = planned(self, &rel);
        }
        for join in joins {
            let mut alias = join.path.replace('.', "_");
            while rel.frame.tables.iter().any(|scope| scope.alias == alias) {
                alias.push('_');
            }
            let on = Sql::binary(Binary::Eq, join.from, Sql::column(Some(&alias), &join.key));
            rel.select.joins.push(Join {
                left: true,
                source: Source {
                    kind: SourceKind::Table(
                        join.target.name.split('.').map(str::to_string).collect(),
                    ),
                    alias: Some(alias.clone()),
                },
                on,
            });
            rel.frame
                .cols
                .extend(table_cols(&join.target, &alias, true));
            rel.frame.tables.push(Scope {
                alias,
                known: true,
                link: Some(join.path),
            });
        }
        rel
    }

    /// The joins that following a dotted name's links needs: for `account.bank`, the account a row points to and
    /// the bank the account points to. A name that is not a link ends the plan.
    fn link_plan(&self, frame: &Frame, names: &[String], bindings: &Bindings) -> Vec<LinkJoin> {
        let Some(schema) = self.schema else {
            return Vec::new();
        };
        let Some(first) = names.first() else {
            return Vec::new();
        };
        if bindings.contains_key(first) || self.find(self.current, first).is_some() {
            return Vec::new();
        }
        // `cases.account` starts from the table `cases`; `account` from any table that has the link.
        let table_named = frame
            .tables
            .iter()
            .any(|scope| scope.link.is_none() && &scope.alias == first);
        let (mut path, links) = if table_named && names.len() > 1 {
            (first.clone(), &names[1..])
        } else {
            (String::new(), names)
        };
        let mut joins = Vec::new();
        // The columns the next link is looked for in: at first the query's, then each linked table's.
        let mut from_alias: Option<String> = table_named
            .then(|| first.clone())
            .filter(|_| names.len() > 1);
        let mut from_table: Option<&crate::schema::Table> = None;
        for link in links {
            let found: Option<(Sql, Reference)> = match from_table {
                None => {
                    let candidates: Vec<&Col> = frame
                        .cols
                        .iter()
                        .filter(|col| !col.hidden)
                        .filter(|col| from_alias.is_none() || col.table == from_alias)
                        .filter(|col| col.value.reference.is_some() && link_name(&col.name) == link)
                        .collect();
                    match candidates.as_slice() {
                        [col] => col
                            .value
                            .reference
                            .clone()
                            .map(|reference| (col.value.sql.clone(), reference)),
                        _ => None,
                    }
                }
                Some(table) => table
                    .columns
                    .iter()
                    .find(|column| column.link() == Some(link.as_str()))
                    .and_then(|column| {
                        let alias = path.replace('.', "_");
                        column
                            .references
                            .clone()
                            .map(|reference| (Sql::column(Some(&alias), &column.name), reference))
                    }),
            };
            let Some((from, reference)) = found else {
                break;
            };
            let Some(target) = schema.table(&reference.table) else {
                break;
            };
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(link);
            joins.push(LinkJoin {
                from,
                target: target.clone(),
                key: reference.column.clone(),
                path: path.clone(),
            });
            from_table = Some(target);
            from_alias = None;
        }
        joins
    }

    // ---- The steps

    fn filter(&mut self, mut rel: Rel, condition: &'a ast::Expr, bindings: &Bindings) -> Rel {
        if rel.limited || (rel.grouped && rel.select.group.is_empty()) {
            rel = self.wrap(rel);
        }
        let qualify = self.dialect == Dialect::DuckDb;
        if rel.windowed && !qualify {
            // Window values are worked out after `WHERE`, so a condition on the rows they make reads them from a
            // query of its own.
            rel = self.wrap(rel);
        }
        let Some(value) = self.expr(condition, Cx::of(&rel.frame, bindings).condition()) else {
            self.poisoned = true;
            return rel;
        };
        if value.window && !qualify {
            let problem = self
                .error(condition.span, "a window value in a condition needs a name of its own")
                .with_help("name it first, as in `place = rank(sort=-risk)`, then keep the rows with `where place <= 3`");
            self.report(problem);
            return rel;
        }
        let sql = self.truth(value.clone());
        if qualify && (rel.windowed || value.window) {
            rel.select.qualify.push(sql);
        } else if rel.grouped {
            rel.select.having.push(sql);
        } else {
            rel.select.filter.push(sql);
        }
        rel
    }

    /// Compiles a value for a new column, starting a new `SELECT` first when the value is a window value that
    /// must count only the rows the steps before kept.
    fn new_value(
        &mut self,
        mut rel: Rel,
        value: &'a ast::Expr,
        bindings: &Bindings,
    ) -> (Rel, Option<Typed>) {
        let mark = self.diagnostics.len();
        let typed = self.expr(value, Cx::of(&rel.frame, bindings));
        let rewrap = typed.as_ref().is_some_and(|typed| {
            typed.fresh && (rel.limited || rel.distinct || !rel.select.qualify.is_empty())
        });
        if !rewrap {
            return (rel, typed);
        }
        self.diagnostics.truncate(mark);
        rel = self.wrap(rel);
        let typed = self.expr(value, Cx::of(&rel.frame, bindings));
        (rel, typed)
    }

    /// Compiles a value only to learn what it is, keeping no problem it finds: the real compile reports them.
    fn trial(&mut self, value: &'a ast::Expr, frame: &Frame, bindings: &Bindings) -> Option<Typed> {
        let mark = self.diagnostics.len();
        let poisoned = self.poisoned;
        let typed = self.expr(value, Cx::of(frame, bindings));
        self.diagnostics.truncate(mark);
        self.poisoned = poisoned;
        typed
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
        let (mut rel, typed) = self.new_value(rel, value, bindings);
        let Some(typed) = typed else {
            self.poisoned = true;
            rel.frame.cols.push(Col {
                name: name.text.clone(),
                value: Typed::new(Sql::Keyword("NULL"), Type::Unknown).nullable(true),
                table: None,
                base: false,
                hidden: false,
            });
            return rel;
        };
        if !self.storable(&typed, value.span) {
            return rel;
        }
        rel.windowed |= typed.window;
        let col = Col {
            name: name.text.clone(),
            value: typed,
            table: None,
            base: false,
            hidden: false,
        };
        match rel
            .frame
            .cols
            .iter()
            .position(|old| old.name == name.text && !old.hidden)
        {
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
        // A window value counts the rows the steps before kept, so it is worked out after them.
        if rel.limited || !rel.select.qualify.is_empty() {
            let frame = rel.frame.clone();
            let windowed = fields.iter().any(|field| {
                self.trial(&field.value, &frame, bindings)
                    .is_some_and(|value| value.fresh)
            });
            if windowed {
                rel = self.wrap(rel);
            }
        }
        let mut cols: Vec<Col> = Vec::new();
        for field in fields {
            let Some(value) = self.expr(&field.value, Cx::of(&rel.frame, bindings)) else {
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
            rel.windowed |= value.window;
            let table = match &value.sql {
                Sql::Column { table, .. } => table.clone(),
                _ => None,
            };
            cols.push(Col {
                name,
                value,
                table,
                base: false,
                hidden: false,
            });
        }
        // The columns a link reached stay, hidden, for the rows' order and for later steps that read them.
        let hidden: Vec<Col> = rel
            .frame
            .cols
            .iter()
            .filter(|col| col.hidden)
            .cloned()
            .collect();
        rel.frame.cols = cols;
        rel.frame.cols.extend(hidden);
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
            let Some(value) = self.expr(column, Cx::of(&rel.frame, bindings)) else {
                continue;
            };
            let before = rel.frame.cols.len();
            rel.frame
                .cols
                .retain(|col| col.hidden || col.value.sql != value.sql);
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
            let Some(value) = self.expr(&key.value, Cx::of(&rel.frame, bindings)) else {
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
                .any(|key| !rel.frame.visible().any(|col| col.value.sql == key.expr))
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
                Dialect::Sqlite => !descending,
                // DuckDB and ClickHouse put missing values last both ways.
                Dialect::DuckDb | Dialect::ClickHouse => false,
            }
    }

    fn take(&mut self, mut rel: Rel, count: &'a ast::Expr, skip: bool, bindings: &Bindings) -> Rel {
        let Some(count) = self.count(count, bindings) else {
            return rel;
        };
        let given = |count: &Option<Count>| matches!(count, Some(Count::Param(_)));
        match count {
            Count::Rows(rows) if skip => {
                if given(&rel.select.limit) || given(&rel.select.offset) {
                    rel = self.wrap(rel);
                }
                let offset = match rel.select.offset {
                    Some(Count::Rows(offset)) => offset + rows,
                    _ => rows,
                };
                rel.select.offset = Some(Count::Rows(offset));
                if let Some(Count::Rows(limit)) = rel.select.limit {
                    rel.select.limit = Some(Count::Rows(limit.saturating_sub(rows)));
                }
            }
            Count::Rows(rows) => {
                if given(&rel.select.limit) {
                    rel = self.wrap(rel);
                }
                let limit = match rel.select.limit {
                    Some(Count::Rows(limit)) => limit.min(rows),
                    _ => rows,
                };
                rel.select.limit = Some(Count::Rows(limit));
            }
            Count::Param(param) => {
                // A count given when the query runs can't be added to another before then.
                if rel.select.limit.is_some() || rel.select.offset.is_some() {
                    rel = self.wrap(rel);
                }
                if skip {
                    rel.select.offset = Some(Count::Param(param));
                } else {
                    rel.select.limit = Some(Count::Param(param));
                }
            }
        }
        rel.limited = true;
        rel
    }

    /// A count of rows, for `take` and `skip`: a whole number, written or named, or a query's argument.
    fn count(&mut self, value: &'a ast::Expr, bindings: &Bindings) -> Option<Count> {
        let frame = Frame::default();
        let poisoned = std::mem::replace(&mut self.poisoned, false);
        let typed = self.expr(value, Cx::of(&frame, bindings));
        self.poisoned = poisoned;
        match &typed {
            Some(Typed {
                sql: Sql::Number(digits),
                ty: Type::Int,
                ..
            }) => {
                if let Ok(count) = digits.parse::<u64>() {
                    return Some(Count::Rows(count));
                }
            }
            Some(Typed {
                sql: Sql::Param(param @ Param::Arg(_), _),
                ty: Type::Int | Type::Unknown,
                ..
            }) => return Some(Count::Param(param.clone())),
            _ => {}
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
            .any(|key| !rel.frame.visible().any(|col| col.value.sql == key.expr))
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
        if rel.grouped || rel.distinct || rel.limited || rel.windowed {
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
            let Some(value) = self.expr(&field.value, Cx::of(&rel.frame, bindings)) else {
                self.poisoned = true;
                continue;
            };
            let Some(name) = self.field_name(field, "group") else {
                continue;
            };
            if value.window {
                let problem = self
                    .error(field.value.span, "a group can't be made from a window value written here")
                    .with_help("name the window value with a step of its own first, then group by its name");
                self.report(problem);
                continue;
            }
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
                hidden: false,
            });
        }
        let keys = cols.len();
        for total in totals {
            // A total can use the totals before it: `rate = big / n`.
            let mut frame = rel.frame.clone();
            frame.cols.extend(cols[keys..].iter().cloned());
            let cx = Cx {
                totals: true,
                ..Cx::of(&frame, bindings)
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
                hidden: false,
            });
        }
        rel.frame = Frame {
            cols,
            tables: rel
                .frame
                .tables
                .into_iter()
                .filter(|scope| scope.link.is_none())
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
        if !rel.joinable() {
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
        let Some(condition) = self.expr(on, Cx::of(&rel.frame, bindings).condition()) else {
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
            (None, ExprKind::Call(function, _)) => path_of(function).join("_"),
            (None, ExprKind::Query(_)) => {
                let problem = self
                    .error(
                        source.value.span,
                        "a query in brackets that is joined needs a name",
                    )
                    .with_help("name it with `as`, as in `join (from accounts) as a on ...`");
                self.report(problem);
                return None;
            }
            (None, _) => path_of(&source.value).join("_"),
        };
        let rel = self.relation(&source.value, bindings)?;
        let known = rel.frame.open().is_empty() || !rel.frame.star;
        let cols: Vec<Col> = rel
            .frame
            .visible()
            .map(|col| {
                let mut value =
                    Typed::column(Some(&alias), &col.name, col.value.ty, col.value.nullable);
                value.reference = col.value.reference.clone();
                Col {
                    name: col.name.clone(),
                    value,
                    table: Some(alias.clone()),
                    base: true,
                    hidden: false,
                }
            })
            .collect();
        let select = self.output(rel);
        let joined = Source {
            kind: SourceKind::Query(Box::new(select)),
            alias: Some(alias.clone()),
        };
        Some((
            joined,
            Scope {
                alias,
                known,
                link: None,
            },
            cols,
        ))
    }

    // ---- Defs

    /// `recent(30)`: the steps of a `def`, put in place.
    fn call_steps(&mut self, mut rel: Rel, call: &'a ast::Expr, bindings: &Bindings) -> Rel {
        let ExprKind::Call(function, args) = &call.kind else {
            return rel;
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
        let Some(bound) = self.bind(module, def, args, call.span, Cx::of(&rel.frame, bindings))
        else {
            self.poisoned = true;
            return rel;
        };
        if self.enter(module, &def.name.text, call.span).is_none() {
            return rel;
        }
        rel = self.within(module, |compiler| {
            for step in &def.body {
                rel = compiler.step(rel, step, &bound);
            }
            rel
        });
        self.stack.pop();
        rel
    }

    /// A query `def`, called with its arguments: `from cases_for("A1")`.
    fn query_def(
        &mut self,
        module: usize,
        def: &'a ast::Def,
        args: &'a [ast::Arg],
        span: Span,
        bindings: &Bindings,
    ) -> Option<Rel> {
        let name = &def.name.text;
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
        let bound = self.bind(module, def, args, span, Cx::of(&frame, bindings))?;
        self.enter(module, name, span)?;
        let rel = self.within(module, |compiler| compiler.query(&def.body, &bound));
        self.stack.pop();
        rel
    }

    /// The values of a `def`'s parameters for one call, compiled where the call is.
    fn bind(
        &mut self,
        module: usize,
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
                    // A default is the `def`'s own: it means what its module says.
                    let frame = Frame::default();
                    let empty = Bindings::new();
                    let value = self.within(module, |compiler| {
                        compiler.argument(param, default, Cx::of(&frame, &empty))
                    });
                    match value {
                        Some(value) => {
                            bound.insert(param.name.text.clone(), value);
                        }
                        None => ok = false,
                    }
                }
                None => {
                    let mut problem = self.error(
                        span,
                        format!("`{name}` needs a value for `{}`", param.name.text),
                    );
                    if module == self.current {
                        problem = problem.with_related(param.span, "the parameter");
                    }
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
        if param.annotation.is_some() {
            let wanted = annotation_type(param);
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
}

/// A table's columns, as the query reads them under `alias`.
fn table_cols(table: &crate::schema::Table, alias: &str, hidden: bool) -> Vec<Col> {
    table
        .columns
        .iter()
        .map(|column| {
            // A table joined to follow a link may have no row for a given one, so its columns can be missing.
            let mut value = Typed::column(
                Some(alias),
                &column.name,
                column.ty,
                column.nullable || hidden,
            );
            value.reference = column.references.clone();
            Col {
                name: column.name.clone(),
                value,
                table: Some(alias.to_string()),
                base: !hidden,
                hidden,
            }
        })
        .collect()
}

/// The type a parameter's annotation names: `str`, `int`, `float`, `bool`, `date` or `datetime`.
fn annotation_type(param: &ast::Param) -> Type {
    let name = param
        .annotation
        .as_ref()
        .and_then(|annotation| annotation.name.parts.last())
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
}

/// A value written in the query: a string, a number, `True`, `False` or `None`.
fn literal_value(expr: &ast::Expr) -> Option<Value> {
    match &expr.kind {
        ExprKind::Str(text) => Some(Value::Text(text.clone())),
        ExprKind::Bool(value) => Some(Value::Bool(*value)),
        ExprKind::None => Some(Value::Null),
        ExprKind::Number(number) if number.float => number.text.parse().ok().map(Value::Float),
        ExprKind::Number(number) => number.text.parse().ok().map(Value::Int),
        ExprKind::Unary(ast::UnaryOp::Neg, operand) => match literal_value(operand)? {
            Value::Int(value) => Some(Value::Int(-value)),
            Value::Float(value) => Some(Value::Float(-value)),
            _ => None,
        },
        _ => None,
    }
}

/// The name a query follows a column's reference by: `account_id` is followed as `account`.
fn link_name(column: &str) -> &str {
    column
        .strip_suffix("_id")
        .filter(|stem| !stem.is_empty())
        .unwrap_or(column)
}

/// The expressions a step reads.
fn step_exprs(step: &ast::Step) -> Vec<&ast::Expr> {
    match &step.kind {
        StepKind::Where(expr)
        | StepKind::Take(expr)
        | StepKind::Skip(expr)
        | StepKind::Call(expr) => vec![expr],
        StepKind::Column { value, .. } => vec![value],
        StepKind::Select(fields) | StepKind::Summarize(fields) => {
            fields.iter().map(|field| &field.value).collect()
        }
        StepKind::Group { by, totals } => {
            by.iter().chain(totals).map(|field| &field.value).collect()
        }
        StepKind::Del(columns) => columns.iter().collect(),
        StepKind::Sort { keys, .. } => keys.iter().map(|key| &key.value).collect(),
        StepKind::Update(fields) => fields.iter().map(|field| &field.value).collect(),
        StepKind::Pivot {
            column,
            by,
            expanded,
            ..
        } => std::iter::once(column)
            .chain(by.iter().chain(expanded).map(|field| &field.value))
            .collect(),
        StepKind::From(_)
        | StepKind::Join { .. }
        | StepKind::Distinct
        | StepKind::Pass
        | StepKind::Delete
        | StepKind::Insert(_) => Vec::new(),
    }
}

/// The dotted names before a dot anywhere in an expression: for `account.bank.name`, `account.bank` and `account`.
fn attribute_bases(expr: &ast::Expr, out: &mut Vec<Vec<String>>) {
    match &expr.kind {
        ExprKind::Attribute(base, _) => {
            if let Some(names) = names_of(base) {
                if !out.contains(&names) {
                    out.push(names);
                }
            }
            attribute_bases(base, out);
        }
        ExprKind::Call(function, args) => {
            attribute_bases(function, out);
            for arg in args {
                attribute_bases(&arg.value, out);
            }
        }
        ExprKind::FString(parts) => {
            for part in parts {
                if let ast::FPart::Value(value) = part {
                    attribute_bases(value, out);
                }
            }
        }
        ExprKind::Unary(_, operand) => attribute_bases(operand, out),
        ExprKind::Binary(_, left, right) => {
            attribute_bases(left, out);
            attribute_bases(right, out);
        }
        ExprKind::Compare(first, rest) => {
            attribute_bases(first, out);
            for (_, value) in rest {
                attribute_bases(value, out);
            }
        }
        ExprKind::IfElse {
            test,
            then,
            otherwise,
        } => {
            attribute_bases(test, out);
            attribute_bases(then, out);
            attribute_bases(otherwise, out);
        }
        ExprKind::List(items) | ExprKind::Tuple(items) | ExprKind::Set(items) => {
            for item in items {
                attribute_bases(item, out);
            }
        }
        _ => {}
    }
}

/// The names of a name or a dotted name: `shared.recent` is `shared` and `recent`.
fn names_of(expr: &ast::Expr) -> Option<Vec<String>> {
    match &expr.kind {
        ExprKind::Name(name) => Some(vec![name.clone()]),
        ExprKind::Attribute(base, name) => {
            let mut names = names_of(base)?;
            names.push(name.text.clone());
            Some(names)
        }
        _ => None,
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
            .any(|part| matches!(part, ast::FPart::Value(value) if mentions(value, name))),
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

/// Names in a list: "`a`, `b` and `c`".
fn list(names: &[&str]) -> String {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    match quoted.as_slice() {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}
