use colored::Colorize;

use crate::rib::{self, Mark, Reading, Tier};
use crate::sheet::Directory;

/// Ordered from worst to best, so `max` picks the best outcome.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Status {
    NotScanned,
    Yellow,
    /// Pages out of physical order; the document could not be completed.
    Order,
    /// A page is upside down, on its side or too tilted to match.
    Rotated,
    Red,
    /// 4+ RIB digits match, anywhere.
    Weak,
    /// 7+ last RIB digits match.
    Partial,
    /// 11+ first or last RIB digits match.
    Strong,
    Green,
}

impl Status {
    pub const ALL: [Status; 9] = [
        Status::Green,
        Status::Strong,
        Status::Partial,
        Status::Weak,
        Status::Red,
        Status::Rotated,
        Status::Order,
        Status::Yellow,
        Status::NotScanned,
    ];

    /// MATCH, STRONG, PARTIAL or WEAK: the check shows this RIB, fully or in part.
    pub fn is_match(self) -> bool {
        self >= Status::Weak
    }

    /// STRONG, PARTIAL or WEAK.
    pub fn is_tier(self) -> bool {
        self.is_match() && self != Status::Green
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
            Status::Green => "MATCH",
            Status::Strong => "STRONG",
            Status::Partial => "PARTIAL",
            Status::Weak => "WEAK",
            Status::Red => "MISMATCH",
            Status::Rotated => "ROTATED",
            Status::Order => "PAGE ORDER",
            Status::Yellow => "UNREADABLE",
            Status::NotScanned => "NOT IN PDF",
        }
    }

    /// Green to blue to pink as confidence drops, then Excel's bad / neutral fills.
    pub fn fill_argb(self) -> &'static str {
        match self {
            Status::Green => "FFC6EFCE",
            Status::Strong => "FFE2EFDA",
            Status::Partial => "FFDDEBF7",
            Status::Weak => "FFFCE4EC",
            Status::Red => "FFFFC7CE",
            Status::Rotated => "FFE4DFEC",
            Status::Order => "FFF8CBAD",
            Status::Yellow => "FFFFEB9C",
            Status::NotScanned => "FFD9D9D9",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Status::Green => "All 20 RIB digits match.",
            Status::Strong => "11+ first or last RIB digits match. Check the rest by hand.",
            Status::Partial => "7+ last RIB digits match. Check by hand.",
            Status::Weak => "Only 4+ RIB digits match. Check by hand.",
            Status::Red => "Wrong RIB on the check. Verify.",
            Status::Rotated => "Page rotated or tilted. Rescan it.",
            Status::Order => "Pages out of order. Fix and rescan.",
            Status::Yellow => "Not readable. Check by hand.",
            Status::NotScanned => "No check scanned. Scan it.",
        }
    }

    fn paint(self, text: &str) -> colored::ColoredString {
        match self {
            Status::Green => text.green(),
            Status::Strong => text.bright_green(),
            Status::Partial => text.cyan(),
            Status::Weak => text.bright_magenta(),
            Status::Red => text.red(),
            Status::Rotated => text.bright_blue(),
            Status::Order => text.magenta(),
            Status::Yellow => text.yellow(),
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
}

impl DocResult {
    pub fn new(pages: String) -> Self {
        Self {
            pages,
            person: None,
            matched_entry: None,
            status: Status::Yellow,
            rib_read: String::new(),
            reading: None,
            detail: String::new(),
            order_issue: None,
            rotation: None,
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
    pub full_name: String,
}

impl ResultRow {
    pub fn marks(&self) -> Option<Vec<Mark>> {
        self.aligned
            .as_deref()
            .map(|aligned| rib::score(&self.rib_digits, aligned))
    }
}

/// Documents in PDF page order, then the spreadsheet rows no document matched.
pub fn result_rows(dir: &Directory, docs: &[DocResult]) -> Vec<ResultRow> {
    let mut rows: Vec<ResultRow> = docs.iter().map(|d| doc_row(dir, docs, d)).collect();

    for (id, entry) in dir.entries.iter().enumerate() {
        let person = dir.person_of(id);
        let person_docs: Vec<&DocResult> =
            docs.iter().filter(|d| d.person == Some(person)).collect();
        // Listed already: matched, or a failing document of this person shows this RIB (or
        // all of the person's RIBs).
        let listed = person_docs.iter().any(|d| {
            d.matched_entry == Some(id)
                || (d.matched_entry.is_none() && d.reading.as_ref().is_none_or(|r| r.entry == id))
        });
        if listed {
            continue;
        }
        let detail = match person_docs.first() {
            Some(d) => format!(
                "No check for this RIB (pages {} show another). Scan it.",
                d.pages
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
            full_name: entry.full_name.clone(),
        });
    }
    rows
}

fn doc_row(dir: &Directory, docs: &[DocResult], d: &DocResult) -> ResultRow {
    // The RIB shown: the one matched, else the one the reading was lined up on, else all of
    // the person's RIBs.
    let entry = d
        .matched_entry
        .or_else(|| d.reading.as_ref().map(|r| r.entry));
    let (rib, rib_digits) = match (entry, d.person) {
        (Some(e), _) => (
            dir.entries[e].rib.clone(),
            dir.entries[e].rib_digits.clone(),
        ),
        (None, Some(p)) => {
            let ribs: Vec<&str> = dir.persons[p]
                .entry_ids
                .iter()
                .map(|&e| dir.entries[e].rib.as_str())
                .collect();
            (ribs.join(" | "), String::new())
        }
        (None, None) => (String::new(), String::new()),
    };
    // Without a known person, a RIB found on the check tells whose row it is.
    let full_name = match (d.person, entry) {
        (Some(p), _) => dir.person_name(p).to_string(),
        (None, Some(e)) => dir.entries[e].full_name.clone(),
        (None, None) => "(not found)".to_string(),
    };

    let mut detail = d.detail.clone();
    if let Some(e) = d.matched_entry {
        let others: Vec<&str> = docs
            .iter()
            .filter(|o| o.matched_entry == Some(e) && o.pages != d.pages)
            .map(|o| o.pages.as_str())
            .collect();
        if !others.is_empty() {
            detail = format!(
                "{detail} Same RIB as pages {}. Remove the duplicate.",
                others.join(", ")
            );
        }
    }
    let aligned = d
        .reading
        .as_ref()
        .filter(|_| !rib_digits.is_empty())
        .map(|r| r.aligned.clone());
    ResultRow {
        pages: d.pages.clone(),
        status: d.status,
        detail,
        aligned,
        read: d.rib_read.clone(),
        rib,
        rib_digits,
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

/// Count of each mark, skipping zeros, as "13 right, 1 misplaced, 6 unread".
pub fn describe_marks(marks: &[Mark]) -> String {
    let n = |m: Mark| marks.iter().filter(|&&x| x == m).count();
    [
        (Mark::Bull, "right"),
        (Mark::Cow, "misplaced"),
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
                Mark::Cow => c.yellow().bold(),
                Mark::Miss => c.red().bold(),
                Mark::NotRead => c.dimmed(),
            }
            .to_string()
        })
        .collect()
}

/// The results as in the results file: pages, result, details, RIB read, RIB, full name.
pub fn print_results(rows: &[ResultRow], docs: &[DocResult]) {
    println!("{}", "Results (PDF page order)".bold());
    for r in rows {
        let read = match (r.aligned.as_deref(), r.marks()) {
            (Some(aligned), Some(marks)) => paint_marks(aligned, &marks),
            _ => format!("{:<20}", r.read.chars().take(20).collect::<String>()),
        };
        let head = format!("  {:<7} {:<11}", r.pages, r.status.label());
        println!(
            "{} {} {read} {}",
            r.status.paint(&head),
            r.detail,
            r.status.paint(&format!("{} | {}", r.rib, r.full_name))
        );
    }
    println!(
        "  RIB digits read on the check: {}  {}  {}  {}",
        "right place".green().bold(),
        "wrong place".yellow().bold(),
        "not in RIB".red().bold(),
        "? unread".dimmed()
    );

    let issues = page_issues(docs);
    if !issues.is_empty() {
        println!("\n{}", "Page issues".magenta().bold());
        for (pages, issue) in issues {
            println!("  {}", format!("{pages:<7} {issue}").magenta());
        }
    }

    let totals: Vec<String> = Status::ALL
        .iter()
        .filter_map(|&s| {
            let n = count(rows, s, |r| r.status);
            (n > 0).then(|| {
                s.paint(&format!("{n} {}", s.label().to_lowercase()))
                    .to_string()
            })
        })
        .collect();
    println!("\n{}", totals.join("  "));
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

    #[test]
    fn rows_follow_pdf_order_then_rows_not_in_pdf() {
        let dir = directory();
        let docs = [
            doc("1-2", Some(1), None, Status::Yellow),
            doc("3-4", Some(0), Some(1), Status::Green),
        ];
        let rows = result_rows(&dir, &docs);
        let pages: Vec<&str> = rows.iter().map(|r| r.pages.as_str()).collect();
        assert_eq!(pages, vec!["1-2", "3-4", ""]);
        assert_eq!(rows[0].rib, "333");
        assert_eq!(rows[1].rib, "222");
        assert_eq!(rows[2].rib, "111");
        assert_eq!(rows[2].status, Status::NotScanned);
    }

    #[test]
    fn other_rib_of_a_matched_person_is_not_in_pdf() {
        let dir = directory();
        let rows = result_rows(&dir, &[doc("1-2", Some(0), Some(0), Status::Green)]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].rib, "222");
        assert!(rows[1].detail.contains("pages 1-2 show another"));
    }

    #[test]
    fn failing_document_shows_the_rib_it_was_compared_with() {
        let dir = directory();
        let mut failing = doc("1-2", Some(0), None, Status::Red);
        failing.reading = Some(Reading {
            entry: 1,
            aligned: "999".to_string(),
        });
        let rows = result_rows(&dir, &[failing]);
        assert_eq!(rows[0].rib, "222");
        assert_eq!(rows[0].aligned.as_deref(), Some("999"));
        assert_eq!(rows[1].rib, "111", "the other RIB is still listed");
    }

    #[test]
    fn duplicate_scans_are_flagged() {
        let dir = directory();
        let docs = [
            doc("1-2", Some(0), Some(0), Status::Green),
            doc("5-6", Some(0), Some(0), Status::Green),
        ];
        let rows = result_rows(&dir, &docs);
        assert!(rows[0].detail.contains("pages 5-6"));
        assert!(rows[1].detail.contains("pages 1-2"));
    }

    #[test]
    fn statuses_rank_from_worst_to_best() {
        assert!(Status::Green > Status::Strong);
        assert!(Status::Strong > Status::Partial);
        assert!(Status::Partial > Status::Weak);
        assert!(Status::Weak > Status::Red);
        assert!(Status::Weak.is_tier() && !Status::Green.is_tier() && !Status::Red.is_match());
    }
}
