use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use calamine::{Data, Reader, open_workbook_auto};

use crate::rib;
use crate::text::normalize_name;

/// One spreadsheet row we care about.
#[derive(Debug, Clone)]
pub struct Entry {
    /// 1-based row number as shown in Excel.
    pub row: u32,
    pub full_name: String,
    /// As written in the spreadsheet.
    pub rib: String,
    /// The 20 digits compared with the checks.
    pub rib_digits: String,
}

/// A person may own several bank accounts, so entries are grouped by normalized name.
pub struct Person {
    pub key: String,
    pub tokens: Vec<String>,
    pub entry_ids: Vec<usize>,
}

pub struct Directory {
    pub entries: Vec<Entry>,
    pub persons: Vec<Person>,
    entry_person: Vec<usize>,
    /// Every run of `rib::INDEX_RUN` digits of every RIB, to the entries holding it.
    rib_runs: HashMap<String, Vec<usize>>,
}

impl Directory {
    pub fn new(entries: Vec<Entry>) -> Self {
        let mut by_key: HashMap<String, usize> = HashMap::new();
        let mut persons: Vec<Person> = Vec::new();
        let mut entry_person = Vec::with_capacity(entries.len());
        for (id, entry) in entries.iter().enumerate() {
            let key = normalize_name(&entry.full_name);
            let idx = *by_key.entry(key.clone()).or_insert_with(|| {
                persons.push(Person {
                    tokens: key.split(' ').map(str::to_string).collect(),
                    key,
                    entry_ids: Vec::new(),
                });
                persons.len() - 1
            });
            persons[idx].entry_ids.push(id);
            entry_person.push(idx);
        }
        let mut rib_runs: HashMap<String, Vec<usize>> = HashMap::new();
        for (id, entry) in entries.iter().enumerate() {
            for run in rib::runs(&entry.rib_digits) {
                let holders = rib_runs.entry(run.to_string()).or_default();
                if holders.last() != Some(&id) {
                    holders.push(id);
                }
            }
        }
        Self {
            rib_runs,
            entries,
            persons,
            entry_person,
        }
    }

    /// Entries whose RIB holds `run`.
    pub fn entries_with_run(&self, run: &str) -> &[usize] {
        self.rib_runs.get(run).map_or(&[], Vec::as_slice)
    }

    pub fn person_of(&self, entry: usize) -> usize {
        self.entry_person[entry]
    }

    pub fn person_name(&self, person: usize) -> &str {
        &self.entries[self.persons[person].entry_ids[0]].full_name
    }
}

pub fn load(path: &Path) -> Result<Directory> {
    let (rows, (first_row, _)) = read_rows(path)?;
    let Some(header) = rows.first() else {
        bail!("spreadsheet is empty");
    };

    let header_norm: Vec<String> = header.iter().map(|h| normalize_name(h)).collect();
    let find = |keys: &[&str]| {
        header_norm
            .iter()
            .position(|h| keys.iter().any(|k| h.split(' ').any(|w| w == *k)))
    };
    let name_col = find(&["NAME", "NOM", "FULLNAME", "FULL"]);
    let rib_col = find(&["RIB", "RIP", "IBAN", "COMPTE", "ACCOUNT"]);

    let (name_col, rib_col, skip) = match (name_col, rib_col) {
        (Some(n), Some(r)) => (n, r, 1),
        _ => {
            eprintln!("No 'full name' / 'rib' headers found, using column A as name and B as RIB.");
            (0, 1, 0)
        }
    };

    let entries: Vec<Entry> = rows
        .iter()
        .enumerate()
        .skip(skip)
        .filter_map(|(i, row)| {
            let full_name = row.get(name_col)?.trim().to_string();
            let rib = row.get(rib_col).map_or("", |r| r.trim()).to_string();
            let rib_digits = rib::from_sheet(&rib);
            if full_name.is_empty() {
                return None;
            }
            Some(Entry {
                row: first_row + i as u32,
                full_name,
                rib,
                rib_digits,
            })
        })
        .collect();

    for e in entries
        .iter()
        .filter(|e| e.rib_digits.len() == rib::RIB_LEN && !rib::key_ok(&e.rib_digits))
    {
        eprintln!(
            "Warning: row {} ({}): RIB \"{}\" has a wrong key (last 2 digits). Check it.",
            e.row, e.full_name, e.rib
        );
    }
    for e in entries
        .iter()
        .filter(|e| e.rib_digits.len() != rib::RIB_LEN)
    {
        if e.rib_digits.is_empty() {
            eprintln!(
                "Warning: row {} ({}): no RIB. Add it in the spreadsheet.",
                e.row, e.full_name
            );
            continue;
        }
        eprintln!(
            "Warning: row {} ({}): RIB \"{}\" has {} digits, not {}. Fix it in the spreadsheet.",
            e.row,
            e.full_name,
            e.rib,
            e.rib_digits.len(),
            rib::RIB_LEN
        );
    }

    if entries.is_empty() {
        bail!("no rows with a full name were found");
    }
    Ok(Directory::new(entries))
}

/// Used cells as text, and the 1-based (row, column) of the top-left one.
type Cells = (Vec<Vec<String>>, (u32, u32));

fn read_rows(path: &Path) -> Result<Cells> {
    let is_csv = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    if is_csv {
        return Ok((read_csv(path)?, (1, 1)));
    }

    let mut workbook = open_workbook_auto(path).context("cannot open spreadsheet")?;
    let range = workbook
        .worksheet_range_at(0)
        .context("spreadsheet has no sheets")?
        .context("cannot read first sheet")?;

    // The range starts at the first non-empty cell, not necessarily A1.
    let (start_row, start_col) = range.start().unwrap_or((0, 0));
    let mut warned = false;
    let rows = range
        .rows()
        .map(|row| {
            row.iter()
                .map(|cell| match cell {
                    // Excel stores long numbers as f64, which silently loses digits past ~15.
                    Data::Float(f) => {
                        if !warned && f.abs() >= 1e15 {
                            eprintln!(
                                "Warning: some RIBs are stored as numbers and may have lost digits. \
                                 Format the RIB column as Text in the spreadsheet."
                            );
                            warned = true;
                        }
                        if f.fract() == 0.0 {
                            format!("{f:.0}")
                        } else {
                            f.to_string()
                        }
                    }
                    other => other.to_string(),
                })
                .collect()
        })
        .collect();
    Ok((rows, (start_row + 1, start_col + 1)))
}

fn read_csv(path: &Path) -> Result<Vec<Vec<String>>> {
    let raw = std::fs::read_to_string(path).context("cannot read csv")?;
    let first_line = raw.lines().next().unwrap_or_default();
    let delimiter = if first_line.matches(';').count() > first_line.matches(',').count() {
        b';'
    } else {
        b','
    };
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(raw.as_bytes());
    reader
        .records()
        .map(|r| Ok(r?.iter().map(str::to_string).collect()))
        .collect()
}
