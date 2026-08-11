use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A single Russian/English sentence with its IPA transcription and id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentencePair {
    pub id: i64,
    pub ru: String,
    pub ipa: String,
    pub en: String,
    pub words: String,
}

/// Parse a printer-style id list, e.g. "1,3,5-8" -> [1, 3, 5, 6, 7, 8].
pub fn parse_id_list(spec: &str) -> Result<Vec<i64>, std::num::ParseIntError> {
    let mut result = Vec::new();
    for part in spec.split(',') {
        if let Some((start, end)) = part.split_once('-') {
            let start: i64 = start.parse()?;
            let end: i64 = end.parse()?;
            result.extend(start..=end);
        } else {
            result.push(part.parse()?);
        }
    }
    Ok(result)
}

/// Load all `SentencePair` records from a JSON file.
///
/// Each record in the file must contain the fields `id`, `ru`, `ipa`,
/// `en`, and `words`.
pub fn load_sentence_pairs<P: AsRef<Path>>(path: P) -> io::Result<Vec<SentencePair>> {
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

    let wanted_ids: HashSet<i64> = match parse_id_list(id_spec) {
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

    let selected: Vec<&SentencePair> = pairs.iter().filter(|p| wanted_ids.contains(&p.id)).collect();

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
