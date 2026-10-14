use colored::Colorize;

use crate::rib::{self, Mark, Reading};
use crate::sheet::Directory;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    NotScanned,
    Yellow,
    /// Pages out of physical order; the document could not be completed.
    Order,
    /// A page is upside down, on its side or too tilted to match.
    Rotated,
    Red,
    /// Some RIB digits match, not all 20.
    Partial,
    Green,
}

impl Status {
    pub const ALL: [Status; 7] = [
        Status::Green,
        Status::Partial,
        Status::Red,
        Status::Rotated,
        Status::Order,
        Status::Yellow,
        Status::NotScanned,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Status::Green => "MATCH",
            Status::Partial => "PARTIAL MATCH",
            Status::Red => "MISMATCH",
            Status::Rotated => "ROTATED",
            Status::Order => "PAGE ORDER",
            Status::Yellow => "UNREADABLE",
            Status::NotScanned => "NOT IN PDF",
        }
    }

    /// Excel's own "Good / Bad / Neutral" cell fills, plus blue, purple, orange and grey.
    pub fn fill_argb(self) -> &'static str {
        match self {
            Status::Green => "FFC6EFCE",
            Status::Partial => "FFDDEBF7",
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
            Status::Partial => "Some RIB digits match. Check by hand.",
            Status::Red => "Wrong RIB on the check. Verify.",
            Status::Rotated => "Page rotated or tilted. Rescan upright.",
            Status::Order => "Pages out of order. Fix and rescan.",
            Status::Yellow => "Not readable. Check by hand.",
            Status::NotScanned => "No check scanned. Scan it.",
        }
    }

    fn paint(self, text: &str) -> colored::ColoredString {
        match self {
            Status::Green => text.green(),
            Status::Partial => text.cyan(),
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

    pub fn person_name<'a>(&self, dir: &'a Directory) -> &'a str {
        self.person.map_or("", |p| dir.person_name(p))
    }
}

/// Outcome for one spreadsheet row.
pub struct EntryReport {
    pub status: Status,
    pub pages: String,
    pub rib_read: String,
    /// 20 characters lined up on this row's RIB, `?` where nothing was read.
    pub aligned: Option<String>,
    pub detail: String,
}

/// Roll document results up to spreadsheet rows. A row is green once any check proves its RIB;
/// otherwise it takes the worst outcome among the documents of the same person.
pub fn entry_reports(dir: &Directory, docs: &[DocResult]) -> Vec<EntryReport> {
    (0..dir.entries.len())
        .map(|id| {
            let person = dir.person_of(id);
            let matches: Vec<&DocResult> = docs
                .iter()
                .filter(|d| d.matched_entry == Some(id))
                .collect();
            if let Some(doc) = matches.iter().max_by_key(|d| d.status) {
                let mut detail = doc.detail.clone();
                if matches.len() > 1 {
                    let pages: Vec<&str> = matches.iter().map(|d| d.pages.as_str()).collect();
                    detail = format!(
                        "{detail} Also matched on pages {}. Remove the duplicate.",
                        pages.join(", ")
                    );
                }
                return EntryReport {
                    status: doc.status,
                    pages: doc.pages.clone(),
                    rib_read: doc.rib_read.clone(),
                    aligned: doc.reading.as_ref().map(|r| r.aligned.clone()),
                    detail,
                };
            }

            let failed: Vec<&DocResult> = docs
                .iter()
                .filter(|d| d.person == Some(person) && d.matched_entry.is_none())
                .collect();
            let Some(status) = failed.iter().map(|d| d.status).max() else {
                let other_account = docs.iter().find(|d| d.person == Some(person));
                return EntryReport {
                    status: Status::NotScanned,
                    pages: String::new(),
                    rib_read: String::new(),
                    aligned: None,
                    detail: match other_account {
                        Some(d) => format!(
                            "No check for this RIB (pages {} show another RIB). Scan it.",
                            d.pages
                        ),
                        None => "Not in the PDF. Scan it.".to_string(),
                    },
                };
            };

            let worst: Vec<&&DocResult> = failed.iter().filter(|d| d.status == status).collect();
            let join = |f: fn(&DocResult) -> &str| {
                worst
                    .iter()
                    .map(|d| f(d))
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" | ")
            };
            EntryReport {
                status,
                pages: join(|d| &d.pages),
                rib_read: join(|d| &d.rib_read),
                aligned: worst
                    .iter()
                    .find_map(|d| d.reading.as_ref().map(|r| r.aligned.clone())),
                detail: join(|d| &d.detail),
            }
        })
        .collect()
}

pub fn count<T>(items: &[T], status: Status, get: impl Fn(&T) -> Status) -> usize {
    items.iter().filter(|i| get(i) == status).count()
}

pub fn print_documents(dir: &Directory, docs: &[DocResult]) {
    println!("{}", "Documents".bold());
    for d in docs {
        let name = d.person.map_or("?", |p| dir.person_name(p));
        let line = format!("  pages {:<7} {:<30} {}", d.pages, name, d.detail);
        println!("{}", d.status.paint(&line));
    }
    println!();

    let issues = page_issues(docs);
    if !issues.is_empty() {
        println!("{}", "Page issues".magenta().bold());
        for (pages, issue) in issues {
            println!("  {}", format!("pages {pages:<7} {issue}").magenta());
        }
        println!();
    }
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

pub fn print_entries(dir: &Directory, reports: &[EntryReport]) {
    println!("{}", "Spreadsheet entries".bold());
    for (entry, r) in dir.entries.iter().zip(reports) {
        let line = format!(
            "  row {:<5} {:<30} {:<26} {:<13} {:<7}",
            entry.row,
            entry.full_name,
            entry.rib,
            r.status.label(),
            r.pages
        );
        let digits = r.aligned.as_deref().map_or(String::new(), |aligned| {
            paint_marks(aligned, &rib::score(&entry.rib_digits, aligned))
        });
        println!("{} {digits}", r.status.paint(&line));
    }
    println!(
        "  RIB digits read on the check: {}  {}  {}  {}",
        "right place".green().bold(),
        "wrong place".yellow().bold(),
        "not in RIB".red().bold(),
        "? not read".dimmed()
    );

    let totals: Vec<String> = Status::ALL
        .iter()
        .map(|&s| {
            let text = format!(
                "{} {}",
                count(reports, s, |r| r.status),
                s.label().to_lowercase()
            );
            s.paint(&text).to_string()
        })
        .collect();
    println!("\n{}", totals.join("  "));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::{Entry, Layout};

    fn directory() -> Directory {
        let entry = |row, full_name: &str, rib: &str| Entry {
            row,
            full_name: full_name.to_string(),
            rib: rib.to_string(),
            rib_digits: rib.to_string(),
        };
        let layout = Layout {
            rows: Vec::new(),
            first_row: 1,
            first_col: 1,
            header_row: Some(1),
            rib_col: 2,
        };
        Directory::new(
            vec![
                entry(2, "Aymen Mammeri", "111"),
                entry(3, "Aymen Mammeri", "222"),
                entry(4, "Sarah Benali", "333"),
            ],
            layout,
        )
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
    fn matched_account_is_green_and_other_account_not_scanned() {
        let dir = directory();
        let reports = entry_reports(&dir, &[doc("1-2", Some(0), Some(0), Status::Green)]);
        assert!(reports[0].status == Status::Green);
        assert!(reports[1].status == Status::NotScanned);
        assert!(reports[1].detail.contains("another RIB"));
        assert!(reports[2].status == Status::NotScanned);
    }

    #[test]
    fn mismatch_marks_every_account_of_the_person_red() {
        let dir = directory();
        let reports = entry_reports(&dir, &[doc("1-2", Some(0), None, Status::Red)]);
        assert!(reports[0].status == Status::Red);
        assert!(reports[1].status == Status::Red);
        assert_eq!(reports[1].pages, "1-2");
    }

    #[test]
    fn red_beats_yellow_for_the_same_person() {
        let dir = directory();
        let docs = [
            doc("1-2", Some(0), None, Status::Yellow),
            doc("3-4", Some(0), None, Status::Red),
        ];
        let reports = entry_reports(&dir, &docs);
        assert!(reports[2].status == Status::NotScanned);
        assert!(reports[0].status == Status::Red);
        assert_eq!(reports[0].pages, "3-4");
    }
}
