//! What ButtercupQL has built in, described for people: its steps, functions, methods and the parts of a date.
//! Editors show these as completions and when the pointer rests on a name.

/// A built-in name, how it is written, and what it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Builtin {
    /// The name, as it is written.
    pub name: &'static str,
    /// How it is written, with its arguments.
    pub signature: &'static str,
    /// What it does.
    pub summary: &'static str,
}

const fn builtin(name: &'static str, signature: &'static str, summary: &'static str) -> Builtin {
    Builtin {
        name,
        signature,
        summary,
    }
}

/// The steps, each at the start of a line. A step that adds a column, `name = expression`, has no word of its own.
pub const STEPS: &[Builtin] = &[
    builtin(
        "from",
        "from cases",
        "Start from a table, a view or a named query.",
    ),
    builtin(
        "where",
        "where condition",
        "Keep the rows where the condition is true. After `group by`, it keeps groups by their totals.",
    ),
    builtin(
        "select",
        "select a, b, total = x + y",
        "Keep these columns, in this order, naming any computed ones.",
    ),
    builtin("del", "del a, b", "Drop these columns."),
    builtin(
        "sort",
        "sort -risk, raised",
        "Order the rows. `-` sorts a column high to low; missing values come last either way.",
    ),
    builtin("take", "take 20", "Keep the first 20 rows."),
    builtin("skip", "skip 40", "Leave out the first 40 rows."),
    builtin("distinct", "distinct", "Drop repeated rows."),
    builtin(
        "group",
        "group by operator:",
        "One row for each group, with the totals of the indented block below.",
    ),
    builtin(
        "summarize",
        "summarize:",
        "One row of totals for all the rows, from the indented block below.",
    ),
    builtin(
        "join",
        "join accounts on account_id == accounts.id",
        "Add another table's columns to the rows that match.",
    ),
    builtin(
        "left",
        "left join accounts on account_id == accounts.id",
        "Add another table's columns, keeping the rows that have no match.",
    ),
    builtin(
        "update",
        "update status = \"closed\", risk += 1",
        "Change the rows the steps before chose: give columns new values. A `select` after it gives the rows changed.",
    ),
    builtin(
        "delete",
        "delete",
        "Delete the rows the steps before chose.",
    ),
    builtin(
        "insert",
        "insert into archive",
        "Add the rows of the steps before to a table, its columns matched by name.",
    ),
    builtin(
        "pivot",
        "pivot band in [\"critical\", \"high\"] by operator:",
        "One row for each group, with a column of each total of the block below for each value.",
    ),
    builtin(
        "pass",
        "pass",
        "Do nothing, as in Python: a block with no steps yet.",
    ),
];

/// The statements that change the database itself, each on a line of its own.
pub const STATEMENTS: &[Builtin] = &[
    builtin(
        "create",
        "create table cases:",
        "Create a table, its columns written below as Python annotations: `name: type`. `create index on cases(-risk)` creates an index.",
    ),
    builtin(
        "alter",
        "alter table cases:",
        "Change a table: add a column written as in `create table`, `del` one, or `rename old to new`.",
    ),
    builtin(
        "drop",
        "drop table archive",
        "Drop a table, or with `drop index`, an index.",
    ),
    builtin(
        "grant",
        "grant read on cases to analyst",
        "Let users or roles read or write tables: `read`, `write`, `insert`, `update`, `delete` or `all`.",
    ),
    builtin(
        "revoke",
        "revoke write on cases from analyst",
        "Take permissions away again.",
    ),
    builtin(
        "with",
        "with transaction:",
        "Run the statements below together: all of them, or none if one fails.",
    ),
];

/// What is written with `@` on the line before a `def`.
pub const DECORATORS: &[Builtin] = &[builtin(
    "stored",
    "@stored",
    "Keep the def in the database, to be run by its name: a query as a function, a macro or a view, and on PostgreSQL a def that changes rows as a procedure.",
)];

/// The functions.
pub const FUNCTIONS: &[Builtin] = &[
    builtin(
        "count",
        "count(), count(x, where=condition)",
        "A total: the number of rows, or of the rows where `x` is not missing. `where=` counts only the rows where the condition is true.",
    ),
    builtin(
        "count_distinct",
        "count_distinct(x, where=condition)",
        "A total: the number of different values of `x`, leaving out missing ones.",
    ),
    builtin(
        "sum",
        "sum(x, where=condition)",
        "A total: the sum of `x`. The sum of no rows is 0, as in Python.",
    ),
    builtin(
        "mean",
        "mean(x, where=condition)",
        "A total: the mean of `x`. The mean of no rows is missing.",
    ),
    builtin("avg", "avg(x, where=condition)", "The same as `mean(x)`."),
    builtin(
        "min",
        "min(x), min(a, b, ...)",
        "Given one value, a total: the smallest `x`. Given several, the smallest of them on each row.",
    ),
    builtin(
        "max",
        "max(x), max(a, b, ...)",
        "Given one value, a total: the largest `x`. Given several, the largest of them on each row.",
    ),
    builtin(
        "any",
        "any(condition)",
        "A total: whether the condition is true on any row. For no rows, False.",
    ),
    builtin(
        "all",
        "all(condition)",
        "A total: whether the condition is true on every row. For no rows, True.",
    ),
    builtin(
        "row_number",
        "row_number(over=group, sort=order)",
        "A window function: numbers the rows 1, 2, 3 in the order of `sort=`, starting again for each group of `over=`.",
    ),
    builtin(
        "rank",
        "rank(over=group, sort=order)",
        "A window function: each row's place in the order of `sort=`. Rows that tie share a place, and leave a gap after it: 1, 1, 3.",
    ),
    builtin(
        "dense_rank",
        "dense_rank(over=group, sort=order)",
        "A window function: as `rank()`, without gaps: 1, 1, 2.",
    ),
    builtin(
        "lag",
        "lag(x, n=1, default=None, over=group, sort=order)",
        "A window function: the value of `x` on the row `n` rows before, in the order of `sort=`, or `default=` when there is none.",
    ),
    builtin(
        "lead",
        "lead(x, n=1, default=None, over=group, sort=order)",
        "A window function: the value of `x` on the row `n` rows after, in the order of `sort=`, or `default=` when there is none.",
    ),
    builtin(
        "first",
        "first(x, over=group, sort=order)",
        "A window function: the value of `x` on the first row of its group, in the order of `sort=`.",
    ),
    builtin(
        "last",
        "last(x, over=group, sort=order)",
        "A window function: the value of `x` on the last row of its group, in the order of `sort=`.",
    ),
    builtin("len", "len(text)", "The number of characters in the text."),
    builtin("abs", "abs(x)", "The number without its sign."),
    builtin(
        "round",
        "round(x), round(x, places)",
        "The number rounded to a whole number, or to `places` decimal places.",
    ),
    builtin("str", "str(x)", "The value as text."),
    builtin(
        "int",
        "int(x)",
        "The value as a whole number. A fraction is dropped, as Python's `int()` drops it.",
    ),
    builtin(
        "float",
        "float(x)",
        "The value as a number with a fraction.",
    ),
    builtin("now", "now()", "The date and time now."),
    builtin("today", "today()", "Today's date."),
    builtin(
        "date",
        "date(\"2026-10-20\")",
        "A date, written as text: year, month and day.",
    ),
    builtin(
        "datetime",
        "datetime(\"2026-10-20 14:30\")",
        "A date and time, written as text.",
    ),
    builtin(
        "days",
        "days(n)",
        "A length of time of `n` days, to add to a date or take from it: `now() - days(7)`.",
    ),
    builtin("hours", "hours(n)", "A length of time of `n` hours."),
    builtin("minutes", "minutes(n)", "A length of time of `n` minutes."),
    builtin("seconds", "seconds(n)", "A length of time of `n` seconds."),
    builtin("weeks", "weeks(n)", "A length of time of `n` weeks."),
    builtin(
        "literal",
        "literal(x)",
        "Writes a value into the SQL rather than passing it as a parameter, for a partial index that needs to see it.",
    ),
];

/// The methods, written after a value and a dot.
pub const METHODS: &[Builtin] = &[
    builtin("lower", "text.lower()", "The text in small letters."),
    builtin("upper", "text.upper()", "The text in capitals."),
    builtin(
        "strip",
        "text.strip(), text.strip(characters)",
        "The text without spaces at either end, or without the characters given.",
    ),
    builtin(
        "lstrip",
        "text.lstrip(), text.lstrip(characters)",
        "The text without spaces at its start, or without the characters given.",
    ),
    builtin(
        "rstrip",
        "text.rstrip(), text.rstrip(characters)",
        "The text without spaces at its end, or without the characters given.",
    ),
    builtin(
        "startswith",
        "text.startswith(prefix)",
        "Whether the text starts with the prefix, matching capitals exactly. An index can serve it.",
    ),
    builtin(
        "endswith",
        "text.endswith(suffix)",
        "Whether the text ends with the suffix, matching capitals exactly.",
    ),
    builtin(
        "replace",
        "text.replace(old, new)",
        "The text with every `old` in it replaced by `new`.",
    ),
    builtin("date", "moment.date()", "The day of a date and time."),
];

/// The parts of a date or a time, read with a dot: `raised.year`.
pub const DATE_PARTS: &[Builtin] = &[
    builtin("year", "moment.year", "The year, as a number."),
    builtin(
        "month",
        "moment.month",
        "The month, as a number from 1 to 12.",
    ),
    builtin("day", "moment.day", "The day of the month, as a number."),
    builtin("hour", "moment.hour", "The hour, as a number from 0 to 23."),
    builtin("minute", "moment.minute", "The minute, as a number."),
    builtin("second", "moment.second", "The second, as a number."),
];

/// Python's words that ButtercupQL's expressions and definitions use.
pub const KEYWORDS: &[&str] = &[
    "and", "as", "by", "def", "else", "False", "if", "import", "in", "is", "None", "not", "on",
    "or", "True",
];

/// The built-in name of a kind, if there is one.
pub fn find(list: &'static [Builtin], name: &str) -> Option<&'static Builtin> {
    list.iter().find(|builtin| builtin.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_function_the_compiler_knows_is_described() {
        let described: Vec<&str> = FUNCTIONS.iter().map(|function| function.name).collect();
        assert_eq!(described, crate::compile::FUNCTIONS);
    }

    #[test]
    fn every_step_word_is_described() {
        let described: Vec<&str> = STEPS.iter().map(|step| step.name).collect();
        let mut words: Vec<&str> = crate::parser::STEP_WORDS.to_vec();
        words.push("pass");
        assert_eq!(described, words);
        let statements: Vec<&str> = STATEMENTS.iter().map(|statement| statement.name).collect();
        assert_eq!(statements, crate::parser::STATEMENT_WORDS);
    }

    #[test]
    fn every_description_is_a_sentence() {
        for builtin in STEPS
            .iter()
            .chain(STATEMENTS)
            .chain(DECORATORS)
            .chain(FUNCTIONS)
            .chain(METHODS)
            .chain(DATE_PARTS)
        {
            assert!(
                builtin.summary.ends_with('.') && builtin.summary.starts_with(char::is_uppercase),
                "{}: {}",
                builtin.name,
                builtin.summary
            );
        }
    }
}
