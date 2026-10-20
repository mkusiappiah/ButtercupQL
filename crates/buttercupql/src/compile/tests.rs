use crate::{
    Column, Dialect, Loader, ModuleName, Options, ParamStyle, Schema, SourceText, Type, Value,
    compile,
};

fn sql_for(text: &str, dialect: Dialect) -> String {
    match compile(text, &Options::new(dialect)) {
        Ok(compiled) => compiled.sql,
        Err(problems) => panic!("{text:?} does not compile for {dialect}: {problems:#?}"),
    }
}

/// The PostgreSQL a text compiles to.
fn pg(text: &str) -> String {
    sql_for(text, Dialect::Postgres)
}

/// The SQLite a text compiles to.
fn lite(text: &str) -> String {
    sql_for(text, Dialect::Sqlite)
}

/// The condition of a query's one `where`, compiled for each database.
fn condition(text: &str) -> (String, String) {
    let query = format!("from t\nwhere {text}\n");
    let strip = |sql: String| {
        sql.strip_prefix("SELECT *\nFROM t\nWHERE ")
            .unwrap_or_else(|| panic!("not a single condition: {sql}"))
            .to_string()
    };
    (strip(pg(&query)), strip(lite(&query)))
}

fn params(text: &str, dialect: Dialect) -> Vec<Value> {
    compile(text, &Options::new(dialect))
        .expect("the text compiles")
        .values()
        .expect("the query has no arguments")
}

fn text(value: &str) -> Value {
    Value::Text(value.to_string())
}

fn schema() -> Schema {
    Schema::new()
        .with_table(
            "cases",
            [
                Column::new("id", Type::Int).not_null(),
                Column::new("key", Type::Text).not_null(),
                Column::new("account_id", Type::Int),
                Column::new("status", Type::Text).not_null(),
                Column::new("band", Type::Text),
                Column::new("risk", Type::Int),
                Column::new("raised", Type::Timestamp).not_null(),
                Column::new("notes", Type::Text),
            ],
        )
        .with_table(
            "accounts",
            [
                Column::new("id", Type::Int).not_null(),
                Column::new("operator", Type::Text).not_null(),
                Column::new("balance", Type::Decimal),
            ],
        )
}

fn with_schema(text: &str, dialect: Dialect) -> Result<String, Vec<String>> {
    let schema = schema();
    compile(text, &Options::new(dialect).with_schema(&schema))
        .map(|compiled| compiled.sql)
        .map_err(|problems| {
            problems
                .into_iter()
                .map(|problem| problem.message)
                .collect()
        })
}

fn problems(text: &str) -> Vec<String> {
    match compile(text, &Options::new(Dialect::Postgres)) {
        Ok(compiled) => panic!("{text:?} compiled to {}", compiled.sql),
        Err(problems) => problems
            .into_iter()
            .map(|problem| problem.message)
            .collect(),
    }
}

// ---- Steps

#[test]
fn a_table_alone_is_every_row() {
    assert_eq!(pg("from cases"), "SELECT *\nFROM cases");
    assert_eq!(pg("from analytics.daily"), "SELECT *\nFROM analytics.daily");
    assert_eq!(pg("from `order lines`"), "SELECT *\nFROM \"order lines\"");
}

#[test]
fn values_are_parameters_and_numbers_are_written() {
    let query = "from cases\nwhere status == \"open\" and risk >= 90\n";
    assert_eq!(
        pg(query),
        "SELECT *\nFROM cases\nWHERE status = $1 AND risk >= 90"
    );
    assert_eq!(
        lite(query),
        "SELECT *\nFROM cases\nWHERE status = ? AND risk >= 90"
    );
    assert_eq!(params(query, Dialect::Postgres), [text("open")]);
}

#[test]
fn a_new_column_is_used_by_the_steps_after_it() {
    assert_eq!(
        pg("from t\nn = risk * 2\nwhere n > 10\nsort -n\n"),
        "SELECT *, risk * 2 AS n\nFROM t\nWHERE risk * 2 > 10\nORDER BY n DESC NULLS LAST"
    );
}

#[test]
fn select_keeps_and_names_columns() {
    assert_eq!(
        pg("from t\nselect key, total = a + b\n"),
        "SELECT key, a + b AS total\nFROM t"
    );
    assert_eq!(
        problems("from t\nselect key, a + b\n"),
        ["this column needs a name"]
    );
}

#[test]
fn sorting_is_stable_and_puts_missing_values_last() {
    assert_eq!(
        pg("from t\nsort a\nsort -b\n"),
        "SELECT *\nFROM t\nORDER BY b DESC NULLS LAST, a"
    );
    assert_eq!(
        lite("from t\nsort a\nsort -b\n"),
        "SELECT *\nFROM t\nORDER BY b DESC, a NULLS LAST"
    );
    assert_eq!(
        pg("from t\nsort a, -b, reverse=True\n"),
        "SELECT *\nFROM t\nORDER BY a DESC NULLS LAST, b"
    );
    // Never missing, so nothing to say about missing values.
    assert_eq!(
        with_schema("from cases\nsort -id\n", Dialect::Postgres).unwrap(),
        "SELECT *\nFROM cases\nORDER BY id DESC"
    );
}

#[test]
fn take_and_skip_combine() {
    assert_eq!(
        pg("from t\ntake 10\nskip 3\n"),
        "SELECT *\nFROM t\nLIMIT 7 OFFSET 3"
    );
    assert_eq!(
        pg("from t\nskip 3\ntake 10\n"),
        "SELECT *\nFROM t\nLIMIT 10 OFFSET 3"
    );
    assert_eq!(pg("from t\ntake 10\ntake 5\n"), "SELECT *\nFROM t\nLIMIT 5");
    assert_eq!(pg("from t\nskip 5\n"), "SELECT *\nFROM t\nOFFSET 5");
    assert_eq!(
        lite("from t\nskip 5\n"),
        "SELECT *\nFROM t\nLIMIT -1 OFFSET 5"
    );
    assert_eq!(
        problems("from t\ntake -1\n"),
        ["expected a whole number of rows, 0 or more"]
    );
    assert_eq!(
        problems("from t\ntake 2.5\n"),
        ["expected a whole number of rows, 0 or more"]
    );
}

#[test]
fn a_where_after_take_filters_the_rows_taken() {
    assert_eq!(
        pg("from t\nsort -risk\ntake 10\nwhere band == \"high\"\n"),
        "\
SELECT *
FROM (
  SELECT *
  FROM t
  ORDER BY risk DESC NULLS LAST
  LIMIT 10
) AS _q1
WHERE band = $1
ORDER BY risk DESC NULLS LAST"
    );
}

#[test]
fn grouping_writes_each_column_once() {
    let query = "\
from transactions
where time > now() - days(30)
group by operator:
    total = count()
    money = sum(amount)
    big = count(where=amount > 1_000_000)
    share = big / total
where money > 1_000_000
sort -money
";
    assert_eq!(
        pg(query),
        "\
SELECT operator, count(*) AS total, coalesce(sum(amount), 0) AS money, count(*) FILTER (WHERE amount > 1000000) AS big, count(*) FILTER (WHERE amount > 1000000) * 1.0 / count(*) AS share
FROM transactions
WHERE time > now() - INTERVAL '30 days'
GROUP BY operator
HAVING coalesce(sum(amount), 0) > 1000000
ORDER BY money DESC"
    );
}

#[test]
fn every_total() {
    let query = "\
from t
summarize:
    rows = count()
    filled = count(x)
    kinds = count_distinct(kind)
    average = mean(x)
    also = avg(x)
    low = min(x)
    high = max(x)
    some = any(flag)
    every = all(flag)
";
    assert_eq!(
        pg(query),
        "SELECT count(*) AS rows, count(x) AS filled, count(DISTINCT kind) AS kinds, avg(x) AS average, avg(x) AS also, min(x) AS low, max(x) AS high, coalesce(bool_or(flag), FALSE) AS \"some\", coalesce(bool_and(flag), TRUE) AS every\nFROM t"
    );
    assert!(
        lite(query)
            .contains("coalesce(max(flag), FALSE) AS \"some\", coalesce(min(flag), TRUE) AS every")
    );
}

#[test]
fn a_total_belongs_in_a_block() {
    assert_eq!(problems("from t\nn = count()\n"), ["`count()` is a total"]);
    assert_eq!(
        problems("from t\nsummarize:\n    n = sum(count())\n"),
        ["a total can't be worked out from another total"]
    );
    assert_eq!(
        problems("from t\ngroup by a:\n    n = b\n"),
        ["a total is worked out from the rows of each group"]
    );
}

#[test]
fn a_second_grouping_reads_the_first() {
    assert_eq!(
        pg(
            "from cases\ngroup by operator:\n    n = count()\nwhere n > 100\nsummarize:\n    operators = count()\n"
        ),
        "\
SELECT count(*) AS operators
FROM (
  SELECT operator, count(*) AS n
  FROM cases
  GROUP BY operator
  HAVING count(*) > 100
) AS _q1"
    );
}

#[test]
fn a_where_after_summarize_reads_its_one_row() {
    assert_eq!(
        pg("from t\nsummarize:\n    n = count()\nwhere n > 5\n"),
        "SELECT *\nFROM (\n  SELECT count(*) AS n\n  FROM t\n) AS _q1\nWHERE n > 5"
    );
}

#[test]
fn a_sort_before_grouping_is_reported_as_doing_nothing() {
    let compiled = compile(
        "from t\nsort a\ngroup by b:\n    n = count()\n",
        &Options::new(Dialect::Postgres),
    )
    .expect("it compiles");
    assert_eq!(compiled.warnings.len(), 1);
    assert_eq!(compiled.warnings[0].message, "this sort has no effect");
    assert!(!compiled.sql.contains("ORDER BY"));
}

#[test]
fn distinct_drops_repeated_rows() {
    assert_eq!(
        pg("from t\nselect a\ndistinct\n"),
        "SELECT DISTINCT a\nFROM t"
    );
    assert_eq!(
        pg("from t\ndistinct\nselect a\n"),
        "SELECT a\nFROM (\n  SELECT DISTINCT *\n  FROM t\n) AS _q1"
    );
}

#[test]
fn joins_read_another_tables_columns() {
    assert_eq!(
        pg(
            "from cases as c\njoin accounts as a on c.account_id == a.id\nwhere a.operator == \"Acme\"\n"
        ),
        "SELECT *\nFROM cases AS c\nJOIN accounts AS a ON c.account_id = a.id\nWHERE a.operator = $1"
    );
    assert_eq!(
        pg("from cases\nwhere status == \"open\"\nleft join notes on notes.case_id == cases.id\n"),
        "SELECT *\nFROM cases\nLEFT JOIN notes ON notes.case_id = cases.id\nWHERE cases.status = $1"
    );
}

#[test]
fn a_join_after_grouping_reads_the_groups() {
    assert_eq!(
        pg(
            "from cases\ngroup by account_id:\n    n = count()\njoin accounts on account_id == accounts.id\n"
        ),
        "\
SELECT *
FROM (
  SELECT account_id, count(*) AS n
  FROM cases
  GROUP BY account_id
) AS _q1
JOIN accounts ON _q1.account_id = accounts.id"
    );
}

#[test]
fn a_named_query_is_written_in_place() {
    let query = "\
open_cases = (
    from cases
    where status == \"open\"
)
from open_cases
group by band:
    n = count()
";
    assert_eq!(
        pg(query),
        "SELECT band, count(*) AS n\nFROM cases\nWHERE status = $1\nGROUP BY band"
    );
    assert_eq!(
        pg(
            "blocked = (from blocks; select account_id)\nfrom cases\nwhere account_id not in blocked\n"
        ),
        "SELECT *\nFROM cases\nWHERE account_id NOT IN (\n  SELECT account_id\n  FROM blocks\n)"
    );
    assert_eq!(
        pg(
            "open_accounts = (from accounts; where open)\nfrom cases\njoin open_accounts on account_id == open_accounts.id\n"
        ),
        "\
SELECT *
FROM cases
JOIN (
  SELECT *
  FROM accounts
  WHERE open
) AS open_accounts ON account_id = open_accounts.id"
    );
}

#[test]
fn a_query_that_uses_itself_is_refused() {
    assert_eq!(problems("a = (from a)\nfrom a\n"), ["`a` uses itself"]);
}

#[test]
fn values_before_the_query_are_used_in_it() {
    assert_eq!(
        pg(
            "threshold = 1_000_000\nwanted = \"open\"\nfrom t\nwhere amount > threshold and status == wanted\n"
        ),
        "SELECT *\nFROM t\nWHERE amount > 1000000 AND status = $1"
    );
}

#[test]
fn defs_put_their_steps_in_place() {
    let query = "\
def recent(days_back=7):
    where raised > now() - days(days_back)

def for_account(account: str, days_back: int = 30):
    from cases
    where account_id == account
    recent(days_back)

from for_account(\"A1\", days_back=3)
recent()
take 5
";
    assert_eq!(
        pg(query),
        "\
SELECT *
FROM cases
WHERE account_id = $1 AND raised > now() - INTERVAL '3 days' AND raised > now() - INTERVAL '7 days'
LIMIT 5"
    );
    assert_eq!(
        problems("def f(n: int):\n    take n\nfrom t\nf(\"x\")\n"),
        ["`n` takes a whole number, but this is text"]
    );
    assert_eq!(
        problems("def f(n):\n    take n\nfrom t\nf()\n"),
        ["`f` needs a value for `n`"]
    );
    assert_eq!(
        problems("def f(n):\n    take n\nfrom t\nf(1, 2)\n"),
        ["`f` takes 1 argument, but 2 were given"]
    );
    assert_eq!(
        problems("from t\nrecnt(3)\n"),
        ["there is no `def` named `recnt`"]
    );
}

// ---- Expressions

#[test]
fn missing_values_mean_what_they_say() {
    assert_eq!(condition("x == None").0, "x IS NULL");
    assert_eq!(condition("x != None").0, "x IS NOT NULL");
    assert_eq!(condition("x is not None").0, "x IS NOT NULL");
    assert_eq!(condition("flag is True").0, "flag IS TRUE");
    assert_eq!(condition("x in [1, None]").0, "x IN (1) OR x IS NULL");
    assert_eq!(
        condition("x not in [1, 2, None]").0,
        "x NOT IN (1, 2) AND x IS NOT NULL"
    );
    assert_eq!(condition("x in []").0, "FALSE");
    assert_eq!(condition("x not in [None]").0, "x IS NOT NULL");
    assert_eq!(
        problems("from t\nwhere x is 5\n"),
        ["`is` compares with `None`, `True` or `False`"]
    );
}

#[test]
fn comparisons_chain_as_in_python() {
    assert_eq!(condition("1 <= risk <= 100").0, "risk BETWEEN 1 AND 100");
    assert_eq!(condition("0 < a < b").0, "0 < a AND a < b");
}

#[test]
fn arithmetic_is_pythons() {
    // `/` divides exactly, and `//` rounds down, even with negative numbers.
    assert_eq!(condition("a / b > 1").0, "a * 1.0 / b > 1");
    assert_eq!(
        condition("x == 7 // 2").0,
        "x = 7 / 2 - CASE WHEN 7 % 2 <> 0 AND (7 < 0) <> (2 < 0) THEN 1 ELSE 0 END"
    );
    assert_eq!(condition("a // b > 1").0, "floor(a * 1.0 / b) > 1");
    assert_eq!(
        condition("a // b > 1").1,
        "CAST(a * 1.0 / b AS INTEGER) - (a * 1.0 / b < CAST(a * 1.0 / b AS INTEGER)) > 1"
    );
    // `%` takes the sign of the divisor.
    assert_eq!(condition("a % 3 == 1").0, "(a % 3 + 3) % 3 = 1");
    assert_eq!(condition("a ** 2 > b ** c").0, "a * a > power(b, c)");
    assert_eq!(condition("-a > 1").0, "-a > 1");
    assert_eq!(condition("a - -1 > 0").0, "a - -1 > 0");
}

#[test]
fn text_is_matched_exactly() {
    let (pg_sql, lite_sql) = condition("\"Acme\" in operator");
    assert_eq!(pg_sql, "strpos(operator, $1) > 0");
    assert_eq!(lite_sql, "instr(operator, ?) > 0");
    assert_eq!(
        condition("\"Acme\" not in operator").0,
        "strpos(operator, $1) = 0"
    );
    let (pg_sql, lite_sql) = condition("name.startswith(\"50%_\")");
    assert_eq!(pg_sql, "name LIKE $1 ESCAPE '\\'");
    assert_eq!(lite_sql, "name GLOB ?");
    assert_eq!(
        params(
            "from t\nwhere name.startswith(\"50%_\")\n",
            Dialect::Postgres
        ),
        [text("50\\%\\_%")]
    );
    assert_eq!(
        params("from t\nwhere name.endswith(\"*.pdf\")\n", Dialect::Sqlite),
        [text("*[*].pdf")]
    );
    assert_eq!(
        condition("name.startswith(prefix)").1,
        "substr(name, 1, length(prefix)) = prefix"
    );
    assert_eq!(condition("name.lower() == \"a\"").0, "lower(name) = $1");
    assert_eq!(condition("name.strip() == \"a\"").0, "trim(name) = $1");
    assert_eq!(
        condition("name.replace(\"-\", \"\") == \"a\"").0,
        "replace(name, $1, $2) = $3"
    );
    assert_eq!(condition("len(name) > 3").0, "length(name) > 3");
}

#[test]
fn f_strings_and_plus_join_text() {
    assert_eq!(
        pg("from t\nlabel = f\"{first} {last}\"\n"),
        "SELECT *, CAST(first AS TEXT) || $1 || CAST(last AS TEXT) AS label\nFROM t"
    );
    assert_eq!(
        pg("from t\nlabel = \"A-\" + key\n"),
        "SELECT *, $1 || key AS label\nFROM t"
    );
    assert_eq!(
        with_schema("from cases\nlabel = key + risk\n", Dialect::Postgres),
        Err(vec!["`+` can't join text and a whole number".to_string()])
    );
}

#[test]
fn dates_and_lengths_of_time() {
    let (pg_sql, lite_sql) = condition("raised > now() - days(7)");
    assert_eq!(pg_sql, "raised > now() - INTERVAL '7 days'");
    assert_eq!(lite_sql, "raised > datetime('now', '-7 days')");
    let (pg_sql, lite_sql) = condition("due < today() + weeks(2)");
    assert_eq!(pg_sql, "due < CURRENT_DATE + 14");
    assert_eq!(lite_sql, "due < date('now', '+14 days')");
    let (pg_sql, lite_sql) = condition("raised + hours(6) + days(1) < now()");
    assert_eq!(
        pg_sql,
        "raised + INTERVAL '6 hours' + INTERVAL '1 days' < now()"
    );
    assert_eq!(
        lite_sql,
        "datetime(raised, '+6 hours', '+1 days') < datetime('now')"
    );
    let (pg_sql, lite_sql) = condition("raised > now() - days(n)");
    assert_eq!(pg_sql, "raised > now() - n * INTERVAL '1 day'");
    assert_eq!(
        lite_sql,
        "raised > datetime('now', CAST(-n AS TEXT) || ' days')"
    );
    let (pg_sql, lite_sql) = condition("raised >= date(\"2026-10-20\")");
    assert_eq!(pg_sql, "raised >= DATE '2026-10-20'");
    assert_eq!(lite_sql, "raised >= '2026-10-20'");
    assert_eq!(
        condition("raised < datetime(\"2026-10-20T14:30\")").1,
        "raised < '2026-10-20 14:30:00'"
    );
    let (pg_sql, lite_sql) = condition("raised.year == 2026");
    assert_eq!(pg_sql, "CAST(EXTRACT(YEAR FROM raised) AS INTEGER) = 2026");
    assert_eq!(lite_sql, "CAST(strftime('%Y', raised) AS INTEGER) = 2026");
    assert_eq!(
        condition("raised.date() == today()").1,
        "date(raised) = date('now')"
    );
    assert_eq!(
        problems("from t\nwhere raised > date(\"2026-02-30\")\n"),
        ["`2026-02-30` is not a valid date"]
    );
    let compiled = compile("from t\nd = days(1)\n", &Options::new(Dialect::Sqlite));
    assert_eq!(
        compiled.unwrap_err()[0].message,
        "SQLite has no type for a length of time"
    );
}

#[test]
fn conditional_values_and_casts() {
    assert_eq!(
        pg("from t\nlabel = \"urgent\" if risk >= 90 else \"normal\"\n"),
        "SELECT *, CASE WHEN risk >= 90 THEN $1 ELSE $2 END AS label\nFROM t"
    );
    assert_eq!(condition("int(x) == 1").0, "CAST(x AS BIGINT) = 1");
    assert_eq!(condition("int(x) == 1").1, "CAST(x AS INTEGER) = 1");
    assert_eq!(condition("str(x) == \"1\"").0, "CAST(x AS TEXT) = $1");
    assert_eq!(
        condition("round(x, 2) > 1").0,
        "round(CAST(x AS NUMERIC), 2) > 1"
    );
    assert_eq!(condition("max(a, b) > 1").0, "greatest(a, b) > 1");
    assert_eq!(condition("max(a, b) > 1").1, "max(a, b) > 1");
}

#[test]
fn a_literal_is_written_into_the_sql() {
    assert_eq!(
        condition("status == literal(\"open's\")").0,
        "status = 'open''s'"
    );
    assert!(
        params(
            "from t\nwhere status == literal(\"open\")\n",
            Dialect::Postgres
        )
        .is_empty()
    );
}

#[test]
fn where_takes_pythons_truth() {
    let schema = Schema::new().with_table(
        "t",
        [
            Column::new("n", Type::Int),
            Column::new("name", Type::Text),
            Column::new("flag", Type::Bool),
        ],
    );
    let compiled = |text: &str| {
        compile(text, &Options::new(Dialect::Postgres).with_schema(&schema))
            .expect("it compiles")
            .sql
    };
    assert_eq!(
        compiled("from t\nwhere n\n"),
        "SELECT *\nFROM t\nWHERE n <> 0"
    );
    assert_eq!(
        compiled("from t\nwhere name and flag\n"),
        "SELECT *\nFROM t\nWHERE name <> $1 AND flag"
    );
    assert_eq!(
        compiled("from t\nwhere not n\n"),
        "SELECT *\nFROM t\nWHERE NOT (n <> 0)"
    );
}

// ---- With a schema

#[test]
fn names_are_checked_against_the_schema() {
    let problems =
        with_schema("from cases\nwhere stauts == \"open\"\n", Dialect::Postgres).unwrap_err();
    assert_eq!(problems, ["there is no column `stauts` in `cases`"]);
    let schema = schema();
    let found = compile(
        "from cases\nwhere stauts == \"open\"\n",
        &Options::new(Dialect::Postgres).with_schema(&schema),
    )
    .unwrap_err();
    assert_eq!(found[0].label, "did you mean `status`?");
    assert_eq!(
        with_schema("from case\n", Dialect::Postgres).unwrap_err(),
        ["there is no table `case`"]
    );
    assert_eq!(
        with_schema(
            "from cases\njoin accounts on account_id == accounts.id\nwhere id > 1\n",
            Dialect::Postgres
        )
        .unwrap_err(),
        ["`id` is a column of both `cases` and `accounts`"]
    );
    assert_eq!(
        with_schema("from cases\njoin accounts on account_id == accounts.id\nwhere accounts.opertor == \"A\"\n", Dialect::Postgres)
            .unwrap_err(),
        ["there is no column `opertor` in `accounts`"]
    );
}

#[test]
fn a_schema_lets_columns_be_replaced_and_dropped() {
    assert_eq!(
        with_schema("from accounts\nbalance = balance * 2\n", Dialect::Postgres).unwrap(),
        "SELECT id, operator, balance * 2 AS balance\nFROM accounts"
    );
    assert_eq!(
        with_schema("from accounts\ndel balance\n", Dialect::Postgres).unwrap(),
        "SELECT id, operator\nFROM accounts"
    );
    assert_eq!(
        problems("from accounts\nbalance = balance * 2\n"),
        ["a new value for the column `balance` needs the table's columns"]
    );
    assert_eq!(
        problems("from accounts\ndel balance\n"),
        ["`del` needs the table's columns"]
    );
}

#[test]
fn a_schema_knows_the_kinds_of_values() {
    assert_eq!(
        with_schema("from accounts\nwhere balance / 2 > 1\n", Dialect::Postgres).unwrap(),
        "SELECT *\nFROM accounts\nWHERE balance / 2 > 1"
    );
    assert_eq!(
        with_schema("from cases\nwhere status > 5\n", Dialect::Postgres).unwrap_err(),
        ["Text can't be compared with a whole number"]
    );
    assert_eq!(
        with_schema("from Cases\nwhere Status == \"open\"\n", Dialect::Postgres).unwrap(),
        "SELECT *\nFROM cases\nWHERE status = $1"
    );
}

// ---- Placeholders

#[test]
fn placeholders_are_written_as_the_driver_takes_them() {
    let query = "from t\nwhere a == \"x\" and b % 2 == 1\n";
    let format = compile(
        query,
        &Options::new(Dialect::Postgres).with_params(ParamStyle::Format),
    )
    .unwrap();
    assert_eq!(
        format.sql,
        "SELECT *\nFROM t\nWHERE a = %s AND (b %% 2 + 2) %% 2 = 1"
    );
    let colon = compile(
        query,
        &Options::new(Dialect::Sqlite).with_params(ParamStyle::Colon),
    )
    .unwrap();
    assert!(colon.sql.contains("a = :p1"));
}

#[test]
fn compiling_needs_a_query() {
    assert_eq!(problems("x = 1\n"), ["this file has no query to compile"]);
    assert_eq!(
        problems("from t\nwhere x ==\n"),
        ["expected a value after `==`"]
    );
}

#[test]
fn unknown_functions_get_a_suggestion() {
    let schema = schema();
    let found = compile(
        "from cases\nsummarize:\n    n = cont()\n",
        &Options::new(Dialect::Postgres).with_schema(&schema),
    )
    .unwrap_err();
    assert_eq!(found[0].message, "there is no function `cont`");
    assert_eq!(found[0].label, "did you mean `count`?");
    let method = compile(
        "from t\nwhere name.lowr() == \"a\"\n",
        &Options::new(Dialect::Postgres),
    )
    .unwrap_err();
    assert_eq!(method[0].label, "did you mean `.lower()`?");
}

// ---- DuckDB and ClickHouse

fn duck(text: &str) -> String {
    sql_for(text, Dialect::DuckDb)
}

fn click(text: &str) -> String {
    sql_for(text, Dialect::ClickHouse)
}

#[test]
fn duckdb_divides_exactly_and_sorts_missing_values_last_itself() {
    assert_eq!(
        duck("from t\nwhere a / b > 1 and status == \"open\"\nsort -risk\n"),
        "SELECT *\nFROM t\nWHERE a / b > 1 AND status = $1\nORDER BY risk DESC"
    );
    assert_eq!(
        duck("from t\nx = 7 // 2\n"),
        "SELECT *, CAST(floor(7 / 2) AS BIGINT) AS x\nFROM t"
    );
    assert_eq!(
        duck("from t\nwhere raised > now() - days(7) and due < today() + weeks(2)\n"),
        "SELECT *\nFROM t\nWHERE raised > now() - INTERVAL '7 days' AND due < CURRENT_DATE + 14"
    );
}

#[test]
fn clickhouse_has_its_own_forms() {
    assert_eq!(
        click("from t\nwhere status == \"open\" and \"Acme\" in operator\n"),
        "SELECT *\nFROM t\nWHERE status = {p1:String} AND position(operator, {p2:String}) > 0"
    );
    assert_eq!(
        click("from t\nwhere name.startswith(\"50%\") and not name.endswith(\"_x\")\n"),
        "SELECT *\nFROM t\nWHERE startsWith(name, {p1:String}) AND NOT endsWith(name, {p2:String})"
    );
    assert_eq!(
        click(
            "from t\nwhere raised > now() - days(7) and due < today() + weeks(2) and raised < date(\"2026-10-20\")\n"
        ),
        "SELECT *\nFROM t\nWHERE raised > now() - INTERVAL 7 DAY AND due < today() + INTERVAL 14 DAY AND raised < toDate('2026-10-20')"
    );
    assert_eq!(
        click("from t\nwhere raised.year == 2026 and len(name.lower()) > 3\n"),
        "SELECT *\nFROM t\nWHERE toYear(raised) = 2026 AND lengthUTF8(lowerUTF8(name)) > 3"
    );
    assert_eq!(
        click("from t\nlabel = f\"{key}:{risk}\"\nwhole = 7 // 2\n"),
        "SELECT *, toString(key) || {p1:String} || toString(risk) AS label, toInt64(floor(7 / 2)) AS whole\nFROM t"
    );
    assert_eq!(
        click("from t\nwhere flag is True and other is not False\n"),
        "SELECT *\nFROM t\nWHERE ifNull(flag, false) AND ifNull(other, true)"
    );
}

#[test]
fn clickhouse_totals_take_their_conditions_in_if_functions() {
    let query = "\
from transactions
group by operator:
    big = count(where=amount > 1_000_000)
    money = sum(amount, where=kind == \"in\")
    filled = count(note, where=amount > 0)
    kinds = count_distinct(kind, where=amount > 0)
";
    assert_eq!(
        click(query),
        "SELECT operator, countIf(amount > 1000000) AS big, coalesce(sumIf(amount, kind = {p1:String}), 0) AS money, countIf(note IS NOT NULL AND amount > 0) AS filled, uniqExactIf(kind, amount > 0) AS kinds\nFROM transactions\nGROUP BY operator"
    );
}

#[test]
fn clickhouse_totals_of_no_rows_are_missing_as_in_sql() {
    assert_eq!(
        click(
            "from t\nsummarize:\n    high = max(x)\n    average = mean(x)\n    every = all(flag)\n    low = min(x, where=x > 1)\n"
        ),
        "SELECT maxOrNull(x) AS high, avgOrNull(x) AS average, coalesce(minOrNull(flag), TRUE) AS every, minIfOrNull(x, x > 1) AS low\nFROM t"
    );
}

#[test]
fn clickhouse_is_told_that_rows_with_no_match_are_missing() {
    assert_eq!(
        click(
            "from cases\nleft join accounts on account_id == accounts.id\nwhere accounts.id is None\n"
        ),
        "SELECT *\nFROM cases\nLEFT JOIN accounts ON account_id = accounts.id\nWHERE accounts.id IS NULL\nSETTINGS join_use_nulls = 1"
    );
}

// ---- Window functions

#[test]
fn window_functions_take_over_and_sort() {
    assert_eq!(
        pg("from t\nplace = row_number(over=operator, sort=-risk)\n"),
        "SELECT *, row_number() OVER (PARTITION BY operator ORDER BY risk DESC NULLS LAST) AS place\nFROM t"
    );
    assert_eq!(
        pg("from t\nso_far = sum(amount, sort=time)\n"),
        "SELECT *, coalesce(sum(amount) OVER (ORDER BY time ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW), 0) AS so_far\nFROM t"
    );
    assert_eq!(
        pg("from t\nweek = mean(amount, over=[operator, kind], sort=day, rows=7)\n"),
        "SELECT *, avg(amount) OVER (PARTITION BY operator, kind ORDER BY day ROWS BETWEEN 6 PRECEDING AND CURRENT ROW) AS week\nFROM t"
    );
    assert_eq!(
        pg("from t\nshare = amount / sum(amount, over=operator)\nall_rows = count(over=())\n"),
        "SELECT *, amount * 1.0 / coalesce(sum(amount) OVER (PARTITION BY operator), 0) AS share, count(*) OVER () AS all_rows\nFROM t"
    );
    assert_eq!(
        pg("from t\nbefore = lag(risk, sort=raised)\nafter = lead(risk, 2, sort=raised, default=0)\nfirst_seen = first(raised, over=account, sort=raised)\n"),
        "SELECT *, lag(risk, 1) OVER (ORDER BY raised NULLS LAST) AS before, lead(risk, 2, 0) OVER (ORDER BY raised NULLS LAST) AS after, first_value(raised) OVER (PARTITION BY account ORDER BY raised NULLS LAST ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS first_seen\nFROM t".replace("raised NULLS LAST", "raised")
    );
    assert_eq!(
        click("from t\nbefore = lag(risk, sort=raised)\n"),
        "SELECT *, lagInFrame(toNullable(risk), 1) OVER (ORDER BY raised ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS before\nFROM t"
    );
}

#[test]
fn a_condition_on_a_window_value_comes_after_it() {
    let query = "from t\nplace = rank(over=operator, sort=-risk)\nwhere place <= 3\n";
    assert_eq!(
        pg(query),
        "\
SELECT *
FROM (
  SELECT *, rank() OVER (PARTITION BY operator ORDER BY risk DESC NULLS LAST) AS place
  FROM t
) AS _q1
WHERE place <= 3"
    );
    assert_eq!(
        duck(query),
        "SELECT *, rank() OVER (PARTITION BY operator ORDER BY risk DESC) AS place\nFROM t\nQUALIFY rank() OVER (PARTITION BY operator ORDER BY risk DESC) <= 3"
    );
    // Any condition after a window value: the ranks are of all the rows.
    assert!(
        pg("from t\nplace = rank(sort=-risk)\nwhere band == \"high\"\n")
            .contains(") AS _q1\nWHERE band = $1")
    );
    assert_eq!(
        problems("from t\nwhere rank(sort=-risk) <= 3\n"),
        ["a window value in a condition needs a name of its own"]
    );
}

#[test]
fn a_window_counts_the_rows_the_steps_before_kept() {
    assert_eq!(
        pg("from t\nsort -risk\ntake 5\nplace = row_number(sort=-risk)\n"),
        "\
SELECT *, row_number() OVER (ORDER BY risk DESC NULLS LAST) AS place
FROM (
  SELECT *
  FROM t
  ORDER BY risk DESC NULLS LAST
  LIMIT 5
) AS _q1
ORDER BY risk DESC NULLS LAST"
    );
    assert_eq!(
        pg("from t\ngroup by operator:\n    money = sum(amount)\nplace = rank(sort=-money)\n"),
        "SELECT operator, coalesce(sum(amount), 0) AS money, rank() OVER (ORDER BY coalesce(sum(amount), 0) DESC) AS place\nFROM t\nGROUP BY operator"
    );
}

#[test]
fn window_values_are_checked() {
    assert_eq!(
        problems("from t\nbefore = lag(risk)\n"),
        ["`lag()` needs `sort=` to know which row comes before"]
    );
    assert_eq!(
        problems("from t\ngroup by a:\n    place = rank(sort=-b)\n"),
        ["a window value can't be one of a block's totals"]
    );
    assert_eq!(
        problems("from t\nn = sum(x, rows=3)\n"),
        ["`rows=` needs `sort=`, to know which rows come before"]
    );
    assert_eq!(
        problems("from t\nn = sum(x, sort=t, rows=0)\n"),
        ["`rows=` takes how many rows the window holds, 1 or more"]
    );
    assert_eq!(
        problems("from t\nn = count_distinct(x, over=a)\n"),
        ["`count_distinct()` can't be worked out over a window"]
    );
}

// ---- Links

fn linked() -> Schema {
    Schema::from_sql(
        "\
CREATE TABLE banks (id bigint PRIMARY KEY, name text NOT NULL);
CREATE TABLE accounts (id bigint PRIMARY KEY, operator text NOT NULL, bank_id bigint REFERENCES banks);
CREATE TABLE cases (id bigint PRIMARY KEY, key text NOT NULL, account_id bigint REFERENCES accounts (id), risk integer);",
    )
    .expect("the schema reads")
}

fn with_links(text: &str) -> Result<String, Vec<String>> {
    let schema = linked();
    compile(text, &Options::new(Dialect::Postgres).with_schema(&schema))
        .map(|compiled| compiled.sql)
        .map_err(|problems| {
            problems
                .into_iter()
                .map(|problem| problem.message)
                .collect()
        })
}

#[test]
fn a_link_is_followed_with_a_dot() {
    assert_eq!(
        with_links("from cases\nwhere account.operator == \"Acme\"\n").unwrap(),
        "SELECT cases.*\nFROM cases\nLEFT JOIN accounts AS account ON cases.account_id = account.id\nWHERE account.operator = $1"
    );
    assert_eq!(
        with_links("from cases\nselect key, bank = account.bank.name\n").unwrap(),
        "\
SELECT cases.key, account_bank.name AS bank
FROM cases
LEFT JOIN accounts AS account ON cases.account_id = account.id
LEFT JOIN banks AS account_bank ON account.bank_id = account_bank.id"
    );
    assert_eq!(
        with_links("from cases\ngroup by operator = account.operator:\n    n = count()\n").unwrap(),
        "SELECT account.operator, count(*) AS n\nFROM cases\nLEFT JOIN accounts AS account ON cases.account_id = account.id\nGROUP BY account.operator"
    );
}

#[test]
fn a_link_after_grouping_reads_the_groups() {
    assert_eq!(
        with_links("from cases\ngroup by account_id:\n    n = count()\nwhere account.operator == \"Acme\"\n").unwrap(),
        "\
SELECT _q1.*
FROM (
  SELECT account_id, count(*) AS n
  FROM cases
  GROUP BY account_id
) AS _q1
LEFT JOIN accounts AS account ON _q1.account_id = account.id
WHERE account.operator = $1"
    );
}

#[test]
fn links_are_checked() {
    assert_eq!(
        with_links("from cases\nwhere account.opertor == \"A\"\n").unwrap_err(),
        ["there is no column `opertor` in `account`"]
    );
    assert_eq!(
        problems("from cases\nwhere account.operator == \"A\"\n"),
        ["there is no table `account` here"]
    );
}

// ---- Queries with arguments

#[test]
fn a_query_def_compiles_with_its_arguments_open() {
    let text = "\
def for_account(account: str, days_back: int = 30, rows: int = 10):
    from cases
    where account_id == account or payee == account
    where raised > now() - days(days_back)
    take rows
";
    let compiled = compile(
        text,
        &Options::new(Dialect::Postgres).with_query("for_account"),
    )
    .unwrap();
    assert_eq!(
        compiled.sql,
        "SELECT *\nFROM cases\nWHERE (account_id = $1 OR payee = $1) AND raised > now() - $2 * INTERVAL '1 day'\nLIMIT $3"
    );
    assert_eq!(compiled.names, ["account", "days_back", "rows"]);
    assert_eq!(
        compiled.bind(&[("account", text_value("A1"))]).unwrap(),
        [text_value("A1"), Value::Int(30), Value::Int(10)]
    );
    assert_eq!(
        compiled.bind(&[]).unwrap_err().to_string(),
        "the query needs a value for `account`"
    );
    assert_eq!(
        compiled
            .bind(&[("acount", text_value("A1"))])
            .unwrap_err()
            .to_string(),
        "the query has no argument `acount`: it takes account, days_back, rows"
    );
    let clickhouse = compile(
        text,
        &Options::new(Dialect::ClickHouse).with_query("for_account"),
    )
    .unwrap();
    assert_eq!(
        clickhouse.sql,
        "SELECT *\nFROM cases\nWHERE (account_id = {account:String} OR payee = {account:String}) AND raised > now() - toIntervalDay({days_back:Int64})\nLIMIT {rows:Int64}"
    );
    let named = clickhouse
        .bind_named(&[("account", text_value("A1"))])
        .unwrap();
    assert_eq!(
        named
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["account", "days_back", "rows"]
    );
}

fn text_value(value: &str) -> Value {
    Value::Text(value.to_string())
}

#[test]
fn a_named_query_can_be_compiled_alone() {
    let text = "open_cases = (from cases; where status == \"open\")\nfrom open_cases\ntake 5\n";
    let compiled = compile(
        text,
        &Options::new(Dialect::Sqlite).with_query("open_cases"),
    )
    .unwrap();
    assert_eq!(compiled.sql, "SELECT *\nFROM cases\nWHERE status = ?");
    let missing =
        compile(text, &Options::new(Dialect::Sqlite).with_query("open_case")).unwrap_err();
    assert_eq!(missing[0].message, "there is no query named `open_case`");
    assert_eq!(missing[0].label, "did you mean `open_cases`?");
}

// ---- Imports

struct Modules(Vec<(&'static str, &'static str)>);

impl Loader for Modules {
    fn load(&self, module: &ModuleName<'_>, _importer: &str) -> Result<SourceText, String> {
        let name = module.parts.join(".");
        self.0
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(known, text)| SourceText {
                name: format!("{known}.bcql"),
                text: text.to_string(),
            })
            .ok_or_else(|| format!("there is no module `{name}`"))
    }
}

const SHARED: &str = "\
threshold = 1_000_000

def recent(days_back=7):
    where raised > now() - days(days_back)

open_cases = (
    from cases
    where status == \"open\"
)
";

fn with_modules(text: &str, modules: &Modules) -> Result<String, Vec<String>> {
    compile(text, &Options::new(Dialect::Postgres).with_loader(modules))
        .map(|compiled| compiled.sql)
        .map_err(|problems| {
            problems
                .into_iter()
                .map(|problem| problem.message)
                .collect()
        })
}

#[test]
fn imports_bring_a_modules_definitions() {
    let modules = Modules(vec![("shared", SHARED)]);
    assert_eq!(
        with_modules("from shared import recent, threshold\nfrom cases\nrecent(30)\nwhere amount > threshold\n", &modules).unwrap(),
        "SELECT *\nFROM cases\nWHERE raised > now() - INTERVAL '30 days' AND amount > 1000000"
    );
    assert_eq!(
        with_modules(
            "import shared\nfrom shared.open_cases\nshared.recent()\n",
            &modules
        )
        .unwrap(),
        "SELECT *\nFROM cases\nWHERE status = $1 AND raised > now() - INTERVAL '7 days'"
    );
    assert_eq!(
        with_modules(
            "import shared as s\nfrom cases\nwhere amount > s.threshold\n",
            &modules
        )
        .unwrap(),
        "SELECT *\nFROM cases\nWHERE amount > 1000000"
    );
}

#[test]
fn import_problems_are_reported_where_they_are() {
    let modules = Modules(vec![
        ("shared", SHARED),
        ("broken", "def f():\n    where x =\n"),
        ("a", "from b import x\nx = 1\n"),
        ("b", "from a import x\n"),
    ]);
    assert_eq!(
        with_modules("from shared import recnt\nfrom cases\n", &modules).unwrap_err(),
        ["`shared.bcql` has no `recnt`"]
    );
    assert_eq!(
        with_modules("import nope\nfrom cases\n", &modules).unwrap_err(),
        ["can't import `nope`"]
    );
    let broken = compile(
        "import broken\nfrom cases\n",
        &Options::new(Dialect::Postgres).with_loader(&modules),
    )
    .unwrap_err();
    assert_eq!(
        broken[0].file.as_ref().map(|file| file.name.as_str()),
        Some("broken.bcql")
    );
    assert!(
        with_modules("import a\nfrom cases\n", &modules)
            .unwrap_err()
            .contains(&"`a` imports itself".to_string())
    );
    assert_eq!(
        problems("import shared\nfrom cases\n"),
        ["`shared` can't be imported here"]
    );
}

#[test]
fn a_def_reads_its_own_modules_names() {
    // `recent` uses the module's `cutoff`, not the importing file's.
    let modules = Modules(vec![(
        "dates",
        "cutoff = 3\ndef recent():\n    where age < cutoff\n",
    )]);
    assert_eq!(
        with_modules(
            "from dates import recent\ncutoff = 99\nfrom cases\nrecent()\nwhere size < cutoff\n",
            &modules
        )
        .unwrap(),
        "SELECT *\nFROM cases\nWHERE age < 3 AND size < 99"
    );
}
