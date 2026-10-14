use unicode_normalization::UnicodeNormalization;

/// Uppercase, strip accents, turn every non-alphanumeric char into a single space.
pub fn normalize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.nfd().filter(|c| c.is_ascii() || c.is_alphanumeric()) {
        // OCR has no accented letters and reads them as `?`; in French that is nearly always
        // an é or è ("B?n?ficiaire", "Assur?").
        if c == '?' {
            out.push('E');
        } else if c.is_alphanumeric() {
            out.extend(c.to_uppercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_question_mark_is_an_e() {
        assert_eq!(
            normalize_name("B?n?ficiaire de l'indemnit? Assur?:"),
            "BENEFICIAIRE DE L INDEMNITE ASSURE"
        );
    }

    #[test]
    fn name_strips_accents_and_punctuation() {
        assert_eq!(
            normalize_name("  Benali-Hélène  O'Brien "),
            "BENALI HELENE O BRIEN"
        );
    }
}
