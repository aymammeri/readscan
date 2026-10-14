//! Turning the OCR text of a document's pages into a verdict.

use anyhow::Result;

use crate::matcher::{self, NameMatch};
use crate::pairing::Issue;
use crate::report::{DocResult, Status};
use crate::rib::{self, RibMatch};
use crate::scan::{self, PageText, Scanner, Turn};
use crate::sheet::Directory;
use crate::text;

/// Text shorter than this (in letters/digits) means OCR got nothing usable from the page.
const MIN_READABLE_CHARS: usize = 3;
/// Account and key digits that must match before a check alone can tell who a document
/// belongs to.
const IDENTIFY_MIN_DIGITS: usize = 9;
/// Text slanted more than this is too tilted to read reliably.
const MAX_TILT_DEGREES: f32 = 10.0;

/// 1-based label of a document, e.g. "3-4".
pub fn page_range(name_page: usize, check_page: Option<usize>) -> String {
    match check_page {
        Some(c) if c < name_page => format!("{}-{}", c + 1, name_page + 1),
        Some(c) => format!("{}-{}", name_page + 1, c + 1),
        None => format!("{}", name_page + 1),
    }
}

/// Check one document. When it doesn't fully match, a failing page that looks rotated is
/// read again turned, one turn at a time, stopping at the first that gives a clearly better
/// result; that page is flagged. Pages that don't look rotated are never re-read, and a
/// tilted page is flagged without re-reading (turning can't fix a tilt). A document that
/// still fails on a rotated or tilted page is ROTATED.
pub fn process_document(
    dir: &Directory,
    scanner: &Scanner,
    name_page: usize,
    check_page: Option<usize>,
    texts: &[Result<PageText>],
) -> DocResult {
    let mut result = check_document(dir, scanner, name_page, check_page, texts);
    result.page = check_page.unwrap_or(name_page);
    result.page_ids = [Some(name_page), check_page]
        .into_iter()
        .flatten()
        .collect();
    result.page_ids.sort_unstable();
    if let Some(c) = check_page
        && let Some(detail) = wrong_pair(dir, &result, &texts[name_page], &texts[c], c)
    {
        result.status = Status::WrongPair;
        result.detail = detail;
    }
    if result.case.is_none() {
        result.case = texts[name_page]
            .as_ref()
            .ok()
            .and_then(|t| matcher::case_number(&t.text));
    }
    if result.amount.is_none() {
        result.amount = texts[name_page]
            .as_ref()
            .ok()
            .and_then(|t| matcher::amount(&t.text));
    }
    if result.case.is_none() {
        result.detail = format!(
            "Page {}: case number not read. Check it. {}",
            name_page + 1,
            result.detail
        );
    }
    result
}

/// Secondary rule, only when the RIB did not confirm the document. The pages are paired
/// wrongly when the QUITTANCE's name is not on the check and either:
/// - the check names another person of the spreadsheet, whose name does not look like the
///   QUITTANCE's;
/// - the check shows another RIB with a good key (a MISMATCH), so it is surely someone
///   else's check, even one not in the spreadsheet.
///
/// A check whose name and RIB both can't be read never makes a WRONG PAIR.
fn wrong_pair(
    dir: &Directory,
    result: &DocResult,
    name_page: &Result<PageText>,
    check_page: &Result<PageText>,
    c: usize,
) -> Option<String> {
    let unconfirmed = matches!(
        result.status,
        Status::Partial | Status::Weak | Status::Mismatch | Status::MissingRib | Status::Unreadable
    );
    if !unconfirmed {
        return None;
    }
    let NameMatch::Found(named) = matcher::find_named(dir, name_text_of(name_page).ok()?) else {
        return None;
    };
    let check_text = text_of(check_page).ok()?;
    if matcher::names_person(dir, named, check_text) {
        return None;
    }
    let p = c + 1;
    let name = dir.person_name(named);
    match matcher::find_person(dir, check_text) {
        NameMatch::Found(holder) if holder != named && !matcher::look_alike(dir, holder, named) => {
            let row = dir.entries[dir.persons[holder].entry_ids[0]].row;
            let holder = dir.person_name(holder);
            Some(format!(
                "Page {p}: check is for {holder} (row {row}), not {name}. Fix the pairing."
            ))
        }
        _ if result.status == Status::Mismatch && rib::key_ok(&result.rib_read) => {
            let holder = matcher::check_holder(check_text)
                .map_or(String::new(), |h| format!(", made out to \"{h}\""));
            Some(format!(
                "Page {p}: check is not for {name} (other RIB{holder}). Fix the pairing."
            ))
        }
        _ => None,
    }
}

/// Give each document its spreadsheet row. A person can have several rows: several claims, or
/// several accounts. A document only takes a row of its person with the RIB its check was
/// compared with (any row when no RIB was read). Rows go first to documents whose QUITTANCE
/// amount equals the row's amount, then to the others; each time full MATCHes first, then in
/// PDF order. A document left without a row is a claim that is not in the spreadsheet: NOT IN
/// SHEET, unless it already has a red result, which stays. Duplicates and stray pages take no
/// row.
pub fn assign_rows(dir: &Directory, docs: &mut [DocResult]) {
    let mut taken = vec![false; dir.entries.len()];
    let claims: Vec<usize> = (0..docs.len())
        .filter(|&i| {
            docs[i].person.is_some() && !matches!(docs[i].status, Status::Duplicate | Status::Order)
        })
        .collect();
    let passes = [(true, true), (true, false), (false, true), (false, false)];
    for (by_amount, matched_only) in passes {
        for &i in &claims {
            let d = &docs[i];
            if d.row_entry.is_some() || (matched_only && d.status != Status::Match) {
                continue;
            }
            let compared = d.matched_entry.or(d.reading.as_ref().map(|r| r.entry));
            let fits = |e: usize| {
                let entry = &dir.entries[e];
                !taken[e]
                    && compared.is_none_or(|c| dir.entries[c].rib_digits == entry.rib_digits)
                    && (!by_amount || (d.amount.is_some() && entry.amount == d.amount))
            };
            let person = d.person.expect("claims have a person");
            if let Some(e) = dir.persons[person]
                .entry_ids
                .iter()
                .copied()
                .find(|&e| fits(e))
            {
                taken[e] = true;
                docs[i].row_entry = Some(e);
            }
        }
    }
    for &i in &claims {
        let d = &mut docs[i];
        if d.row_entry.is_none() {
            let amount = d.amount.map_or(String::new(), |a| {
                format!(" (amount {})", text::format_amount(a))
            });
            let note = format!("Claim not in the original spreadsheet{amount}.");
            if d.status.is_red() {
                d.detail = format!("{note} {}", d.detail);
            } else {
                d.status = Status::NotInSheet;
                d.detail = format!("{note} Check it.");
            }
        }
    }
}

/// A document with the same full name and case number as an earlier one is a DUPLICATE.
/// The same name with another case number is a separate case.
pub fn flag_duplicates(docs: &mut [DocResult]) {
    for i in 0..docs.len() {
        let (earlier, rest) = docs.split_at_mut(i);
        let doc = &mut rest[0];
        let (Some(person), Some(case)) = (doc.person, doc.case.clone()) else {
            continue;
        };
        let first = earlier
            .iter()
            .find(|d| d.person == Some(person) && d.case.as_ref() == Some(&case));
        if let Some(first) = first {
            doc.status = Status::Duplicate;
            doc.detail = format!(
                "Duplicate of pages {} (case {case}). Remove it.",
                first.pages
            );
        }
    }
}

fn check_document(
    dir: &Directory,
    scanner: &Scanner,
    name_page: usize,
    check_page: Option<usize>,
    texts: &[Result<PageText>],
) -> DocResult {
    let (mut best, name_ok) = evaluate(
        dir,
        name_page,
        check_page,
        name_text_of(&texts[name_page]),
        check_page.map(|c| text_of(&texts[c])),
    );
    if best.status == Status::Match {
        return best;
    }

    // Name page first: the check is only compared once the name is known.
    let mut fixed_name: Option<String> = None;
    let mut fixed_check: Option<String> = None;
    let mut turned: Vec<(usize, Turn)> = Vec::new();
    // Cheap signs a page is already upright, so it is never re-read: a readable title on the
    // name page; check wording, or RIB digits matching in place, on the check.
    let name_upright = texts[name_page]
        .as_ref()
        .is_ok_and(|t| matcher::has_title(&t.text));
    let check_upright = matched_digits(dir, &best) >= TURN_MIN_DIGITS
        || check_page.is_some_and(|c| {
            texts[c]
                .as_ref()
                .is_ok_and(|t| matcher::looks_like_check(&t.text))
        });
    let pages = [
        (!name_ok && !name_upright).then_some(name_page),
        check_page.filter(|_| !check_upright),
    ];
    for page in pages.into_iter().flatten() {
        let Ok(scanned) = &texts[page] else { continue };
        if !is_readable(&scanned.text) && !scanned.sideways {
            continue;
        }
        for turn in turns_to_try(scanned) {
            let Ok(read) = scanner.read_page(page, turn) else {
                continue;
            };
            let name_text = match &fixed_name {
                _ if page == name_page => Ok(read.top.as_str()),
                Some(t) => Ok(t.as_str()),
                None => name_text_of(&texts[name_page]),
            };
            let check_text = match (&fixed_check, check_page) {
                _ if Some(page) == check_page => Some(Ok(read.text.as_str())),
                (Some(t), _) => Some(Ok(t.as_str())),
                (None, c) => c.map(|c| text_of(&texts[c])),
            };
            let (result, _) = evaluate(dir, name_page, check_page, name_text, check_text);
            if clearly_better(dir, &result, &best) {
                best = result;
                turned.push((page, turn));
                if page == name_page {
                    best.case = matcher::case_number(&read.text);
                    best.amount = matcher::amount(&read.text);
                    fixed_name = Some(read.top);
                } else {
                    fixed_check = Some(read.text);
                }
                break;
            }
        }
        if best.status == Status::Match {
            break;
        }
    }

    // A RIB not fully matched: read the check again at another resolution, which misreads
    // different digits, and look at both readings together. Kept only if that completes it.
    if let Some(c) = check_page
        && best.person.is_some()
        && best.status != Status::Match
        && best.status != Status::MissingRib
    {
        let turn = turned
            .iter()
            .find(|(p, _)| *p == c)
            .map_or(Turn::None, |(_, t)| *t);
        let first = fixed_check
            .clone()
            .or_else(|| texts[c].as_ref().ok().map(|t| t.text.clone()))
            .unwrap_or_default();
        if let Ok(again) = scanner.read_page_at(c, turn, scan::RETRY_DPI) {
            let both = format!("{first}\n{}", again.text);
            let name_text = match &fixed_name {
                Some(t) => Ok(t.as_str()),
                None => name_text_of(&texts[name_page]),
            };
            let (result, _) = evaluate(dir, name_page, check_page, name_text, Some(Ok(&both)));
            // Only a complete RIB is kept: a stray digit read differently could otherwise
            // raise a tier by chance.
            // A RIB with a good key is what is printed, so another person's RIB is kept too.
            let complete = matched_digits(dir, &result) == rib::RIB_LEN;
            let other_rib = result.status == Status::Mismatch && rib::key_ok(&result.rib_read);
            if (result.status > best.status && complete)
                || (other_rib && best.status != Status::Mismatch)
            {
                best = DocResult {
                    rotation: best.rotation.take(),
                    ..result
                };
            }
        }
    }

    let evidence: Vec<String> = [Some(name_page), check_page]
        .into_iter()
        .flatten()
        .filter_map(|page| {
            let turn = turned.iter().find(|(p, _)| *p == page).map(|(_, t)| *t);
            rotation_of(page, turn, texts[page].as_ref().ok())
        })
        .collect();
    if evidence.is_empty() {
        return best;
    }
    let evidence = evidence.join(" ");
    let matched = best.status.is_match();
    if matched && !turned.is_empty() {
        let message = format!("{evidence} Read turned. Rescan it.");
        best.detail = format!("{} {message}", best.detail);
        best.rotation = Some(message);
    } else if !matched {
        let message = format!("{evidence} Rescan it.");
        best.status = Status::Rotated;
        best.detail = message.clone();
        best.rotation = Some(message);
    }
    best
}

/// RIB digits a turned check must match before the turn is trusted over the page as scanned.
const TURN_MIN_DIGITS: usize = 10;

/// A turned read is kept only for a real gain: a better verdict that is a full match, a found
/// name, or at least `TURN_MIN_DIGITS` RIB digits; or many more matching digits than before.
fn clearly_better(dir: &Directory, turned: &DocResult, current: &DocResult) -> bool {
    let digits = |r: &DocResult| matched_digits(dir, r);
    let trusted = match turned.status {
        Status::Match => true,
        s if s.is_tier() => digits(turned) >= TURN_MIN_DIGITS,
        _ => turned.person.is_some() && current.person.is_none(),
    };
    (turned.status > current.status && trusted)
        || (turned.status == current.status
            && turned.status.is_tier()
            && digits(turned) >= digits(current) + TURN_MIN_DIGITS / 2)
}

/// RIB digits of the document's reading that match in place.
fn matched_digits(dir: &Directory, result: &DocResult) -> usize {
    result.reading.as_ref().map_or(0, |reading| {
        let rib = &dir.entries[reading.entry].rib_digits;
        rib::score(rib, &reading.aligned)
            .iter()
            .filter(|m| **m == rib::Mark::Bull)
            .count()
    })
}

/// Turns worth reading a failing page under, cheapest bet first: none for a tilted page
/// (turning can't fix a tilt), both quarter turns for a page on its side, otherwise one read
/// upside down.
fn turns_to_try(scanned: &PageText) -> Vec<Turn> {
    if scanned.tilt > MAX_TILT_DEGREES {
        Vec::new()
    } else if scanned.sideways {
        vec![Turn::Right, Turn::Left]
    } else {
        vec![Turn::UpsideDown]
    }
}

/// What is wrong with a page's orientation, if anything: the turn that fixed it, or signs
/// on the page as scanned.
fn rotation_of(page: usize, turn: Option<Turn>, scanned: Option<&PageText>) -> Option<String> {
    let p = page + 1;
    match turn {
        Some(Turn::UpsideDown) => return Some(format!("Page {p} upside down.")),
        Some(Turn::Right | Turn::Left) => return Some(format!("Page {p} on its side.")),
        _ => {}
    }
    let scanned = scanned?;
    if scanned.tilt > MAX_TILT_DEGREES {
        Some(format!("Page {p} tilted {:.0}°.", scanned.tilt))
    } else if scanned.sideways {
        Some(format!("Page {p} on its side."))
    } else {
        None
    }
}

/// The name page's text where names are looked for: its top half.
fn name_text_of(page: &Result<PageText>) -> Result<&str, String> {
    match page {
        Ok(p) if is_readable(&p.top) => Ok(&p.top),
        Ok(_) => Err("unreadable".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn text_of(page: &Result<PageText>) -> Result<&str, String> {
    match page {
        Ok(p) if is_readable(&p.text) => Ok(&p.text),
        Ok(_) => Err("unreadable".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn is_readable(text: &str) -> bool {
    text.chars().filter(|c| c.is_alphanumeric()).count() >= MIN_READABLE_CHARS
}

/// Match one document from its two sides, in this order:
///
/// 1. The name on the name page is looked up in the spreadsheet. If the page is unreadable,
///    or its name matches several people or none, the check's RIB is used to tell who it is
///    (all 20 digits, or at least `IDENTIFY_MIN_DIGITS` of the account and key); such a document is never better
///    than STRONG.
/// 2. The check page must be readable.
/// 3. The check's RIB is compared with that person's RIBs only (see `rib::check`).
///
/// Also returns whether the name came from the name page itself.
fn evaluate(
    dir: &Directory,
    name_page: usize,
    check_page: Option<usize>,
    name: Result<&str, String>,
    check: Option<Result<&str, String>>,
) -> (DocResult, bool) {
    let n = name_page + 1;
    let mut result = DocResult::new(page_range(name_page, check_page));
    let check_ok = check.as_ref().and_then(|c| c.as_ref().ok().copied());

    let found = match name {
        Ok(text) => matcher::find_named(dir, text),
        Err(_) => NameMatch::NotFound,
    };
    let (person, name_note) = match found {
        NameMatch::Found(p) => (p, None),
        NameMatch::Ambiguous(ids) => {
            let names: Vec<&str> = ids.iter().map(|&p| dir.person_name(p)).collect();
            let note = format!("Page {n}: name could be {}.", names.join(" or "));
            match check_ok.and_then(|t| identify_by_rib(dir, Some(&ids), t)) {
                Some(p) => (p, Some(note)),
                None => return (result.unreadable(format!("{note} Check which one.")), false),
            }
        }
        NameMatch::NotFound => {
            let (note, action) = match name {
                Ok(text) => (
                    format!(
                        "Page {n}: name not in spreadsheet (read \"{}\").",
                        name_excerpt(text)
                    ),
                    "Check the name.".to_string(),
                ),
                Err(_) => (
                    format!("Page {n}: name unreadable."),
                    format!("Rescan page {n}."),
                ),
            };
            match check_ok.and_then(|t| identify_by_rib(dir, None, t)) {
                Some(p) => (p, Some(note)),
                None => return (result.unreadable(format!("{note} {action}")), false),
            }
        }
    };
    result.person = Some(person);
    let name_ok = name_note.is_none();

    let check_text = match check {
        None => {
            return (
                result.with_status(Status::NoCheck, format!("No check after page {n}. Add it.")),
                name_ok,
            );
        }
        Some(Err(_)) => {
            let c = check_page.map_or(n + 1, |c| c + 1);
            return (
                result.unreadable(format!("Page {c}: check unreadable. Rescan it.")),
                name_ok,
            );
        }
        Some(Ok(t)) => t,
    };
    let c = check_page.map_or(n + 1, |c| c + 1);
    let entries = &dir.persons[person].entry_ids;
    if entries
        .iter()
        .all(|&e| dir.entries[e].rib_digits.is_empty())
    {
        let e = entries[0];
        result.matched_entry = Some(e);
        let row = dir.entries[e].row;
        let detail = format!("Row {row}: no RIB in the spreadsheet. Add it.");
        return (result.with_status(Status::MissingRib, detail), name_ok);
    }
    let found = rib::check(dir, entries, check_text);
    result.apply_rib(dir, found, c);

    if let Some(note) = name_note {
        result.status = result.status.min(Status::Strong);
        result.detail = format!("{note} Found by RIB: {} Check the name.", result.detail);
    }
    (result, name_ok)
}

/// The name line of a name page, shortened, to show what OCR read.
fn name_excerpt(text: &str) -> String {
    let line = matcher::name_fields(text)
        .into_iter()
        .next()
        .or_else(|| {
            text.lines()
                .find(|l| !matcher::has_title(l) && is_readable(l))
        })
        .unwrap_or_default();
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(40)
        .collect()
}

/// Which of `persons` the check belongs to, if its RIB says so clearly enough. With no
/// list, the whole spreadsheet is searched through the RIB index.
fn identify_by_rib(dir: &Directory, persons: Option<&[usize]>, check_text: &str) -> Option<usize> {
    let entries: Vec<usize> = match persons {
        Some(persons) => persons
            .iter()
            .flat_map(|&p| dir.persons[p].entry_ids.iter().copied())
            .collect(),
        None => rib::likely_entries(dir, check_text),
    };
    match rib::check(dir, &entries, check_text).outcome {
        RibMatch::Full { entry, .. } => Some(dir.person_of(entry)),
        RibMatch::Partial {
            entry, agreement, ..
        } if agreement.matched >= IDENTIFY_MIN_DIGITS => Some(dir.person_of(entry)),
        _ => None,
    }
}

/// A page that fits no document. Tell whose check it is from its RIB, else from a name on it.
pub fn process_stray(dir: &Directory, page: usize, text: &Result<PageText>) -> DocResult {
    let issue = crate::pairing::describe_stray(page);
    let mut result = DocResult::new(page_range(page, None));
    result.page = page;
    result.page_ids = vec![page];
    result.status = Status::Order;
    result.order_issue = Some(issue.clone());
    let Ok(text) = text_of(text) else {
        result.detail = issue;
        return result;
    };

    let found = rib::check(dir, &rib::likely_entries(dir, text), text);
    result.reading = found.reading;
    let owner = match found.outcome {
        RibMatch::Full { entry, .. } => {
            let e = &dir.entries[entry];
            result.person = Some(dir.person_of(entry));
            result.rib_read = e.rib.clone();
            format!("Check of {}, row {}.", e.full_name, e.row)
        }
        RibMatch::Partial {
            entry, agreement, ..
        } if agreement.matched >= IDENTIFY_MIN_DIGITS => {
            let e = &dir.entries[entry];
            result.person = Some(dir.person_of(entry));
            format!("Likely check of {}, row {}.", e.full_name, e.row)
        }
        _ => match matcher::find_person(dir, text) {
            NameMatch::Found(p) => {
                result.person = Some(p);
                format!("Mentions {}.", dir.person_name(p))
            }
            _ => "Owner unknown.".to_string(),
        },
    };
    result.detail = format!("{issue} {owner}");
    result
}

impl DocResult {
    /// Turn the RIB search on check page `c` (1-based) into this document's verdict.
    fn apply_rib(&mut self, dir: &Directory, found: rib::RibCheck, c: usize) {
        self.reading = found.reading;
        match found.outcome {
            RibMatch::Full { entry, joined } => {
                self.matched_entry = Some(entry);
                self.status = Status::Match;
                self.rib_read = dir.entries[entry].rib.clone();
                self.detail = if joined {
                    "RIB OK (read in pieces).".to_string()
                } else {
                    "RIB OK.".to_string()
                };
            }
            RibMatch::Partial {
                entry,
                tier,
                agreement: a,
            } => {
                self.matched_entry = Some(entry);
                self.status = Status::from_tier(tier);
                self.rib_read = dir.entries[entry].rib.clone();
                self.detail = format!(
                    "{}/{} account digits match. Check the RIB by hand.",
                    a.matched,
                    rib::ACCOUNT_AND_KEY
                );
            }
            RibMatch::Mismatch { read, .. } => {
                self.status = Status::Mismatch;
                self.detail = format!("Wrong RIB on check: {read}. Verify check and spreadsheet.");
                self.rib_read = read;
            }
            RibMatch::Unclear { read } => {
                self.status = Status::Unreadable;
                self.rib_read = read.join(" | ");
                self.detail = format!("Page {c}: RIB unreadable. Check it by hand.");
            }
            RibMatch::Unreadable => {
                self.status = Status::MissingRib;
                self.detail = format!("Page {c}: no RIB found. Check by hand or rescan.");
            }
        }
    }

    fn unreadable(self, detail: String) -> Self {
        self.with_status(Status::Unreadable, detail)
    }

    fn with_status(mut self, status: Status, detail: String) -> Self {
        self.status = status;
        self.detail = detail;
        self
    }

    /// Record a page order problem. A re-paired document keeps its verdict; one missing its
    /// check becomes NO CHECK.
    pub fn with_issue(mut self, issue: Issue, name_page: usize) -> Self {
        let message = issue.describe(name_page);
        match issue {
            Issue::SidesSwapped => self.detail = format!("{message} {}", self.detail),
            Issue::CheckMissing { .. } => {
                self.status = Status::NoCheck;
                self.detail = message.clone();
            }
        }
        self.order_issue = Some(message);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(pages: &str, person: usize, case: Option<&str>) -> DocResult {
        DocResult {
            person: Some(person),
            status: Status::Match,
            case: case.map(str::to_string),
            ..DocResult::new(pages.to_string())
        }
    }

    #[test]
    fn same_name_and_case_is_a_duplicate() {
        let mut docs = [
            doc("1-2", 0, Some("2605-2026-110201")),
            doc("3-4", 0, Some("2605-2026-110202")),
            doc("5-6", 0, Some("2605-2026-110201")),
            doc("7-8", 1, Some("2605-2026-110201")),
            doc("9-10", 0, None),
            doc("11-12", 0, None),
        ];
        flag_duplicates(&mut docs);
        let statuses: Vec<Status> = docs.iter().map(|d| d.status).collect();
        assert_eq!(
            statuses,
            [
                Status::Match,
                Status::Match,
                Status::Duplicate,
                Status::Match,
                Status::Match,
                Status::Match
            ]
        );
        assert!(docs[2].detail.contains("pages 1-2"));
    }

    fn page(text: &str) -> Result<PageText> {
        Ok(PageText {
            text: text.to_string(),
            tilt: 0.0,
            sideways: false,
            top: text.to_string(),
        })
    }

    #[test]
    fn check_naming_someone_else_is_a_wrong_pair() {
        use crate::sheet::Entry;
        let entry = |row, name: &str| Entry {
            row,
            full_name: name.to_string(),
            rib: String::new(),
            amount: None,
            rib_digits: String::new(),
        };
        let dir = Directory::new(vec![
            entry(2, "Sarah Benali"),
            entry(3, "Karim Haddad"),
            entry(4, "Khechal Nabil"),
            entry(5, "Khechai Nabil"),
        ]);
        let pair = |status, quittance: &str, check: &str| {
            let result = DocResult {
                status,
                ..DocResult::new("1-2".to_string())
            };
            let quittance = format!("Bénéficiaire de l'indemnité {quittance}");
            wrong_pair(&dir, &result, &page(&quittance), &page(check), 1)
        };
        let detail = pair(Status::Mismatch, "SARAH BENALI", "M. KARIM HADDAD");
        assert_eq!(
            detail.as_deref(),
            Some("Page 2: check is for Karim Haddad (row 3), not Sarah Benali. Fix the pairing.")
        );
        assert!(pair(Status::Mismatch, "SARAH BENALI", "MME SARAH BENALI").is_none());
        assert!(pair(Status::Mismatch, "SARAH BENALI", "Payez contre ce cheque").is_none());
        assert!(
            pair(Status::Match, "SARAH BENALI", "M. KARIM HADDAD").is_none(),
            "RIB decides"
        );
        assert!(
            pair(Status::Strong, "SARAH BENALI", "M. KARIM HADDAD").is_none(),
            "RIB decides"
        );
        assert!(
            pair(Status::Mismatch, "KHECHAL NABIL", "M. KHECHAI NABIL").is_none(),
            "look alike"
        );

        // Another RIB with a good key, and a holder who is not in the spreadsheet.
        let other = |quittance: &str, check: &str| {
            let result = DocResult {
                status: Status::Mismatch,
                rib_read: "00799999000880744783".to_string(),
                ..DocResult::new("9-10".to_string())
            };
            let quittance = format!("Bénéficiaire de l'indemnité {quittance}");
            wrong_pair(&dir, &result, &page(&quittance), &page(check), 9)
        };
        assert_eq!(
            other("SARAH BENALI", "0008807447 83 M.BOUYEDDOUNHAKIM 04 RUE").as_deref(),
            Some(
                "Page 10: check is not for Sarah Benali (other RIB, made out to \"BOUYEDDOUNHAKIM\"). Fix the pairing."
            )
        );
        assert!(
            other("SARAH BENALI", "Payez").is_some_and(|d| d.contains("(other RIB)")),
            "no holder read"
        );
        assert!(
            other("SARAH BENALI", "MME SARAH BENALI").is_none(),
            "her own other account"
        );
    }
}
