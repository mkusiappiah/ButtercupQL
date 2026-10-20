//! `bcql`, the ButtercupQL command.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(bcql::main(&args))
}
