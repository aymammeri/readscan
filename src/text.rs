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

/// Most digits before the first separator of an amount ("574641.02" as stored by Excel).
const MAX_LEADING_DIGITS: usize = 6;

/// An amount in cents, from text like "574.641,02", "12.865 15", "7500.00" or "25000". Digit
/// groups are read left to right: a first group of up to 6 digits, groups of 3 for the
/// thousands, then 1 or 2 digits of cents (a group of 5 is 3 + 2 glued by OCR). Whatever
/// follows the cents is ignored. None when the text starts with no such number.
pub fn parse_amount(text: &str) -> Option<u64> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let groups: Vec<&str> = text[start..]
        .split(['.', ',', ':', ' ', '\u{a0}', '\''])
        .take_while(|g| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit()))
        .collect();
    let (first, rest) = groups.split_first()?;
    if first.len() > MAX_LEADING_DIGITS {
        return None;
    }
    let mut units: u64 = first.parse().ok()?;
    let mut cents = 0;
    for group in rest {
        match group.len() {
            3 => units = units * 1000 + group.parse::<u64>().ok()?,
            5 => {
                units = units * 1000 + group[..3].parse::<u64>().ok()?;
                cents = group[3..].parse().ok()?;
                break;
            }
            2 => {
                cents = group.parse().ok()?;
                break;
            }
            1 => {
                cents = group.parse::<u64>().ok()? * 10;
                break;
            }
            _ => break,
        }
    }
    Some(units * 100 + cents)
}

/// Cents as printed on a QUITTANCE: "5.500,00".
pub fn format_amount(cents: u64) -> String {
    let units = (cents / 100).to_string();
    let mut grouped = String::new();
    for (i, c) in units.chars().enumerate() {
        if i > 0 && (units.len() - i).is_multiple_of(3) {
            grouped.push('.');
        }
        grouped.push(c);
    }
    format!("{grouped},{:02}", cents % 100)
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

    #[test]
    fn amounts_as_printed_or_misread() {
        use super::parse_amount;
        assert_eq!(parse_amount("574.641.02"), Some(57_464_102));
        assert_eq!(parse_amount("12.549,50"), Some(1_254_950));
        assert_eq!(parse_amount("9.062:51"), Some(906_251));
        assert_eq!(parse_amount("27.596 26"), Some(2_759_626));
        assert_eq!(parse_amount("12.86515"), Some(1_286_515), "3 + 2 glued");
        assert_eq!(
            parse_amount("43.837.50 6"),
            Some(4_383_750),
            "junk after the cents"
        );
        assert_eq!(parse_amount("7500.00"), Some(750_000));
        assert_eq!(
            parse_amount("574641.02"),
            Some(57_464_102),
            "as stored by Excel"
        );
        assert_eq!(parse_amount("574641.5"), Some(57_464_150));
        assert_eq!(parse_amount("25000"), Some(2_500_000));
        assert_eq!(parse_amount("25 000 DA"), Some(2_500_000));
        assert_eq!(parse_amount("DA"), None);
        assert_eq!(super::format_amount(57_464_102), "574.641,02");
        assert_eq!(super::format_amount(550_000), "5.500,00");
    }
}
