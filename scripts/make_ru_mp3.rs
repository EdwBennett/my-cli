//! Render Russian sentences from the bundled dataset to a single mp3.
//!
//! Each sentence is spoken in the chosen voice (denis by default) with a
//! 1-second gap between entries, plus a 0.5s lead-in on each clip so
//! playback devices that take a moment to wake up don't clip the start of
//! the speech (the actual gap heard is ~1.5s, matching the `play`
//! subcommand's convention).
//!
//! Standalone by design: no dependency on this repo's `src/` crate and no
//! external crates (it hand-rolls the tiny bit of JSON parsing it needs
//! instead of pulling in serde), so it can be copied out and run on its own
//! with plain `rustc`. It does embed a copy of the bundled sentence-pairs
//! dataset at compile time — that's data, not code, and keeps this script
//! in sync with the dataset without hand-copying it.
//!
//! Usage:
//!     rustc --edition 2024 scripts/make_ru_mp3.rs -o /tmp/make_ru_mp3
//!     /tmp/make_ru_mp3 -o ru_sentences.mp3
//!     /tmp/make_ru_mp3 1,3,5-8 -o subset.mp3
//!     /tmp/make_ru_mp3 --voice irina -o ru_sentences.mp3

use std::collections::HashMap;
use std::env;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Output, Stdio};

const SENTENCE_PAIRS_JSON: &str = include_str!("../src/db/50_russian_english_ipa_words.json");

const SAMPLE_RATE: u32 = 22050;
const LEAD_IN_SECONDS: f64 = 0.5;
const GAP_SECONDS: f64 = 1.0;

// --- minimal JSON parsing (just enough for a flat array of flat objects) ---

#[derive(Debug)]
enum Json {
    Number(f64),
    Str(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
    Bool,
    Null,
}

struct Parser<'a> {
    src: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, chars: src.char_indices().peekable() }
    }

    fn skip_ws(&mut self) {
        while let Some(&(_, c)) = self.chars.peek() {
            if c.is_whitespace() {
                self.chars.next();
            } else {
                break;
            }
        }
    }

    fn peek_char(&mut self) -> Option<char> {
        self.chars.peek().map(|&(_, c)| c)
    }

    fn expect(&mut self, expected: char) -> Result<(), String> {
        match self.chars.next() {
            Some((_, c)) if c == expected => Ok(()),
            other => Err(format!("expected {expected:?}, got {other:?}")),
        }
    }

    fn expect_literal(&mut self, literal: &str) -> Result<(), String> {
        for expected in literal.chars() {
            self.expect(expected)?;
        }
        Ok(())
    }

    fn parse_value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self.peek_char() {
            Some('{') => self.parse_object(),
            Some('[') => self.parse_array(),
            Some('"') => Ok(Json::Str(self.parse_string()?)),
            Some('t') => {
                self.expect_literal("true")?;
                Ok(Json::Bool)
            }
            Some('f') => {
                self.expect_literal("false")?;
                Ok(Json::Bool)
            }
            Some('n') => {
                self.expect_literal("null")?;
                Ok(Json::Null)
            }
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
            other => Err(format!("unexpected token: {other:?}")),
        }
    }

    fn parse_object(&mut self) -> Result<Json, String> {
        self.expect('{')?;
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek_char() == Some('}') {
            self.chars.next();
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_ws();
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(':')?;
            let value = self.parse_value()?;
            entries.push((key, value));
            self.skip_ws();
            match self.chars.next() {
                Some((_, ',')) => continue,
                Some((_, '}')) => break,
                other => return Err(format!("expected ',' or '}}', got {other:?}")),
            }
        }
        Ok(Json::Object(entries))
    }

    fn parse_array(&mut self) -> Result<Json, String> {
        self.expect('[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek_char() == Some(']') {
            self.chars.next();
            return Ok(Json::Array(items));
        }
        loop {
            items.push(self.parse_value()?);
            self.skip_ws();
            match self.chars.next() {
                Some((_, ',')) => continue,
                Some((_, ']')) => break,
                other => return Err(format!("expected ',' or ']', got {other:?}")),
            }
        }
        Ok(Json::Array(items))
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect('"')?;
        let mut s = String::new();
        loop {
            match self.chars.next() {
                Some((_, '"')) => break,
                Some((_, '\\')) => match self.chars.next() {
                    Some((_, '"')) => s.push('"'),
                    Some((_, '\\')) => s.push('\\'),
                    Some((_, '/')) => s.push('/'),
                    Some((_, 'b')) => s.push('\u{8}'),
                    Some((_, 'f')) => s.push('\u{c}'),
                    Some((_, 'n')) => s.push('\n'),
                    Some((_, 'r')) => s.push('\r'),
                    Some((_, 't')) => s.push('\t'),
                    Some((_, 'u')) => {
                        let code = self.parse_hex4()?;
                        s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                    }
                    other => return Err(format!("invalid escape: {other:?}")),
                },
                Some((_, c)) => s.push(c),
                None => return Err("unterminated string".to_string()),
            }
        }
        Ok(s)
    }

    fn parse_hex4(&mut self) -> Result<u32, String> {
        let mut code = 0u32;
        for _ in 0..4 {
            let (_, c) = self.chars.next().ok_or("unterminated unicode escape")?;
            code = code * 16 + c.to_digit(16).ok_or("invalid hex digit in unicode escape")?;
        }
        Ok(code)
    }

    fn parse_number(&mut self) -> Result<Json, String> {
        let start = self.chars.peek().map(|&(i, _)| i).unwrap_or(self.src.len());
        while let Some(&(_, c)) = self.chars.peek() {
            if c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E') {
                self.chars.next();
            } else {
                break;
            }
        }
        let end = self.chars.peek().map(|&(i, _)| i).unwrap_or(self.src.len());
        self.src[start..end].parse::<f64>().map(Json::Number).map_err(|err| err.to_string())
    }
}

/// Extract `(id, ru)` pairs, in file order, from the bundled dataset's JSON.
fn load_ru_sentences(json_src: &str) -> Result<Vec<(u64, String)>, String> {
    let Json::Array(items) = Parser::new(json_src).parse_value()? else {
        return Err("expected the dataset to be a JSON array".to_string());
    };
    let mut result = Vec::with_capacity(items.len());
    for item in items {
        let Json::Object(fields) = item else {
            return Err("expected each dataset record to be a JSON object".to_string());
        };
        let mut id = None;
        let mut ru = None;
        for (key, value) in fields {
            match key.as_str() {
                "id" => {
                    if let Json::Number(n) = value {
                        id = Some(n as u64);
                    }
                }
                "ru" => {
                    if let Json::Str(s) = value {
                        ru = Some(s);
                    }
                }
                _ => {}
            }
        }
        match (id, ru) {
            (Some(id), Some(ru)) => result.push((id, ru)),
            _ => return Err("dataset record missing \"id\" or \"ru\" field".to_string()),
        }
    }
    Ok(result)
}

/// Parse a printer-style id list, e.g. "1,3,5-8" -> [1, 3, 5, 6, 7, 8].
fn parse_id_list(spec: &str) -> Result<Vec<u64>, String> {
    const MAX_RANGE_LEN: u64 = 1_000_000;
    let mut result = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if let Some((start_str, end_str)) = part.split_once('-') {
            let start: u64 = start_str
                .trim()
                .parse()
                .map_err(|_| format!("invalid id {start_str:?}"))?;
            let end: u64 = end_str.trim().parse().map_err(|_| format!("invalid id {end_str:?}"))?;
            if start > end {
                return Err(format!("invalid range {part:?}: start {start} is greater than end {end}"));
            }
            if end - start + 1 > MAX_RANGE_LEN {
                return Err(format!("range {part:?} spans more than {MAX_RANGE_LEN} ids"));
            }
            result.extend(start..=end);
        } else {
            result.push(part.parse().map_err(|_| format!("invalid id {part:?}"))?);
        }
    }
    Ok(result)
}

fn voice_model_paths(home: &Path, voice: &str) -> Result<(PathBuf, PathBuf), String> {
    let (model_rel, config_rel) = match voice {
        "denis" => (
            "ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx",
            "ru/ru_RU/denis/medium/ru_RU-denis-medium.onnx.json",
        ),
        "irina" => (
            "ru/ru_RU/irina/medium/ru_RU-irina-medium.onnx",
            "ru/ru_RU/irina/medium/ru_RU-irina-medium.onnx.json",
        ),
        other => {
            return Err(format!("invalid choice for --voice: {other:?} (choose from \"irina\", \"denis\")"));
        }
    };
    let root = home.join(".local/share/piper-voices");
    Ok((root.join(model_rel), root.join(config_rel)))
}

/// Return zeroed S16LE mono PCM audio of the requested duration at [`SAMPLE_RATE`].
fn silence(duration_seconds: f64) -> Vec<u8> {
    let num_samples = (f64::from(SAMPLE_RATE) * duration_seconds).round() as usize;
    vec![0u8; num_samples * 2]
}

/// Run `command` with `input` written to its stdin, returning its captured
/// stdout/stderr/status once it exits. Writing happens on a separate thread
/// so a child that fills its stdout/stderr pipe before finishing reading
/// stdin (or vice versa) can't deadlock against us.
fn run_with_stdin(mut command: Command, input: Vec<u8>) -> io::Result<Output> {
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().expect("stdin was piped");
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child.wait_with_output()?;
    let _ = writer.join();
    Ok(output)
}

fn synthesize(model: &Path, config: &Path, text: &str) -> Result<Vec<u8>, String> {
    let mut command = Command::new("piper");
    command.arg("-m").arg(model).arg("-c").arg(config).arg("--output-raw");
    let output = run_with_stdin(command, text.as_bytes().to_vec())
        .map_err(|err| format!("failed to run piper: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "piper failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn render_to_mp3(
    ids: &[u64],
    sentences: &HashMap<u64, String>,
    voice: &str,
    home: &Path,
    output: &str,
) -> Result<(), String> {
    if !output.ends_with(".mp3") {
        return Err(format!("output path must end with .mp3: {output}"));
    }

    let (model, config) = voice_model_paths(home, voice)?;
    for path in [&model, &config] {
        if !path.exists() {
            return Err(format!(
                "voice file not found: {} (run setup_piper_voices --install)",
                path.display()
            ));
        }
    }

    let gap = silence(GAP_SECONDS);
    let mut audio = Vec::new();
    for &id in ids {
        let text = sentences.get(&id).ok_or_else(|| format!("no sentence pair with id {id}"))?;
        audio.extend(silence(LEAD_IN_SECONDS));
        audio.extend(synthesize(&model, &config, text)?);
        audio.extend(&gap);
    }

    let mut command = Command::new("ffmpeg");
    command.args([
        "-y",
        "-hide_banner",
        "-loglevel", "error",
        "-f", "s16le",
        "-ar", &SAMPLE_RATE.to_string(),
        "-ac", "1",
        "-i", "-",
        output,
    ]);
    let result = run_with_stdin(command, audio).map_err(|err| format!("failed to run ffmpeg: {err}"))?;
    if !result.status.success() {
        return Err(format!(
            "ffmpeg failed ({}): {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(())
}

struct Args {
    ids: Option<String>,
    voice: String,
    output: String,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut ids: Option<String> = None;
    let mut voice = "denis".to_string();
    let mut output: Option<String> = None;
    let mut positional_seen = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--voice" => {
                let value = iter.next().ok_or("--voice requires a value")?;
                if value != "irina" && value != "denis" {
                    return Err(format!(
                        "invalid choice for --voice: {value:?} (choose from \"irina\", \"denis\")"
                    ));
                }
                voice = value.clone();
            }
            "-o" | "--output" => {
                let value = iter.next().ok_or("--output requires a value")?;
                output = Some(value.clone());
            }
            other if other.starts_with('-') && other != "-" => {
                return Err(format!("unexpected argument: {other:?}"));
            }
            other => {
                if positional_seen {
                    return Err(format!("unexpected argument: {other:?}"));
                }
                ids = Some(other.to_string());
                positional_seen = true;
            }
        }
    }

    let output = output.ok_or("--output is required")?;
    Ok(Args { ids, voice, output })
}

fn main() -> ExitCode {
    let usage = "usage: make_ru_mp3 [ids] -o|--output OUTPUT.mp3 [--voice irina|denis]";
    let raw_args: Vec<String> = env::args().skip(1).collect();
    let args = match parse_args(&raw_args) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{usage}");
            return ExitCode::FAILURE;
        }
    };

    let sentences = match load_ru_sentences(SENTENCE_PAIRS_JSON) {
        Ok(sentences) => sentences,
        Err(message) => {
            eprintln!("error: failed to parse the bundled dataset: {message}");
            return ExitCode::FAILURE;
        }
    };

    let ids = match &args.ids {
        Some(spec) => match parse_id_list(spec) {
            Ok(ids) => ids,
            Err(message) => {
                eprintln!("error: invalid id spec {spec:?}: {message}");
                return ExitCode::FAILURE;
            }
        },
        None => sentences.iter().map(|(id, _)| *id).collect(),
    };
    let by_id: HashMap<u64, String> = sentences.into_iter().collect();

    let home = match env::var_os("HOME") {
        Some(home) => PathBuf::from(home),
        None => {
            eprintln!("error: HOME environment variable is not set");
            return ExitCode::FAILURE;
        }
    };

    match render_to_mp3(&ids, &by_id, &args.voice, &home, &args.output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_value_reads_flat_object_array() {
        let json = r#"[{"id": 1, "ru": "привет", "ipa": "x", "en": "hi", "words": "hi"}]"#;
        let sentences = load_ru_sentences(json).unwrap();
        assert_eq!(sentences, vec![(1, "привет".to_string())]);
    }

    #[test]
    fn load_ru_sentences_preserves_file_order() {
        let json = r#"[{"id": 5, "ru": "a"}, {"id": 2, "ru": "b"}]"#;
        let sentences = load_ru_sentences(json).unwrap();
        assert_eq!(sentences, vec![(5, "a".to_string()), (2, "b".to_string())]);
    }

    #[test]
    fn load_ru_sentences_handles_escaped_quotes_and_unicode_escapes() {
        let json = r#"[{"id": 1, "ru": "say \"hi\" A"}]"#;
        let sentences = load_ru_sentences(json).unwrap();
        assert_eq!(sentences[0].1, "say \"hi\" A");
    }

    #[test]
    fn load_ru_sentences_errors_on_missing_field() {
        assert!(load_ru_sentences(r#"[{"id": 1}]"#).is_err());
    }

    #[test]
    fn bundled_dataset_parses_and_is_non_empty() {
        let sentences = load_ru_sentences(SENTENCE_PAIRS_JSON).unwrap();
        assert!(!sentences.is_empty());
        assert!(sentences.iter().any(|(id, _)| *id == 1));
    }

    #[test]
    fn parse_id_list_mixed_ids_and_ranges() {
        assert_eq!(parse_id_list("1,3,5-8").unwrap(), vec![1, 3, 5, 6, 7, 8]);
    }

    #[test]
    fn parse_id_list_rejects_reversed_range() {
        assert!(parse_id_list("8-5").is_err());
    }

    #[test]
    fn parse_id_list_rejects_oversized_range() {
        assert!(parse_id_list("1-2000000").is_err());
    }

    #[test]
    fn voice_model_paths_denis_and_irina_differ() {
        let home = Path::new("/home/test");
        let denis = voice_model_paths(home, "denis").unwrap();
        let irina = voice_model_paths(home, "irina").unwrap();
        assert_ne!(denis, irina);
    }

    #[test]
    fn voice_model_paths_rejects_unknown_voice() {
        assert!(voice_model_paths(Path::new("/home/test"), "amy").is_err());
    }

    #[test]
    fn silence_scales_with_duration() {
        assert_eq!(silence(0.5).len(), silence(1.0).len() / 2);
    }

    #[test]
    fn parse_args_requires_output() {
        assert!(parse_args(&[]).is_err());
    }

    #[test]
    fn parse_args_accepts_ids_voice_and_output() {
        let args = parse_args(&[
            "1,3".to_string(),
            "--voice".to_string(),
            "irina".to_string(),
            "-o".to_string(),
            "out.mp3".to_string(),
        ])
        .unwrap();
        assert_eq!(args.ids.as_deref(), Some("1,3"));
        assert_eq!(args.voice, "irina");
        assert_eq!(args.output, "out.mp3");
    }

    #[test]
    fn render_to_mp3_rejects_output_not_ending_in_mp3() {
        let sentences = HashMap::from([(1, "привет".to_string())]);
        let err = render_to_mp3(&[1], &sentences, "denis", Path::new("/home/test"), "out.wav")
            .unwrap_err();
        assert!(err.contains(".mp3"), "error message was: {err}");
    }
}
