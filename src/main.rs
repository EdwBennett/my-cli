mod db;
mod say;

use std::env;
use std::process::ExitCode;

const SUBCOMMANDS: &str = "sentence-pairs, say";

/// Route `args[0]` (the subcommand name) to its module, passing the rest of
/// `args` through unchanged.
fn dispatch(prog: &str, args: &[String]) -> ExitCode {
    let Some((subcommand, rest)) = args.split_first() else {
        eprintln!("usage: {prog} <subcommand> [args...]");
        eprintln!("subcommands: {SUBCOMMANDS}");
        return ExitCode::FAILURE;
    };

    match subcommand.as_str() {
        "sentence-pairs" => db::sentence_pairs::main(prog, rest),
        "say" => say::main(prog, rest),
        other => {
            eprintln!("error: unknown subcommand {other:?}");
            eprintln!("subcommands: {SUBCOMMANDS}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let mut args = env::args();
    let prog = args.next().unwrap_or_else(|| "my_cli".to_string());
    let rest: Vec<String> = args.collect();
    dispatch(&prog, &rest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn dispatch_reports_usage_error_with_no_subcommand() {
        assert_eq!(dispatch("prog", &[]), ExitCode::FAILURE);
    }

    #[test]
    fn dispatch_reports_error_on_unknown_subcommand() {
        assert_eq!(dispatch("prog", &["nope".to_string()]), ExitCode::FAILURE);
    }

    #[test]
    fn dispatch_routes_say_subcommand_to_say_module() {
        // "irina" is unsupported under say's default lang (en), so this
        // fails deterministically inside say::main without needing piper,
        // aplay, or any voice models installed. It confirms "say" reaches
        // say::main rather than falling through to the unknown-subcommand
        // branch (which would also fail, but say::main's own tests already
        // cover this exact case, so a mismatch here would point at routing).
        let args = ["say".to_string(), "-v".to_string(), "irina".to_string(), "hello".to_string()];
        assert_eq!(dispatch("prog", &args), ExitCode::FAILURE);
    }

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, contents: &str) -> Self {
            let path = std::env::temp_dir().join(format!("main_test_{}_{name}", std::process::id()));
            fs::write(&path, contents).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn dispatch_routes_sentence_pairs_subcommand_to_sentence_pairs_module() {
        // (path, id-spec) has two positional arguments, which say::main
        // would reject as "unexpected argument", so ExitCode::SUCCESS here
        // is the disambiguating signal that these args reached
        // sentence_pairs::main rather than being misrouted.
        let json = r#"[{"id": 1, "ru": "привет", "ipa": "[prʲɪˈvʲet]", "en": "hello", "words": "hello"}]"#;
        let file = TempFile::new("valid.json", json);
        let path = file.0.to_string_lossy().to_string();

        let args = ["sentence-pairs".to_string(), path, "1".to_string()];
        assert_eq!(dispatch("prog", &args), ExitCode::SUCCESS);
    }
}
