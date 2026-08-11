mod db;

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = env::args();
    let prog = args.next().unwrap_or_else(|| "my_cli".to_string());
    let rest: Vec<String> = args.collect();
    ExitCode::from(db::sentence_pairs::main(&prog, &rest) as u8)
}
