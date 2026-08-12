use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::Path;
use std::process::ExitCode;

use serde::{Deserialize, Serialize};

/// A single Russian/English sentence with its IPA transcription and id.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SentencePair {
    pub id: u64,
    pub ru: String,
    pub ipa: String,
    pub en: String,
    pub words: String,
}

/// Error returned by [`parse_id_list`].
#[derive(Debug)]
pub enum IdListError {
    ParseInt {
        token: String,
        source: std::num::ParseIntError,
    },
    InvalidRange {
        token: String,
        start: u64,
        end: u64,
    },
    RangeTooLarge {
        token: String,
    },
}

/// Largest number of ids a single range (e.g. "1-1000000") may expand to.
///
/// Without this bound, a range like "0-18446744073709551615" would try to
/// build a multi-exabyte `Vec<u64>`, hanging or exhausting memory instead
/// of failing fast.
const MAX_RANGE_LEN: u64 = 1_000_000;

impl fmt::Display for IdListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParseInt { token, source } => {
                write!(f, "invalid id {token:?}: {source}")
            }
            Self::InvalidRange { token, start, end } => {
                write!(
                    f,
                    "invalid range {token:?}: start {start} is greater than end {end}"
                )
            }
            Self::RangeTooLarge { token } => {
                write!(f, "range {token:?} spans more than {MAX_RANGE_LEN} ids")
            }
        }
    }
}

impl std::error::Error for IdListError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ParseInt { source, .. } => Some(source),
            Self::InvalidRange { .. } | Self::RangeTooLarge { .. } => None,
        }
    }
}

fn parse_id(token: &str) -> Result<u64, IdListError> {
    token.parse().map_err(|source| IdListError::ParseInt {
        token: token.to_string(),
        source,
    })
}

/// Parse a printer-style id list, e.g. "1,3,5-8" -> [1, 3, 5, 6, 7, 8].
///
/// Whitespace around ids, ranges, and range endpoints is ignored, so
/// "1, 3, 5 - 8" parses the same as "1,3,5-8".
///
/// Ids are unsigned, so a `-` is only ever treated as a range separator;
/// a leading minus sign (e.g. "-3") is rejected as an invalid digit.
///
/// A reversed range (e.g. "8-5") is rejected rather than silently
/// producing an empty list, and a range spanning more than
/// [`MAX_RANGE_LEN`] ids is rejected rather than exhausting memory.
pub fn parse_id_list(spec: &str) -> Result<Vec<u64>, IdListError> {
    let mut result = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if let Some((start_str, end_str)) = part.split_once('-') {
            let start = parse_id(start_str.trim())?;
            let end = parse_id(end_str.trim())?;
            if start > end {
                return Err(IdListError::InvalidRange {
                    token: part.to_string(),
                    start,
                    end,
                });
            }
            let span = u128::from(end) - u128::from(start) + 1;
            if span > u128::from(MAX_RANGE_LEN) {
                return Err(IdListError::RangeTooLarge {
                    token: part.to_string(),
                });
            }
            result.extend(start..=end);
        } else {
            result.push(parse_id(part)?);
        }
    }
    Ok(result)
}

/// Error returned by [`load_sentence_pairs`].
#[derive(Debug)]
pub enum LoadError {
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Json(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
        }
    }
}

impl From<io::Error> for LoadError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for LoadError {
    fn from(err: serde_json::Error) -> Self {
        Self::Json(err)
    }
}

/// Load all `SentencePair` records from a JSON file.
///
/// Each record in the file must contain the fields `id`, `ru`, `ipa`,
/// `en`, and `words`.
pub fn load_sentence_pairs<P: AsRef<Path>>(path: P) -> Result<Vec<SentencePair>, LoadError> {
    let data = fs::read_to_string(path)?;
    let pairs = serde_json::from_str(&data)?;
    Ok(pairs)
}

/// Error returned by [`run`].
#[derive(Debug)]
enum RunError {
    IdList { spec: String, source: IdListError },
    Load(LoadError),
    Serialize(serde_json::Error),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdList { spec, source } => write!(f, "invalid id spec {spec:?}: {source}"),
            Self::Load(err) => write!(f, "{err}"),
            Self::Serialize(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IdList { source, .. } => Some(source),
            Self::Load(err) => Some(err),
            Self::Serialize(err) => Some(err),
        }
    }
}

impl From<LoadError> for RunError {
    fn from(err: LoadError) -> Self {
        Self::Load(err)
    }
}

impl From<serde_json::Error> for RunError {
    fn from(err: serde_json::Error) -> Self {
        Self::Serialize(err)
    }
}

/// Loads records from `path` and renders the ones whose `id` appears in
/// the parsed `id_spec` (e.g. "1,3,5-8") as a pretty-printed JSON array.
///
/// The result follows `id_spec`'s order exactly, repeating a record once
/// per repeated id (e.g. "2,2" yields id 2 twice).
///
/// Any requested id with no matching record is reported to stderr as a
/// warning rather than being silently dropped; this does not affect the
/// return value.
fn run(path: &str, id_spec: &str) -> Result<String, RunError> {
    let wanted_ids = parse_id_list(id_spec).map_err(|source| RunError::IdList {
        spec: id_spec.to_string(),
        source,
    })?;

    let pairs = load_sentence_pairs(path)?;
    let by_id: BTreeMap<u64, &SentencePair> = pairs.iter().map(|p| (p.id, p)).collect();

    let mut selected = Vec::with_capacity(wanted_ids.len());
    let mut missing_ids = BTreeSet::new();
    for id in wanted_ids {
        match by_id.get(&id) {
            Some(pair) => selected.push(pair),
            None => {
                missing_ids.insert(id);
            }
        }
    }
    if !missing_ids.is_empty() {
        eprintln!("warning: id(s) not found: {missing_ids:?}");
    }

    Ok(serde_json::to_string_pretty(&selected)?)
}

/// CLI entry point: `<path> <id-spec>`.
///
/// Loads records from `path`, keeps only those whose `id` appears in the
/// parsed `id-spec` (e.g. "1,3,5-8"), and prints the resulting records to
/// stdout as a JSON array.
pub fn main(prog: &str, args: &[String]) -> ExitCode {
    if args.len() != 2 {
        eprintln!("usage: {prog} <path> <id-spec>");
        return ExitCode::FAILURE;
    }

    match run(&args[0], &args[1]) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_id_list_single_ids() {
        assert_eq!(parse_id_list("1,3,5").unwrap(), vec![1, 3, 5]);
    }

    #[test]
    fn parse_id_list_range() {
        assert_eq!(parse_id_list("5-8").unwrap(), vec![5, 6, 7, 8]);
    }

    #[test]
    fn parse_id_list_mixed_ids_and_ranges() {
        assert_eq!(parse_id_list("1,3,5-8").unwrap(), vec![1, 3, 5, 6, 7, 8]);
    }

    #[test]
    fn parse_id_list_single_element_range() {
        assert_eq!(parse_id_list("4-4").unwrap(), vec![4]);
    }

    #[test]
    fn parse_id_list_ignores_whitespace() {
        assert_eq!(
            parse_id_list(" 1, 3 , 5 - 8 ").unwrap(),
            vec![1, 3, 5, 6, 7, 8]
        );
    }

    #[test]
    fn parse_id_list_rejects_non_numeric_input() {
        assert!(parse_id_list("abc").is_err());
        assert!(parse_id_list("1,abc-3").is_err());
    }

    #[test]
    fn parse_id_list_error_names_bad_token() {
        let err = parse_id_list("1,abc,3").unwrap_err().to_string();
        assert!(err.contains("abc"), "error message was: {err}");

        let err = parse_id_list("1,3-xyz").unwrap_err().to_string();
        assert!(err.contains("xyz"), "error message was: {err}");
    }

    #[test]
    fn parse_id_list_rejects_negative_ids() {
        assert!(parse_id_list("-3").is_err());
        assert!(parse_id_list("1,-3").is_err());
        assert!(parse_id_list("3--1").is_err());
    }

    #[test]
    fn parse_id_list_rejects_reversed_range() {
        assert!(parse_id_list("8-5").is_err());
        assert!(parse_id_list("1,8-5").is_err());
    }

    #[test]
    fn parse_id_list_reversed_range_error_names_values() {
        let err = parse_id_list("8-5").unwrap_err().to_string();
        assert!(err.contains("8-5"), "error message was: {err}");
        assert!(err.contains('8'), "error message was: {err}");
        assert!(err.contains('5'), "error message was: {err}");
    }

    #[test]
    fn parse_id_list_rejects_oversized_range() {
        assert!(parse_id_list("1-2000000").is_err());
    }

    #[test]
    fn parse_id_list_rejects_huge_range_without_hanging() {
        // Regression test: end - start + 1 must not overflow u64 when
        // computing the range's span.
        assert!(parse_id_list("0-18446744073709551615").is_err());
    }

    #[test]
    fn parse_id_list_accepts_range_at_the_limit() {
        let ids = parse_id_list("1-1000000").unwrap();
        assert_eq!(ids.len(), 1_000_000);
    }

    struct TempFile(PathBuf);

    impl TempFile {
        fn new(name: &str, contents: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("sentence_pairs_test_{}_{name}", std::process::id()));
            fs::write(&path, contents).unwrap();
            Self(path)
        }
    }

    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    const SAMPLE_JSON: &str = r#"[
        {"id": 1, "ru": "привет", "ipa": "[prʲɪˈvʲet]", "en": "hello", "words": "hello"},
        {"id": 2, "ru": "мир", "ipa": "[mʲir]", "en": "world", "words": "world"}
    ]"#;

    #[test]
    fn load_sentence_pairs_parses_all_fields() {
        let file = TempFile::new("valid.json", SAMPLE_JSON);
        let pairs = load_sentence_pairs(&file.0).unwrap();

        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].id, 1);
        assert_eq!(pairs[0].ru, "привет");
        assert_eq!(pairs[0].ipa, "[prʲɪˈvʲet]");
        assert_eq!(pairs[0].en, "hello");
        assert_eq!(pairs[0].words, "hello");
        assert_eq!(pairs[1].id, 2);
    }

    #[test]
    fn load_sentence_pairs_missing_file_is_error() {
        let result = load_sentence_pairs("/nonexistent/path/to/sentence_pairs.json");
        assert!(result.is_err());
    }

    #[test]
    fn load_sentence_pairs_malformed_json_is_error() {
        let file = TempFile::new("malformed.json", "{ not valid json");
        assert!(load_sentence_pairs(&file.0).is_err());
    }

    #[test]
    fn load_sentence_pairs_rejects_unknown_fields() {
        let json = r#"[
            {"id": 1, "ru": "привет", "ipa": "[prʲɪˈvʲet]", "en": "hello", "words": "hello", "extra": "surprise"}
        ]"#;
        let file = TempFile::new("unknown_field.json", json);
        assert!(load_sentence_pairs(&file.0).is_err());
    }

    #[test]
    fn main_reports_usage_error_on_wrong_arg_count() {
        assert_eq!(main("prog", &[]), ExitCode::FAILURE);
        assert_eq!(main("prog", &["only-one".to_string()]), ExitCode::FAILURE);
    }

    #[test]
    fn main_reports_error_on_invalid_id_spec() {
        let file = TempFile::new("for_bad_spec.json", SAMPLE_JSON);
        let path = file.0.to_string_lossy().to_string();
        assert_eq!(
            main("prog", &[path, "not-a-number".to_string()]),
            ExitCode::FAILURE
        );
    }

    #[test]
    fn main_succeeds_for_valid_input() {
        let file = TempFile::new("for_success.json", SAMPLE_JSON);
        let path = file.0.to_string_lossy().to_string();
        assert_eq!(main("prog", &[path, "1".to_string()]), ExitCode::SUCCESS);
    }

    #[test]
    fn run_succeeds_but_warns_when_some_ids_are_missing() {
        let file = TempFile::new("for_missing_id.json", SAMPLE_JSON);
        let path = file.0.to_string_lossy().to_string();

        let json = run(&path, "1,999").unwrap();
        assert!(json.contains("\"id\": 1"));
        assert!(!json.contains("999"));
    }

    #[test]
    fn run_succeeds_but_warns_when_no_ids_match() {
        let file = TempFile::new("for_no_match.json", SAMPLE_JSON);
        let path = file.0.to_string_lossy().to_string();

        let json = run(&path, "999").unwrap();
        assert_eq!(json.trim(), "[]");
    }

    #[test]
    fn run_repeats_records_for_repeated_ids() {
        let file = TempFile::new("for_repeated_id.json", SAMPLE_JSON);
        let path = file.0.to_string_lossy().to_string();

        let json = run(&path, "2,1,1").unwrap();
        let parsed: Vec<SentencePair> = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![2, 1, 1]
        );
    }
}
