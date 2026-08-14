mod db;
mod say;

use std::env;
use std::process::ExitCode;

const SUBCOMMANDS: &str = "sentence-pairs, say";

fn main() -> ExitCode {
    let mut args = env::args();
    let prog = args.next().unwrap_or_else(|| "my_cli".to_string());
    let rest: Vec<String> = args.collect();

    let Some((subcommand, rest)) = rest.split_first() else {
        eprintln!("usage: {prog} <subcommand> [args...]");
        eprintln!("subcommands: {SUBCOMMANDS}");
        return ExitCode::FAILURE;
    };

    match subcommand.as_str() {
        "sentence-pairs" => db::sentence_pairs::main(&prog, rest),
        "say" => say::main(&prog, rest),
        other => {
            eprintln!("error: unknown subcommand {other:?}");
            eprintln!("subcommands: {SUBCOMMANDS}");
            ExitCode::FAILURE
        }
    }
}
