//! Calls: functions, totals, window functions and methods.

use super::*;
use crate::sql::Frame;

/// The functions ButtercupQL knows, for suggestions.
const FUNCTIONS: [&str; 32] = [
    "count",
    "count_distinct",
    "sum",
    "mean",
    "avg",
    "min",
    "max",
    "any",
    "all",
    "row_number",
    "rank",
    "dense_rank",
    "lag",
    "lead",
    "first",
    "last",
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
    "minutes",
    "seconds",
    "weeks",
    "literal",
];

/// The functions that only work over a window of rows.
const WINDOW_ONLY: [&str; 7] = [
    "row_number",
    "rank",
    "dense_rank",
    "lag",
    "lead",
    "first",
    "last",
];

/// The totals, which work over a group's rows, or over a window of rows when given `over=`, `sort=` or `rows=`.
const TOTALS: [&str; 9] = [
    "count",
    "count_distinct",
    "sum",
    "mean",
    "avg",
    "min",
    "max",
    "any",
    "all",
];

/// A call's arguments, compiled: those given by place, and those given by name with where they are written.
type Arguments = (Vec<Typed>, HashMap<String, (Typed, Span)>);

/// The rows a window function works over.
struct Window {
    partition: Vec<Sql>,
    order: Vec<Order>,
    frame: Option<Frame>,
}

impl<'a> Compiler<'a> {
    pub(super) fn call(
        &mut self,
        function: &'a ast::Expr,
        args: &'a [ast::Arg],
        span: Span,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        match &function.kind {
            ExprKind::Name(name) => self.function(name, function.span, args, span, cx),
            ExprKind::Attribute(base, method) => {
                if let Some(Found::Def(..)) = self.find_path(function) {
                    let name = path_of(function).join(".");
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
                if let ExprKind::Name(module) = &base.kind {
                    let is_value = cx.bindings.contains_key(module)
                        || self.find(self.current, module).is_some()
                        || cx.frame.visible().any(|col| &col.name == module)
                        || !cx.frame.open().is_empty()
                        || cx.frame.tables.iter().any(|scope| {
                            &scope.alias == module || scope.link.as_deref() == Some(module.as_str())
                        });
                    if !is_value {
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

    /// The arguments of a call, compiled, with the arguments given by name kept apart.
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
                (low, high) if high == usize::MAX => format!("{low} or more arguments"),
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
        let named: Vec<&str> = args
            .iter()
            .filter_map(|arg| arg.name.as_ref().map(|name| name.text.as_str()))
            .collect();
        let windowed = WINDOW_ONLY.contains(&name)
            || (TOTALS.contains(&name)
                && named
                    .iter()
                    .any(|arg| matches!(*arg, "over" | "sort" | "rows")));
        if windowed {
            return self.window(name, args, span, cx);
        }
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
                    .with_help("it goes in the block of a `group by` or a `summarize`:\n\n    summarize:\n        n = count()")
                    .with_help(format!(
                        "to keep every row with its total beside it, give the total a window: `{name}(..., over=operator)`"
                    ));
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
        if !cx.bindings.contains_key(name) {
            if let Some(Found::Def(..)) = self.find(self.current, name) {
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
        }
        let (values, _) = self.args(name, args, &[], cx)?;
        let given = values.len();
        let dialect = self.dialect;
        match name {
            "len" => {
                self.arity(name, given, 1, 1, span)?;
                let value = &values[0];
                let function = if dialect == Dialect::ClickHouse {
                    "lengthUTF8"
                } else {
                    "length"
                };
                Some(Typed::from(
                    Sql::call(function, vec![value.sql.clone()]),
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
                    (Dialect::Sqlite, name) => name,
                    (_, "min") => "least",
                    (_, _) => "greatest",
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
                let fraction = matches!(value.ty, Type::Float | Type::Decimal);
                let (sql, ty) = match (name, dialect) {
                    ("str", Dialect::ClickHouse) => {
                        (Sql::call("toString", vec![value.sql.clone()]), Type::Text)
                    }
                    ("str", _) => (Sql::Cast(Box::new(value.sql.clone()), "TEXT"), Type::Text),
                    ("int", Dialect::ClickHouse) => {
                        (Sql::call("toInt64", vec![value.sql.clone()]), Type::Int)
                    }
                    ("int", Dialect::Sqlite) => {
                        (Sql::Cast(Box::new(value.sql.clone()), "INTEGER"), Type::Int)
                    }
                    ("int", _) => {
                        // Python's int() drops the fraction; PostgreSQL's and DuckDB's casts would round it.
                        let whole = if fraction {
                            Sql::call("trunc", vec![value.sql.clone()])
                        } else {
                            value.sql.clone()
                        };
                        (Sql::Cast(Box::new(whole), "BIGINT"), Type::Int)
                    }
                    (_, Dialect::ClickHouse) => {
                        (Sql::call("toFloat64", vec![value.sql.clone()]), Type::Float)
                    }
                    (_, Dialect::Sqlite) => {
                        (Sql::Cast(Box::new(value.sql.clone()), "REAL"), Type::Float)
                    }
                    (_, _) => (
                        Sql::Cast(Box::new(value.sql.clone()), "DOUBLE PRECISION"),
                        Type::Float,
                    ),
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
                    Dialect::ClickHouse => Sql::call("today", Vec::new()),
                    Dialect::Postgres | Dialect::DuckDb => Sql::Keyword("CURRENT_DATE"),
                };
                Some(Typed::new(sql, Type::Date))
            }
            "date" | "datetime" => {
                self.arity(name, given, 1, 1, span)?;
                let date = name == "date";
                let text = match &args[0].value.kind {
                    ExprKind::Str(text) => text,
                    _ => {
                        let problem = self
                            .error(
                                args[0].value.span,
                                format!("`{name}()` takes the date written as text"),
                            )
                            .with_help(if date {
                                "for example: `date(\"2026-10-20\")`"
                            } else {
                                "for example: `datetime(\"2026-10-20 14:30\")`"
                            });
                        self.report(problem);
                        return None;
                    }
                };
                let Some(normal) = (if date {
                    valid_date(text)
                } else {
                    valid_datetime(text)
                }) else {
                    let problem = self
                        .error(
                            args[0].value.span,
                            format!(
                                "`{text}` is not a valid {}",
                                if date { "date" } else { "date and time" }
                            ),
                        )
                        .with_help(if date {
                            "write it as year, month and day: `2026-10-20`"
                        } else {
                            "write it as `2026-10-20 14:30` or `2026-10-20 14:30:00`"
                        });
                    self.report(problem);
                    return None;
                };
                let ty = if date { Type::Date } else { Type::Timestamp };
                let sql = match dialect {
                    Dialect::Sqlite => Sql::Text(normal),
                    Dialect::ClickHouse => Sql::call(
                        if date { "toDate" } else { "toDateTime" },
                        vec![Sql::Text(normal)],
                    ),
                    Dialect::Postgres | Dialect::DuckDb => {
                        Sql::Typed(if date { "DATE" } else { "TIMESTAMP" }, normal)
                    }
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
                                Err(_) => {
                                    Sql::binary(Binary::Mul, amount.sql.clone(), Sql::number("7"))
                                }
                            },
                            sql => Sql::binary(Binary::Mul, sql.clone(), Sql::number("7")),
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
                if let Some(best) = suggest::closest(name, FUNCTIONS) {
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
        let filter = match named.get("where") {
            Some((condition, _)) if condition.total || condition.window => {
                let problem = self.error(
                    span,
                    "a total's `where=` is about each row, and can't hold a total",
                );
                self.report(problem);
                return None;
            }
            Some((condition, _)) => Some(self.truth(condition.clone())),
            None => None,
        };
        let wanted = if name == "count" { (0, 1) } else { (1, 1) };
        self.arity(name, values.len(), wanted.0, wanted.1, span)?;
        let mut typed = self.aggregate(name, values.first(), filter, None);
        typed.total = true;
        typed.reads = true;
        Some(typed)
    }

    /// The SQL of a total, as each database writes it: `sum(x) FILTER (WHERE c)`, or ClickHouse's `sumIf(x, c)`;
    /// over a window when one is given. The sum of no rows is 0, as in Python, and `any()` and `all()` of none are
    /// `False` and `True`.
    fn aggregate(
        &self,
        name: &str,
        arg: Option<&Typed>,
        filter: Option<Sql>,
        window: Option<Window>,
    ) -> Typed {
        let click = self.dialect == Dialect::ClickHouse;
        let argument = arg.map(|value| Box::new(value.sql.clone()));
        let plain = |name: &'static str, distinct: bool, filter: Option<Sql>| Sql::Aggregate {
            name,
            arg: argument.clone(),
            distinct,
            filter: filter.map(Box::new),
        };
        let conditional = |name: &str, filter: Sql| {
            let mut args: Vec<Sql> = argument.iter().map(|arg| (**arg).clone()).collect();
            args.push(filter);
            Sql::call(&format!("{name}If"), args)
        };
        let function: &'static str = match (name, self.dialect) {
            ("mean" | "avg", _) => "avg",
            ("sum", _) => "sum",
            ("min", _) => "min",
            ("max", _) => "max",
            ("any", Dialect::Postgres | Dialect::DuckDb) => "bool_or",
            ("all", Dialect::Postgres | Dialect::DuckDb) => "bool_and",
            ("any", _) => "max",
            ("all", _) => "min",
            _ => "count",
        };
        // ClickHouse gives a total of no rows its type's empty value, such as 0, where SQL gives NULL; its
        // `-OrNull` forms give NULL. A window's rows are never none: they hold the row itself.
        let or_null = click && window.is_none() && matches!(function, "avg" | "min" | "max");
        let function: &'static str = match (or_null, function) {
            (true, "avg") => "avgOrNull",
            (true, "min") => "minOrNull",
            (true, "max") => "maxOrNull",
            (_, function) => function,
        };
        let conditional = |name: &str, filter: Sql| match name.strip_suffix("OrNull") {
            Some(base) => {
                let mut call = conditional(base, filter);
                if let Sql::Call(function, _) = &mut call {
                    function.push_str("OrNull");
                }
                call
            }
            None => conditional(name, filter),
        };
        let core = match (name, click, filter) {
            ("count", true, Some(filter)) => match &argument {
                // countIf counts the rows where its condition holds; count(x) counts those where x is there.
                Some(arg) => Sql::call(
                    "countIf",
                    vec![Sql::binary(
                        Binary::And,
                        Sql::IsNull(arg.clone(), true),
                        filter,
                    )],
                ),
                None => Sql::call("countIf", vec![filter]),
            },
            ("count_distinct", true, Some(filter)) => conditional("uniqExact", filter),
            ("count_distinct", _, filter) => plain("count", true, filter),
            (_, true, Some(filter)) => conditional(function, filter),
            (_, _, filter) => plain(function, false, filter),
        };
        let core = match window {
            Some(window) => Sql::Window {
                function: Box::new(core),
                partition: window.partition,
                order: window.order,
                frame: window.frame,
            },
            None => core,
        };
        let ty = arg.map_or(Type::Int, |value| value.ty);
        let mut typed = match name {
            "sum" => Typed::new(Sql::call("coalesce", vec![core, Sql::number("0")]), ty),
            "any" => Typed::new(
                Sql::call("coalesce", vec![core, Sql::Keyword("FALSE")]),
                Type::Bool,
            ),
            "all" => Typed::new(
                Sql::call("coalesce", vec![core, Sql::Keyword("TRUE")]),
                Type::Bool,
            ),
            "mean" | "avg" => {
                let ty = if ty == Type::Decimal {
                    Type::Decimal
                } else {
                    Type::Float
                };
                Typed::new(core, ty).nullable(true)
            }
            "min" | "max" => Typed::new(core, ty).nullable(true),
            _ => Typed::new(core, Type::Int),
        };
        if let Some(arg) = arg {
            typed.total = arg.total;
        }
        typed
    }

    /// A window function: `rank(sort=-risk)`, `row_number(over=operator, sort=raised)`, a running
    /// `sum(amount, sort=time)`, a moving `mean(amount, sort=day, rows=7)`, or a total on every row:
    /// `sum(amount, over=operator)`.
    fn window(
        &mut self,
        name: &str,
        args: &'a [ast::Arg],
        span: Span,
        cx: Cx<'_>,
    ) -> Option<Typed> {
        if cx.totals {
            let problem = self
                .error(span, "a window value can't be one of a block's totals")
                .with_help(format!(
                    "work it out after the block, on a line of its own: `place = {name}(...)`"
                ));
            self.report(problem);
            return None;
        }
        if cx.in_total {
            let problem = self.error(span, "a total can't be worked out from a window value");
            self.report(problem);
            return None;
        }
        let aggregate = TOTALS.contains(&name);
        let mut over = None;
        let mut sort = None;
        let mut rows = None;
        let mut filter = None;
        let mut default = None;
        let mut positional = Vec::new();
        let mut ok = true;
        for arg in args {
            match arg.name.as_ref().map(|name| name.text.as_str()) {
                None => positional.push(&arg.value),
                Some("over") => over = Some(&arg.value),
                Some("sort") => sort = Some(&arg.value),
                Some("rows") if aggregate => rows = Some(&arg.value),
                Some("where") if aggregate => filter = Some(&arg.value),
                Some("default") if matches!(name, "lag" | "lead") => default = Some(&arg.value),
                Some(other) => {
                    let span = arg.name.as_ref().map_or(arg.value.span, |name| name.span);
                    let problem = self.error(span, format!("`{name}` has no argument `{other}`"));
                    self.report(problem);
                    ok = false;
                }
            }
        }
        let row_cx = Cx {
            totals: false,
            in_total: false,
            ..cx
        };
        let mut partition = Vec::new();
        for key in over.map(items).unwrap_or_default() {
            partition.push(self.expr(key, row_cx)?.sql);
        }
        let mut order = Vec::new();
        for key in sort.map(items).unwrap_or_default() {
            let (descending, key) = match &key.kind {
                ExprKind::Unary(ast::UnaryOp::Neg, inner) => (true, inner.as_ref()),
                _ => (false, key),
            };
            let value = self.expr(key, row_cx)?;
            order.push(Order {
                nulls_last: self.nulls_last(descending, &value),
                expr: value.sql,
                descending,
            });
        }
        let rows = match rows {
            None => None,
            Some(value) => match &value.kind {
                ExprKind::Number(number) if !number.float && number.text != "0" => {
                    number.text.parse::<u64>().ok()
                }
                _ => None,
            }
            .or_else(|| {
                let problem = self
                    .error(
                        value.span,
                        "`rows=` takes how many rows the window holds, 1 or more",
                    )
                    .with_help(
                        "for example: `mean(amount, sort=day, rows=7)`, the mean of each 7 days",
                    );
                self.report(problem);
                ok = false;
                None
            }),
        };
        let arg_cx = Cx {
            totals: false,
            in_total: true,
            ..cx
        };
        let mut values = Vec::new();
        for value in &positional {
            values.push(self.expr(value, arg_cx)?);
        }
        if !ok {
            return None;
        }
        let needs_sort = |compiler: &mut Self, what: &str| {
            if order.is_empty() {
                let problem = compiler
                    .error(span, format!("`{name}()` needs `sort=` to know {what}"))
                    .with_help(format!("for example: `{name}(sort=-risk)`"));
                compiler.report(problem);
                return None;
            }
            Some(())
        };
        let dialect = self.dialect;
        let mut typed = if aggregate {
            if name == "count_distinct" {
                let problem =
                    self.error(span, "`count_distinct()` can't be worked out over a window");
                self.report(problem);
                return None;
            }
            let wanted = if name == "count" { (0, 1) } else { (1, 1) };
            self.arity(name, values.len(), wanted.0, wanted.1, span)?;
            if rows.is_some() && order.is_empty() {
                let problem = self
                    .error(
                        span,
                        "`rows=` needs `sort=`, to know which rows come before",
                    )
                    .with_help("for example: `mean(amount, sort=day, rows=7)`");
                self.report(problem);
                return None;
            }
            let filter = match filter {
                None => None,
                Some(condition) => {
                    let condition = self.expr(condition, row_cx)?;
                    Some(self.truth(condition))
                }
            };
            // Sorted, a total runs: each row's total is of the rows up to it, or of the last `rows` rows.
            let frame = (!order.is_empty()).then(|| rows.map_or(Frame::Running, Frame::Last));
            let window = Window {
                partition,
                order,
                frame,
            };
            self.aggregate(name, values.first(), filter, Some(window))
        } else {
            let (function, args, frame, ty, nullable) = match name {
                "row_number" | "rank" | "dense_rank" => {
                    self.arity(name, values.len(), 0, 0, span)?;
                    if name != "row_number" {
                        needs_sort(self, "what ranks first")?;
                    }
                    (name.to_string(), Vec::new(), None, Type::Int, false)
                }
                "lag" | "lead" => {
                    self.arity(name, values.len(), 1, 2, span)?;
                    needs_sort(
                        self,
                        if name == "lag" {
                            "which row comes before"
                        } else {
                            "which row comes after"
                        },
                    )?;
                    // ClickHouse fills a row with none before or after with its type's empty value, unless the
                    // value can be missing.
                    let value = match dialect {
                        Dialect::ClickHouse => Sql::call("toNullable", vec![values[0].sql.clone()]),
                        _ => values[0].sql.clone(),
                    };
                    let mut call = vec![value];
                    let steps = values
                        .get(1)
                        .map_or(Sql::number("1"), |steps| steps.sql.clone());
                    call.push(steps);
                    if let Some(default) = default {
                        call.push(self.expr(default, row_cx)?.sql);
                    }
                    // ClickHouse reads the rows before and after within the window's frame, so it takes them all.
                    let (function, frame) = match dialect {
                        Dialect::ClickHouse => (format!("{name}InFrame"), Some(Frame::Whole)),
                        _ => (name.to_string(), None),
                    };
                    (function, call, frame, values[0].ty, true)
                }
                _ => {
                    self.arity(name, values.len(), 1, 1, span)?;
                    needs_sort(
                        self,
                        if name == "first" {
                            "which row is first"
                        } else {
                            "which row is last"
                        },
                    )?;
                    let function = if name == "first" {
                        "first_value"
                    } else {
                        "last_value"
                    };
                    (
                        function.to_string(),
                        vec![values[0].sql.clone()],
                        Some(Frame::Whole),
                        values[0].ty,
                        values[0].nullable,
                    )
                }
            };
            let sql = Sql::Window {
                function: Box::new(Sql::Call(function, args)),
                partition,
                order,
                frame,
            };
            Typed::new(sql, ty).nullable(nullable)
        };
        typed.window = true;
        typed.fresh = true;
        typed.reads = true;
        typed.total = false;
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
                let function = match (name, dialect) {
                    ("lower", Dialect::ClickHouse) => "lowerUTF8",
                    (_, Dialect::ClickHouse) => "upperUTF8",
                    (name, _) => name,
                };
                Some(Typed::from(
                    Sql::call(function, vec![value.sql.clone()]),
                    Type::Text,
                    &inputs,
                ))
            }
            "strip" | "lstrip" | "rstrip" => {
                self.arity(name, given, 0, 1, span)?;
                if dialect == Dialect::ClickHouse && given == 1 {
                    let problem = self.error(span, format!("`.{name}()` with the characters to strip is not supported on ClickHouse yet"));
                    self.report(problem);
                    return None;
                }
                let function = match (name, given, dialect) {
                    ("strip", _, Dialect::ClickHouse) => "trimBoth",
                    ("lstrip", _, Dialect::ClickHouse) => "trimLeft",
                    (_, _, Dialect::ClickHouse) => "trimRight",
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
                let function = if dialect == Dialect::ClickHouse {
                    "replaceAll"
                } else {
                    "replace"
                };
                let call = vec![
                    value.sql.clone(),
                    values[0].sql.clone(),
                    values[1].sql.clone(),
                ];
                Some(Typed::from(Sql::call(function, call), Type::Text, &inputs))
            }
            "startswith" | "endswith" => {
                self.arity(name, given, 1, 1, span)?;
                let start = name == "startswith";
                let fix = &values[0].sql;
                let sql = match (dialect, fix) {
                    // ClickHouse has functions that compare exactly, which its index can serve.
                    (Dialect::ClickHouse, fix) => Sql::call(
                        if start { "startsWith" } else { "endsWith" },
                        vec![value.sql.clone(), fix.clone()],
                    ),
                    // A prefix written in the query becomes a pattern, which an index can serve. Its wildcards are
                    // escaped, and the match is exact about case on every database.
                    (_, Sql::Param(Param::Value(Value::Text(text)), _)) => {
                        let (pattern, glob) = match dialect {
                            Dialect::Sqlite => {
                                let text = glob_escape(text);
                                (
                                    if start {
                                        format!("{text}*")
                                    } else {
                                        format!("*{text}")
                                    },
                                    true,
                                )
                            }
                            _ => {
                                let text = like_escape(text);
                                (
                                    if start {
                                        format!("{text}%")
                                    } else {
                                        format!("%{text}")
                                    },
                                    false,
                                )
                            }
                        };
                        Sql::Like {
                            expr: Box::new(value.sql.clone()),
                            pattern: Box::new(Sql::text(&pattern)),
                            glob,
                        }
                    }
                    (_, fix) => {
                        let length = Sql::call("length", vec![fix.clone()]);
                        let part = match (start, dialect) {
                            (true, _) => Sql::call(
                                "substr",
                                vec![value.sql.clone(), Sql::number("1"), length],
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
                    Dialect::ClickHouse => Sql::call("toDate", vec![value.sql.clone()]),
                    Dialect::Postgres | Dialect::DuckDb => {
                        Sql::Cast(Box::new(value.sql.clone()), "DATE")
                    }
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

/// The values of a window's `over=` or `sort=`: a list or a tuple of them, or one alone.
fn items(expr: &ast::Expr) -> Vec<&ast::Expr> {
    match &expr.kind {
        ExprKind::List(items) | ExprKind::Tuple(items) => items.iter().collect(),
        _ => vec![expr],
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
