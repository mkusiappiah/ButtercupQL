//! Expressions: names, operators, comparisons and dates, each written as the database writes it.

use super::*;
use crate::ast::{BinaryOp, CompareOp, FPart, UnaryOp};

impl<'a> Compiler<'a> {
    // ---- Names

    /// A named value: `threshold = 1_000_000`, in the module being compiled or imported into it.
    fn value(&mut self, name: &str, span: Span) -> Option<Option<Typed>> {
        let (module, declared) = match self.find(self.current, name)? {
            Found::Let(module, declared) if !matches!(declared.value.kind, ExprKind::Query(_)) => {
                (module, declared)
            }
            _ => return None,
        };
        Some(self.let_value(module, declared, span))
    }

    /// The value of a `name = value` line, compiled in its own module.
    fn let_value(&mut self, module: usize, declared: &'a ast::Let, span: Span) -> Option<Typed> {
        let key = (module, declared.name.text.clone());
        if let Some(value) = self.values.get(&key) {
            return Some(value.clone());
        }
        self.enter(module, &declared.name.text, span)?;
        let frame = Frame::default();
        let bindings = Bindings::new();
        let typed = self.within(module, |compiler| {
            compiler.expr(&declared.value, Cx::of(&frame, &bindings))
        });
        self.stack.pop();
        if let Some(typed) = &typed {
            self.values.insert(key, typed.clone());
        }
        typed
    }

    /// A name: a parameter, a named value, or a column.
    pub(super) fn name(&mut self, name: &str, span: Span, cx: Cx<'_>) -> Option<Typed> {
        if let Some(value) = cx.bindings.get(name) {
            return Some(value.clone());
        }
        if let Some(value) = self.value(name, span) {
            return value;
        }
        let exact: Vec<&Col> = cx.frame.visible().filter(|col| col.name == name).collect();
        let mut found: Vec<&Col> = if exact.is_empty() {
            cx.frame
                .visible()
                .filter(|col| col.name.eq_ignore_ascii_case(name))
                .collect()
        } else {
            exact
        };
        found.reverse();
        // A column made before is read as it is: a window value in it was worked out where it was made.
        let read = |col: &Col| Typed {
            fresh: false,
            ..col.value.clone()
        };
        match found.as_slice() {
            [col] => return Some(read(col)),
            [latest, ..] if !latest.base => return Some(read(latest)),
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
        match self.find(self.current, name) {
            Some(Found::Let(..)) | Some(Found::Def(..)) => {
                let problem = self
                    .error(span, format!("`{name}` is a query, not a value"))
                    .with_help(format!(
                        "read it with `from {name}` or `join {name}`, or test values with `in {name}`"
                    ));
                self.report(problem);
                return None;
            }
            Some(Found::Module(_)) => {
                let problem = self
                    .error(span, format!("`{name}` is a module, not a value"))
                    .with_help(format!(
                        "read one of its names with a dot: `{name}.recent()`"
                    ));
                self.report(problem);
                return None;
            }
            None => {}
        }
        let open = cx.frame.open();
        if !open.is_empty() || self.poisoned {
            let table = match open.as_slice() {
                [only] => Some(only.alias.as_str()),
                _ => None,
            };
            return Some(Typed::column(table, name, Type::Unknown, true));
        }
        let names: Vec<&str> = cx.frame.visible().map(|col| col.name.as_str()).collect();
        let tables: Vec<&str> = cx
            .frame
            .tables
            .iter()
            .filter(|scope| scope.link.is_none())
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

    /// `table.column`, a link's column such as `account.operator`, a module's value, or a part of a date:
    /// `raised.year`.
    pub(super) fn attribute(
        &mut self,
        base: &'a ast::Expr,
        field: &'a ast::Name,
        span: Span,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        // A table joined to follow a link.
        if let Some(names) = names_of(base) {
            let path = names.join(".");
            if let Some(scope) = cx
                .frame
                .tables
                .iter()
                .find(|scope| scope.link.as_deref() == Some(&path))
            {
                return self.table_column(cx.frame, &scope.alias, &path, field);
            }
        }
        if let ExprKind::Name(table) = &base.kind {
            let is_table = cx
                .frame
                .tables
                .iter()
                .any(|scope| scope.link.is_none() && &scope.alias == table)
                && !cx.bindings.contains_key(table)
                && !cx.frame.visible().any(|col| &col.name == table);
            if is_table {
                let scope = cx
                    .frame
                    .tables
                    .iter()
                    .find(|scope| scope.link.is_none() && &scope.alias == table)
                    .expect("the table is in scope");
                if !scope.known || self.poisoned {
                    let found = cx.frame.cols.iter().rev().find(|col| {
                        col.table.as_ref() == Some(table)
                            && col.name.eq_ignore_ascii_case(&field.text)
                    });
                    return Some(match found {
                        Some(col) => col.value.clone(),
                        None => Typed::column(Some(table), &field.text, Type::Unknown, true),
                    });
                }
                return self.table_column(cx.frame, table, table, field);
            }
        }
        // A value of a module: `shared.threshold`.
        if let Some(mut names) = names_of(base) {
            names.push(field.text.clone());
            if let Some(found) = self.find_names(&names) {
                return match found {
                    Found::Let(module, declared)
                        if !matches!(declared.value.kind, ExprKind::Query(_)) =>
                    {
                        self.let_value(module, declared, span)
                    }
                    _ => {
                        let problem = self
                            .error(span, format!("`{}` is not a value", names.join(".")))
                            .with_help("a query is read with `from`, and a `def` of steps is called on a line of its own");
                        self.report(problem);
                        None
                    }
                };
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
                    ExprKind::Name(name) if cx.frame.tables.is_empty() || !value.reads => {
                        self.error(span, format!("`{name}` has no `{}`", field.text))
                    }
                    ExprKind::Name(name) => self
                        .error(base.span, format!("there is no table `{name}` here"))
                        .with_help(
                            "a dot after a table's name reads one of its columns: `accounts.operator`. With a schema, a column that points to another table, such as `account_id`, is followed by its name: `account.operator`",
                        ),
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

    /// What a dotted name means, from its names.
    pub(super) fn find_names(&self, parts: &[String]) -> Option<Found<'a>> {
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

    /// A column of a table in scope, by the table's name in the query, with a suggestion when there is none.
    fn table_column(
        &mut self,
        frame: &Frame,
        alias: &str,
        shown: &str,
        field: &ast::Name,
    ) -> Option<Typed> {
        let found = frame
            .cols
            .iter()
            .rev()
            .find(|col| col.table.as_deref() == Some(alias) && col.name == field.text)
            .or_else(|| {
                frame.cols.iter().rev().find(|col| {
                    col.table.as_deref() == Some(alias)
                        && col.name.eq_ignore_ascii_case(&field.text)
                })
            });
        if let Some(col) = found {
            return Some(col.value.clone());
        }
        let names: Vec<&str> = frame
            .cols
            .iter()
            .filter(|col| col.table.as_deref() == Some(alias))
            .map(|col| col.name.as_str())
            .collect();
        let mut problem = self.error(
            field.span,
            format!("there is no column `{}` in `{shown}`", field.text),
        );
        if let Some(best) = suggest::closest(&field.text, names) {
            problem = problem.with_label(format!("did you mean `{best}`?"));
        }
        self.report(problem);
        None
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
            Dialect::ClickHouse => {
                let function = match part {
                    "YEAR" => "toYear",
                    "MONTH" => "toMonth",
                    "DAY" => "toDayOfMonth",
                    "HOUR" => "toHour",
                    "MINUTE" => "toMinute",
                    _ => "toSecond",
                };
                Sql::call(function, vec![value.sql.clone()])
            }
            Dialect::Postgres | Dialect::DuckDb => Sql::Cast(
                Box::new(Sql::Extract(part, Box::new(value.sql.clone()))),
                "INTEGER",
            ),
        };
        Typed::from(sql, Type::Int, &[&value])
    }

    // ---- Expressions

    pub(super) fn expr(&mut self, expr: &'a ast::Expr, cx: Cx<'_>) -> Option<Typed> {
        // A condition is one through its `and`s and `or`s; anything else in it is a value.
        let cx = match &expr.kind {
            ExprKind::Binary(BinaryOp::And | BinaryOp::Or, ..) => cx,
            _ => Cx {
                condition: false,
                ..cx
            },
        };
        match &expr.kind {
            ExprKind::Name(name) => self.name(name, expr.span, cx),
            ExprKind::Attribute(base, field) => self.attribute(base, field, expr.span, cx),
            ExprKind::Call(function, args) => self.call(function, args, expr.span, cx),
            ExprKind::Number(number) => Some(Typed::new(
                Sql::Number(number.text.clone()),
                if number.float { Type::Float } else { Type::Int },
            )),
            ExprKind::Str(text) => Some(Typed::new(Sql::text(text), Type::Text)),
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
                let test = self.expr(test, cx.condition())?;
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
            ExprKind::Dict(_) => {
                let problem = self
                    .error(expr.span, "a dictionary is a row, written in `from [...]`")
                    .with_help("for example: `from [{\"id\": \"A1\", \"operator\": \"Acme\"}]`, then `insert into accounts`");
                self.report(problem);
                None
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
    pub(super) fn truth(&self, value: Typed) -> Sql {
        self.truth_of(value, false)
    }

    /// A condition whose value is read again, by `not` or by `and` and `or`. A value that can be missing is false
    /// when it is, rather than neither true nor false as SQL has it: `not notes` holds where `notes` is missing, as
    /// `not None` does in Python. A comparison keeps SQL's meaning for a missing side.
    fn definite_truth(&self, value: Typed) -> Sql {
        self.truth_of(value, true)
    }

    fn truth_of(&self, value: Typed, definite: bool) -> Sql {
        let test = match value.ty {
            Type::Int | Type::Float | Type::Decimal => {
                Sql::binary(Binary::NotEq, value.sql, Sql::number("0"))
            }
            Type::Text => Sql::binary(Binary::NotEq, value.sql, Sql::text("")),
            // A column of truth values, rather than a comparison.
            Type::Bool if matches!(value.sql, Sql::Column { .. }) => value.sql,
            _ => return value.sql,
        };
        if definite && value.nullable {
            self.is_bool(test, true, false)
        } else {
            test
        }
    }

    /// A value as text, for an f-string.
    fn as_text(&self, value: &Typed) -> Sql {
        if value.ty == Type::Text {
            return value.sql.clone();
        }
        match self.dialect {
            // ClickHouse's cast to a text that can't be missing fails on a missing value; toString() keeps it.
            Dialect::ClickHouse => Sql::call("toString", vec![value.sql.clone()]),
            _ => Sql::Cast(Box::new(value.sql.clone()), "TEXT"),
        }
    }

    fn fstring(&mut self, parts: &'a [FPart], cx: Cx<'_>) -> Option<Typed> {
        let mut pieces: Vec<Typed> = Vec::new();
        for part in parts {
            match part {
                FPart::Text(text) if text.is_empty() => {}
                FPart::Text(text) => pieces.push(Typed::new(Sql::text(text), Type::Text)),
                FPart::Value(value) => {
                    let typed = self.expr(value, cx)?;
                    let sql = self.as_text(&typed);
                    pieces.push(Typed::from(sql, Type::Text, &[&typed]));
                }
            }
        }
        let Some(first) = pieces.first().cloned() else {
            return Some(Typed::new(Sql::text(""), Type::Text));
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
        // What `not` reads, it reads again, as a value: a missing value in it is false.
        let value = self.expr(operand, cx)?;
        match op {
            UnaryOp::Pos => Some(value),
            UnaryOp::Not => {
                let condition = self.definite_truth(value.clone());
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
        if matches!(op, BinaryOp::BitOr | BinaryOp::BitAnd) {
            let problem = self
                .error(
                    span,
                    format!("`{}` combines the rows of two queries", op.symbol()),
                )
                .with_help("between values, Python writes `or` and `and`");
            self.report(problem);
            return None;
        }
        let a = self.expr(left, cx);
        let b = self.expr(right, cx);
        let (a, b) = (a?, b?);
        match op {
            BinaryOp::BitOr | BinaryOp::BitAnd => None,
            BinaryOp::And | BinaryOp::Or => {
                let sql_op = if op == BinaryOp::And {
                    Binary::And
                } else {
                    Binary::Or
                };
                // In a condition, `and` and `or` keep the rows their sides keep, a missing value as a false one.
                let definite = !cx.condition;
                let sql = Sql::binary(
                    sql_op,
                    self.truth_of(a.clone(), definite),
                    self.truth_of(b.clone(), definite),
                );
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
                // numbers. DuckDB's and ClickHouse's `/` already divides exactly.
                let exact = matches!(a.ty, Type::Float | Type::Decimal)
                    || matches!(b.ty, Type::Float | Type::Decimal)
                    || matches!(self.dialect, Dialect::DuckDb | Dialect::ClickHouse);
                let left = if exact {
                    a.sql.clone()
                } else {
                    Sql::binary(Binary::Mul, a.sql.clone(), Sql::number("1.0"))
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
                        Sql::binary(Binary::Mul, a.sql.clone(), Sql::number("1.0")),
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
                // A small whole power written in the query is multiplied out. SQLite's builds may leave out
                // `power()`, so there every whole power up to 16 is.
                let most = if self.dialect == Dialect::Sqlite {
                    16
                } else {
                    4
                };
                if let Sql::Number(digits) = &b.sql {
                    if let Some(power) = digits
                        .parse::<u32>()
                        .ok()
                        .filter(|power| (1..=most).contains(power))
                    {
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
        // Each part, split into whether it moves back and how far.
        let signed = |amount: &Sql| match amount {
            Sql::Number(digits) if digits.starts_with('-') => {
                (true, Sql::Number(digits[1..].to_string()))
            }
            Sql::Unary(Unary::Neg, inner) => (true, (**inner).clone()),
            amount => (false, amount.clone()),
        };
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
                        return Typed::from(Sql::call(function, flat), ty, &[&moment]);
                    }
                }
                Sql::call(function, args)
            }
            Dialect::ClickHouse => {
                parts
                    .iter()
                    .fold(moment.sql.clone(), |moved, (amount, unit)| {
                        let (back, amount) = signed(amount);
                        let interval = match amount {
                            Sql::Number(_) => Sql::Interval(Box::new(amount), unit.clickhouse()),
                            amount => {
                                let unit = match unit {
                                    Unit::Days => "toIntervalDay",
                                    Unit::Hours => "toIntervalHour",
                                    Unit::Minutes => "toIntervalMinute",
                                    Unit::Seconds => "toIntervalSecond",
                                };
                                Sql::call(unit, vec![amount])
                            }
                        };
                        Sql::binary(
                            if back { Binary::Sub } else { Binary::Add },
                            moved,
                            interval,
                        )
                    })
            }
            Dialect::Postgres | Dialect::DuckDb if date => {
                // A date moved by whole days stays a date: date + integer.
                parts.iter().fold(moment.sql.clone(), |moved, (amount, _)| {
                    let (back, amount) = signed(amount);
                    Sql::binary(if back { Binary::Sub } else { Binary::Add }, moved, amount)
                })
            }
            Dialect::Postgres | Dialect::DuckDb => {
                if subtract {
                    // Written as `now() - INTERVAL '7 days'` rather than adding a negative length.
                    let back: Vec<(Sql, Unit)> = parts
                        .iter()
                        .map(|(amount, unit)| (negate(amount.clone()), *unit))
                        .collect();
                    Sql::binary(Binary::Sub, moment.sql.clone(), self.interval(&back))
                } else {
                    Sql::binary(Binary::Add, moment.sql.clone(), self.interval(&parts))
                }
            }
        };
        Typed::from(sql, ty, &[&moment])
    }

    /// A length of time as the database writes it: `INTERVAL '7 days'`, `n * INTERVAL '1 day'`, or ClickHouse's
    /// `INTERVAL 7 DAY`.
    pub(super) fn interval(&self, parts: &[(Sql, Unit)]) -> Sql {
        if self.dialect == Dialect::ClickHouse {
            let mut terms = parts.iter().map(|(amount, unit)| match amount {
                Sql::Number(_) => Sql::Interval(Box::new(amount.clone()), unit.clickhouse()),
                amount => Sql::call(
                    match unit {
                        Unit::Days => "toIntervalDay",
                        Unit::Hours => "toIntervalHour",
                        Unit::Minutes => "toIntervalMinute",
                        Unit::Seconds => "toIntervalSecond",
                    },
                    vec![amount.clone()],
                ),
            });
            let first = terms
                .next()
                .unwrap_or(Sql::Interval(Box::new(Sql::number("0")), "DAY"));
            return terms.fold(first, |sum, term| Sql::binary(Binary::Add, sum, term));
        }
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
        let sql = match self.dialect {
            Dialect::Postgres | Dialect::Sqlite if both_whole => {
                // Whole numbers: SQL's `/` rounds towards zero, Python's `//` down; one less when the signs differ
                // and the division is not exact.
                let quotient = Sql::binary(Binary::Div, a.sql.clone(), b.sql.clone());
                let signs_differ = Sql::binary(
                    Binary::NotEq,
                    Sql::binary(Binary::Lt, a.sql.clone(), Sql::number("0")),
                    Sql::binary(Binary::Lt, b.sql.clone(), Sql::number("0")),
                );
                let inexact = Sql::binary(
                    Binary::NotEq,
                    Sql::binary(Binary::Mod, a.sql.clone(), b.sql.clone()),
                    Sql::number("0"),
                );
                let adjust = Sql::Case(
                    vec![(
                        Sql::binary(Binary::And, inexact, signs_differ),
                        Sql::number("1"),
                    )],
                    Some(Box::new(Sql::number("0"))),
                );
                Sql::binary(Binary::Sub, quotient, adjust)
            }
            Dialect::Postgres | Dialect::Sqlite => {
                let quotient = Sql::binary(
                    Binary::Div,
                    Sql::binary(Binary::Mul, a.sql.clone(), Sql::number("1.0")),
                    b.sql.clone(),
                );
                self.floor(quotient)
            }
            // DuckDB's and ClickHouse's `/` divides exactly, so the quotient is rounded down, and whole numbers stay
            // whole.
            Dialect::DuckDb | Dialect::ClickHouse => {
                let floored = self.floor(Sql::binary(Binary::Div, a.sql.clone(), b.sql.clone()));
                match (both_whole, self.dialect) {
                    (true, Dialect::ClickHouse) => Sql::call("toInt64", vec![floored]),
                    (true, _) => Sql::Cast(Box::new(floored), "BIGINT"),
                    (false, _) => floored,
                }
            }
        };
        let ty = if both_whole { Type::Int } else { Type::Float };
        let mut typed = Typed::from(sql, ty, &[&a, &b]);
        typed.nullable = true;
        typed
    }

    /// A number rounded down. Many builds of SQLite leave out `floor()`, so there it is worked out from a cast,
    /// which rounds towards zero: one less for a negative number with a fraction.
    /// A number rounded to a whole one as Python's `round()` rounds it: a half goes to the even neighbour, where
    /// SQL's `round()` takes it away from 0.
    pub(super) fn round_half_even(&self, value: Sql) -> Sql {
        let below = self.floor(value.clone());
        let nearest = self.floor(Sql::binary(Binary::Add, value.clone(), Sql::number("0.5")));
        let half = Sql::binary(
            Binary::Eq,
            Sql::binary(Binary::Sub, value, below.clone()),
            Sql::number("0.5"),
        );
        // PostgreSQL's `%` takes no fraction, and `below` is one when the number is: it is made whole first.
        let whole = match self.dialect {
            Dialect::Postgres => Sql::Cast(Box::new(below), "BIGINT"),
            _ => below,
        };
        let even = Sql::binary(
            Binary::Eq,
            Sql::binary(Binary::Mod, whole, Sql::number("2")),
            Sql::number("0"),
        );
        let back = Sql::Case(
            vec![(Sql::binary(Binary::And, half, even), Sql::number("1"))],
            Some(Box::new(Sql::number("0"))),
        );
        Sql::binary(Binary::Sub, nearest, back)
    }

    pub(super) fn floor(&self, value: Sql) -> Sql {
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

    /// `x IS TRUE` and its kin. ClickHouse writes them with `ifNull`, which reads a missing value as the answer
    /// that makes the comparison false.
    fn is_bool(&self, value: Sql, truth: bool, negated: bool) -> Sql {
        if self.dialect != Dialect::ClickHouse {
            return Sql::IsBool(Box::new(value), truth, negated);
        }
        let missing_as = Sql::Keyword(if truth { "false" } else { "true" });
        let filled = Sql::call("ifNull", vec![value, missing_as]);
        // `x IS TRUE` is ifNull(x, false); `x IS FALSE` is NOT ifNull(x, true); `IS NOT` turns each around.
        if truth != negated {
            filled
        } else {
            Sql::not(filled)
        }
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
                    ExprKind::Bool(truth) => self.is_bool(value.sql.clone(), *truth, negated),
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
            _ if self.named_query(right).is_some()
                && !names_of(right).is_some_and(|names| cx.bindings.contains_key(&names[0])) =>
            {
                let (module, query) = self.named_query(right)?;
                let name = path_of(right).join(".");
                self.enter(module, &name, right.span)?;
                let select = self.within(module, |compiler| {
                    compiler.subquery(&query.steps, right.span, &Bindings::new())
                });
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
                let function = match self.dialect {
                    Dialect::Sqlite => "instr",
                    Dialect::ClickHouse => "position",
                    Dialect::Postgres | Dialect::DuckDb => "strpos",
                };
                let position = Sql::call(function, vec![text.sql.clone(), value.sql.clone()]);
                let op = if negated { Binary::Eq } else { Binary::Gt };
                let sql = Sql::binary(op, position, Sql::number("0"));
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
            Some(rel.frame.visible().count())
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
