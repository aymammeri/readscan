use colored::Colorize;

use crate::rib::{self, Mark, Reading, Tier};
use crate::sheet::Directory;

/// Ordered from worst to best, so comparing results picks the better one.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Status {
    /// A scanned claim with no row of its own in the spreadsheet.
    NotInSheet,
    NotScanned,
    /// Same full name and case number as an earlier document.
    Duplicate,
    /// A QUITTANCE with no check page after it.
    NoCheck,
    /// A page that fits no document.
    Order,
    /// No RIB on the check, or none in the spreadsheet.
    MissingRib,
    Unreadable,
    /// A page is upside down, on its side or too tilted to match.
    Rotated,
    /// The check names another person than the QUITTANCE.
    WrongPair,
    Mismatch,
    /// 4 or 5 of the 12 account and key digits match.
    Weak,
    /// 6 or 7 of the 12 account and key digits match.
    Partial,
    /// 8 to 11 of the 12 account and key digits match (12 when the RIB's key is wrong).
    Strong,
    Match,
}

impl Status {
    pub const ALL: [Status; 14] = [
        Status::Match,
        Status::Strong,
        Status::Partial,
        Status::Weak,
        Status::Mismatch,
        Status::WrongPair,
        Status::Duplicate,
        Status::NoCheck,
        Status::MissingRib,
        Status::Unreadable,
        Status::NotInSheet,
        Status::Rotated,
        Status::Order,
        Status::NotScanned,
    ];

    /// Results shown in red, in `ALL` order: each needs an action.
    pub const RED: [Status; 8] = [
        Status::Partial,
        Status::Weak,
        Status::Mismatch,
        Status::WrongPair,
        Status::Duplicate,
        Status::NoCheck,
        Status::MissingRib,
        Status::Unreadable,
    ];
    pub const RED_LABEL: &str = "RED TOTAL";
    pub const RED_MEANING: &str = "Needs action. Details below.";

    pub fn is_red(self) -> bool {
        Status::RED.contains(&self)
    }

    /// MATCH, STRONG, PARTIAL or WEAK: the check shows this RIB, fully or in part.
    pub fn is_match(self) -> bool {
        self >= Status::Weak
    }

    /// STRONG, PARTIAL or WEAK.
    pub fn is_tier(self) -> bool {
        self.is_match() && self != Status::Match
    }

    pub fn from_tier(tier: Tier) -> Self {
        match tier {
            Tier::Strong => Status::Strong,
            Tier::Partial => Status::Partial,
            Tier::Weak => Status::Weak,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Match => "MATCH",
            Status::Strong => "STRONG",
            Status::Partial => "PARTIAL",
            Status::Weak => "WEAK",
            Status::Mismatch => "MISMATCH",
            Status::WrongPair => "WRONG PAIR",
            Status::Duplicate => "DUPLICATE",
            Status::NoCheck => "NO CHECK",
            Status::MissingRib => "MISSING RIB",
            Status::Unreadable => "UNREADABLE",
            Status::Rotated => "ROTATED",
            Status::Order => "PAGE ORDER",
            Status::NotScanned => "NOT IN PDF",
            Status::NotInSheet => "NOT IN SHEET",
        }
    }

    /// Green, blue, yellow by confidence; red for everything that failed.
    pub fn fill_argb(self) -> &'static str {
        match self {
            Status::Match => "FFC6EFCE",
            Status::Strong => "FFBDD7EE",
            Status::NotInSheet => "FFFFEB9C",
            Status::Partial
            | Status::Weak
            | Status::Mismatch
            | Status::WrongPair
            | Status::Duplicate
            | Status::NoCheck
            | Status::MissingRib
            | Status::Unreadable => "FFFFC7CE",
            Status::Rotated => "FFE4DFEC",
            Status::Order => "FFF8CBAD",
            Status::NotScanned => "FFD9D9D9",
        }
    }

    /// Excel fill of a row, so a test can tell the colors apart.
    #[cfg(test)]
    fn is_yellow(self) -> bool {
        self.fill_argb() == "FFFFEB9C"
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Status::Match => "All 20 RIB digits, or all 12 account digits with a good key, match.",
            Status::Strong => "8+ of 12 account digits match. Check the rest by hand.",
            Status::Partial => "6 or 7 of 12 account digits match. Check by hand.",
            Status::Weak => "4 or 5 of 12 account digits match. Check by hand.",
            Status::Mismatch => "Wrong RIB on the check. Verify.",
            Status::WrongPair => "Check names another person than the QUITTANCE. Fix the pairing.",
            Status::Duplicate => "Same name and case number as an earlier document. Remove it.",
            Status::NoCheck => "QUITTANCE without its check. Add the check.",
            Status::MissingRib => "No RIB on the check or in the spreadsheet. Add it.",
            Status::Unreadable => "Not readable. Check by hand.",
            Status::Rotated => "Page rotated or tilted. Rescan it.",
            Status::Order => "Page fits no document. Fix the order.",
            Status::NotScanned => "No check scanned. Scan it.",
            Status::NotInSheet => "Claim not in the original spreadsheet. Check it.",
        }
    }

    fn paint(self, text: &str) -> colored::ColoredString {
        match self {
            Status::Match => text.green(),
            Status::Strong => text.blue(),
            Status::NotInSheet => text.yellow(),
            Status::Partial
            | Status::Weak
            | Status::Mismatch
            | Status::WrongPair
            | Status::Duplicate
            | Status::NoCheck
            | Status::MissingRib
            | Status::Unreadable => text.red(),
            Status::Rotated => text.bright_blue(),
            Status::Order => text.magenta(),
            Status::NotScanned => text.dimmed(),
        }
    }
}

/// Outcome for one two-sided document (name page + check page).
pub struct DocResult {
    pub pages: String,
    pub person: Option<usize>,
    pub matched_entry: Option<usize>,
    pub status: Status,
    /// RIB found on the check, or the digit lines that were read instead.
    pub rib_read: String,
    /// The digits read on the check, lined up on the expected RIB.
    pub reading: Option<Reading>,
    pub detail: String,
    /// Set when the pages around this document were out of physical order.
    pub order_issue: Option<String>,
    /// Set when a page of this document is upside down, on its side or tilted.
    pub rotation: Option<String>,
    /// Case number on the QUITTANCE ("Dossier Sinistre 2605-2026-110201").
    pub case: Option<String>,
    /// Page (0-based) where the RIB to check is: the check, else the QUITTANCE.
    pub page: usize,
    /// Every page (0-based) of the document, in PDF order.
    pub page_ids: Vec<usize>,
    /// Amount on the QUITTANCE, in cents.
    pub amount: Option<u64>,
    /// The spreadsheet row (entry) this document is listed on, see `document::assign_rows`.
    pub row_entry: Option<usize>,
    /// A claim of a known person with no row left in the spreadsheet.
    pub not_in_sheet: bool,
}

impl DocResult {
    pub fn new(pages: String) -> Self {
        Self {
            pages,
            person: None,
            matched_entry: None,
            status: Status::Unreadable,
            rib_read: String::new(),
            reading: None,
            detail: String::new(),
            order_issue: None,
            rotation: None,
            case: None,
            page: 0,
            page_ids: Vec::new(),
            amount: None,
            row_entry: None,
            not_in_sheet: false,
        }
    }
}

/// One line of the results: a document, or a spreadsheet row no document matched.
pub struct ResultRow {
    pub pages: String,
    pub status: Status,
    pub detail: String,
    /// Digits read on the check, lined up on `rib_digits` (`?` where nothing was read).
    pub aligned: Option<String>,
    /// What was read on the check when it could not be lined up on a RIB.
    pub read: String,
    /// The spreadsheet RIB compared, as written there.
    pub rib: String,
    pub rib_digits: String,
    /// Row number of the RIB in the input spreadsheet ("4", or "4 | 5" for all of a person's
    /// RIBs), empty when nobody was found.
    pub sheet_row: String,
    pub full_name: String,
    /// Case number, `CASE_NOT_READ` when a QUITTANCE was scanned but its number not read.
    pub case: String,
}

/// Case number cell of a document whose QUITTANCE number could not be read.
pub const CASE_NOT_READ: &str = "NOT READ";

impl ResultRow {
    pub fn marks(&self) -> Option<Vec<Mark>> {
        self.aligned
            .as_deref()
            .map(|aligned| rib::score(&self.rib_digits, aligned))
    }
}

/// Documents in PDF page order, then the spreadsheet rows no document took (NOT IN PDF), then
/// the claims that are not in the spreadsheet. Those come last so they never stand in a row of
/// the spreadsheet.
pub fn result_rows(dir: &Directory, docs: &[DocResult]) -> Vec<ResultRow> {
    let mut rows: Vec<ResultRow> = docs
        .iter()
        .filter(|d| !d.not_in_sheet)
        .map(|d| doc_row(dir, d))
        .collect();
    for (id, entry) in dir.entries.iter().enumerate() {
        if docs.iter().any(|d| d.row_entry == Some(id)) {
            continue;
        }
        let person = dir.person_of(id);
        let other = docs
            .iter()
            .find(|d| d.person == Some(person) && d.row_entry.is_some());
        let detail = match other {
            Some(d) => format!(
                "Row {}: not in the PDF (pages {} are another row). Scan it.",
                entry.row, d.pages
            ),
            None => format!("Row {}: not in the PDF. Scan it.", entry.row),
        };
        rows.push(ResultRow {
            pages: String::new(),
            status: Status::NotScanned,
            detail,
            aligned: None,
            read: String::new(),
            rib: entry.rib.clone(),
            rib_digits: entry.rib_digits.clone(),
            sheet_row: entry.row.to_string(),
            full_name: entry.full_name.clone(),
            case: String::new(),
        });
    }
    rows.extend(
        docs.iter()
            .filter(|d| d.not_in_sheet)
            .map(|d| doc_row(dir, d)),
    );
    rows
}

fn doc_row(dir: &Directory, d: &DocResult) -> ResultRow {
    // The RIB shown: the document's row, else the one matched, else the one the reading was
    // lined up on, else all of the person's RIBs.
    let entry = d
        .row_entry
        .or(d.matched_entry)
        .or_else(|| d.reading.as_ref().map(|r| r.entry));
    let (rib, rib_digits, sheet_row) = match (entry, d.person) {
        (Some(e), _) => (
            dir.entries[e].rib.clone(),
            dir.entries[e].rib_digits.clone(),
            dir.entries[e].row.to_string(),
        ),
        (None, Some(p)) => {
            let entries = || dir.persons[p].entry_ids.iter().map(|&e| &dir.entries[e]);
            let ribs: Vec<&str> = entries().map(|e| e.rib.as_str()).collect();
            let rows: Vec<String> = entries().map(|e| e.row.to_string()).collect();
            (ribs.join(" | "), String::new(), rows.join(" | "))
        }
        (None, None) => (String::new(), String::new(), String::new()),
    };
    // Without a known person, a RIB found on the check tells whose row it is.
    let full_name = match (d.person, entry) {
        (Some(p), _) => dir.person_name(p).to_string(),
        (None, Some(e)) => dir.entries[e].full_name.clone(),
        (None, None) => "(not found)".to_string(),
    };

    let aligned = d
        .reading
        .as_ref()
        .filter(|_| !rib_digits.is_empty())
        .map(|r| r.aligned.clone());
    // A claim not in the spreadsheet has no row of its own.
    let sheet_row = if d.not_in_sheet {
        String::new()
    } else {
        sheet_row
    };
    ResultRow {
        pages: d.pages.clone(),
        status: d.status,
        detail: d.detail.clone(),
        // Only a page set aside has no QUITTANCE to read a case number from.
        case: match &d.case {
            Some(case) => case.clone(),
            None if d.status == Status::Order => String::new(),
            None => CASE_NOT_READ.to_string(),
        },
        aligned,
        read: d.rib_read.clone(),
        rib,
        rib_digits,
        sheet_row,
        full_name,
    }
}

pub fn count<T>(items: &[T], status: Status, get: impl Fn(&T) -> Status) -> usize {
    items.iter().filter(|i| get(i) == status).count()
}

/// Every page order and rotation problem, as (pages, message), in page order.
pub fn page_issues(docs: &[DocResult]) -> Vec<(&str, &str)> {
    docs.iter()
        .flat_map(|d| {
            [d.order_issue.as_deref(), d.rotation.as_deref()]
                .into_iter()
                .flatten()
                .map(|issue| (d.pages.as_str(), issue))
        })
        .collect()
}

/// Count of each mark, skipping zeros, as "13 right, 1 wrong, 6 unread".
pub fn describe_marks(marks: &[Mark]) -> String {
    let n = |m: Mark| marks.iter().filter(|&&x| x == m).count();
    [
        (Mark::Bull, "right"),
        (Mark::Miss, "wrong"),
        (Mark::NotRead, "unread"),
    ]
    .into_iter()
    .filter(|(m, _)| n(*m) > 0)
    .map(|(m, word)| format!("{} {word}", n(m)))
    .collect::<Vec<_>>()
    .join(", ")
}

/// The reading with each digit colored by its mark.
fn paint_marks(aligned: &str, marks: &[Mark]) -> String {
    aligned
        .chars()
        .zip(marks)
        .map(|(c, mark)| {
            let c = c.to_string();
            match mark {
                Mark::Bull => c.green().bold(),
                Mark::Miss => c.red().bold(),
                Mark::NotRead => c.dimmed(),
            }
            .to_string()
        })
        .collect()
}

/// The results as in the results file: pages, case number, result, details, RIB read, RIB,
/// full name.
pub fn print_results(rows: &[ResultRow], docs: &[DocResult]) {
    println!("{}", "Results (PDF page order)".bold());
    for r in rows {
        let read = match (r.aligned.as_deref(), r.marks()) {
            (Some(aligned), Some(marks)) => paint_marks(aligned, &marks),
            _ => format!("{:<20}", r.read.chars().take(20).collect::<String>()),
        };
        let case = if r.case == CASE_NOT_READ {
            r.case.red().bold().to_string()
        } else {
            r.case.clone()
        };
        let head = format!("  {:<7} ", r.pages);
        let status = format!(" {:<12}", r.status.label());
        println!(
            "{}{case:<16}{} {} {read} {}",
            r.status.paint(&head),
            r.status.paint(&status),
            r.detail,
            r.status.paint(&format!("{} | {}", r.rib, r.full_name))
        );
    }
    println!(
        "  RIB digits read on the check: {}  {}  {}",
        "right".green().bold(),
        "wrong".red().bold(),
        "? unread".dimmed()
    );

    let issues = page_issues(docs);
    if !issues.is_empty() {
        println!("\n{}", "Page issues".magenta().bold());
        for (pages, issue) in issues {
            println!("  {}", format!("{pages:<7} {issue}").magenta());
        }
    }

    println!("\n{}", format!("Total: {}", rows.len()).bold());
    let red = red_total(rows);
    for status in Status::ALL {
        if status == Status::RED[0] && red > 0 {
            let line = format!("{red} red, need action");
            println!("  {}", status.paint(&line).bold());
        }
        let n = count(rows, status, |r| r.status);
        if n > 0 {
            let indent = if status.is_red() { "    " } else { "  " };
            let line = format!("{n} {}", status.label().to_lowercase());
            println!("{indent}{}", status.paint(&line));
        }
    }
}

/// Rows shown in red.
pub fn red_total(rows: &[ResultRow]) -> usize {
    rows.iter().filter(|r| r.status.is_red()).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::Entry;

    fn directory() -> Directory {
        let entry = |row, full_name: &str, rib: &str| Entry {
            row,
            full_name: full_name.to_string(),
            rib: rib.to_string(),
            amount: None,
            rib_digits: rib.to_string(),
        };
        Directory::new(vec![
            entry(2, "Aymen Mammeri", "111"),
            entry(3, "Aymen Mammeri", "222"),
            entry(4, "Sarah Benali", "333"),
        ])
    }

    fn doc(
        pages: &str,
        person: Option<usize>,
        matched: Option<usize>,
        status: Status,
    ) -> DocResult {
        DocResult {
            person,
            matched_entry: matched,
            status,
            ..DocResult::new(pages.to_string())
        }
    }

    /// Documents get their rows as in a run.
    fn rows_of(docs: &mut [DocResult]) -> Vec<ResultRow> {
        let dir = directory();
        crate::document::assign_rows(&dir, docs);
        result_rows(&dir, docs)
    }

    #[test]
    fn rows_follow_pdf_order_then_rows_not_in_pdf() {
        let mut docs = [
            doc("1-2", Some(1), None, Status::Unreadable),
            doc("3-4", Some(0), Some(1), Status::Match),
            doc("5-6", None, None, Status::Unreadable),
        ];
        let rows = rows_of(&mut docs);
        let pages: Vec<&str> = rows.iter().map(|r| r.pages.as_str()).collect();
        assert_eq!(pages, vec!["1-2", "3-4", "5-6", ""]);
        let sheet_rows: Vec<&str> = rows.iter().map(|r| r.sheet_row.as_str()).collect();
        assert_eq!(sheet_rows, vec!["4", "3", "", "2"]);
        assert_eq!(rows[3].status, Status::NotScanned);
        assert!(rows[3].detail.contains("pages 3-4 are another row"));
    }

    #[test]
    fn failing_document_takes_the_row_of_the_rib_it_was_compared_with() {
        let mut failing = doc("1-2", Some(0), None, Status::Mismatch);
        failing.reading = Some(Reading {
            entry: 1,
            aligned: "999".to_string(),
        });
        let rows = rows_of(&mut [failing]);
        assert_eq!(rows[0].rib, "222");
        assert_eq!(rows[0].aligned.as_deref(), Some("999"));
        assert_eq!(rows[1].rib, "111");
        assert_eq!(rows[1].status, Status::NotScanned);
    }

    #[test]
    fn amount_tells_claims_of_one_person_apart() {
        let entry = |row, amount| Entry {
            row,
            full_name: "Douadi Amel".to_string(),
            rib: "444".to_string(),
            amount: Some(amount),
            rib_digits: "444".to_string(),
        };
        let dir = Directory::new(vec![entry(2, 550_000), entry(3, 1_200_000)]);
        let claim = |pages: &str, amount| DocResult {
            person: Some(0),
            matched_entry: Some(0),
            status: Status::Match,
            amount,
            ..DocResult::new(pages.to_string())
        };
        let mut docs = [
            claim("1-2", Some(1_200_000)),
            claim("3-4", Some(990_000)),
            claim("5-6", Some(550_000)),
        ];
        crate::document::assign_rows(&dir, &mut docs);
        let rows = result_rows(&dir, &docs);
        let pages: Vec<&str> = rows.iter().map(|r| r.pages.as_str()).collect();
        assert_eq!(
            pages,
            vec!["1-2", "5-6", "3-4"],
            "PDF order, the extra claim last"
        );
        let sheet_rows: Vec<&str> = rows.iter().map(|r| r.sheet_row.as_str()).collect();
        assert_eq!(sheet_rows, vec!["3", "2", ""]);
        assert_eq!(rows[2].status, Status::NotInSheet);
        assert_eq!(
            rows[2].detail,
            "Claim not in the original spreadsheet (amount 9.900,00). Check it."
        );
        assert!(Status::NotInSheet.is_yellow() && !Status::NotInSheet.is_red());
    }

    #[test]
    fn red_claim_not_in_the_spreadsheet_stays_red_below() {
        let mut docs = [
            doc("1-2", Some(1), Some(2), Status::WrongPair),
            doc("3-4", Some(1), Some(2), Status::Strong),
            doc("5-6", Some(1), Some(2), Status::Match),
        ];
        let rows = rows_of(&mut docs);
        let pages: Vec<&str> = rows.iter().map(|r| r.pages.as_str()).collect();
        assert_eq!(
            pages,
            vec!["5-6", "", "", "1-2", "3-4"],
            "the full match takes the row"
        );
        assert_eq!(rows[3].status, Status::WrongPair);
        assert!(
            rows[3]
                .detail
                .starts_with("Claim not in the original spreadsheet.")
        );
        assert_eq!(rows[4].status, Status::NotInSheet);
    }

    #[test]
    fn without_amounts_claims_take_rows_in_pdf_order() {
        let mut docs = [
            doc("1-2", Some(1), Some(2), Status::Match),
            doc("3-4", Some(1), Some(2), Status::Match),
        ];
        let rows = rows_of(&mut docs);
        let pages: Vec<&str> = rows.iter().map(|r| r.pages.as_str()).collect();
        assert_eq!(pages, vec!["1-2", "", "", "3-4"]);
        assert_eq!(rows[0].sheet_row, "4");
        assert_eq!(rows[3].status, Status::NotInSheet);
    }

    #[test]
    fn statuses_rank_from_worst_to_best() {
        assert!(Status::Match > Status::Strong);
        assert!(Status::Strong > Status::Partial);
        assert!(Status::Partial > Status::Weak);
        assert!(Status::Weak > Status::Mismatch);
        assert_eq!(Status::Weak.fill_argb(), Status::Duplicate.fill_argb());
        for status in Status::ALL {
            let red = status.fill_argb() == Status::Weak.fill_argb();
            assert_eq!(status.is_red(), red, "{status:?}");
        }
        assert!(Status::Weak.is_tier() && !Status::Match.is_tier() && !Status::Mismatch.is_match());
        assert!(Status::Partial.is_red(), "PARTIAL is checked by hand");
    }
}
