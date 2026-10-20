//! Statements: queries that give rows and queries that change them, and queries combined with `+`, `-`, `|` and
//! `&`, recursive ones among them.

use super::*;
use crate::sql::{Cte, Rows, SetOp, Statement};

/// Whether a value is queries combined: `(from a) + (from b)`.
pub(crate) fn is_compound(expr: &ast::Expr) -> bool {
    fn query(expr: &ast::Expr) -> bool {
        match &expr.kind {
            ExprKind::Query(_) => true,
            ExprKind::Binary(op, left, right) if set_op(*op).is_some() => {
                query(left) && query(right)
            }
            _ => false,
        }
    }
    matches!(&expr.kind, ExprKind::Binary(op, _, _) if set_op(*op).is_some()) && query(expr)
}

fn set_op(op: ast::BinaryOp) -> Option<SetOp> {
    match op {
        ast::BinaryOp::Add => Some(SetOp::UnionAll),
        ast::BinaryOp::BitOr => Some(SetOp::Union),
        ast::BinaryOp::BitAnd => Some(SetOp::Intersect),
        ast::BinaryOp::Sub => Some(SetOp::Except),
        _ => None,
    }
}

/// Whether a query, or queries combined, reads a name with `from` or `join`.
fn reads(expr: &ast::Expr, name: &str) -> bool {
    match &expr.kind {
        ExprKind::Query(query) => query.steps.iter().any(|step| match &step.kind {
            StepKind::From(source) | StepKind::Join { source, .. } => {
                matches!(&source.value.kind, ExprKind::Name(read) if read == name)
                    || reads(&source.value, name)
            }
            _ => false,
        }),
        ExprKind::Binary(_, left, right) => reads(left, name) || reads(right, name),
        _ => false,
    }
}

impl<'a> Compiler<'a> {
    /// A statement of the file, outside any `def`.
    pub(crate) fn top_statement(&mut self, steps: &'a [ast::Step]) -> Option<Statement> {
        self.statement(steps, &Bindings::new())
    }

    /// A statement: a query that gives rows, or one that changes them.
    pub(super) fn statement(
        &mut self,
        steps: &'a [ast::Step],
        bindings: &Bindings,
    ) -> Option<Statement> {
        let Some(at) = steps.iter().position(ast::Step::changes) else {
            let rel = self.query(steps, bindings)?;
            return Some(Statement::Select(self.finish(rel)));
        };
        let (before, rest) = steps.split_at(at);
        let (change, after) = rest.split_first()?;
        for step in after {
            if !matches!(step.kind, StepKind::Select(_)) {
                let problem = self
                    .error(step.span, "after a change, only a `select` can follow")
                    .with_label("a `select` here gives the rows changed");
                self.report(problem);
                return None;
            }
        }
        if !after.is_empty() && self.dialect == Dialect::ClickHouse {
            let problem = self.error(
                after[0].span,
                "ClickHouse can't give the rows a change makes",
            );
            self.report(problem);
            return None;
        }
        match &change.kind {
            StepKind::Insert(table) => self.insert(before, table, after, bindings),
            StepKind::Update(fields) => self.update(before, Some(fields), after, change, bindings),
            _ => self.update(before, None, after, change, bindings),
        }
    }

    /// `update` and `delete`: the rows of a table, chosen by `where`s.
    fn update(
        &mut self,
        before: &'a [ast::Step],
        fields: Option<&'a [ast::Field]>,
        after: &'a [ast::Step],
        change: &'a ast::Step,
        bindings: &Bindings,
    ) -> Option<Statement> {
        let word = if fields.is_some() { "update" } else { "delete" };
        let (first, rest) = before.split_first()?;
        let StepKind::From(source) = &first.kind else {
            return None;
        };
        if self.is_query(&source.value, bindings) {
            let problem = self
                .error(
                    source.value.span,
                    format!("`{word}` changes the rows of a table"),
                )
                .with_label("this is a query, not a table")
                .with_help(format!(
                    "start from the table, and choose its rows with `where`: `from cases` then `where ...` then `{word}`"
                ));
            self.report(problem);
            return None;
        }
        for step in rest {
            if !matches!(
                step.kind,
                StepKind::Where(_) | StepKind::Pass | StepKind::Call(_)
            ) {
                let problem = self
                    .error(
                        step.span,
                        format!("only `where` can choose the rows to {word}"),
                    )
                    .with_label(format!("this can't come before `{word}`"))
                    .with_help(format!(
                        "`{word}` works on every row the `where`s keep, in no order"
                    ));
                self.report(problem);
                return None;
            }
        }
        let rel = self.query(before, bindings)?;
        if rel.grouped || rel.distinct || rel.limited || rel.projected || rel.windowed {
            let problem = self.error(
                change.span,
                format!("only `where` can choose the rows to {word}"),
            );
            self.report(problem);
            return None;
        }
        let Some(Source {
            kind: SourceKind::Table(table),
            ..
        }) = rel.select.from.clone()
        else {
            return None;
        };
        let name = table.join(".");
        let known = self.schema.and_then(|schema| schema.table(&name));
        let filter = if rel.select.joins.is_empty() {
            rel.select.filter.clone()
        } else {
            // A link joins another table, so the rows are chosen by their key.
            let key = known
                .map(|table| table.primary_key.clone())
                .unwrap_or_default();
            let [key] = key.as_slice() else {
                let problem = self
                    .error(
                        change.span,
                        format!("to {word} rows chosen through a link, ButtercupQL needs the table's key"),
                    )
                    .with_help(format!(
                        "give the schema with `{name}`'s primary key, of one column"
                    ));
                self.report(problem);
                return None;
            };
            let alias = rel.frame.tables[0].alias.clone();
            let mut inner = self.output(rel.clone());
            inner.columns = vec![Item::Expr {
                expr: Sql::column(Some(&alias), key),
                alias: None,
            }];
            inner.order.clear();
            vec![Sql::InQuery(
                Box::new(Sql::column(None, key)),
                Box::new(inner),
                false,
            )]
        };
        let returning = self.returning(source, after, bindings)?;
        let Some(fields) = fields else {
            if filter.is_empty() {
                let warning = Diagnostic::warning(change.span, "this deletes every row")
                    .with_label(format!("no `where` chooses the rows of `{name}`"));
                self.report(warning);
            }
            return Some(Statement::Delete {
                table,
                filter,
                returning,
            });
        };
        let frame = &rel.frame;
        let mut set = Vec::new();
        let mut ok = true;
        for field in fields {
            let Some(column) = &field.name else {
                continue;
            };
            if let Some(table) = known {
                if table.column(&column.text).is_none() {
                    let names: Vec<&str> = table
                        .columns
                        .iter()
                        .map(|column| column.name.as_str())
                        .collect();
                    let mut problem = self.error(
                        column.span,
                        format!("there is no column `{}` in `{name}`", column.text),
                    );
                    if let Some(best) = suggest::closest(&column.text, names.iter().copied()) {
                        problem = problem.with_label(format!("did you mean `{best}`?"));
                    }
                    self.report(problem);
                    ok = false;
                    continue;
                }
            }
            match self.expr(&field.value, Cx::of(frame, bindings)) {
                Some(value) if self.storable(&value, field.value.span) => {
                    set.push((column.text.clone(), value.sql));
                }
                _ => ok = false,
            }
        }
        ok.then_some(Statement::Update {
            table,
            set,
            filter,
            returning,
        })
    }

    /// `insert into archive`: the rows of the steps before, or rows written in the query.
    fn insert(
        &mut self,
        before: &'a [ast::Step],
        target: &'a ast::Path,
        after: &'a [ast::Step],
        bindings: &Bindings,
    ) -> Option<Statement> {
        let table: Vec<String> = target.parts.iter().map(|part| part.text.clone()).collect();
        let name = table.join(".");
        let known = match self.schema {
            Some(schema) => match schema.table(&name) {
                Some(found) => Some(found),
                None if !self.complete => None,
                None => {
                    let names: Vec<&str> = schema
                        .tables()
                        .iter()
                        .map(|table| table.name.as_str())
                        .collect();
                    let mut problem =
                        self.error(target.span, format!("there is no table `{name}`"));
                    if let Some(best) = suggest::closest(&name, names.iter().copied()) {
                        problem = problem.with_label(format!("did you mean `{best}`?"));
                    }
                    self.report(problem);
                    return None;
                }
            },
            None => None,
        };
        let target_source = ast::Source {
            value: ast::Expr {
                kind: ExprKind::Name(name.clone()),
                span: target.span,
            },
            alias: None,
            span: target.span,
        };
        let returning = self.returning(&target_source, after, bindings)?;
        // Rows written in the query go in as they are.
        if let [first] = before {
            if let StepKind::From(source) = &first.kind {
                if let ExprKind::List(rows) = &source.value.kind {
                    let (columns, values) = self.literal_rows(rows)?;
                    self.check_columns(&columns, known, &name, source.value.span)?;
                    return Some(Statement::Insert {
                        table,
                        columns,
                        rows: Rows::Values(values),
                        returning,
                    });
                }
            }
        }
        let rel = self.query(before, bindings)?;
        let columns: Vec<String> = if rel.frame.star && !rel.frame.open().is_empty() {
            // The query's columns are not all known: they go in by their places.
            Vec::new()
        } else {
            rel.frame.visible().map(|col| col.name.clone()).collect()
        };
        let span = before.last().map_or(target.span, |step| step.span);
        self.check_columns(&columns, known, &name, span)?;
        let select = self.finish(rel);
        Some(Statement::Insert {
            table,
            columns,
            rows: Rows::Query(Box::new(select)),
            returning,
        })
    }

    /// Checks that a table has the columns a row gives it.
    fn check_columns(
        &mut self,
        columns: &[String],
        table: Option<&crate::schema::Table>,
        name: &str,
        span: Span,
    ) -> Option<()> {
        let Some(table) = table else {
            return Some(());
        };
        let mut ok = true;
        for column in columns {
            if table.column(column).is_none() {
                let names: Vec<&str> = table
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect();
                let mut problem =
                    self.error(span, format!("there is no column `{column}` in `{name}`"));
                if let Some(best) = suggest::closest(column, names.iter().copied()) {
                    problem = problem.with_label(format!("did you mean `{best}`?"));
                }
                self.report(problem);
                ok = false;
            }
        }
        ok.then_some(())
    }

    /// The columns a `select` after a change gives: `RETURNING`.
    fn returning(
        &mut self,
        table: &ast::Source,
        after: &'a [ast::Step],
        bindings: &Bindings,
    ) -> Option<Vec<Item>> {
        let mut items = Vec::new();
        for step in after {
            let StepKind::Select(fields) = &step.kind else {
                continue;
            };
            let rel = self.table(table);
            for field in fields {
                let value = self.expr(&field.value, Cx::of(&rel.frame, bindings))?;
                let alias = match (&field.name, &field.value.kind) {
                    (Some(name), _) => Some(name.text.clone()),
                    (None, ExprKind::Name(_)) => None,
                    (None, _) => {
                        let problem = self
                            .error(field.value.span, "this column needs a name")
                            .with_help(
                                "give it one with `=`, as in `select total = price * quantity`",
                            );
                        self.report(problem);
                        return None;
                    }
                };
                items.push(Item::Expr {
                    expr: value.sql,
                    alias,
                });
            }
        }
        Some(items)
    }

    /// The columns and values of rows written as dictionaries: `[{"id": "A1", "operator": "Acme"}, ...]`.
    fn literal_rows(&mut self, rows: &'a [ast::Expr]) -> Option<(Vec<String>, Vec<Vec<Sql>>)> {
        let (columns, typed) = self.typed_rows(rows)?;
        let values = typed
            .into_iter()
            .map(|row| row.into_iter().map(|value| value.sql).collect())
            .collect();
        Some((columns, values))
    }

    fn typed_rows(&mut self, rows: &'a [ast::Expr]) -> Option<(Vec<String>, Vec<Vec<Typed>>)> {
        let frame = Frame::default();
        let bindings = Bindings::new();
        let mut columns: Vec<String> = Vec::new();
        let mut values = Vec::new();
        for (n, row) in rows.iter().enumerate() {
            let ExprKind::Dict(pairs) = &row.kind else {
                return None;
            };
            let mut named: Vec<(String, &'a ast::Expr, Span)> = Vec::new();
            for (key, value) in pairs {
                let ExprKind::Str(column) = &key.kind else {
                    let problem = self
                        .error(key.span, "a row's columns are named by strings")
                        .with_help("for example: `{\"id\": \"A1\", \"operator\": \"Acme\"}`");
                    self.report(problem);
                    return None;
                };
                if named.iter().any(|(seen, _, _)| seen == column) {
                    let problem = self.error(key.span, format!("`{column}` is given twice"));
                    self.report(problem);
                    return None;
                }
                named.push((column.clone(), value, key.span));
            }
            if n == 0 {
                columns = named.iter().map(|(column, _, _)| column.clone()).collect();
            } else {
                let mut theirs: Vec<&str> =
                    named.iter().map(|(column, _, _)| column.as_str()).collect();
                let mut ours: Vec<&str> = columns.iter().map(String::as_str).collect();
                theirs.sort_unstable();
                ours.sort_unstable();
                if theirs != ours {
                    let problem = self
                        .error(row.span, "every row has the same columns")
                        .with_label(format!("the first row has {}", list(&ours)));
                    self.report(problem);
                    return None;
                }
            }
            let mut row_values = Vec::new();
            for column in &columns {
                let (_, value, _) = named
                    .iter()
                    .find(|(name, _, _)| name == column)
                    .expect("every row has the first row's columns");
                row_values.push(self.expr(value, Cx::of(&frame, &bindings))?);
            }
            values.push(row_values);
        }
        Some((columns, values))
    }

    /// Rows written in the query, read as a table: `from [{"id": "A1"}, {"id": "A2"}]`.
    pub(super) fn rows(&mut self, rows: &'a [ast::Expr], span: Span) -> Option<Rel> {
        if rows.is_empty() {
            let problem = self.error(span, "there are no rows here");
            self.report(problem);
            return None;
        }
        let (columns, typed) = self.typed_rows(rows)?;
        let alias = self.alias();
        let cols = columns
            .iter()
            .enumerate()
            .map(|(n, column)| {
                let ty = typed
                    .iter()
                    .map(|row| row[n].ty)
                    .find(|ty| *ty != Type::Unknown)
                    .unwrap_or(Type::Unknown);
                let nullable = typed.iter().any(|row| row[n].nullable);
                Col {
                    name: column.clone(),
                    value: Typed::column(Some(&alias), column, ty, nullable),
                    table: Some(alias.clone()),
                    base: true,
                    hidden: false,
                }
            })
            .collect();
        let values = typed
            .into_iter()
            .map(|row| row.into_iter().map(|value| value.sql).collect())
            .collect();
        Some(Rel {
            select: Select {
                from: Some(Source {
                    kind: SourceKind::Rows(values, columns),
                    alias: Some(alias.clone()),
                }),
                ..Select::default()
            },
            frame: Frame {
                cols,
                tables: vec![Scope {
                    alias,
                    known: true,
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
        })
    }

    /// Queries combined with `+`, `-`, `|` and `&`, read as one. A named query whose second part reads its own name
    /// is recursive: its first part gives the rows it starts with, and its second the rows each round adds.
    pub(super) fn compound(
        &mut self,
        expr: &'a ast::Expr,
        name: Option<&str>,
        bindings: &Bindings,
    ) -> Option<Rel> {
        let (op, left, right) = match &expr.kind {
            ExprKind::Query(query) => return self.query(&query.steps, bindings),
            ExprKind::Binary(op, left, right) => (*op, left.as_ref(), right.as_ref()),
            _ => {
                let problem = self.error(expr.span, "this is not a query");
                self.report(problem);
                return None;
            }
        };
        let Some(set) = set_op(op) else {
            let problem = self
                .error(expr.span, format!("queries can't be combined with `{}`", op.symbol()))
                .with_help("`+` keeps every row of both, `|` drops repeats, `&` keeps the rows in both, and `-` the rows of the first that are not in the second");
            self.report(problem);
            return None;
        };
        if name.is_some_and(|name| reads(left, name)) {
            let problem = self
                .error(left.span, "a recursive query reads itself only in its second part")
                .with_help("the first part gives the rows it starts with; the second reads them, and adds rows");
            self.report(problem);
            return None;
        }
        let recursive = name.filter(|name| reads(right, name));
        let left_rel = self.compound(left, None, bindings)?;
        let columns = columns_of(&left_rel);
        let left_select = self.member(left_rel);
        let Some(name) = recursive else {
            let right_rel = self.compound(right, None, bindings)?;
            let right_select = match &columns {
                Some(columns) => self.aligned(right_rel, columns, right.span)?,
                None => self.member(right_rel),
            };
            let alias = self.alias();
            let cols = columns.unwrap_or_default();
            let known = !cols.is_empty();
            return Some(Rel {
                select: Select {
                    from: Some(Source {
                        kind: SourceKind::Compound(
                            Box::new(left_select),
                            set,
                            Box::new(right_select),
                        ),
                        alias: Some(alias.clone()),
                    }),
                    ..Select::default()
                },
                frame: Frame {
                    cols: cols
                        .iter()
                        .map(|(column, ty, nullable)| Col {
                            name: column.clone(),
                            value: Typed::column(Some(&alias), column, *ty, *nullable),
                            table: Some(alias.clone()),
                            base: true,
                            hidden: false,
                        })
                        .collect(),
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
                sorted_at: None,
            });
        };
        if set == SetOp::Union && self.dialect == Dialect::ClickHouse {
            let problem = self
                .error(expr.span, "ClickHouse's recursive queries keep every row, so they add rows with `+`")
                .with_help("on ClickHouse, a recursive query over a loop of links never ends: `|`, which drops rows met before, works on the other databases");
            self.report(problem);
            return None;
        }
        if !matches!(set, SetOp::UnionAll | SetOp::Union) {
            let problem = self
                .error(expr.span, "a recursive query adds its rows with `+` or `|`")
                .with_help("`|` drops rows met before, so a query over a loop of links ends; `+` keeps them all");
            self.report(problem);
            return None;
        }
        let Some(columns) = columns else {
            let problem = self
                .error(
                    left.span,
                    "the first part of a recursive query names its columns",
                )
                .with_help("for example: `select id`");
            self.report(problem);
            return None;
        };
        self.ctes.push((name.to_string(), columns.clone()));
        let right_rel = self.compound(right, None, bindings);
        self.ctes.pop();
        let right_select = self.aligned(right_rel?, &columns, right.span)?;
        let source = ast::Source {
            value: ast::Expr {
                kind: ExprKind::Name(name.to_string()),
                span: expr.span,
            },
            alias: None,
            span: expr.span,
        };
        self.ctes.push((name.to_string(), columns));
        let mut rel = self.table(&source);
        self.ctes.pop();
        rel.select.with.push(Cte {
            name: name.to_string(),
            anchor: Box::new(left_select),
            op: set,
            step: Box::new(right_select),
        });
        Some(rel)
    }

    /// A query as one part of queries combined: with no order of its own, which combining does not keep.
    fn member(&mut self, rel: Rel) -> Select {
        let rel = if rel.limited || !rel.select.with.is_empty() {
            let mut wrapped = self.wrap(rel);
            wrapped.select.order.clear();
            wrapped
        } else {
            rel
        };
        let mut select = self.output(rel);
        select.order.clear();
        select
    }

    /// The second part of queries combined, its columns in the order of the first's: they are matched by name.
    fn aligned(&mut self, rel: Rel, columns: &Columns, span: Span) -> Option<Select> {
        let Some(theirs) = columns_of(&rel) else {
            // The columns are not all known: they are matched by their places.
            return Some(self.member(rel));
        };
        let mut ours: Vec<&str> = columns.iter().map(|(name, _, _)| name.as_str()).collect();
        let mut names: Vec<&str> = theirs.iter().map(|(name, _, _)| name.as_str()).collect();
        let first = ours.clone();
        let second = names.clone();
        ours.sort_unstable();
        names.sort_unstable();
        if ours != names {
            let problem = self
                .error(span, "the queries have different columns")
                .with_label(format!(
                    "the first gives {}, and this one {}",
                    list(&first),
                    list(&second)
                ))
                .with_help("give both the same columns with `select`; they are matched by name");
            self.report(problem);
            return None;
        }
        let values: Vec<(String, Sql)> = rel
            .frame
            .visible()
            .map(|col| (col.name.clone(), col.value.sql.clone()))
            .collect();
        let mut select = self.member(rel);
        select.columns = columns
            .iter()
            .map(|(name, _, _)| {
                let expr = values
                    .iter()
                    .find(|(column, _)| column == name)
                    .map(|(_, sql)| sql.clone())
                    .unwrap_or_else(|| Sql::column(None, name));
                Item::Expr {
                    expr,
                    alias: Some(name.clone()),
                }
            })
            .collect();
        Some(select)
    }

    /// A recursive query's own name, read as a table in its second part, or by the steps after it.
    pub(super) fn cte_table(&mut self, name: &str, source: &ast::Source) -> Option<Rel> {
        let (_, columns) = self.ctes.iter().rev().find(|(cte, _)| cte == name)?.clone();
        let alias = source
            .alias
            .as_ref()
            .map_or_else(|| name.to_string(), |alias| alias.text.clone());
        Some(Rel {
            select: Select {
                from: Some(Source {
                    kind: SourceKind::Table(vec![name.to_string()]),
                    alias: source.alias.as_ref().map(|alias| alias.text.clone()),
                }),
                ..Select::default()
            },
            frame: Frame {
                cols: columns
                    .iter()
                    .map(|(column, ty, nullable)| Col {
                        name: column.clone(),
                        value: Typed::column(Some(&alias), column, *ty, *nullable),
                        table: Some(alias.clone()),
                        base: true,
                        hidden: false,
                    })
                    .collect(),
                tables: vec![Scope {
                    alias,
                    known: true,
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
        })
    }
}

/// The columns a query gives, when they are all known: their names, types, and whether each can be missing.
fn columns_of(rel: &Rel) -> Option<Columns> {
    if rel.frame.star && !rel.frame.open().is_empty() {
        return None;
    }
    Some(
        rel.frame
            .visible()
            .map(|col| (col.name.clone(), col.value.ty, col.value.nullable))
            .collect(),
    )
}
