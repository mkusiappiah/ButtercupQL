# Rust

ButtercupQL is written in Rust, and the `buttercupql` crate is the compiler. It has no dependencies.

```toml
[dependencies]
buttercupql = { git = "https://github.com/mkusiappiah/ButtercupQL" }
```

## Compiling

`compile()` takes the text and the options: the database, and optionally a schema, a loader for modules, a style of
placeholder and a query to compile in place of the main one.

```rust
use buttercupql::{Dialect, Options, Value};

let text = "from cases\nwhere status == \"open\"\nsort -risk\ntake 20\n";
let compiled = buttercupql::compile(text, &Options::new(Dialect::Postgres)).unwrap();
assert_eq!(
    compiled.sql,
    "SELECT *\nFROM cases\nWHERE status = $1\nORDER BY risk DESC NULLS LAST\nLIMIT 20"
);
assert_eq!(compiled.values().unwrap(), [Value::Text("open".to_string())]);
```

On failure, `compile()` returns every problem found. `Diagnostic::render()` shows one as `bcql` does, under its line:

```rust
use buttercupql::{Dialect, Options, SourceFile};

let text = "from cases\nwhere status = \"open\"\n";
let problems = buttercupql::compile(text, &Options::new(Dialect::Sqlite)).unwrap_err();
let shown = problems[0].render(&SourceFile::new("open.bcql", text), false);
assert!(shown.starts_with("error: use `==` to compare\n  ┌─ open.bcql:2:14\n"));
```

## Options

```rust
use buttercupql::{Dialect, FileLoader, Options, ParamStyle, Schema};

let schema = Schema::from_sql("CREATE TABLE cases (key text NOT NULL, risk integer);").unwrap();
let loader = FileLoader::new("queries");
let options = Options::new(Dialect::DuckDb)
    .with_schema(&schema)                  // check names, and use what the tables say
    .with_name("queries/report.bcql")      // the name messages show, and where imports are found from
    .with_loader(&loader)                  // where modules come from
    .with_params(ParamStyle::Colon)        // `:p1` placeholders
    .with_query("for_operator");           // a named query or a query `def`
```

## Queries with arguments

A query `def` compiled with `with_query()` keeps its parameters as placeholders, and `bind()` gives the values, in
order, with defaults for those not given:

```rust
use buttercupql::{Dialect, Options, Value};

let text = "def for_account(account: str, rows: int = 10):\n    from cases\n    where account_id == account\n    take rows\n";
let compiled = buttercupql::compile(text, &Options::new(Dialect::Postgres).with_query("for_account")).unwrap();
assert_eq!(compiled.sql, "SELECT *\nFROM cases\nWHERE account_id = $1\nLIMIT $2");
let values = compiled.bind(&[("account", Value::Text("A1".to_string()))]).unwrap();
assert_eq!(values, [Value::Text("A1".to_string()), Value::Int(10)]);
```

## Reading without compiling

`parse()` reads a text into its syntax tree, `buttercupql::ast`, with every problem found, as editors do while you
type. `buttercupql::builtins` describes every step, function and method.

The crate's own documentation, with `cargo doc -p buttercupql --open`, describes each of these.
