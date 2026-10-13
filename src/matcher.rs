use strsim::normalized_levenshtein;

use crate::sheet::Directory;
use crate::text::{normalize_name, normalize_rib};

/// Minimum similarity for an OCR word to count as a name word (tolerates 1 typo in ~5 letters).
const WORD_SIMILARITY: f64 = 0.8;
/// A check page with no line holding at least this many digits has no readable RIB on it.
const MIN_RIB_DIGITS: usize = 10;

pub enum NameMatch {
    Found(usize),
    Ambiguous(Vec<usize>),
    NotFound,
}

/// Find which person of the directory is named in the OCR text of a page.
pub fn find_person(dir: &Directory, ocr_text: &str) -> NameMatch {
    let words: Vec<String> = normalize_name(ocr_text)
        .split(' ')
        .map(str::to_string)
        .collect();
    let compact_lines: Vec<String> = ocr_text
        .lines()
        .map(|l| normalize_name(l).replace(' ', ""))
        .collect();

    let mut scored: Vec<(usize, f64)> = dir
        .persons
        .iter()
        .enumerate()
        .filter_map(|(i, person)| {
            // OCR often glues words together, so first try the name without spaces.
            let compact_name = person.key.replace(' ', "");
            if compact_lines.iter().any(|l| l.contains(&compact_name)) {
                return Some((i, compact_name.len() as f64));
            }
            let mut score = 0.0;
            for token in &person.tokens {
                let best = words
                    .iter()
                    .map(|w| normalized_levenshtein(token, w))
                    .fold(0.0, f64::max);
                if best < WORD_SIMILARITY {
                    return None;
                }
                score += best * token.len() as f64;
            }
            Some((i, score))
        })
        .collect();

    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    match scored.as_slice() {
        [] => NameMatch::NotFound,
        [(best, score), rest @ ..] => {
            let tied: Vec<usize> = rest
                .iter()
                .take_while(|(_, s)| (score - s).abs() < 0.5)
                .map(|(i, _)| *i)
                .collect();
            if tied.is_empty() {
                NameMatch::Found(*best)
            } else {
                NameMatch::Ambiguous([vec![*best], tied].concat())
            }
        }
    }
}

pub enum RibMatch {
    Matched(usize),
    Mismatch { seen: Vec<String> },
    Unreadable,
}

/// Look for any of the person's RIBs in the OCR text of the check page.
pub fn check_rib(dir: &Directory, entry_ids: &[usize], ocr_text: &str) -> RibMatch {
    let lines: Vec<String> = ocr_text.lines().map(normalize_rib).collect();
    // A RIB is sometimes split over two OCR lines.
    let whole_page = lines.concat();

    for &id in entry_ids {
        let rib = normalize_rib(&dir.entries[id].rib);
        if lines.iter().any(|l| l.contains(&rib)) || whole_page.contains(&rib) {
            return RibMatch::Matched(id);
        }
    }

    let seen: Vec<String> = ocr_text
        .lines()
        .map(|l| l.chars().filter(char::is_ascii_digit).collect::<String>())
        .filter(|digits| digits.len() >= MIN_RIB_DIGITS)
        .collect();
    if seen.is_empty() {
        RibMatch::Unreadable
    } else {
        RibMatch::Mismatch { seen }
    }
}
