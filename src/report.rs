use colored::Colorize;

use crate::sheet::Directory;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    NotScanned,
    Yellow,
    Red,
    Green,
}

impl Status {
    pub const ALL: [Status; 4] = [
        Status::Green,
        Status::Red,
        Status::Yellow,
        Status::NotScanned,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Status::Green => "MATCH",
            Status::Red => "MISMATCH",
            Status::Yellow => "UNREADABLE",
            Status::NotScanned => "NOT IN PDF",
        }
    }

    /// Excel's own "Good / Bad / Neutral" cell fills, plus grey.
    pub fn fill_argb(self) -> &'static str {
        match self {
            Status::Green => "FFC6EFCE",
            Status::Red => "FFFFC7CE",
            Status::Yellow => "FFFFEB9C",
            Status::NotScanned => "FFD9D9D9",
        }
    }

    pub fn meaning(self) -> &'static str {
        match self {
            Status::Green => "Name found and the check shows one of this person's RIBs.",
            Status::Red => "Name found but the RIB on the check is none of this person's RIBs.",
            Status::Yellow => {
                "Could not be verified (page unreadable, no RIB readable on the check, \
                 or name not recognized). Review manually."
            }
            Status::NotScanned => "No check in the PDF for this row.",
        }
    }

    fn paint(self, text: &str) -> colored::ColoredString {
        match self {
            Status::Green => text.green(),
            Status::Red => text.red(),
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
    pub detail: String,
}

impl DocResult {
    pub fn person_name<'a>(&self, dir: &'a Directory) -> &'a str {
        self.person.map_or("", |p| dir.person_name(p))
    }
}

/// Outcome for one spreadsheet row.
pub struct EntryReport {
    pub status: Status,
    pub pages: String,
    pub rib_read: String,
    pub detail: String,
}

/// Roll document results up to spreadsheet rows. A row is green once any check proves its RIB;
/// otherwise it takes the worst outcome among the documents of the same person.
pub fn entry_reports(dir: &Directory, docs: &[DocResult]) -> Vec<EntryReport> {
    (0..dir.entries.len())
        .map(|id| {
            let person = dir.person_of(id);
            if let Some(doc) = docs.iter().find(|d| d.matched_entry == Some(id)) {
                return EntryReport {
                    status: Status::Green,
                    pages: doc.pages.clone(),
                    rib_read: doc.rib_read.clone(),
                    detail: "RIB found on the check.".to_string(),
                };
            }

            let failed: Vec<&DocResult> = docs
                .iter()
                .filter(|d| d.person == Some(person) && d.status != Status::Green)
                .collect();
            let Some(status) = failed.iter().map(|d| d.status).max() else {
                let other_account = docs.iter().find(|d| d.person == Some(person));
                return EntryReport {
                    status: Status::NotScanned,
                    pages: String::new(),
                    rib_read: String::new(),
                    detail: match other_account {
                        Some(d) => format!(
                            "No check for this account (another account of this person matched on pages {}).",
                            d.pages
                        ),
                        None => "No document for this person in the PDF.".to_string(),
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
}

pub fn print_entries(dir: &Directory, reports: &[EntryReport]) {
    println!("{}", "Spreadsheet entries".bold());
    for (entry, r) in dir.entries.iter().zip(reports) {
        let line = format!(
            "  row {:<5} {:<30} {:<26} {:<11} {}",
            entry.row,
            entry.full_name,
            entry.rib,
            r.status.label(),
            r.pages
        );
        println!("{}", r.status.paint(&line));
    }

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
        };
        let layout = Layout {
            rows: Vec::new(),
            first_row: 1,
            first_col: 1,
            header_row: Some(1),
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
            pages: pages.to_string(),
            person,
            matched_entry: matched,
            status,
            rib_read: String::new(),
            detail: String::new(),
        }
    }

    #[test]
    fn matched_account_is_green_and_other_account_not_scanned() {
        let dir = directory();
        let reports = entry_reports(&dir, &[doc("1-2", Some(0), Some(0), Status::Green)]);
        assert!(reports[0].status == Status::Green);
        assert!(reports[1].status == Status::NotScanned);
        assert!(reports[1].detail.contains("another account"));
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
