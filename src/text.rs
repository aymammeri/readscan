use unicode_normalization::UnicodeNormalization;

/// Uppercase, strip accents, turn every non-alphanumeric char into a single space.
pub fn normalize_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.nfd().filter(|c| c.is_ascii() || c.is_alphanumeric()) {
        if c.is_alphanumeric() {
            out.extend(c.to_uppercase());
        } else if !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.trim().to_string()
}

/// Keep only alphanumerics, uppercased, with OCR look-alikes folded onto digits.
/// Applied to both the spreadsheet RIB and the OCR text so the comparison is symmetric.
pub fn normalize_rib(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| match c.to_ascii_uppercase() {
            'O' | 'Q' | 'D' => '0',
            'I' | 'L' | 'T' => '1',
            'Z' => '2',
            'S' => '5',
            'G' => '6',
            'B' => '8',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_strips_accents_and_punctuation() {
        assert_eq!(
            normalize_name("  Benali-Hélène  O'Brien "),
            "BENALI HELENE O BRIEN"
        );
    }

    #[test]
    fn rib_folds_lookalikes() {
        assert_eq!(normalize_rib("0O1 2345-I0S8"), "00123451058");
    }
}
