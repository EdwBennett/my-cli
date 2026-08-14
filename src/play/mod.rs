//! Play, print, or render (to mp3) English/Russian sentence pairs.
//!
//! CLI entry point: `play <id> <delay> [clause] [--ru-voice irina|denis]
//! [-o OUTPUT | -t | -i]`.

mod lang;
mod wait;

use std::fmt;
use std::io;
use std::process::{Command, ExitCode};

use crate::db::sentence_pairs::{parse_id_list, IdListError};
use crate::say::{self, SayError};

/// A step in a [`play`] sequence: prints/speaks something (or waits), and
/// may fail (unknown sentence-pair id, out-of-range clause, playback error).
type PlayFn = Box<dyn Fn() -> Result<(), PlayError>>;

/// Error returned while playing, printing, or rendering sentence pairs.
#[derive(Debug)]
pub enum PlayError {
    PairNotFound(u64),
    ClauseOutOfRange { clause: usize, available: usize },
    Say(SayError),
    Termios(io::Error),
    IdList(IdListError),
    InvalidOutputExtension(String),
    Ffmpeg(String),
}

impl fmt::Display for PlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PairNotFound(id) => write!(f, "no sentence pair with id {id}"),
            Self::ClauseOutOfRange { clause, available } => {
                write!(f, "clause {clause} out of range (sentence has {available} clause(s))")
            }
            Self::Say(err) => write!(f, "{err}"),
            Self::Termios(err) => write!(f, "failed to configure the terminal: {err}"),
            Self::IdList(err) => write!(f, "{err}"),
            Self::InvalidOutputExtension(output) => {
                write!(f, "output path must end with .mp3: {output}")
            }
            Self::Ffmpeg(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for PlayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Say(err) => Some(err),
            Self::Termios(err) => Some(err),
            Self::IdList(err) => Some(err),
            _ => None,
        }
    }
}

/// Run a call/wait/response/wait sequence; any step may be omitted. Stops
/// (and returns the error) at the first step that fails.
fn play(
    fn_call: Option<PlayFn>,
    fn_wait_before: Option<PlayFn>,
    fn_response: Option<PlayFn>,
    fn_wait_after: Option<PlayFn>,
) -> Result<(), PlayError> {
    for step in [fn_call, fn_wait_before, fn_response, fn_wait_after].into_iter().flatten() {
        step()?;
    }
    Ok(())
}

/// Synthesize every id's en/ru pair (with delay silence) and encode straight
/// to mp3 via `ffmpeg`. Skips live playback entirely, so this only takes as
/// long as synthesis and encoding, not the real-time duration of the audio
/// and delay.
fn render_to_mp3(
    ids: &[u64],
    delay: u64,
    clause: Option<usize>,
    output: &str,
    ru_voice: &str,
) -> Result<(), PlayError> {
    if !output.ends_with(".mp3") {
        return Err(PlayError::InvalidOutputExtension(output.to_string()));
    }

    let delay_silence = say::silence(delay as f64);
    let mut audio = Vec::new();
    for &id in ids {
        audio.extend(lang::render_en(id, clause)?);
        audio.extend(&delay_silence);
        audio.extend(lang::render_ru(id, clause, Some(ru_voice))?);
        audio.extend(&delay_silence);
    }

    let mut command = Command::new("ffmpeg");
    command.args([
        "-y",
        "-hide_banner",
        "-loglevel", "error",
        "-f", "s16le",
        "-ar", &say::SAMPLE_RATE.to_string(),
        "-ac", "1",
        "-i", "-",
        output,
    ]);

    let result = say::run_with_stdin(command, audio)
        .map_err(|err| PlayError::Ffmpeg(format!("failed to run ffmpeg: {err}")))?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        return Err(PlayError::Ffmpeg(format!(
            "ffmpeg failed ({}): {}",
            result.status,
            stderr.trim()
        )));
    }
    Ok(())
}

/// Flags and positionals parsed from `play`'s CLI arguments.
#[derive(Debug)]
struct ParsedArgs {
    id: String,
    delay: u64,
    clause: Option<usize>,
    ru_voice: String,
    output: Option<String>,
    text_only: bool,
    interactive: bool,
}

fn parse_args(args: &[String]) -> Result<ParsedArgs, String> {
    let mut positionals: Vec<String> = Vec::new();
    let mut ru_voice = "denis".to_string();
    let mut output: Option<String> = None;
    let mut text_only = false;
    let mut interactive = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--ru-voice" => {
                let value = iter.next().ok_or("--ru-voice requires a value")?;
                if value != "irina" && value != "denis" {
                    return Err(format!(
                        "invalid choice for --ru-voice: {value:?} (choose from \"irina\", \"denis\")"
                    ));
                }
                ru_voice = value.clone();
            }
            "-o" | "--output" => {
                let value = iter.next().ok_or("--output requires a value")?;
                output = Some(value.clone());
            }
            "-t" | "--text-only" => text_only = true,
            "-i" | "--interactive" => interactive = true,
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unexpected argument: {other:?}"));
            }
            other => positionals.push(other.to_string()),
        }
    }

    let mode_count = [output.is_some(), text_only, interactive].into_iter().filter(|&b| b).count();
    if mode_count > 1 {
        return Err("--output, --text-only, and --interactive are mutually exclusive".to_string());
    }

    if positionals.len() < 2 || positionals.len() > 3 {
        return Err(format!(
            "expected 2 or 3 positional arguments (id, delay, [clause]), got {}",
            positionals.len()
        ));
    }

    let id = positionals[0].clone();
    let delay: u64 = positionals[1]
        .parse()
        .map_err(|_| format!("invalid delay: {:?}", positionals[1]))?;
    let clause = match positionals.get(2) {
        Some(value) => Some(
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid clause: {value:?}"))?,
        ),
        None => None,
    };

    Ok(ParsedArgs { id, delay, clause, ru_voice, output, text_only, interactive })
}

fn run(parsed: &ParsedArgs) -> Result<(), PlayError> {
    let ids = parse_id_list(&parsed.id).map_err(PlayError::IdList)?;

    if let Some(output) = &parsed.output {
        return render_to_mp3(&ids, parsed.delay, parsed.clause, output, &parsed.ru_voice);
    }

    if parsed.text_only {
        for &id in &ids {
            play(
                Some(lang::print_en_only(id, parsed.clause)),
                None,
                Some(lang::print_ru_only(id, parsed.clause)),
                None,
            )?;
        }
        return Ok(());
    }

    for &id in &ids {
        let fn_call = lang::play_en(id, parsed.clause);
        let fn_response = lang::play_ru(id, parsed.clause, Some(parsed.ru_voice.clone()));

        let (fn_wait_before, fn_wait_after): (PlayFn, PlayFn) = if parsed.interactive {
            let replay_id = id;
            let replay_clause = parsed.clause;
            let replay_voice = parsed.ru_voice.clone();
            let repeat_fn = lang::play_ru(replay_id, replay_clause, Some(replay_voice));
            (wait::play_wait_key(None, None), wait::play_wait_key(Some('r'), Some(repeat_fn)))
        } else {
            (wait::play_wait(parsed.delay), wait::play_wait(parsed.delay))
        };

        play(Some(fn_call), Some(fn_wait_before), Some(fn_response), Some(fn_wait_after))?;
    }
    Ok(())
}

/// CLI entry point: `<id> <delay> [clause] [--ru-voice irina|denis]
/// [-o OUTPUT | -t | -i]`.
pub fn main(prog: &str, args: &[String]) -> ExitCode {
    let usage = || {
        eprintln!(
            "usage: {prog} play <id> <delay> [clause] [--ru-voice irina|denis] [-o OUTPUT | -t | -i]"
        );
    };

    let parsed = match parse_args(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("error: {message}");
            usage();
            return ExitCode::FAILURE;
        }
    };

    match run(&parsed) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_defaults_ru_voice_to_denis_with_no_flags() {
        let parsed = parse_args(&["1".to_string(), "2".to_string()]).unwrap();
        assert_eq!(parsed.id, "1");
        assert_eq!(parsed.delay, 2);
        assert_eq!(parsed.clause, None);
        assert_eq!(parsed.ru_voice, "denis");
        assert_eq!(parsed.output, None);
        assert!(!parsed.text_only);
        assert!(!parsed.interactive);
    }

    #[test]
    fn parse_args_accepts_optional_clause_positional() {
        let parsed = parse_args(&["1".to_string(), "2".to_string(), "3".to_string()]).unwrap();
        assert_eq!(parsed.clause, Some(3));
    }

    #[test]
    fn parse_args_rejects_invalid_ru_voice_choice() {
        let err = parse_args(&[
            "--ru-voice".to_string(),
            "amy".to_string(),
            "1".to_string(),
            "2".to_string(),
        ])
        .unwrap_err();
        assert!(err.contains("amy"), "error message was: {err}");
    }

    #[test]
    fn parse_args_rejects_multiple_output_modes() {
        let err = parse_args(&[
            "1".to_string(),
            "2".to_string(),
            "-t".to_string(),
            "-i".to_string(),
        ])
        .unwrap_err();
        assert!(err.contains("mutually exclusive"), "error message was: {err}");
    }

    #[test]
    fn parse_args_rejects_wrong_positional_count() {
        assert!(parse_args(&["1".to_string()]).is_err());
        assert!(parse_args(&[
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
            "4".to_string()
        ])
        .is_err());
    }

    #[test]
    fn parse_args_rejects_non_numeric_delay() {
        assert!(parse_args(&["1".to_string(), "soon".to_string()]).is_err());
    }

    #[test]
    fn main_succeeds_in_text_only_mode_without_needing_piper_or_aplay() {
        // Text-only mode never calls say(), so it's a deterministic success
        // path that doesn't depend on piper/aplay being installed.
        let args = ["1".to_string(), "0".to_string(), "-t".to_string()];
        assert_eq!(main("prog", &args), ExitCode::SUCCESS);
    }

    #[test]
    fn main_fails_in_text_only_mode_for_unknown_id() {
        let args = ["999".to_string(), "0".to_string(), "-t".to_string()];
        assert_eq!(main("prog", &args), ExitCode::FAILURE);
    }

    #[test]
    fn main_fails_deterministically_for_unknown_id_without_needing_piper_or_aplay() {
        // The first step of a normal (non-text-only) sequence is play_en,
        // which fails looking up the id before ever shelling out to piper
        // or aplay, so this exercises run()'s error propagation without
        // needing those binaries installed.
        let args = ["999".to_string(), "0".to_string()];
        assert_eq!(main("prog", &args), ExitCode::FAILURE);
    }

    #[test]
    fn main_reports_usage_error_on_bad_args() {
        assert_eq!(main("prog", &["1".to_string()]), ExitCode::FAILURE);
    }

    #[test]
    fn render_to_mp3_rejects_output_not_ending_in_mp3() {
        let err = render_to_mp3(&[1], 0, None, "out.wav", "denis").unwrap_err();
        assert!(matches!(err, PlayError::InvalidOutputExtension(_)));
    }
}
