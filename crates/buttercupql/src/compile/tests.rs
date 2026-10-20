use crate::{Column, Dialect, Options, ParamStyle, Schema, Type, Value, compile};

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
        .params
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
fn compiling_needs_a_query_and_a_supported_database() {
    assert_eq!(problems("x = 1\n"), ["this file has no query to compile"]);
    let duck = compile("from t", &Options::new(Dialect::DuckDb)).unwrap_err();
    assert_eq!(duck[0].message, "compiling for duckdb is not supported yet");
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
