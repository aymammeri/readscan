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

/// Digit groups of a case number, as printed: "2605-2026-110201".
const CASE_GROUPS: [usize; 3] = [4, 4, 6];
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

/// Case number of a QUITTANCE, as "2605-2026-110201": three digit groups of 4, 4 and 6 on
/// one line, or 14 digits on the "Dossier Sinistre" line when OCR lost a separator. The
/// "Dossier Sinistre" line is tried first.
pub fn case_number(ocr_text: &str) -> Option<String> {
    let format = |digits: &str| format!("{}-{}-{}", &digits[..4], &digits[4..8], &digits[8..]);
    let labeled = || ocr_text.lines().filter(|l| has_word(l, &CASE_WORDS));
    let lines = || labeled().chain(ocr_text.lines());
    let grouped = lines().find_map(|line| {
        let groups: Vec<&str> = line
            .split(|c: char| !c.is_ascii_digit())
            .filter(|g| !g.is_empty())
            .collect();
        groups
            .windows(3)
            .find(|w| w.iter().map(|g| g.len()).eq(CASE_GROUPS))
            .map(|w| w.join("-"))
    });
    grouped.or_else(|| {
        labeled().find_map(|line| {
            let digits: String = line.chars().filter(char::is_ascii_digit).collect();
            (digits.len() == CASE_GROUPS.iter().sum::<usize>()).then(|| format(&digits))
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
