use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A single Russian/English sentence with its IPA transcription and id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentencePair {
    pub id: u64,
    pub ru: String,
    pub ipa: String,
    pub en: String,
    pub words: String,
}

/// Parse a printer-style id list, e.g. "1,3,5-8" -> [1, 3, 5, 6, 7, 8].
///
/// Whitespace around ids, ranges, and range endpoints is ignored, so
/// "1, 3, 5 - 8" parses the same as "1,3,5-8".
///
/// Ids are unsigned, so a `-` is only ever treated as a range separator;
/// a leading minus sign (e.g. "-3") is rejected as an invalid digit.
pub fn parse_id_list(spec: &str) -> Result<Vec<u64>, std::num::ParseIntError> {
    let mut result = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if let Some((start, end)) = part.split_once('-') {
            let start: u64 = start.trim().parse()?;
            let end: u64 = end.trim().parse()?;
            result.extend(start..=end);
        } else {
            result.push(part.parse()?);
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

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(err) => write!(f, "{err}"),
            LoadError::Json(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LoadError::Io(err) => Some(err),
            LoadError::Json(err) => Some(err),
        }
    }
}

impl From<io::Error> for LoadError {
    fn from(err: io::Error) -> Self {
        LoadError::Io(err)
    }
}

impl From<serde_json::Error> for LoadError {
    fn from(err: serde_json::Error) -> Self {
        LoadError::Json(err)
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

/// CLI entry point: `<path> <id-spec>`.
///
/// Loads records from `path`, keeps only those whose `id` appears in the
/// parsed `id-spec` (e.g. "1,3,5-8"), and prints the resulting records to
/// stdout as a JSON array.
pub fn main(prog: &str, args: &[String]) -> i32 {
    if args.len() != 2 {
        eprintln!("usage: {prog} <path> <id-spec>");
        return 1;
    }

    let path = &args[0];
    let id_spec = &args[1];

    let wanted_ids: HashSet<u64> = match parse_id_list(id_spec) {
        Ok(ids) => ids.into_iter().collect(),
        Err(err) => {
            eprintln!("error: invalid id spec {id_spec:?}: {err}");
            return 1;
        }
    };

    let pairs = match load_sentence_pairs(path) {
        Ok(pairs) => pairs,
        Err(err) => {
            eprintln!("error: {err}");
            return 1;
        }
    };

    let selected: Vec<&SentencePair> = pairs
        .iter()
        .filter(|p| wanted_ids.contains(&p.id))
        .collect();

    match serde_json::to_string_pretty(&selected) {
        Ok(json) => {
            println!("{json}");
            0
        }
        Err(err) => {
            eprintln!("error: {err}");
            1
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
    fn parse_id_list_rejects_negative_ids() {
        assert!(parse_id_list("-3").is_err());
        assert!(parse_id_list("1,-3").is_err());
        assert!(parse_id_list("3--1").is_err());
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
    fn main_reports_usage_error_on_wrong_arg_count() {
        assert_eq!(main("prog", &[]), 1);
        assert_eq!(main("prog", &["only-one".to_string()]), 1);
    }

    #[test]
    fn main_reports_error_on_invalid_id_spec() {
        let file = TempFile::new("for_bad_spec.json", SAMPLE_JSON);
        let path = file.0.to_string_lossy().to_string();
        assert_eq!(main("prog", &[path, "not-a-number".to_string()]), 1);
    }

    #[test]
    fn main_succeeds_for_valid_input() {
        let file = TempFile::new("for_success.json", SAMPLE_JSON);
        let path = file.0.to_string_lossy().to_string();
        assert_eq!(main("prog", &[path, "1".to_string()]), 0);
    }
}
