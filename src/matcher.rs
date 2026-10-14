use strsim::normalized_levenshtein;

use crate::sheet::Directory;
use crate::text::normalize_name;

/// Minimum similarity for an OCR word to count as a name word (tolerates 1 typo in ~5 letters).
const WORD_SIMILARITY: f64 = 0.8;

/// Every name page carries this title; checks never do.
const TITLE_WORDS: [&str; 2] = ["QUITTANCE", "REGLEMENT"];

/// True when the page shows the QUITTANCE DE REGLEMENT title, tolerating accents,
/// OCR typos and words glued together.
pub fn has_title(ocr_text: &str) -> bool {
    let normalized = normalize_name(ocr_text);
    let words: Vec<&str> = normalized.split(' ').collect();
    let compact = words.concat();
    TITLE_WORDS.iter().all(|title| {
        compact.contains(title)
            || words
                .iter()
                .any(|w| normalized_levenshtein(title, w) >= WORD_SIMILARITY)
    })
}

/// Words printed on every check ("Payez contre ce chèque"). Reading one proves the page is
/// upright: upside-down print never comes out of OCR as real words.
const CHECK_WORDS: [&str; 3] = ["PAYEZ", "CHEQUE", "BANQUE"];

/// True when the page reads like an upright check.
pub fn looks_like_check(ocr_text: &str) -> bool {
    has_word(ocr_text, &CHECK_WORDS)
}

/// Digits of each part of a case number, as printed: "2605-2026-110201", "26741-2026-110334".
const CASE_AGENCY_DIGITS: [usize; 2] = [4, 5];
const CASE_YEAR_DIGITS: usize = 4;
const CASE_SERIAL_DIGITS: usize = 6;
/// Words before the case number on a QUITTANCE ("Dossier Sinistre").
const CASE_WORDS: [&str; 2] = ["DOSSIER", "SINISTRE"];

/// Labels of the name fields on a QUITTANCE, most reliable first: "Bénéficiaire de
/// l'indemnité OMARA BADIS", then "Assuré: OMARA BADIS". Names elsewhere (Par, signatures,
/// the text below) are never used.
const NAME_LABELS: [&str; 2] = ["BENEFICIAIRE", "ASSURE"];

/// The name field lines of a QUITTANCE: Bénéficiaire lines, then Assuré lines.
pub fn name_fields(ocr_text: &str) -> Vec<&str> {
    NAME_LABELS
        .iter()
        .flat_map(|label| ocr_text.lines().filter(|line| has_word(line, &[label])))
        .collect()
}

/// The person named in the QUITTANCE's name fields: the Bénéficiaire if it names someone in
/// the spreadsheet, else the Assuré. NotFound when neither field was read.
pub fn find_named(dir: &Directory, ocr_text: &str) -> NameMatch {
    let mut found = NameMatch::NotFound;
    for label in NAME_LABELS {
        let lines: Vec<&str> = ocr_text.lines().filter(|l| has_word(l, &[label])).collect();
        if lines.is_empty() {
            continue;
        }
        found = find_person(dir, &lines.join("\n"));
        if !matches!(found, NameMatch::NotFound) {
            break;
        }
    }
    found
}

/// True when one word of `line` is close to one of `words`.
fn has_word(line: &str, words: &[&str]) -> bool {
    normalize_name(line).split(' ').any(|w| {
        words
            .iter()
            .any(|word| normalized_levenshtein(word, w) >= WORD_SIMILARITY)
    })
}

/// Case number of a QUITTANCE, as "2605-2026-110201": the agency code (4 or 5 digits), a
/// year, 6 digits. OCR often drops a separator ("2617 2026110388"), so neighbouring digit
/// groups on a line are joined until they make a whole case number, as long as they only
/// break between its parts. The middle part must be a year (20xx), which rules out other
/// numbers ("Police 2610 1100049211"). The "Dossier Sinistre" line is tried first.
pub fn case_number(ocr_text: &str) -> Option<String> {
    let labeled = ocr_text.lines().filter(|l| has_word(l, &CASE_WORDS));
    labeled.chain(ocr_text.lines()).find_map(case_in_line)
}

fn case_in_line(line: &str) -> Option<String> {
    let groups: Vec<&str> = line
        .split(|c: char| !c.is_ascii_digit())
        .filter(|g| !g.is_empty())
        .collect();
    (0..groups.len()).find_map(|start| {
        CASE_AGENCY_DIGITS.into_iter().find_map(|agency| {
            let year_end = agency + CASE_YEAR_DIGITS;
            let total = year_end + CASE_SERIAL_DIGITS;
            let mut digits = String::new();
            for group in &groups[start..] {
                digits.push_str(group);
                let len = digits.len();
                if len == total {
                    break;
                }
                if len > total || (len != agency && len != year_end) {
                    return None;
                }
            }
            let year = digits.get(agency..year_end)?;
            (digits.len() == total && year.starts_with("20"))
                .then(|| format!("{}-{year}-{}", &digits[..agency], &digits[year_end..]))
        })
    })
}

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

#[cfg(test)]
mod tests {
    use super::has_title;

    #[test]
    fn title_found_with_accents_and_typos() {
        assert!(has_title("QUITTANCE DE RÈGLEMENT\nNom : X"));
        assert!(has_title("Quittance de reglement"));
        assert!(has_title("QUITTANCEDEREGLEMENT"));
        assert!(has_title("QU1TTANCE DE REGLEMEN"));
    }

    #[test]
    fn upright_check_words() {
        assert!(super::looks_like_check("Payez contre ce chèque"));
        assert!(!super::looks_like_check("ənbəɥɔ əɔ ǝɹʇuoɔ zəʎɐd"));
    }

    #[test]
    fn case_number_from_noisy_ocr() {
        use super::case_number;
        assert_eq!(
            case_number("Nom : X\nDossie? Sinis:a 2605-2026-110201").as_deref(),
            Some("2605-2026-110201")
        );
        assert_eq!(
            case_number("Dossier Sinistre : 2605 2026 110201").as_deref(),
            Some("2605-2026-110201")
        );
        assert_eq!(
            case_number("Dossier Sinistre 26052026-110201").as_deref(),
            Some("2605-2026-110201")
        );
        assert_eq!(case_number("Date 26-05-2026\nMontant 25 000 DA"), None);
        assert_eq!(
            case_number("N? Dossier Sinistre 2617 2026110388 Survenu le 05/09/2026").as_deref(),
            Some("2617-2026-110388")
        );
        assert_eq!(
            case_number("N Dossier Sinistre 3011. 2026110792 Survenue 15/08/2026").as_deref(),
            Some("3011-2026-110792")
        );
        assert_eq!(case_number("Police 2610 1100049211"), None);
        assert_eq!(
            case_number("N Dossier Sinistre 26741 2026 110334 Survenue 17/07/2026").as_deref(),
            Some("26741-2026-110334")
        );
        assert_eq!(
            case_number("N ?Dossier Sinistre 260 2026110743 Survenu e 09/09/2026"),
            None
        );
        assert_eq!(
            case_number("N° Dossier Sinistre   2610 - 2026 - 110307   Survenu le 03/03/2026")
                .as_deref(),
            Some("2610-2026-110307")
        );
    }

    #[test]
    fn name_fields_beneficiary_first() {
        let page = "Assuré: LAIB ABDELHALIM\nBénéficiaire de l'indemnité   OMARA BADIS\nPar : CHAIR NAOUFEL";
        assert_eq!(
            super::name_fields(page),
            vec![
                "Bénéficiaire de l'indemnité   OMARA BADIS",
                "Assuré: LAIB ABDELHALIM"
            ]
        );
        assert!(super::name_fields("Nom et prenom : X\nPar : CHAIR NAOUFEL").is_empty());
    }

    #[test]
    fn check_page_has_no_title() {
        assert!(!has_title(
            "BANQUE NATIONALE\nPayez contre ce cheque\nRIB : 001 00123"
        ));
        assert!(!has_title("QUITTANCE"));
    }
}
