//! `bcql`, the ButtercupQL command.

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use std::path::Path;

use buttercupql::{
    Diagnostic, Dialect, FileLoader, Options, Param, ParamStyle, Schema, Severity, SourceFile,
    Value,
};

const HELP: &str = "\
bcql: the ButtercupQL command

Usage:
  bcql compile [OPTIONS] FILE
  bcql check [OPTIONS] FILE...
  bcql --version
  bcql --help

Commands:
  compile    Write the SQL a query compiles to, and its parameters
  check      Read each file and report every mistake in it

Options:
  --dialect NAME    The database: sqlite, postgres, duckdb or clickhouse
                    [default: postgres]
  --schema FILE     The database's CREATE TABLE statements, to check names against
  --query NAME      Compile a named query or a query def of the file, with the
                    def's parameters left as placeholders
  --params STYLE    Placeholders: dollar ($1), question (?), format (%s),
                    colon (:p1) or braces ({p1:String}) [default: the database's own]
  --json            Write the SQL and its parameters as JSON
  --color WHEN      Colour the messages: auto, always or never [default: auto]

A FILE of `-` is read from standard input. Both commands exit with 0 when
there is no error, with 1 when there is one, and with 2 when the command is
wrong.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("bcql: {message}");
            eprintln!("Run `bcql --help` to see how to use it.");
            ExitCode::from(2)
        }
    }
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let Some((command, rest)) = args.split_first() else {
        eprint!("{HELP}");
        return Ok(ExitCode::from(2));
    };
    match command.as_str() {
        "compile" => compile(rest),
        "check" => check(rest),
        "-h" | "--help" | "help" => {
            print!("{HELP}");
            Ok(ExitCode::SUCCESS)
        }
        "-V" | "--version" | "version" => {
            println!("bcql {}", buttercupql::VERSION);
            Ok(ExitCode::SUCCESS)
        }
        other if other.starts_with('-') => Err(format!("unknown option `{other}`")),
        other => Err(format!(
            "there is no command `{other}`: the commands are `compile` and `check`"
        )),
    }
}

/// When to colour messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    Auto,
    Always,
    Never,
}

impl Color {
    fn read(value: &str) -> Result<Color, String> {
        match value {
            "auto" => Ok(Color::Auto),
            "always" => Ok(Color::Always),
            "never" => Ok(Color::Never),
            other => Err(format!(
                "`--color` takes auto, always or never, not `{other}`"
            )),
        }
    }

    /// Whether to colour the messages: in a terminal, unless `NO_COLOR` is set or the terminal can't show colour.
    fn wanted(self) -> bool {
        match self {
            Color::Always => true,
            Color::Never => false,
            Color::Auto => {
                io::stderr().is_terminal()
                    && std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
                    && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
            }
        }
    }
}

/// The options both commands take, and the files named.
struct Args {
    dialect: Dialect,
    schema: Option<String>,
    query: Option<String>,
    params: Option<ParamStyle>,
    json: bool,
    color: Color,
    files: Vec<String>,
    help: bool,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args {
        dialect: Dialect::Postgres,
        schema: None,
        query: None,
        params: None,
        json: false,
        color: Color::Auto,
        files: Vec::new(),
        help: false,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let (option, inline) = match arg.split_once('=') {
            Some((option, value)) if option.starts_with("--") => (option, Some(value.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            match &inline {
                Some(value) => Ok(value.clone()),
                None => args
                    .next()
                    .cloned()
                    .ok_or_else(|| format!("`{option}` takes {what}")),
            }
        };
        match option {
            "--dialect" => {
                parsed.dialect = value("a database: sqlite or postgres")?
                    .parse()
                    .map_err(|error: buttercupql::UnknownDialect| error.to_string())?;
            }
            "--schema" => parsed.schema = Some(value("a file of CREATE TABLE statements")?),
            "--query" => parsed.query = Some(value("the name of a query or a query def")?),
            "--params" => {
                parsed.params = Some(
                    match value("a style: dollar, question, format or colon")?.as_str() {
                        "dollar" => ParamStyle::Dollar,
                        "question" | "qmark" => ParamStyle::Question,
                        "format" => ParamStyle::Format,
                        "colon" | "named" => ParamStyle::Colon,
                        "braces" => ParamStyle::Braces,
                        other => {
                            return Err(format!(
                                "`--params` takes dollar, question, format, colon or braces, not `{other}`"
                            ));
                        }
                    },
                );
            }
            "--color" | "--colour" => parsed.color = Color::read(&value("auto, always or never")?)?,
            "--json" => parsed.json = true,
            "-h" | "--help" => parsed.help = true,
            "-" => parsed.files.push(arg.clone()),
            _ if arg.starts_with('-') => return Err(format!("unknown option `{arg}`")),
            _ => parsed.files.push(arg.clone()),
        }
    }
    Ok(parsed)
}

/// Reads the schema named by `--schema`, showing its problems.
fn schema(args: &Args, color: bool) -> Result<Option<Schema>, ExitCode> {
    let Some(path) = &args.schema else {
        return Ok(None);
    };
    let (name, text) = read(path).map_err(|message| {
        eprintln!("bcql: {message}");
        ExitCode::FAILURE
    })?;
    match Schema::from_sql(&text) {
        Ok(schema) => Ok(Some(schema)),
        Err(problems) => {
            let source = SourceFile::new(&name, &text);
            for problem in &problems {
                eprintln!("{}", problem.render(&source, color));
            }
            eprintln!("bcql: the schema in {name} can't be read");
            Err(ExitCode::FAILURE)
        }
    }
}

fn compile(args: &[String]) -> Result<ExitCode, String> {
    let args = parse_args(args)?;
    if args.help {
        print!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    }
    let [path] = args.files.as_slice() else {
        return Err("`bcql compile` takes one file".to_string());
    };
    let color = args.color.wanted();
    let schema = match schema(&args, color) {
        Ok(schema) => schema,
        Err(code) => return Ok(code),
    };
    let (name, text) = match read(path) {
        Ok(file) => file,
        Err(message) => {
            eprintln!("bcql: {message}");
            return Ok(ExitCode::FAILURE);
        }
    };
    let loader = loader_for(&name);
    let mut options = Options::new(args.dialect)
        .with_name(&name)
        .with_loader(&loader);
    if let Some(schema) = &schema {
        options = options.with_schema(schema);
    }
    if let Some(style) = args.params {
        options = options.with_params(style);
    }
    if let Some(query) = &args.query {
        options = options.with_query(query);
    }
    let source = SourceFile::new(&name, &text);
    match buttercupql::compile(&text, &options) {
        Ok(compiled) => {
            show(&compiled.warnings, &source, color);
            let params: Vec<String> = compiled.params.iter().map(json_param).collect();
            if args.json {
                let arguments: Vec<String> = compiled
                    .args
                    .iter()
                    .map(|arg| {
                        let default = arg.default.as_ref().map_or("null".to_string(), json_value);
                        format!(
                            "{{\"name\": {}, \"type\": {}, \"default\": {default}}}",
                            json_string(&arg.name),
                            json_string(arg.ty.describe())
                        )
                    })
                    .collect();
                println!(
                    "{{\"sql\": {}, \"params\": [{}], \"args\": [{}]}}",
                    json_string(&compiled.sql),
                    params.join(", "),
                    arguments.join(", ")
                );
            } else {
                println!("{}", compiled.sql);
                if !compiled.params.is_empty() {
                    println!("-- parameters: [{}]", params.join(", "));
                }
                if !compiled.args.is_empty() {
                    let arguments: Vec<String> = compiled
                        .args
                        .iter()
                        .map(|arg| match &arg.default {
                            Some(default) => format!("{} = {}", arg.name, json_value(default)),
                            None => arg.name.clone(),
                        })
                        .collect();
                    println!("-- arguments: {}", arguments.join(", "));
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(problems) => {
            show(&problems, &source, color);
            let errors = problems
                .iter()
                .filter(|problem| problem.severity == Severity::Error)
                .count();
            eprintln!("bcql: {} in {name}", count(errors, "error"));
            Ok(ExitCode::FAILURE)
        }
    }
}

/// A loader for the modules a file imports: absolute imports from the file's folder.
fn loader_for(name: &str) -> FileLoader {
    let folder = Path::new(name)
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
        .map_or_else(|| Path::new(".").to_path_buf(), Path::to_path_buf);
    FileLoader::new(folder)
}

fn show(problems: &[Diagnostic], source: &SourceFile<'_>, color: bool) {
    for problem in problems {
        eprintln!("{}", problem.render(source, color));
    }
}

fn check(args: &[String]) -> Result<ExitCode, String> {
    let args = parse_args(args)?;
    if args.help {
        print!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    }
    if args.files.is_empty() {
        return Err("`bcql check` takes the files to check".to_string());
    }
    let color = args.color.wanted();
    let schema = match schema(&args, color) {
        Ok(schema) => schema,
        Err(code) => return Ok(code),
    };
    let (mut errors, mut warnings, mut unreadable) = (0, 0, 0);
    for path in &args.files {
        let (name, text) = match read(path) {
            Ok(file) => file,
            Err(message) => {
                eprintln!("bcql: {message}");
                unreadable += 1;
                continue;
            }
        };
        let parsed = buttercupql::parse(&text);
        // A file with a query is compiled too, for the mistakes only compiling finds; a file of definitions alone
        // is checked as it reads.
        let problems = if parsed.has_errors() || parsed.file.query.is_none() {
            parsed.diagnostics
        } else {
            let loader = loader_for(&name);
            let mut options = Options::new(args.dialect)
                .with_name(&name)
                .with_loader(&loader);
            if let Some(schema) = &schema {
                options = options.with_schema(schema);
            }
            match buttercupql::compile(&text, &options) {
                Ok(compiled) => compiled.warnings,
                Err(problems) => problems,
            }
        };
        let source = SourceFile::new(&name, &text);
        show(&problems, &source, color);
        for problem in &problems {
            match problem.severity {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
            }
        }
    }

    let mut found = Vec::new();
    if errors > 0 {
        found.push(count(errors, "error"));
    }
    if warnings > 0 {
        found.push(count(warnings, "warning"));
    }
    if unreadable > 0 {
        found.push(format!("{} could not be read", count(unreadable, "file")));
    }
    if found.is_empty() {
        found.push("no problems found".to_string());
    }
    eprintln!(
        "checked {}: {}",
        count(args.files.len(), "file"),
        found.join(", ")
    );
    Ok(if errors > 0 || unreadable > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// A file's name and its text; `-` is standard input.
fn read(path: &str) -> Result<(String, String), String> {
    let (name, bytes) = if path == "-" {
        let mut bytes = Vec::new();
        io::stdin()
            .read_to_end(&mut bytes)
            .map_err(|error| format!("can't read standard input: {error}"))?;
        ("<stdin>".to_string(), bytes)
    } else {
        let bytes = std::fs::read(path).map_err(|error| format!("can't read {path}: {error}"))?;
        (path.to_string(), bytes)
    };
    let text = String::from_utf8(bytes).map_err(|_| format!("{name} is not UTF-8 text"))?;
    Ok((name, text))
}

/// `1 file`, `2 files`.
fn count(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

fn json_param(param: &Param) -> String {
    match param {
        Param::Value(value) => json_value(value),
        Param::Arg(name) => format!("{{\"arg\": {}}}", json_string(name)),
    }
}

fn json_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Int(value) => value.to_string(),
        Value::Float(value) if value.is_finite() => value.to_string(),
        Value::Float(_) => "null".to_string(),
        Value::Text(text) => json_string(text),
    }
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
