//! `bcql`, the ButtercupQL command.

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;

use buttercupql::{Severity, SourceFile};

const HELP: &str = "\
bcql: the ButtercupQL command

Usage:
  bcql check [--color WHEN] FILE...
  bcql --version
  bcql --help

Commands:
  check    Read each file and report every mistake in it

Options:
  --color WHEN    Colour the messages: auto, always or never [default: auto]

A FILE of `-` is read from standard input. `bcql check` exits with 0 when no
file has an error, with 1 when one has, and with 2 when the command is wrong.
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
            "there is no command `{other}`: the command is `check`"
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

fn check(args: &[String]) -> Result<ExitCode, String> {
    let mut color = Color::Auto;
    let mut files = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg
            .strip_prefix("--color=")
            .or_else(|| arg.strip_prefix("--colour="))
        {
            color = Color::read(value)?;
        } else if arg == "--color" || arg == "--colour" {
            let value = args.next().ok_or("`--color` takes auto, always or never")?;
            color = Color::read(value)?;
        } else if arg == "-h" || arg == "--help" {
            print!("{HELP}");
            return Ok(ExitCode::SUCCESS);
        } else if arg.starts_with('-') && arg != "-" {
            return Err(format!("unknown option `{arg}`"));
        } else {
            files.push(arg.as_str());
        }
    }
    if files.is_empty() {
        return Err("`bcql check` takes the files to check".to_string());
    }

    let color = color.wanted();
    let (mut errors, mut warnings, mut unreadable) = (0, 0, 0);
    for path in &files {
        let (name, text) = match read(path) {
            Ok(file) => file,
            Err(message) => {
                eprintln!("bcql: {message}");
                unreadable += 1;
                continue;
            }
        };
        let parsed = buttercupql::parse(&text);
        let source = SourceFile::new(&name, &text);
        for diagnostic in &parsed.diagnostics {
            eprintln!("{}", diagnostic.render(&source, color));
            match diagnostic.severity {
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
        count(files.len(), "file"),
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
