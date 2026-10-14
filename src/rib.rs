//! Finding a 20-digit RIB on a check page.
//!
//! A row of OCR text is cut into digit chunks wherever letters or garbage interrupt the number,
//! so a prefix like `001DZD` is dropped (`001DZD01400123...` keeps `01400123...`). Spaces and
//! printed separators never cut a chunk: a RIB printed in segments with big gaps reads as one
//! number.

use std::collections::HashMap;

use crate::sheet::Directory;

pub const RIB_LEN: usize = 20;
/// Tiers count only the account and key digits (the last 12). The bank and agency code in
/// front is shared by every customer of the branch (every postal RIB starts with 00799999),
/// so it says nothing about whose RIB it is.
/// WEAK: at least this many account digits match. Fewer can be chance.
const WEAK_MIN_DIGITS: usize = 4;
/// PARTIAL: at least this many account digits match.
const PARTIAL_MIN_DIGITS: usize = 6;
/// STRONG: at least this many account digits match.
const STRONG_MIN_DIGITS: usize = 8;

/// How much of a RIB was confirmed when not all 20 digits were.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    Weak,
    Partial,
    Strong,
}

/// Digits of `aligned` matching `rib`: account and key digits anywhere, then all digits in a
/// row from the start and in a row from the end.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Agreement {
    pub matched: usize,
    pub first: usize,
    pub last: usize,
}

impl Agreement {
    fn of(rib: &str, aligned: &str) -> Self {
        let same: Vec<bool> = rib
            .bytes()
            .zip(aligned.bytes())
            .map(|(a, b)| a == b)
            .collect();
        Self {
            matched: same
                .iter()
                .skip(RIB_LEN - ACCOUNT_AND_KEY)
                .filter(|s| **s)
                .count(),
            first: same.iter().take_while(|s| **s).count(),
            last: same.iter().rev().take_while(|s| **s).count(),
        }
    }

    /// Tier by the count of matching account and key digits, wherever they are.
    pub fn tier(self) -> Option<Tier> {
        match self.matched {
            m if m >= STRONG_MIN_DIGITS => Some(Tier::Strong),
            m if m >= PARTIAL_MIN_DIGITS => Some(Tier::Partial),
            m if m >= WEAK_MIN_DIGITS => Some(Tier::Weak),
            _ => None,
        }
    }
}
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
    /// Not all 20 digits, but enough for a tier.
    Partial {
        entry: usize,
        tier: Tier,
        agreement: Agreement,
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

/// Digits of the bank code, which the RIB key does not cover.
const BANK_DIGITS: usize = 3;
/// Account number and key: what a check prints on its own ("0000015098 17").
pub const ACCOUNT_AND_KEY: usize = 12;

/// The RIB key (last 2 digits) is 97 - (digits 4 to 18 × 100 mod 97). Any single misread digit
/// breaks it, so a reading with a good key is what is printed. The bank code is not covered.
pub fn key_ok(digits: &str) -> bool {
    if digits.len() != RIB_LEN || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let (body, key) = (&digits[BANK_DIGITS..RIB_LEN - 2], &digits[RIB_LEN - 2..]);
    match (body.parse::<u64>(), key.parse::<u64>()) {
        (Ok(body), Ok(key)) => 97 - (body * 100 % 97) == key,
        _ => false,
    }
}

/// Digit strings made of whole consecutive pieces of a row, never cut inside a piece, so
/// neighbouring numbers don't make up false readings.
fn spans(row: &[String]) -> impl Iterator<Item = String> + '_ {
    (0..row.len())
        .flat_map(move |start| (start + 1..=row.len()).map(move |end| row[start..end].concat()))
}

/// Look for any of `entry_ids`' RIBs in the OCR text of a check page. Rules, in order:
///
/// 1. Each row is cut into digit pieces (see [`chunks`]).
/// 2. MATCH: all 20 digits of a RIB, in one piece, in the pieces of a row joined, or over two
///    consecutive rows joined. Also MATCH, when the RIB's key is right: a reading with a good
///    key that differs only in the bank code (not covered by the key), or the account and key
///    printed on their own (12 digits, "0000015098 17").
/// 3. MISMATCH: a reading with a good key that is none of the RIBs. The check really shows
///    another RIB, however many digits it shares (postal RIBs all start with 00799999).
/// 4. Otherwise each row's pieces are laid on each RIB where they share the longest run of
///    digits, and the best reading gets a tier by its matching account and key digits (the
///    last 12), wherever they are: STRONG 8 to 12, PARTIAL 6 or 7, WEAK 4 or 5.
/// 5. MISMATCH: otherwise, a clean 20-digit number was read and it is not the RIB.
/// 6. UNREADABLE: otherwise; whatever digits were read are listed for a manual check.
pub fn check(dir: &Directory, entry_ids: &[usize], ocr_text: &str) -> RibCheck {
    let rows: Vec<Vec<String>> = ocr_text.lines().map(|row| chunks(row, true)).collect();
    let ribs: Vec<(usize, &str)> = entry_ids
        .iter()
        .map(|&id| (id, dir.entries[id].rib_digits.as_str()))
        .filter(|(_, rib)| !rib.is_empty())
        .collect();

    if let Some((entry, joined)) = full_match(&rows, &ribs).or_else(|| keyed_match(&rows, &ribs)) {
        let aligned = dir.entries[entry].rib_digits.clone();
        return RibCheck {
            outcome: RibMatch::Full { entry, joined },
            reading: Some(Reading { entry, aligned }),
        };
    }

    let keyed = rows
        .iter()
        .flat_map(|row| spans(row))
        .find(|digits| key_ok(digits));
    if let Some(read) = keyed {
        let (closest, _) = closest(&read, &ribs);
        return RibCheck {
            outcome: RibMatch::Mismatch { read: read.clone() },
            reading: Some(Reading {
                entry: closest,
                aligned: read,
            }),
        };
    }

    let best = best_reading(&rows, &ribs);
    if let Some((reading, agreement)) = &best
        && let Some(tier) = agreement.tier()
    {
        return RibCheck {
            outcome: RibMatch::Partial {
                entry: reading.entry,
                tier,
                agreement: *agreement,
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

/// A RIB confirmed by its key: a reading with a good key that differs only in the bank code,
/// or the RIB's own account and key printed together.
fn keyed_match(rows: &[Vec<String>], ribs: &[(usize, &str)]) -> Option<(usize, bool)> {
    let readings: Vec<String> = rows.iter().flat_map(|row| spans(row)).collect();
    ribs.iter()
        .filter(|(_, rib)| key_ok(rib))
        .find_map(|&(entry, rib)| {
            let bank_misread = readings
                .iter()
                .any(|r| key_ok(r) && r[BANK_DIGITS..] == rib[BANK_DIGITS..]);
            let account = &rib[RIB_LEN - ACCOUNT_AND_KEY..];
            let printed_alone = readings.iter().any(|r| r == account);
            (bank_misread || printed_alone).then_some((entry, printed_alone && !bank_misread))
        })
}

/// The row reading with the highest tier against an expected RIB, then the most digits.
fn best_reading(rows: &[Vec<String>], ribs: &[(usize, &str)]) -> Option<(Reading, Agreement)> {
    let rank = |a: &Agreement| (a.tier(), a.matched, a.first.max(a.last));
    ribs.iter()
        .flat_map(|&(entry, rib)| {
            rows.iter().filter(|row| !row.is_empty()).map(move |row| {
                let aligned = align_row(row, rib);
                let agreement = Agreement::of(rib, &aligned);
                (Reading { entry, aligned }, agreement)
            })
        })
        .fold(
            None,
            |best: Option<(Reading, Agreement)>, candidate| match best {
                Some(b) if rank(&b.1) >= rank(&candidate.1) => Some(b),
                _ => Some(candidate),
            },
        )
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
    use crate::sheet::Entry;

    const RIB_A: &str = "00100123012345678912";
    const RIB_B: &str = "01400456098765432155";

    fn directory() -> Directory {
        let entry = |row, rib: &str| Entry {
            row,
            full_name: "Aymen Mammeri".to_string(),
            rib: rib.to_string(),
            rib_digits: from_sheet(rib),
        };
        Directory::new(vec![entry(2, RIB_A), entry(3, RIB_B)])
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
    fn rib_key() {
        assert!(key_ok("00799999000001509817"));
        assert!(key_ok("00100832030000081249"));
        assert!(!key_ok("00799999000001509871"), "key wrong");
        assert!(!key_ok("00799999000001509917"), "one digit misread");
        assert!(key_ok("00899999000001509817"), "bank code is not covered");
    }

    fn keyed_directory() -> Directory {
        Directory::new(vec![Entry {
            row: 2,
            full_name: "Ghemired Kamel".to_string(),
            rib: "00799999000001509817".to_string(),
            rib_digits: "00799999000001509817".to_string(),
        }])
    }

    #[test]
    fn key_confirms_account_printed_alone_or_bank_misread() {
        let dir = keyed_directory();
        let full = |text: &str| matches!(check(&dir, &[0], text).outcome, RibMatch::Full { .. });
        assert!(full("Payable 0000015098 17\nAgence: 99999"));
        assert!(full("RIP 00899999000001509817"));
        assert!(!full("Payable 0000015098 71"));
    }

    #[test]
    fn reading_with_a_good_key_that_is_not_the_rib_is_a_mismatch() {
        let dir = keyed_directory();
        // Shares 8+ digits with the RIB (every postal RIB starts with 00799999).
        let read = "00799999001054247325";
        assert!(key_ok(read));
        assert!(matches!(
            check(&dir, &[0], read).outcome,
            RibMatch::Mismatch { .. }
        ));
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
            RibMatch::Partial {
                entry, agreement, ..
            } => Some((entry, agreement.matched)),
            _ => None,
        }
    }

    fn tier(text: &str) -> Option<Tier> {
        match check_text(text) {
            RibMatch::Partial { tier, .. } => Some(tier),
            _ => None,
        }
    }

    #[test]
    fn tiers_by_matching_digits() {
        // RIB_A = 00100123012345678912
        // Only the account and key count: 012345678912.
        assert_eq!(tier("## 45678912"), Some(Tier::Strong), "last 8");
        assert_eq!(tier("0010012301234567 ##"), Some(Tier::Strong), "first 8");
        assert_eq!(tier("## 5678912"), Some(Tier::Partial), "7");
        assert_eq!(tier("00100123012345 ##"), Some(Tier::Partial), "6");
        assert_eq!(tier("## 78912"), Some(Tier::Weak), "5");
        assert_eq!(tier("## 3456 ##"), Some(Tier::Weak), "4 in the middle");
        assert_eq!(tier("RIB 0014 ###"), None, "3 digits");
        assert_eq!(tier("RIB 00100123 ##"), None, "bank and agency only");
    }

    #[test]
    fn agreement_counts_both_ends() {
        let a = Agreement::of(RIB_A, "0010012301234500?912");
        assert_eq!((a.matched, a.first, a.last), (9, 14, 3));
    }

    #[test]
    fn half_read_rib_is_partial() {
        assert_eq!(matched("0010012301234 ##"), Some((0, 5)));
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
        assert_eq!(matched("001001230123 xx# 678912"), Some((0, 10)));
    }

    #[test]
    fn clean_read_with_wrong_digits_is_still_partial() {
        assert_eq!(matched("00100123012345000912"), Some((0, 9)));
        assert_eq!(matched("00100123012345000912\nN° 4412"), Some((0, 9)));
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
        assert_eq!(matched("001001230#2345678912"), Some((0, 11)));
    }

    #[test]
    fn four_matching_account_digits_anywhere_is_partial() {
        assert_eq!(matched("### 8912"), Some((0, 4)));
        assert_eq!(matched("## 3456 ##"), Some((0, 4)));
        assert_eq!(matched("RIB 0010 ###"), None, "bank code");
        assert_eq!(matched("RIB 0014 ###"), None);
    }

    #[test]
    fn postal_prefix_alone_gets_no_tier() {
        let dir = keyed_directory();
        let tier = |text: &str| match check(&dir, &[0], text).outcome {
            RibMatch::Partial { tier, .. } => Some(tier),
            _ => None,
        };
        assert_eq!(tier("RIP 00799999 ### ##"), None);
        // The account's own leading zeros still count, so they alone stay WEAK.
        assert_eq!(tier("RIP 0079999900000 ###"), Some(Tier::Weak));
        assert_eq!(tier("RIP 00799999 0000015 ###"), Some(Tier::Partial));
    }

    #[test]
    fn best_row_wins_over_a_garbage_row() {
        assert_eq!(
            matched("77777777777777777777\n0010012301234567"),
            Some((0, 8))
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
