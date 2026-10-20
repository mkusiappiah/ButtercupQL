use super::parse;
use crate::ast::*;
use crate::diagnostic::Diagnostic;

const OPEN_CRITICAL: &str = include_str!("../../../../examples/open_critical.bcql");
const MONEY_BY_OPERATOR: &str = include_str!("../../../../examples/money_by_operator.bcql");
const BUSY_OPERATORS: &str = include_str!("../../../../examples/busy_operators.bcql");
const REUSE: &str = include_str!("../../../../examples/reuse.bcql");

// ---- Writing trees out, with their grouping shown

fn show_expr(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Name(name) => name.clone(),
        ExprKind::Attribute(base, name) => format!("{}.{}", show_expr(base), name.text),
        ExprKind::Call(function, args) => {
            let mut out = format!("(call {}", show_expr(function));
            for arg in args {
                out.push(' ');
                if let Some(name) = &arg.name {
                    out.push_str(&name.text);
                    out.push('=');
                }
                out.push_str(&show_expr(&arg.value));
            }
            out.push(')');
            out
        }
        ExprKind::Number(number) => number.text.clone(),
        ExprKind::Str(text) => format!("{text:?}"),
        ExprKind::FString(parts) => {
            let parts: Vec<String> = parts
                .iter()
                .map(|part| match part {
                    FPart::Text(text) => format!("{text:?}"),
                    FPart::Value(value) => show_expr(value),
                })
                .collect();
            format!("(f {})", parts.join(" "))
        }
        ExprKind::Bool(true) => "True".to_string(),
        ExprKind::Bool(false) => "False".to_string(),
        ExprKind::None => "None".to_string(),
        ExprKind::Unary(op, operand) => format!("({} {})", op.symbol(), show_expr(operand)),
        ExprKind::Binary(op, left, right) => {
            format!("({} {} {})", op.symbol(), show_expr(left), show_expr(right))
        }
        ExprKind::Compare(first, rest) if rest.len() == 1 => {
            let (op, second) = &rest[0];
            format!(
                "({} {} {})",
                op.symbol(),
                show_expr(first),
                show_expr(second)
            )
        }
        ExprKind::Compare(first, rest) => {
            let mut out = format!("(chain {}", show_expr(first));
            for (op, value) in rest {
                out.push_str(&format!(" {} {}", op.symbol(), show_expr(value)));
            }
            out.push(')');
            out
        }
        ExprKind::IfElse {
            test,
            then,
            otherwise,
        } => format!(
            "(if {} {} {})",
            show_expr(test),
            show_expr(then),
            show_expr(otherwise)
        ),
        ExprKind::List(items) => format!("[{}]", show_list(items, " ")),
        ExprKind::Tuple(items) => format!("(tuple {})", show_list(items, " ")),
        ExprKind::Set(items) => format!("{{{}}}", show_list(items, " ")),
        ExprKind::Query(query) => {
            let steps: Vec<String> = query.steps.iter().map(show_step).collect();
            format!("({})", steps.join("; "))
        }
        ExprKind::Error => "<error>".to_string(),
    }
}

fn show_list(items: &[Expr], between: &str) -> String {
    items
        .iter()
        .map(show_expr)
        .collect::<Vec<_>>()
        .join(between)
}

fn show_fields(fields: &[Field]) -> String {
    fields
        .iter()
        .map(|field| match &field.name {
            Some(name) => format!("{} = {}", name.text, show_expr(&field.value)),
            None => show_expr(&field.value),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn show_source(source: &Source) -> String {
    match &source.alias {
        Some(alias) => format!("{} as {}", show_expr(&source.value), alias.text),
        None => show_expr(&source.value),
    }
}

fn show_step(step: &Step) -> String {
    match &step.kind {
        StepKind::From(source) => format!("from {}", show_source(source)),
        StepKind::Where(condition) => format!("where {}", show_expr(condition)),
        StepKind::Column { name, value } => format!("{} = {}", name.text, show_expr(value)),
        StepKind::Select(fields) => format!("select {}", show_fields(fields)),
        StepKind::Del(columns) => format!("del {}", show_list(columns, ", ")),
        StepKind::Sort { keys, reverse } => {
            let keys: Vec<String> = keys
                .iter()
                .map(|key| {
                    format!(
                        "{}{}",
                        if key.descending { "-" } else { "" },
                        show_expr(&key.value)
                    )
                })
                .collect();
            match reverse {
                Some(reverse) => {
                    format!("sort {}, reverse={}", keys.join(", "), show_expr(reverse))
                }
                None => format!("sort {}", keys.join(", ")),
            }
        }
        StepKind::Take(count) => format!("take {}", show_expr(count)),
        StepKind::Skip(count) => format!("skip {}", show_expr(count)),
        StepKind::Distinct => "distinct".to_string(),
        StepKind::Group { by, totals } if totals.is_empty() => {
            format!("group by {}", show_fields(by))
        }
        StepKind::Group { by, totals } => {
            format!("group by {}: {}", show_fields(by), show_fields(totals))
        }
        StepKind::Summarize(totals) => format!("summarize: {}", show_fields(totals)),
        StepKind::Join { left, source, on } => format!(
            "{}join {} on {}",
            if *left { "left " } else { "" },
            show_source(source),
            show_expr(on)
        ),
        StepKind::Call(call) => show_expr(call),
        StepKind::Pass => "pass".to_string(),
    }
}

fn show_path(path: &Path) -> String {
    path.parts
        .iter()
        .map(|part| part.text.as_str())
        .collect::<Vec<_>>()
        .join(".")
}

fn show_alias(alias: &Option<Name>) -> String {
    alias
        .as_ref()
        .map_or(String::new(), |alias| format!(" as {}", alias.text))
}

fn show_item(item: &Item) -> String {
    match item {
        Item::Import(Import {
            kind: ImportKind::Module { path, alias },
            ..
        }) => format!("import {}{}", show_path(path), show_alias(alias)),
        Item::Import(Import {
            kind:
                ImportKind::Names {
                    level,
                    module,
                    names,
                },
            ..
        }) => {
            let names: Vec<String> = names
                .iter()
                .map(|name| format!("{}{}", name.name.text, show_alias(&name.alias)))
                .collect();
            let module = module.as_ref().map_or(String::new(), show_path);
            format!(
                "from {}{module} import {}",
                ".".repeat(*level as usize),
                names.join(", ")
            )
        }
        Item::Def(def) => format!("def {}", def.name.text),
        Item::Let(value) => format!("{} = {}", value.name.text, show_expr(&value.value)),
    }
}

// ---- Reading

/// Reads a text that has no mistakes.
fn read(text: &str) -> File {
    let (file, diagnostics) = parse(text);
    assert!(
        diagnostics.is_empty(),
        "problems in {text:?}:\n{diagnostics:#?}"
    );
    file
}

/// The steps of a text's query, written out.
fn steps(text: &str) -> Vec<String> {
    read(text)
        .query
        .expect("the text has a query")
        .steps
        .iter()
        .map(show_step)
        .collect()
}

/// A condition, written out with its grouping shown.
fn condition(text: &str) -> String {
    let steps = steps(&format!("from t\nwhere {text}\n"));
    steps[1]
        .strip_prefix("where ")
        .expect("a `where` step")
        .to_string()
}

/// The steps of a text's query, read despite its mistakes.
fn steps_despite_problems(text: &str) -> Vec<String> {
    let (file, _) = parse(text);
    file.query
        .map(|query| query.steps.iter().map(show_step).collect())
        .unwrap_or_default()
}

/// The messages of the problems found in a text.
fn problems(text: &str) -> Vec<String> {
    parse(text)
        .1
        .into_iter()
        .map(|problem| problem.message)
        .collect()
}

/// The one problem in a text.
fn problem(text: &str) -> Diagnostic {
    let mut found = parse(text).1;
    assert_eq!(
        found.len(),
        1,
        "expected one problem in {text:?}, found {found:#?}"
    );
    found.remove(0)
}

/// The text a problem points at.
fn at<'t>(text: &'t str, problem: &Diagnostic) -> &'t str {
    &text[problem.span.range()]
}

// ---- The examples

#[test]
fn the_first_example_reads_step_by_step() {
    assert_eq!(
        steps(OPEN_CRITICAL),
        [
            "from cases",
            "where (and (== status \"open\") (in band [\"critical\" \"high\"]))",
            "where (> raised (- (call now) (call days 7)))",
            "sort -risk",
            "take 20",
            "select key, account, risk",
        ]
    );
}

#[test]
fn group_by_holds_its_totals_in_a_block() {
    assert_eq!(
        steps(MONEY_BY_OPERATOR),
        [
            "from transactions",
            "where (> time (- (call now) (call days 30)))",
            "group by operator: total = (call count), money = (call sum amount), big = (call count where=(> amount 1000000))",
            "where (> money 1000000)",
            "sort -money",
        ]
    );
    assert_eq!(
        steps(BUSY_OPERATORS),
        [
            "from cases",
            "group by operator: n = (call count)",
            "where (> n 100)",
            "summarize: operators = (call count)",
        ]
    );
}

#[test]
fn defs_hold_steps_or_queries() {
    let file = read(REUSE);
    let defs: Vec<&Def> = file
        .items
        .iter()
        .map(|item| match item {
            Item::Def(def) => def,
            other => panic!("expected a def, found {other:?}"),
        })
        .collect();
    assert_eq!(defs.len(), 2);

    let recent = defs[0];
    assert_eq!(recent.name.text, "recent");
    assert!(!recent.is_query());
    assert_eq!(
        recent.docstring.as_ref().map(|text| text.value.as_str()),
        Some("The rows raised in the last `days_back` days.")
    );
    assert_eq!(recent.params.len(), 1);
    assert_eq!(recent.params[0].name.text, "days_back");
    assert_eq!(
        recent.params[0].default.as_ref().map(show_expr).as_deref(),
        Some("7")
    );
    assert_eq!(
        recent.body.iter().map(show_step).collect::<Vec<_>>(),
        ["where (> raised (- (call now) (call days days_back)))"]
    );

    let cases_for = defs[1];
    assert!(cases_for.is_query());
    let params: Vec<String> = cases_for
        .params
        .iter()
        .map(|param| {
            let annotation = param
                .annotation
                .as_ref()
                .map_or(String::new(), |t| show_path(&t.name));
            let default = param
                .default
                .as_ref()
                .map_or(String::new(), |d| format!(" = {}", show_expr(d)));
            format!("{}: {annotation}{default}", param.name.text)
        })
        .collect();
    assert_eq!(params, ["account: str", "days_back: int = 30"]);
    assert_eq!(
        cases_for.body.iter().map(show_step).collect::<Vec<_>>(),
        [
            "from cases",
            "where (== account_id account)",
            "(call recent days_back)"
        ]
    );

    assert_eq!(
        file.query
            .expect("a query")
            .steps
            .iter()
            .map(show_step)
            .collect::<Vec<_>>(),
        [
            "from cases",
            "(call recent 30)",
            "where (is assignee None)",
            "take 10"
        ]
    );
}

// ---- Expressions

#[test]
fn operators_follow_pythons_order() {
    assert_eq!(
        condition("a or b and not c == d"),
        "(or a (and b (not (== c d))))"
    );
    assert_eq!(
        condition("a + b * c ** d > 0"),
        "(> (+ a (* b (** c d))) 0)"
    );
    assert_eq!(condition("-2 ** 2 == x"), "(== (- (** 2 2)) x)");
    assert_eq!(condition("2 ** -1 == x"), "(== (** 2 (- 1)) x)");
    assert_eq!(condition("a ** b ** c == x"), "(== (** a (** b c)) x)");
    assert_eq!(condition("a - b - c == x"), "(== (- (- a b) c) x)");
    assert_eq!(
        condition("a // b % c * d == x"),
        "(== (* (% (// a b) c) d) x)"
    );
    assert_eq!(condition("not a or b"), "(or (not a) b)");
    assert_eq!(condition("(a or b) and c"), "(and (or a b) c)");
    assert_eq!(condition("a - -b > +c"), "(> (- a (- b)) (+ c))");
}

#[test]
fn comparisons_chain_and_read_as_python_reads_them() {
    assert_eq!(condition("1 <= risk <= 100"), "(chain 1 <= risk <= 100)");
    assert_eq!(
        condition("band not in [\"x\", \"y\"]"),
        "(not in band [\"x\" \"y\"])"
    );
    assert_eq!(
        condition("band in (\"x\", \"y\")"),
        "(in band (tuple \"x\" \"y\"))"
    );
    assert_eq!(
        condition("band in {\"x\", \"y\",}"),
        "(in band {\"x\" \"y\"})"
    );
    assert_eq!(condition("assignee is not None"), "(is not assignee None)");
    assert_eq!(condition("\"Acme\" in operator"), "(in \"Acme\" operator)");
    assert_eq!(condition("not a in b"), "(not (in a b))");
    assert_eq!(condition("a != b"), "(!= a b)");
}

#[test]
fn values_calls_and_names() {
    assert_eq!(
        condition("(\"urgent\" if risk >= 90 else \"normal\") == label"),
        "(== (if (>= risk 90) \"urgent\" \"normal\") label)"
    );
    assert_eq!(
        condition("name.lower().startswith(\"a\")"),
        "(call (call name.lower).startswith \"a\")"
    );
    assert_eq!(
        condition("accounts.operator == \"Acme\""),
        "(== accounts.operator \"Acme\")"
    );
    assert_eq!(
        condition("round(x, ndigits=2) > 1"),
        "(> (call round x ndigits=2) 1)"
    );
    assert_eq!(
        condition("`order date` > date(\"2026-10-20\")"),
        "(> order date (call date \"2026-10-20\"))"
    );
    assert_eq!(
        condition("flag == True or x is None"),
        "(or (== flag True) (is x None))"
    );
    assert_eq!(condition("len(name) > 3"), "(> (call len name) 3)");
    assert_eq!(condition("f(\n    a,\n    b,\n)"), "(call f a b)");
}

#[test]
fn numbers_are_read_as_python_writes_them() {
    assert_eq!(condition("x == 1_000_000"), "(== x 1000000)");
    assert_eq!(condition("x == 0xff"), "(== x 255)");
    assert_eq!(condition("x == 0b101"), "(== x 5)");
    assert_eq!(condition("x == 0o17"), "(== x 15)");
    assert_eq!(condition("x == .5"), "(== x 0.5)");
    assert_eq!(condition("x == 5."), "(== x 5.0)");
    assert_eq!(condition("x == 1.5e-3"), "(== x 1.5e-3)");
    assert_eq!(condition("x == 000"), "(== x 0)");
}

#[test]
fn strings_join_and_f_strings_hold_values() {
    assert_eq!(condition("x == \"a\" 'b'"), "(== x \"ab\")");
    assert_eq!(
        condition("x == f\"{first} {last}!\""),
        "(== x (f first \" \" last \"!\"))"
    );
    assert_eq!(condition("x == \"tab\\there\""), "(== x \"tab\\there\")");
    assert_eq!(condition("x == r\"\\d+\""), "(== x \"\\\\d+\")");
    assert_eq!(
        condition("x == f\"{{literal}} {n + 1}\""),
        "(== x (f \"{literal} \" (+ n 1)))"
    );
    assert_eq!(condition("x == \"a\" f\"{b}\""), "(== x (f \"a\" b))");
    assert_eq!(
        condition("x == (\"one \"\n    \"two\")"),
        "(== x \"one two\")"
    );
    assert_eq!(
        condition("x == \"\"\"two\nlines\"\"\""),
        "(== x \"two\\nlines\")"
    );
    assert_eq!(
        condition("x == f\"{name.upper()}\""),
        "(== x (f (call name.upper)))"
    );
}

// ---- Steps

#[test]
fn every_step_reads() {
    let text = "\
from cases as c
join accounts as a on c.account_id == a.id
left join notes on notes.case_id == c.id
n = risk * 2
risk += 1
select key, total = n + 1
del notes, c.raw
sort raised, -risk, reverse=True
skip 40
take 20
distinct
pass
";
    assert_eq!(
        steps(text),
        [
            "from cases as c",
            "join accounts as a on (== c.account_id a.id)",
            "left join notes on (== notes.case_id c.id)",
            "n = (* risk 2)",
            "risk = (+ risk 1)",
            "select key, total = (+ n 1)",
            "del notes, c.raw",
            "sort raised, -risk, reverse=True",
            "skip 40",
            "take 20",
            "distinct",
            "pass",
        ]
    );
}

#[test]
fn blocks_can_be_short() {
    assert_eq!(
        steps("from t\ngroup by operator\n"),
        ["from t", "group by operator"]
    );
    assert_eq!(
        steps("from t\nsummarize: n = count(); total = sum(x)\n"),
        [
            "from t",
            "summarize: n = (call count), total = (call sum x)"
        ]
    );
    assert_eq!(
        steps("from t\ngroup by month = raised.month, operator:\n    n = count()\n"),
        [
            "from t",
            "group by month = raised.month, operator: n = (call count)"
        ]
    );
}

#[test]
fn from_and_join_read_tables_queries_and_calls() {
    assert_eq!(
        steps(
            "from cases_for(\"A123\", days_back=7)\njoin (\n    from accounts\n    where active\n) as a on account_id == a.id\n"
        ),
        [
            "from (call cases_for \"A123\" days_back=7)",
            "join (from accounts; where active) as a on (== account_id a.id)",
        ]
    );
    assert_eq!(
        steps("from analytics.daily_totals\n"),
        ["from analytics.daily_totals"]
    );
}

#[test]
fn a_query_in_brackets_is_a_value() {
    let text = "\
open_cases = (
    from cases
    where status == \"open\"
)
small = (from accounts
         where balance < 10)

from open_cases
where account_id in (from blocked; select account_id)
";
    let file = read(text);
    assert_eq!(
        file.items.iter().map(show_item).collect::<Vec<_>>(),
        [
            "open_cases = (from cases; where (== status \"open\"))",
            "small = (from accounts; where (< balance 10))",
        ]
    );
    assert_eq!(
        file.query
            .expect("a query")
            .steps
            .iter()
            .map(show_step)
            .collect::<Vec<_>>(),
        [
            "from open_cases",
            "where (in account_id (from blocked; select account_id))"
        ]
    );
}

#[test]
fn a_named_query_can_hold_blocks() {
    let text = "\
by_band = (
    from cases
    group by band:
        n = count()
    sort -n
)
from by_band
";
    let file = read(text);
    assert_eq!(
        file.items.iter().map(show_item).collect::<Vec<_>>(),
        ["by_band = (from cases; group by band: n = (call count); sort -n)"]
    );
}

#[test]
fn imports_and_values_come_before_the_query() {
    let text = "\
\"\"\"Reports on cases.\"\"\"
import shared
import shared.dates as d, other
from shared import recent, busy as b
from . import nearby
from ..lib import (
    one,
    two,
)
threshold = 1_000_000

from cases
where amount > threshold
";
    let file = read(text);
    assert_eq!(
        file.docstring.map(|text| text.value).as_deref(),
        Some("Reports on cases.")
    );
    assert_eq!(
        file.items.iter().map(show_item).collect::<Vec<_>>(),
        [
            "import shared",
            "import shared.dates as d",
            "import other",
            "from shared import recent, busy as b",
            "from . import nearby",
            "from ..lib import one, two",
            "threshold = 1000000",
        ]
    );
    assert_eq!(file.query.expect("a query").steps.len(), 2);
}

#[test]
fn a_file_of_definitions_alone_is_a_module() {
    let file = read("def recent(days_back=7):\n    where raised > now() - days(days_back)\n");
    assert!(file.query.is_none());
    assert_eq!(file.items.len(), 1);
    assert!(read("").query.is_none());
    assert!(read("# nothing but a comment\n").query.is_none());
}

#[test]
fn a_def_can_take_its_steps_on_its_own_line() {
    let file = read("def recent(): where raised > now() - days(7); take 5\nfrom cases\nrecent()\n");
    let Item::Def(def) = &file.items[0] else {
        panic!("expected a def");
    };
    assert_eq!(
        def.body.iter().map(show_step).collect::<Vec<_>>(),
        ["where (> raised (- (call now) (call days 7)))", "take 5"]
    );
    assert_eq!(file.query.expect("a query").steps.len(), 2);
}

#[test]
fn parameters_take_types_and_defaults() {
    let file = read("def f(a, b: str, c: int = 3, d: list[str] = None) -> None:\n    pass\n");
    let Item::Def(def) = &file.items[0] else {
        panic!("expected a def");
    };
    let types: Vec<String> = def
        .params
        .iter()
        .map(|param| match &param.annotation {
            Some(annotation) if annotation.args.is_empty() => show_path(&annotation.name),
            Some(annotation) => format!(
                "{}[{}]",
                show_path(&annotation.name),
                show_path(&annotation.args[0].name)
            ),
            None => "-".to_string(),
        })
        .collect();
    assert_eq!(types, ["-", "str", "int", "list[str]"]);
    assert_eq!(
        def.returns.as_ref().map(|t| show_path(&t.name)).as_deref(),
        Some("None")
    );
}

// ---- Layout

#[test]
fn brackets_let_a_step_span_lines() {
    let text = "\
from cases
where band in [
    \"critical\",
    \"high\",
]
select (
    key,
    total = risk
        * 2,
)
";
    assert_eq!(
        steps(text),
        [
            "from cases",
            "where (in band [\"critical\" \"high\"])",
            "select key, total = (* risk 2)",
        ]
    );
}

#[test]
fn a_backslash_joins_lines() {
    assert_eq!(
        steps("from cases\nwhere risk > 1 \\\n    and band == \"high\"\n"),
        ["from cases", "where (and (> risk 1) (== band \"high\"))"]
    );
}

#[test]
fn semicolons_put_steps_on_one_line() {
    assert_eq!(
        steps("from cases; where risk > 1; take 5;"),
        ["from cases", "where (> risk 1)", "take 5"]
    );
}

#[test]
fn a_query_indented_in_a_python_string_reads_as_it_is() {
    let text = "\n        from cases\n        where risk > 1\n\n        take 5\n    ";
    assert_eq!(steps(text), ["from cases", "where (> risk 1)", "take 5"]);
}

#[test]
fn comments_blank_lines_and_windows_line_endings_change_nothing() {
    let text =
        "# Open cases\n\nfrom cases  # every case\n\n# only open ones\nwhere status == \"open\"\n";
    let expected = ["from cases", "where (== status \"open\")"];
    assert_eq!(steps(text), expected);
    assert_eq!(steps(&text.replace('\n', "\r\n")), expected);
}

#[test]
fn backticks_quote_names_that_are_not_plain_words() {
    assert_eq!(
        steps("from `order lines`\nwhere `from` == \"A\"\n`total value` = price * n\n"),
        [
            "from order lines",
            "where (== from \"A\")",
            "total value = (* price n)"
        ]
    );
}

#[test]
fn step_words_are_names_everywhere_else() {
    assert_eq!(
        steps("from t\nwhere sort > take\nselect = 1\ngroup by by:\n    where = count()\n"),
        [
            "from t",
            "where (> sort take)",
            "select = 1",
            "group by by: where = (call count)",
        ]
    );
}

// ---- Problems

#[test]
fn a_single_equals_sign_is_read_as_a_comparison() {
    let text = "from cases\nwhere status = \"open\"\ntake 5\n";
    let found = problem(text);
    assert_eq!(found.message, "use `==` to compare");
    assert_eq!(at(text, &found), "=");
    assert_eq!(
        steps_despite_problems(text),
        ["from cases", "where (== status \"open\")", "take 5"]
    );
}

#[test]
fn a_mistyped_step_gets_a_suggestion() {
    let found = problem("from cases\nwehre risk > 1\n");
    assert_eq!(found.message, "there is no step `wehre`");
    assert_eq!(found.label, "did you mean `where`?");
    assert_eq!(
        problem("from t\nsumarize:\n    n = count()\n").label,
        "did you mean `summarize`?"
    );
    assert_eq!(
        problem("from t\ngrup by a\n").label,
        "did you mean `group by`?"
    );
    assert!(
        problem("from t\nrisk > 1\n").help[0].starts_with("to keep the rows where this is true")
    );
}

#[test]
fn sql_words_point_to_the_step_that_does_their_work() {
    let take = problem("from cases\nlimit 10\n");
    assert_eq!(take.message, "there is no step `limit`");
    assert_eq!(take.label, "ButtercupQL calls this `take`");
    assert_eq!(take.help, ["for example: `take 20`"]);
    assert_eq!(
        problem("from cases\norder by risk desc\n").label,
        "ButtercupQL calls this `sort`"
    );
    assert_eq!(
        problem("from cases\nWHERE risk > 1\n").label,
        "write it in lowercase: `where`"
    );
    assert_eq!(
        problem("FROM cases\n").label,
        "write it in lowercase: `from`"
    );
    assert_eq!(problem("from cases\nhaving n > 1\n").help.len(), 2);
    assert!(problem("from cases\nright join a on x == y\n").help[0].contains("`left join`"));
}

#[test]
fn every_problem_is_found_in_one_reading() {
    let text = "\
from cases
where status = \"open\"
wehre x
sort by risk
take
group by operator:
    count()
";
    assert_eq!(
        problems(text),
        [
            "use `==` to compare",
            "there is no step `wehre`",
            "`sort` takes its columns straight after it, without `by`",
            "expected how many rows to take, as in `take 20`",
            "this total needs a name",
        ]
    );
}

#[test]
fn an_unclosed_bracket_is_reported_where_it_opens() {
    let text = "from cases\nwhere band in [\"a\", \"b\"\nsort risk\n";
    let found = problem(text);
    assert_eq!(found.message, "this `[` is never closed");
    assert_eq!(at(text, &found), "[");
    assert_eq!(steps_despite_problems(text), ["from cases", "sort risk"]);
}

#[test]
fn after_an_unclosed_bracket_the_next_lines_are_still_checked() {
    let text = "from cases\nwhere band in [\"critical\", \"high\"\norder by risk\nlimit 20\n";
    assert_eq!(
        problems(text),
        [
            "this `[` is never closed",
            "there is no step `order`",
            "there is no step `limit`",
        ]
    );
}

#[test]
fn an_unclosed_query_in_brackets_is_reported_and_the_rest_read() {
    let text =
        "open_cases = (\n    from cases\n    where status == \"open\"\n\nfrom open_cases\ntake 5\n";
    assert_eq!(problems(text), ["this `(` is never closed"]);
    assert_eq!(steps_despite_problems(text), ["from open_cases", "take 5"]);
}

#[test]
fn a_query_starts_with_from_and_has_one() {
    assert_eq!(
        problems("where risk > 1\n"),
        ["a query starts with `from` and a table"]
    );
    assert_eq!(problems("from a\nfrom b\n"), ["a query has one `from`"]);
    assert_eq!(
        problems("from a\nwhere x\ndef f():\n    pass\n"),
        ["a `def` goes before the query"]
    );
    assert_eq!(
        problems("def f():\n    where a\n    from b\n"),
        ["a `from` starts a query, so it goes on the query's first line"]
    );
}

#[test]
fn each_step_has_its_own_line() {
    let found = problem("from cases where risk > 1\n");
    assert_eq!(found.message, "expected the end of the step, found `where`");
    assert_eq!(found.label, "a new step starts here");
}

#[test]
fn indentation_is_checked_as_python_checks_it() {
    assert_eq!(
        problems("from cases\n    where x\n"),
        ["unexpected indentation"]
    );
    assert_eq!(
        problems("def f():\n\twhere a\n        where b\nfrom t\n"),
        ["this line's indentation mixes tabs and spaces"]
    );
    assert_eq!(
        problems("from t\ngroup by a:\nn = count()\n"),
        ["expected the totals on the lines after this `:`, indented"]
    );
}

#[test]
fn other_languages_operators_are_read_with_a_hint() {
    let text = "from t\nwhere a > 1 & b < 2\n";
    assert_eq!(problems(text), ["use `and`"]);
    assert_eq!(
        steps_despite_problems(text)[1],
        "where (and (> a 1) (< b 2))"
    );
    assert_eq!(
        problems("from t\nwhere a && b || c\n"),
        ["use `and`", "use `or`"]
    );
    assert_eq!(problems("from t\nwhere !a\n"), ["use `not`"]);
    assert_eq!(
        problems("from t\nwhere a <> b\n"),
        ["use `!=` for “not equal to”"]
    );
    assert_eq!(
        problems("from t\nwhere status == ?\n"),
        ["ButtercupQL writes the SQL parameters itself"]
    );
}

#[test]
fn blocks_hold_only_named_totals() {
    let found = problem("from t\ngroup by a:\n    count()\n");
    assert_eq!(found.message, "this total needs a name");
    assert_eq!(found.label, "give it a name, as in `n = count()`");
    assert_eq!(
        problem("from t\ngroup by a:\n    where x > 1\n").message,
        "a `where` can't go inside a block of totals"
    );
    assert_eq!(
        problem("from t\nsummarize\ntake 1\n").message,
        "expected `:` after `summarize`, found the end of the line"
    );
}

#[test]
fn columns_are_named_with_equals() {
    let found = problem("from t\nselect a as b\n");
    assert_eq!(found.message, "name a column with `=`, not `as`");
    assert_eq!(found.label, "write `b = a`");
}

#[test]
fn a_join_needs_its_condition() {
    assert_eq!(
        problem("from t\njoin a\ntake 1\n").message,
        "expected `on` and the condition that pairs the rows, found the end of the line"
    );
}

#[test]
fn sort_takes_a_minus_sign_rather_than_desc() {
    let text = "from t\nsort risk desc\n";
    let found = problem(text);
    assert_eq!(found.message, "there is no `desc` in ButtercupQL");
    assert_eq!(found.label, "write `-risk` to sort high to low");
    assert_eq!(steps_despite_problems(text), ["from t", "sort -risk"]);
}

#[test]
fn strings_must_be_closed() {
    assert_eq!(
        problems("from t\nwhere a == \"open\ntake 1\n"),
        ["this string is not closed on its line"]
    );
}

#[test]
fn keywords_are_names_only_in_backticks() {
    let found = problem("from t\nselect a, class\n");
    assert_eq!(
        found.message,
        "`class` is a Python keyword that ButtercupQL does not use"
    );
    assert_eq!(
        problem("from t\nimport = 1\n").message,
        "imports go at the top of the file, before the query"
    );
}

#[test]
fn f_string_problems_point_into_the_string() {
    assert_eq!(
        problems("from t\nwhere a == f\"{x:.2f} {}\"\n"),
        [
            "f-string formats such as `:.2f` and `!r` are not supported yet",
            "this f-string field is empty",
        ]
    );
}

#[test]
fn hidden_characters_are_refused() {
    let found = problem("from t\nwhere a == \"x\u{202e}\"\n");
    assert!(found.message.contains("U+202E"));
}

#[test]
fn reading_never_stops_or_breaks_on_broken_text() {
    let samples = [
        OPEN_CRITICAL,
        MONEY_BY_OPERATOR,
        REUSE,
        "x = (\n    from a\n    group by b:\n        n = count()\n)\nfrom x\nwhere y in [1, (2, 3), {4}]\nselect (a, b = f\"{c}\")\n",
    ];
    for sample in samples {
        // Every beginning of the text, cut before any character.
        for (cut, _) in sample.char_indices() {
            let _ = parse(&sample[..cut]);
        }
    }
    let broken = [
        ")",
        "]]]",
        "(((",
        "from",
        "def",
        "def f(",
        "group by",
        "x = (",
        "from t\n)\n",
        ":::",
        "\"",
        "f\"{",
        "`",
        "\\",
        "from t\nwhere (((((",
        "from t\nsort",
        "from t\nselect",
        "from t\njoin",
        "from t\nleft",
        "import",
        "from . import",
        "def f():",
        "from t\ngroup by a:\n    n = \n",
        "x = (from t; summarize: )",
    ];
    for text in broken {
        assert!(!problems(text).is_empty(), "no problem found in {text:?}");
    }
}
