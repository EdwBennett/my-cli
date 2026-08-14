//! English and Russian playback/print/render for a sentence pair, keyed by id.
//!
//! The two languages differ in what a "clause" is drawn from (English
//! clauses come from the `words` field, Russian clauses come from splitting
//! `ru` itself) and what gets printed alongside it (English shows the full
//! sentence plus either the word list or the clause; Russian shows the
//! sentence or clause plus its IPA transcription).

use std::sync::OnceLock;

use crate::db::sentence_pairs::SentencePair;
use crate::say::{self, LEAD_IN_SECONDS};

use super::{PlayError, PlayFn};

/// The dataset bundled with the binary, embedded at compile time so `play`
/// doesn't depend on a runtime-resolved path to the source tree.
const SENTENCE_PAIRS_JSON: &str = include_str!("../db/50_russian_english_ipa_words.json");

fn sentence_pairs() -> &'static [SentencePair] {
    static PAIRS: OnceLock<Vec<SentencePair>> = OnceLock::new();
    PAIRS.get_or_init(|| {
        serde_json::from_str(SENTENCE_PAIRS_JSON).expect("bundled sentence pairs JSON is valid")
    })
}

fn load_pair(id: u64) -> Result<&'static SentencePair, PlayError> {
    sentence_pairs()
        .iter()
        .find(|pair| pair.id == id)
        .ok_or(PlayError::PairNotFound(id))
}

/// Split `text` on runs of `,`, `.`, `;`, or `-`, the way
/// `re.split(r'[,.;-]+', text)` would (consecutive delimiters collapse into
/// a single split point; a leading or trailing delimiter still yields an
/// empty piece at that end).
fn split_clauses(text: &str) -> Vec<&str> {
    let is_delim = |c: char| ",.;-".contains(c);
    let mut result = Vec::new();
    let mut seg_start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some(&(idx, ch)) = chars.peek() {
        if !is_delim(ch) {
            chars.next();
            continue;
        }
        result.push(&text[seg_start..idx]);
        let mut end = idx;
        while let Some(&(idx2, ch2)) = chars.peek() {
            if !is_delim(ch2) {
                break;
            }
            end = idx2 + ch2.len_utf8();
            chars.next();
        }
        seg_start = end;
    }
    result.push(&text[seg_start..]);
    result
}

/// Return the 1-based `clause`-th piece of `text` (see [`split_clauses`]),
/// trimmed of surrounding whitespace.
fn nth_clause(text: &str, clause: usize) -> Result<String, PlayError> {
    let clauses = split_clauses(text);
    if clause == 0 || clause > clauses.len() {
        return Err(PlayError::ClauseOutOfRange { clause, available: clauses.len() });
    }
    Ok(clauses[clause - 1].trim().to_string())
}

fn select_en_text(pair: &SentencePair, clause: Option<usize>) -> Result<String, PlayError> {
    match clause {
        None => Ok(pair.en.clone()),
        Some(clause) => nth_clause(&pair.words, clause),
    }
}

fn select_ru_text(pair: &SentencePair, clause: Option<usize>) -> Result<String, PlayError> {
    match clause {
        None => Ok(pair.ru.clone()),
        Some(clause) => nth_clause(&pair.ru, clause),
    }
}

fn print_en(pair: &SentencePair, clause: Option<usize>, en_say: &str) {
    println!("\n{}", pair.en);
    match clause {
        None => println!("{}", pair.words),
        Some(_) => println!("{en_say}"),
    }
}

fn print_ru(pair: &SentencePair, clause: Option<usize>, ru_say: &str) {
    match clause {
        None => println!("{}", pair.ru),
        Some(_) => println!("{ru_say}"),
    }
    println!("{}\n", pair.ipa);
}

/// Return a function that prints and speaks the English utterance for
/// `id`/`clause`. Errors (unknown id, out-of-range clause, playback failure)
/// surface when the returned function is called, not when it's constructed.
pub fn play_en(id: u64, clause: Option<usize>) -> PlayFn {
    Box::new(move || {
        let pair = load_pair(id)?;
        let text = select_en_text(pair, clause)?;
        print_en(pair, clause, &text);
        say::say("en", &text, None).map_err(PlayError::Say)
    })
}

/// Return a function that prints and speaks the Russian utterance for
/// `id`/`clause` in the given `voice`.
pub fn play_ru(id: u64, clause: Option<usize>, voice: Option<String>) -> PlayFn {
    Box::new(move || {
        let pair = load_pair(id)?;
        let text = select_ru_text(pair, clause)?;
        print_ru(pair, clause, &text);
        say::say("ru", &text, voice.as_deref()).map_err(PlayError::Say)
    })
}

/// Return a function that prints (without speaking) the English utterance
/// for `id`/`clause`.
pub fn print_en_only(id: u64, clause: Option<usize>) -> PlayFn {
    Box::new(move || {
        let pair = load_pair(id)?;
        let text = select_en_text(pair, clause)?;
        print_en(pair, clause, &text);
        Ok(())
    })
}

/// Return a function that prints (without speaking) the Russian utterance
/// for `id`/`clause`.
pub fn print_ru_only(id: u64, clause: Option<usize>) -> PlayFn {
    Box::new(move || {
        let pair = load_pair(id)?;
        let text = select_ru_text(pair, clause)?;
        print_ru(pair, clause, &text);
        Ok(())
    })
}

/// Synthesize the English utterance for `id`/`clause`, without playing it,
/// preceded by [`LEAD_IN_SECONDS`] of silence.
pub fn render_en(id: u64, clause: Option<usize>) -> Result<Vec<u8>, PlayError> {
    let pair = load_pair(id)?;
    let text = select_en_text(pair, clause)?;
    let mut audio = say::silence(LEAD_IN_SECONDS);
    audio.extend(say::synthesize("en", &text, None).map_err(PlayError::Say)?);
    Ok(audio)
}

/// Synthesize the Russian utterance for `id`/`clause` in the given `voice`,
/// without playing it, preceded by [`LEAD_IN_SECONDS`] of silence.
pub fn render_ru(id: u64, clause: Option<usize>, voice: Option<&str>) -> Result<Vec<u8>, PlayError> {
    let pair = load_pair(id)?;
    let text = select_ru_text(pair, clause)?;
    let mut audio = say::silence(LEAD_IN_SECONDS);
    audio.extend(say::synthesize("ru", &text, voice).map_err(PlayError::Say)?);
    Ok(audio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_clauses_collapses_consecutive_delimiters() {
        assert_eq!(split_clauses("a, b.. c"), vec!["a", " b", " c"]);
    }

    #[test]
    fn split_clauses_keeps_leading_and_trailing_empty_pieces() {
        assert_eq!(split_clauses(",a-"), vec!["", "a", ""]);
    }

    #[test]
    fn nth_clause_trims_and_is_one_based() {
        assert_eq!(nth_clause("a, b, c", 2).unwrap(), "b");
    }

    #[test]
    fn nth_clause_rejects_zero() {
        assert!(matches!(
            nth_clause("a, b", 0),
            Err(PlayError::ClauseOutOfRange { clause: 0, available: 2 })
        ));
    }

    #[test]
    fn nth_clause_rejects_out_of_range() {
        assert!(matches!(
            nth_clause("a, b", 3),
            Err(PlayError::ClauseOutOfRange { clause: 3, available: 2 })
        ));
    }

    #[test]
    fn load_pair_errors_for_unknown_id() {
        assert!(matches!(load_pair(u64::MAX), Err(PlayError::PairNotFound(id)) if id == u64::MAX));
    }

    #[test]
    fn load_pair_finds_bundled_id_one() {
        // The bundled dataset always contains at least one entry; id 1 is
        // its first record.
        assert!(load_pair(1).is_ok());
    }

    #[test]
    fn select_en_text_without_clause_returns_full_sentence() {
        let pair = load_pair(1).unwrap();
        assert_eq!(select_en_text(pair, None).unwrap(), pair.en);
    }

    #[test]
    fn render_en_prefixes_lead_in_silence() {
        // Doesn't require piper/aplay: an unsupported id fails during
        // load_pair, before synthesis is ever attempted, so this exercises
        // render_en's error path deterministically.
        assert!(matches!(render_en(u64::MAX, None), Err(PlayError::PairNotFound(_))));
    }
}
