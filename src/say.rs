//! Text-to-speech helper that shells out to the `piper` CLI (from the
//! `piper-tts` package, e.g. via `uv tool install piper-tts`) and `aplay`
//! for English and Russian.
//!
//! Setup:
//!     `piper` must be on `$PATH`. Download the .onnx + .onnx.json pair for
//!     each language in [`language_spec`] from
//!     https://huggingface.co/rhasspy/piper-voices, mirroring its directory
//!     structure under ~/.local/share/piper-voices/ (e.g. en/en_US/amy/medium/...).
//!
//! Usage:
//!     cargo run -- say -l ru "Война и миръ Графъ Л. Н. Толстой"

use std::env;
use std::fmt;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Output, Stdio};

/// Audio sample rate (Hz) produced by the voice models in [`language_spec`]
/// and [`voice_spec`], and the rate `say` asks `aplay` to play back at.
const SAMPLE_RATE: u32 = 22050;

/// Duration of silence `say` plays before the synthesized audio, so playback
/// devices that take a moment to wake up don't clip the start of the speech.
const LEAD_IN_SECONDS: f64 = 0.5;

/// Paths to a Piper voice's ONNX model and its accompanying config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceSpec {
    pub model: PathBuf,
    pub model_config: PathBuf,
}

/// Error returned when resolving a voice or invoking `piper`/`aplay`.
#[derive(Debug)]
pub enum SayError {
    UnsupportedLanguage(String),
    UnsupportedVoice { lang: String, voice: String },
    HomeNotSet,
    MissingFile { field: &'static str, path: PathBuf },
    Io { program: &'static str, source: io::Error },
    ProcessFailed { program: &'static str, status: ExitStatus, stderr: String },
}

impl fmt::Display for SayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedLanguage(lang) => write!(f, "Unsupported language: {lang}"),
            Self::UnsupportedVoice { lang, voice } => {
                write!(f, "Unsupported voice for {lang}: {voice}")
            }
            Self::HomeNotSet => write!(f, "HOME environment variable is not set"),
            Self::MissingFile { field, path } => {
                write!(f, "{field} not found at {}", path.display())
            }
            Self::Io { program, source } => write!(f, "failed to run {program}: {source}"),
            Self::ProcessFailed { program, status, stderr } => {
                if stderr.trim().is_empty() {
                    write!(f, "{program} failed ({status})")
                } else {
                    write!(f, "{program} failed ({status}): {}", stderr.trim())
                }
            }
        }
    }
}

impl std::error::Error for SayError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

fn home_dir() -> Result<PathBuf, SayError> {
    env::var_os("HOME").map(PathBuf::from).ok_or(SayError::HomeNotSet)
}

/// Return the default `VoiceSpec` for `lang`, or `None` if `lang` isn't supported.
fn language_spec(home: &Path, lang: &str) -> Option<VoiceSpec> {
    match lang {
        "en" => Some(VoiceSpec {
            model: home.join(".local/share/piper-voices/en/en_US/amy/medium/en_US-amy-medium.onnx"),
            model_config: home
                .join(".local/share/piper-voices/en/en_US/amy/medium/en_US-amy-medium.onnx.json"),
        }),
        "ru" => Some(VoiceSpec {
            model: home.join(".local/share/piper-voices/ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx"),
            model_config: home
                .join(".local/share/piper-voices/ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx.json"),
        }),
        _ => None,
    }
}

/// Return the named-voice variant `VoiceSpec` for `lang`/`voice`, or `None`
/// if that combination isn't supported. Languages with only one voice (i.e.
/// no entry here) fall back to [`language_spec`] instead.
fn voice_spec(home: &Path, lang: &str, voice: &str) -> Option<VoiceSpec> {
    match (lang, voice) {
        ("ru", "irina") => Some(VoiceSpec {
            model: home.join(".local/share/piper-voices/ru/ru_RU/irina/medium/ru_RU-irina-medium.onnx"),
            model_config: home
                .join(".local/share/piper-voices/ru/ru_RU/irina/medium/ru_RU-irina-medium.onnx.json"),
        }),
        ("ru", "denis") => language_spec(home, "ru"),
        _ => None,
    }
}

/// Return the `VoiceSpec` for `lang`, or its `voice` variant if given.
///
/// Returns [`SayError::UnsupportedVoice`] for a `voice` not recognized for
/// `lang` (even if `lang` itself is supported), or
/// [`SayError::UnsupportedLanguage`] for an unsupported `lang` when no
/// `voice` is given.
pub fn get_voice_spec(home: &Path, lang: &str, voice: Option<&str>) -> Result<VoiceSpec, SayError> {
    match voice {
        Some(voice) => voice_spec(home, lang, voice).ok_or_else(|| SayError::UnsupportedVoice {
            lang: lang.to_string(),
            voice: voice.to_string(),
        }),
        None => language_spec(home, lang).ok_or_else(|| SayError::UnsupportedLanguage(lang.to_string())),
    }
}

/// Return `spec` after checking that its model files exist.
pub fn validate_paths(spec: VoiceSpec) -> Result<VoiceSpec, SayError> {
    for (field, path) in [("Model", &spec.model), ("Model config", &spec.model_config)] {
        if !path.exists() {
            return Err(SayError::MissingFile { field, path: path.clone() });
        }
    }
    Ok(spec)
}

/// Run `command` with `input` written to its stdin, returning its captured
/// stdout/stderr/status once it exits.
///
/// Writing happens on a separate thread so a child that fills its stdout or
/// stderr pipe before finishing reading stdin (or vice versa) can't deadlock
/// against us.
fn run_with_stdin(mut command: Command, input: Vec<u8>) -> io::Result<Output> {
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output()?;
    let _ = writer.join();
    Ok(output)
}

fn run_program(program: &'static str, command: Command, input: Vec<u8>) -> Result<Vec<u8>, SayError> {
    let output = run_with_stdin(command, input).map_err(|source| SayError::Io { program, source })?;
    if !output.status.success() {
        return Err(SayError::ProcessFailed {
            program,
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(output.stdout)
}

/// Render `text` to raw S16LE mono PCM audio at [`SAMPLE_RATE`], without playing it.
pub fn synthesize(lang: &str, text: &str, voice: Option<&str>) -> Result<Vec<u8>, SayError> {
    let home = home_dir()?;
    let spec = validate_paths(get_voice_spec(&home, lang, voice)?)?;

    let mut command = Command::new("piper");
    command
        .arg("-m")
        .arg(&spec.model)
        .arg("-c")
        .arg(&spec.model_config)
        .arg("--output-raw");

    run_program("piper", command, text.as_bytes().to_vec())
}

/// Return zeroed S16LE mono PCM audio of the requested duration at [`SAMPLE_RATE`].
fn silence(duration_seconds: f64) -> Vec<u8> {
    let num_samples = (f64::from(SAMPLE_RATE) * duration_seconds).round() as usize;
    vec![0u8; num_samples * 2]
}

fn say_inner(lang: &str, text: &str, voice: Option<&str>) -> Result<(), SayError> {
    let audio = synthesize(lang, text, voice)?;
    let mut input = silence(LEAD_IN_SECONDS);
    input.extend_from_slice(&audio);

    let mut command = Command::new("aplay");
    command.args(["-q", "-c", "1", "-r", &SAMPLE_RATE.to_string(), "-f", "S16_LE", "-t", "raw"]);

    run_program("aplay", command, input)?;
    Ok(())
}

/// Synthesize `text` in `lang` (optionally a specific `voice`) and play it
/// immediately via `aplay`.
///
/// Errors (missing models, synthesis/playback failure) are caught and
/// reported to stderr rather than returned, so callers can fire-and-forget.
pub fn say(lang: &str, text: &str, voice: Option<&str>) {
    if let Err(err) = say_inner(lang, text, voice) {
        eprintln!("Error during playback: {err}");
    }
}

/// Flags parsed from `say`'s CLI arguments, before stdin fallback is applied.
struct ParsedFlags {
    lang: String,
    voice: Option<String>,
    text_arg: Option<String>,
}

fn parse_flags(args: &[String]) -> Result<ParsedFlags, String> {
    let mut lang = "en".to_string();
    let mut voice: Option<String> = None;
    let mut text_arg: Option<String> = None;
    let mut args = args.iter().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-l" | "--lang" => {
                let value = args.next().ok_or("--lang requires a value")?;
                if value != "en" && value != "ru" {
                    return Err(format!(
                        "invalid choice for --lang: {value:?} (choose from \"en\", \"ru\")"
                    ));
                }
                lang = value.clone();
            }
            "-v" | "--voice" => {
                let value = args.next().ok_or("--voice requires a value")?;
                voice = Some(value.clone());
            }
            other if text_arg.is_none() => {
                text_arg = Some(other.to_string());
            }
            other => return Err(format!("unexpected argument: {other:?}")),
        }
    }
    Ok(ParsedFlags { lang, voice, text_arg })
}

/// CLI entry point: `[-l|--lang en|ru] [-v|--voice VOICE] [text]`.
///
/// Speaks `text` (or, if omitted, stdin) via [`say`]. Reports a usage error
/// if neither a positional `text` argument nor piped stdin is available.
pub fn main(prog: &str, args: &[String]) -> ExitCode {
    let flags = match parse_flags(args) {
        Ok(flags) => flags,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("usage: {prog} say [-l|--lang en|ru] [-v|--voice VOICE] [text]");
            return ExitCode::FAILURE;
        }
    };

    let text = match flags.text_arg {
        Some(text) => text.trim().to_string(),
        None if !io::stdin().is_terminal() => {
            let mut buf = String::new();
            if let Err(err) = io::stdin().read_to_string(&mut buf) {
                eprintln!("error: failed to read stdin: {err}");
                return ExitCode::FAILURE;
            }
            buf.trim().to_string()
        }
        None => {
            eprintln!(
                "usage: {prog} say [-l|--lang en|ru] [-v|--voice VOICE] <text>  (or provide text via stdin)"
            );
            return ExitCode::FAILURE;
        }
    };

    if text.is_empty() {
        eprintln!("Warning: Received empty text input. Nothing to speak.");
        return ExitCode::SUCCESS;
    }

    say(&flags.lang, &text, flags.voice.as_deref());
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn language_spec_supports_en_and_ru() {
        let home = Path::new("/home/test");
        assert!(language_spec(home, "en").is_some());
        assert!(language_spec(home, "ru").is_some());
        assert!(language_spec(home, "fr").is_none());
    }

    #[test]
    fn voice_spec_ru_named_voices() {
        let home = Path::new("/home/test");
        assert!(voice_spec(home, "ru", "irina").is_some());
        assert_eq!(voice_spec(home, "ru", "denis"), language_spec(home, "ru"));
        assert!(voice_spec(home, "ru", "unknown").is_none());
        assert!(voice_spec(home, "en", "irina").is_none());
    }

    #[test]
    fn voice_spec_irina_resolves_to_its_own_model_files() {
        let home = Path::new("/home/test");
        let spec = voice_spec(home, "ru", "irina").unwrap();
        assert_eq!(
            spec.model,
            home.join(".local/share/piper-voices/ru/ru_RU/irina/medium/ru_RU-irina-medium.onnx")
        );
        assert_eq!(
            spec.model_config,
            home.join(".local/share/piper-voices/ru/ru_RU/irina/medium/ru_RU-irina-medium.onnx.json")
        );
        // irina is its own voice, not an alias for the ru default (denis).
        assert_ne!(spec, language_spec(home, "ru").unwrap());
    }

    #[test]
    fn get_voice_spec_falls_back_to_language_default() {
        let home = Path::new("/home/test");
        assert_eq!(
            get_voice_spec(home, "en", None).unwrap(),
            language_spec(home, "en").unwrap()
        );
    }

    #[test]
    fn get_voice_spec_rejects_unsupported_language() {
        let home = Path::new("/home/test");
        assert!(get_voice_spec(home, "fr", None).is_err());
    }

    #[test]
    fn get_voice_spec_rejects_unsupported_voice_even_for_supported_language() {
        let home = Path::new("/home/test");
        let err = get_voice_spec(home, "en", Some("irina")).unwrap_err().to_string();
        assert!(err.contains("en"), "error message was: {err}");
        assert!(err.contains("irina"), "error message was: {err}");
    }

    #[test]
    fn get_voice_spec_uses_named_voice_when_given() {
        let home = Path::new("/home/test");
        assert_eq!(
            get_voice_spec(home, "ru", Some("irina")).unwrap(),
            voice_spec(home, "ru", "irina").unwrap()
        );
    }

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, contents: &[u8]) -> Self {
            let path =
                std::env::temp_dir().join(format!("say_test_{}_{name}", std::process::id()));
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
    fn validate_paths_ok_when_files_exist() {
        let model = TempFile::new("model.onnx", b"");
        let config = TempFile::new("model.onnx.json", b"");
        let spec = VoiceSpec { model: model.0.clone(), model_config: config.0.clone() };
        assert_eq!(validate_paths(spec.clone()).unwrap(), spec);
    }

    #[test]
    fn validate_paths_errors_on_missing_model() {
        let spec = VoiceSpec {
            model: PathBuf::from("/nonexistent/say-test/model.onnx"),
            model_config: PathBuf::from("/nonexistent/say-test/model.onnx.json"),
        };
        let err = validate_paths(spec).unwrap_err().to_string();
        assert!(err.starts_with("Model not found"), "error message was: {err}");
    }

    #[test]
    fn validate_paths_errors_on_missing_config_even_if_model_exists() {
        let model = TempFile::new("model2.onnx", b"");
        let spec = VoiceSpec {
            model: model.0.clone(),
            model_config: PathBuf::from("/nonexistent/say-test/model2.onnx.json"),
        };
        let err = validate_paths(spec).unwrap_err().to_string();
        assert!(err.starts_with("Model config not found"), "error message was: {err}");
    }

    #[test]
    fn silence_produces_correct_byte_length_and_is_zeroed() {
        let bytes = silence(1.0);
        assert_eq!(bytes.len(), SAMPLE_RATE as usize * 2);
        assert!(bytes.iter().all(|&b| b == 0));
    }

    #[test]
    fn silence_scales_with_duration() {
        assert_eq!(silence(0.5).len(), silence(1.0).len() / 2);
    }

    #[test]
    fn parse_flags_defaults_to_en_with_no_lang_flag() {
        let flags = parse_flags(&["hello".to_string()]).unwrap();
        assert_eq!(flags.lang, "en");
        assert_eq!(flags.voice, None);
        assert_eq!(flags.text_arg.as_deref(), Some("hello"));
    }

    #[test]
    fn parse_flags_accepts_short_and_long_lang_flag() {
        let flags = parse_flags(&["-l".to_string(), "ru".to_string(), "привет".to_string()]).unwrap();
        assert_eq!(flags.lang, "ru");
        assert_eq!(flags.text_arg.as_deref(), Some("привет"));

        let flags = parse_flags(&["--lang".to_string(), "ru".to_string()]).unwrap();
        assert_eq!(flags.lang, "ru");
        assert_eq!(flags.text_arg, None);
    }

    #[test]
    fn parse_flags_rejects_invalid_lang_choice() {
        assert!(parse_flags(&["-l".to_string(), "fr".to_string()]).is_err());
    }

    #[test]
    fn parse_flags_rejects_missing_lang_value() {
        assert!(parse_flags(&["-l".to_string()]).is_err());
    }

    #[test]
    fn parse_flags_accepts_short_and_long_voice_flag() {
        let flags = parse_flags(&["-v".to_string(), "irina".to_string(), "hi".to_string()]).unwrap();
        assert_eq!(flags.voice.as_deref(), Some("irina"));
        assert_eq!(flags.text_arg.as_deref(), Some("hi"));

        let flags = parse_flags(&["--voice".to_string(), "irina".to_string()]).unwrap();
        assert_eq!(flags.voice.as_deref(), Some("irina"));
    }

    #[test]
    fn parse_flags_combines_lang_and_voice_flags() {
        let flags = parse_flags(&[
            "-l".to_string(),
            "ru".to_string(),
            "-v".to_string(),
            "irina".to_string(),
            "привет".to_string(),
        ])
        .unwrap();
        assert_eq!(flags.lang, "ru");
        assert_eq!(flags.voice.as_deref(), Some("irina"));
        assert_eq!(flags.text_arg.as_deref(), Some("привет"));
    }

    #[test]
    fn parse_flags_rejects_missing_voice_value() {
        assert!(parse_flags(&["-v".to_string()]).is_err());
    }

    #[test]
    fn parse_flags_rejects_second_positional_argument() {
        assert!(parse_flags(&["one".to_string(), "two".to_string()]).is_err());
    }

    #[test]
    fn parse_flags_allows_missing_text_argument() {
        let flags = parse_flags(&[]).unwrap();
        assert_eq!(flags.lang, "en");
        assert_eq!(flags.voice, None);
        assert_eq!(flags.text_arg, None);
    }
}
