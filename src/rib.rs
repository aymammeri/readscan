//! Finding a 20-digit RIB on a check page.
//!
//! A row of OCR text is cut into digit chunks wherever letters or garbage interrupt the number,
//! so a prefix like `001DZD` is dropped (`001DZD01400123...` keeps `01400123...`). Spaces and
//! printed separators never cut a chunk: a RIB printed in segments with big gaps reads as one
//! number.

use std::collections::HashMap;

use crate::sheet::Directory;

pub const RIB_LEN: usize = 20;
/// Fewest agreeing digits, from the left, the right or both, for a PARTIAL MATCH.
const PARTIAL_MIN_DIGITS: usize = 4;
/// Digits per run in the RIB index used to search the whole spreadsheet.
pub const INDEX_RUN: usize = 5;
/// Most entries compared in full when searching the whole spreadsheet.
const MAX_CANDIDATES: usize = 10;

/// Every run of `INDEX_RUN` consecutive digits.
pub fn runs(digits: &str) -> impl Iterator<Item = &str> {
    (0..=digits.len().saturating_sub(INDEX_RUN))
        .filter(move |_| digits.len() >= INDEX_RUN)
        .map(move |i| &digits[i..i + INDEX_RUN])
}

/// Entries sharing the most digit runs with the page, best first. Searching the whole
/// spreadsheet goes through this so only a few RIBs are compared in full.
pub fn likely_entries(dir: &Directory, ocr_text: &str) -> Vec<usize> {
    let mut hits: HashMap<usize, usize> = HashMap::new();
    for row in ocr_text.lines().map(|row| chunks(row, true)) {
        for digits in row.iter().cloned().chain(std::iter::once(row.concat())) {
            for run in runs(&digits) {
                for &entry in dir.entries_with_run(run) {
                    *hits.entry(entry).or_default() += 1;
                }
            }
        }
    }
    let mut ranked: Vec<(usize, usize)> = hits.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(entry, _)| entry)
        .collect()
}

/// Fragments shorter than this are not listed as "what was read" when nothing agrees.
const MIN_LISTED_DIGITS: usize = 6;

/// Spreadsheet value to RIB digits, with the same "drop what's left of the letters" rule.
pub fn from_sheet(value: &str) -> String {
    chunks(value, false).pop().unwrap_or_default()
}

/// Digit runs of one row, split where letters or garbage symbols interrupt the number. With
/// `fold`, a lone look-alike character touching a digit is read as that digit.
fn chunks(row: &str, fold: bool) -> Vec<String> {
    let chars: Vec<char> = row.chars().collect();
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            current.push(c);
            i += 1;
            continue;
        }
        if is_separator(c) {
            i += 1;
            continue;
        }
        let run_end = (i..chars.len())
            .find(|&j| chars[j].is_ascii_digit() || is_separator(chars[j]))
            .unwrap_or(chars.len());
        let touches_digit = |j: Option<usize>| {
            j.and_then(|j| chars.get(j))
                .is_some_and(char::is_ascii_digit)
        };
        let lone = run_end - i == 1;
        match lookalike(c) {
            Some(digit)
                if fold
                    && lone
                    && (touches_digit(i.checked_sub(1)) || touches_digit(Some(run_end))) =>
            {
                current.push(digit)
            }
            _ => {
                if !current.is_empty() {
                    chunks.push(std::mem::take(&mut current));
                }
            }
        }
        i = run_end;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// Characters printed between RIB segments, as opposed to OCR garbage like `#`, `~` or `*`.
fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '.' | '/' | ':' | ',' | '_')
}

fn lookalike(c: char) -> Option<char> {
    match c.to_ascii_uppercase() {
        'O' | 'Q' | 'D' => Some('0'),
        'I' | 'L' | '|' => Some('1'),
        'Z' => Some('2'),
        'S' => Some('5'),
        'G' => Some('6'),
        'B' => Some('8'),
        _ => None,
    }
}

pub enum RibMatch {
    /// All 20 digits found. `joined` when only found after joining separate segments.
    Full {
        entry: usize,
        joined: bool,
    },
    /// At least `PARTIAL_MIN_DIGITS` digits match, wherever they are.
    Partial {
        entry: usize,
        matched: usize,
    },
    /// A clean 20-digit number was read and almost none of it matches.
    Mismatch {
        read: String,
    },
    /// Digits were read but too few match to decide; needs a manual check.
    Unclear {
        read: Vec<String>,
    },
    Unreadable,
}

/// What was read on the check, lined up digit by digit with one expected RIB.
pub struct Reading {
    pub entry: usize,
    /// 20 characters, `?` where nothing was read.
    pub aligned: String,
}

pub struct RibCheck {
    pub outcome: RibMatch,
    pub reading: Option<Reading>,
}

/// Bulls and cows of one read digit against the expected RIB.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    /// Right digit in the right place.
    Bull,
    /// Wrong here, but this digit belongs elsewhere in the RIB.
    Cow,
    /// Not in the RIB (or more copies than the RIB has).
    Miss,
    NotRead,
}

/// Score `aligned` against `expected` position by position, like bulls and cows.
pub fn score(expected: &str, aligned: &str) -> Vec<Mark> {
    let expected = expected.as_bytes();
    let read = aligned.as_bytes();
    let is_bull = |i: usize| read.get(i) == expected.get(i);
    let mut spare = [0usize; 256];
    for (i, &digit) in expected.iter().enumerate() {
        if !is_bull(i) {
            spare[digit as usize] += 1;
        }
    }
    (0..expected.len())
        .map(|i| match read.get(i) {
            None | Some(b'?') => Mark::NotRead,
            Some(_) if is_bull(i) => Mark::Bull,
            Some(&digit) if spare[digit as usize] > 0 => {
                spare[digit as usize] -= 1;
                Mark::Cow
            }
            Some(_) => Mark::Miss,
        })
        .collect()
}

/// Look for any of `entry_ids`' RIBs in the OCR text of a check page. Rules, in order:
///
/// 1. Each row is cut into digit pieces (see [`chunks`]).
/// 2. MATCH: all 20 digits of a RIB, in one piece, in the pieces of a row joined, or over two
///    consecutive rows joined.
/// 3. PARTIAL MATCH: otherwise each row's pieces are laid on each RIB where they share the
///    longest run of digits; 4 or more matching digits, at any position, is a partial match.
/// 4. MISMATCH: otherwise, a clean 20-digit number was read and it is not the RIB.
/// 5. UNREADABLE: otherwise; whatever digits were read are listed for a manual check.
pub fn check(dir: &Directory, entry_ids: &[usize], ocr_text: &str) -> RibCheck {
    let rows: Vec<Vec<String>> = ocr_text.lines().map(|row| chunks(row, true)).collect();
    let ribs: Vec<(usize, &str)> = entry_ids
        .iter()
        .map(|&id| (id, dir.entries[id].rib_digits.as_str()))
        .filter(|(_, rib)| !rib.is_empty())
        .collect();

    if let Some((entry, joined)) = full_match(&rows, &ribs) {
        let aligned = dir.entries[entry].rib_digits.clone();
        return RibCheck {
            outcome: RibMatch::Full { entry, joined },
            reading: Some(Reading { entry, aligned }),
        };
    }

    let best = best_reading(&rows, &ribs);
    if let Some((reading, matched)) = &best
        && *matched >= PARTIAL_MIN_DIGITS
    {
        return RibCheck {
            outcome: RibMatch::Partial {
                entry: reading.entry,
                matched: *matched,
            },
            reading: best.map(|(reading, _)| reading),
        };
    }

    let complete = rows.iter().flatten().find(|chunk| chunk.len() == RIB_LEN);
    if let Some(read) = complete {
        let (closest, _) = closest(read, &ribs);
        return RibCheck {
            outcome: RibMatch::Mismatch { read: read.clone() },
            reading: Some(Reading {
                entry: closest,
                aligned: read.clone(),
            }),
        };
    }

    let mut read: Vec<String> = rows
        .iter()
        .filter(|row| row.len() > 1)
        .map(|row| row.concat())
        .filter(|digits| digits.len() == RIB_LEN)
        .collect();
    read.extend(
        rows.iter()
            .flatten()
            .filter(|chunk| chunk.len() >= MIN_LISTED_DIGITS)
            .cloned(),
    );
    read.dedup();
    let reading = best
        .map(|(reading, _)| reading)
        .filter(|r| r.aligned.bytes().any(|b| b != b'?'));
    let outcome = if read.is_empty() && reading.is_none() {
        RibMatch::Unreadable
    } else {
        RibMatch::Unclear { read }
    };
    RibCheck { outcome, reading }
}

/// The RIB as one chunk, as segments joined on a row, or split over consecutive rows.
fn full_match(rows: &[Vec<String>], ribs: &[(usize, &str)]) -> Option<(usize, bool)> {
    let find = |digits: &str| {
        ribs.iter()
            .find(|(_, rib)| digits.contains(rib))
            .map(|(id, _)| *id)
    };
    if let Some(entry) = rows.iter().flatten().find_map(|chunk| find(chunk)) {
        return Some((entry, false));
    }
    let row_digits: Vec<String> = rows.iter().map(|row| row.concat()).collect();
    row_digits.iter().enumerate().find_map(|(i, digits)| {
        let with_next = row_digits.get(i + 1).map(|next| format!("{digits}{next}"));
        find(digits)
            .or_else(|| with_next.as_deref().and_then(find))
            .map(|entry| (entry, true))
    })
}

/// The row reading that matches an expected RIB on the most digits, with that count.
fn best_reading(rows: &[Vec<String>], ribs: &[(usize, &str)]) -> Option<(Reading, usize)> {
    ribs.iter()
        .flat_map(|&(entry, rib)| {
            rows.iter().filter(|row| !row.is_empty()).map(move |row| {
                let aligned = align_row(row, rib);
                let matched = matching_digits(rib, &aligned);
                (Reading { entry, aligned }, matched)
            })
        })
        .fold(
            None,
            |best: Option<(Reading, usize)>, candidate| match best {
                Some(b) if b.1 >= candidate.1 => Some(b),
                _ => Some(candidate),
            },
        )
}

fn matching_digits(rib: &str, aligned: &str) -> usize {
    rib.bytes()
        .zip(aligned.bytes())
        .filter(|(a, b)| a == b)
        .count()
}

/// Lay a row's digit pieces over the 20 positions of `rib`. Each piece goes where it shares
/// the longest run of digits with the RIB, so a piece from the left, the right or the middle
/// lands in its place, and garbage digits around it fall outside. Pieces sharing fewer than
/// 2 digits in a row are left out; `?` marks positions nothing was read for.
fn align_row(row: &[String], rib: &str) -> String {
    let rib = rib.as_bytes();
    let mut placed: Vec<(usize, isize, &[u8])> = row
        .iter()
        .map(|piece| {
            let (offset, run) = best_offset(piece.as_bytes(), rib);
            (run, offset, piece.as_bytes())
        })
        .filter(|(run, _, _)| *run >= 2)
        .collect();
    placed.sort_by_key(|p| std::cmp::Reverse(p.0));

    let mut out = vec![b'?'; RIB_LEN];
    for (_, offset, piece) in placed {
        for (k, &digit) in piece.iter().enumerate() {
            let pos = offset + k as isize;
            if (0..RIB_LEN as isize).contains(&pos) && out[pos as usize] == b'?' {
                out[pos as usize] = digit;
            }
        }
    }
    String::from_utf8(out).unwrap_or_default()
}

/// Where `piece` shares its longest run of consecutive digits with `rib`: the RIB position of
/// the piece's first digit (negative when the piece starts with extra digits), and the run.
fn best_offset(piece: &[u8], rib: &[u8]) -> (isize, usize) {
    let longest_run = |offset: isize| {
        let mut best = 0;
        let mut run = 0;
        for (k, digit) in piece.iter().enumerate() {
            let pos = offset + k as isize;
            let same = (0..rib.len() as isize).contains(&pos) && rib[pos as usize] == *digit;
            run = if same { run + 1 } else { 0 };
            best = best.max(run);
        }
        best
    };
    (1 - piece.len() as isize..rib.len() as isize)
        .map(|offset| (offset, longest_run(offset)))
        .fold((0, 0), |best, candidate| {
            if candidate.1 > best.1 {
                candidate
            } else {
                best
            }
        })
}

/// The expected RIB that differs from `read` in the fewest positions, and those 1-based positions.
fn closest(read: &str, ribs: &[(usize, &str)]) -> (usize, Vec<usize>) {
    ribs.iter()
        .map(|&(entry, rib)| {
            let differs: Vec<usize> = read
                .bytes()
                .zip(rib.bytes())
                .enumerate()
                .filter(|(_, (a, b))| a != b)
                .map(|(i, _)| i + 1)
                .collect();
            (entry, differs)
        })
        .min_by_key(|(_, differs)| differs.len())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::{Entry, Layout};

    const RIB_A: &str = "00100123012345678912";
    const RIB_B: &str = "01400456098765432155";

    fn directory() -> Directory {
        let entry = |row, rib: &str| Entry {
            row,
            full_name: "Aymen Mammeri".to_string(),
            rib: rib.to_string(),
            rib_digits: from_sheet(rib),
        };
        let layout = Layout {
            rows: Vec::new(),
            first_row: 1,
            first_col: 1,
            header_row: Some(1),
            rib_col: 2,
        };
        Directory::new(vec![entry(2, RIB_A), entry(3, RIB_B)], layout)
    }

    fn check_text(text: &str) -> RibMatch {
        check(&directory(), &[0, 1], text).outcome
    }

    fn aligned(text: &str) -> String {
        check(&directory(), &[0, 1], text)
            .reading
            .map(|r| r.aligned)
            .unwrap_or_default()
    }

    #[test]
    fn index_finds_the_rib_holder() {
        let dir = directory();
        assert_eq!(likely_entries(&dir, "RIB 01400 456 0987"), vec![1]);
        assert!(likely_entries(&dir, "nothing 12").is_empty());
    }

    #[test]
    fn bulls_and_cows() {
        use Mark::*;
        assert_eq!(score("1234", "1243"), vec![Bull, Bull, Cow, Cow]);
        assert_eq!(score("1234", "1299"), vec![Bull, Bull, Miss, Miss]);
        assert_eq!(score("1234", "12??"), vec![Bull, Bull, NotRead, NotRead]);
        assert_eq!(score("1122", "2222"), vec![Miss, Miss, Bull, Bull]);
    }

    #[test]
    fn partial_reading_is_lined_up_on_the_rib() {
        assert_eq!(aligned("0010012301234 ##"), "0010012301234???????");
        assert_eq!(aligned("%%  98765432155"), "?????????98765432155");
        assert_eq!(aligned("001001230#2345678912"), "001001230?2345678912");
        assert_eq!(aligned("00100123012345000912"), "00100123012345000912");
        assert_eq!(aligned("001001230123 xx# 678912"), "001001230123??678912");
    }

    #[test]
    fn letters_cut_the_number_and_spaces_do_not() {
        assert_eq!(
            chunks("001DZD014 00456 09876", true),
            vec!["001", "0140045609876"]
        );
        assert_eq!(
            chunks("RIB : 001 00123 Clé 12", true),
            vec!["00100123", "12"]
        );
    }

    #[test]
    fn lone_lookalike_letters_are_read_as_digits() {
        assert_eq!(chunks("0O1 0O123 I2", true), vec!["0010012312"]);
        assert_eq!(chunks("SOCIETE 123", true), vec!["123"]);
    }

    #[test]
    fn sheet_value_keeps_digits_after_letters() {
        assert_eq!(
            from_sheet("025DZD00400123012345678912"),
            "00400123012345678912"
        );
        assert_eq!(from_sheet("001 00123 0123456789 12"), RIB_A);
    }

    #[test]
    fn full_match_after_currency_prefix() {
        let text = "BANQUE\n025DZD014 00456 0987654321 55";
        assert!(matches!(
            check_text(text),
            RibMatch::Full {
                entry: 1,
                joined: false
            }
        ));
    }

    #[test]
    fn full_match_by_joining_segments_on_one_row() {
        let text = "RIB: 0010012   3012345678912";
        assert!(matches!(check_text(text), RibMatch::Full { entry: 0, .. }));
        let split_rows = "00100123012\n345678912";
        assert!(matches!(
            check_text(split_rows),
            RibMatch::Full {
                entry: 0,
                joined: true
            }
        ));
    }

    fn matched(text: &str) -> Option<(usize, usize)> {
        match check_text(text) {
            RibMatch::Partial { entry, matched } => Some((entry, matched)),
            _ => None,
        }
    }

    #[test]
    fn half_read_rib_is_partial() {
        assert_eq!(matched("0010012301234 ##"), Some((0, 13)));
        assert_eq!(matched("%%  98765432155"), Some((1, 11)));
    }

    #[test]
    fn middle_piece_and_extra_digits_land_in_place() {
        assert_eq!(aligned("## 0123456789 ##"), "????????0123456789??");
        assert_eq!(matched("## 0123456789 ##"), Some((0, 10)));
        assert_eq!(aligned("7700100123012"), "00100123012?????????");
    }

    #[test]
    fn damaged_middle_combines_left_and_right_pieces() {
        assert_eq!(matched("001001230123 xx# 678912"), Some((0, 18)));
    }

    #[test]
    fn clean_read_with_wrong_digits_is_still_partial() {
        assert_eq!(matched("00100123012345000912"), Some((0, 17)));
        assert_eq!(matched("00100123012345000912\nN° 4412"), Some((0, 17)));
    }

    #[test]
    fn clean_read_matching_almost_nothing_is_a_mismatch() {
        match check_text("99999999999999999999") {
            RibMatch::Mismatch { read } => assert_eq!(read, "99999999999999999999"),
            _ => panic!("expected mismatch"),
        }
    }

    #[test]
    fn garbage_symbols_cut_and_both_pieces_still_count() {
        assert_eq!(
            chunks("001-00123.0#2345~678912", true),
            vec!["001001230", "2345", "678912"]
        );
        assert_eq!(matched("001001230#2345678912"), Some((0, 19)));
    }

    #[test]
    fn four_matching_digits_anywhere_is_partial() {
        assert_eq!(matched("RIB 0010 ###"), Some((0, 4)));
        assert_eq!(matched("### 8912"), Some((0, 4)));
        assert_eq!(matched("## 3456 ##"), Some((0, 4)));
        assert_eq!(matched("00 ## 12"), Some((0, 4)));
        assert_eq!(matched("RIB 0014 ###"), None);
    }

    #[test]
    fn best_row_wins_over_a_garbage_row() {
        assert_eq!(
            matched("98765432109876543210\n0010012301234"),
            Some((0, 13))
        );
    }

    #[test]
    fn nothing_agrees_is_unclear() {
        assert!(matches!(check_text("77777777"), RibMatch::Unclear { .. }));
        assert!(matches!(
            check_text("Payez contre ce cheque"),
            RibMatch::Unreadable
        ));
    }
}
