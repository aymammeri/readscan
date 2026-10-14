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
}
