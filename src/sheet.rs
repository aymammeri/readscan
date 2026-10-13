use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use calamine::{Data, Reader, open_workbook_auto};

use crate::text::{normalize_name, normalize_rib};

/// One spreadsheet row we care about.
#[derive(Debug, Clone)]
pub struct Entry {
    /// 1-based row number as shown in Excel.
    pub row: u32,
    pub full_name: String,
    pub rib: String,
}

/// A person may own several bank accounts, so entries are grouped by normalized name.
pub struct Person {
    pub key: String,
    pub tokens: Vec<String>,
    pub entry_ids: Vec<usize>,
}

/// Where the data sits in the first sheet, so results can be written back next to it.
pub struct Layout {
    /// Every cell of the used range, as text.
    pub rows: Vec<Vec<String>>,
    /// 1-based sheet row / column of `rows[0][0]`.
    pub first_row: u32,
    pub first_col: u32,
    /// 1-based sheet row of the header, if the sheet has one.
    pub header_row: Option<u32>,
}

pub struct Directory {
    pub entries: Vec<Entry>,
    pub persons: Vec<Person>,
    pub layout: Layout,
    entry_person: Vec<usize>,
}

impl Directory {
    pub fn new(entries: Vec<Entry>, layout: Layout) -> Self {
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
        Self {
            entries,
            persons,
            layout,
            entry_person,
        }
    }

    pub fn person_of(&self, entry: usize) -> usize {
        self.entry_person[entry]
    }

    pub fn person_name(&self, person: usize) -> &str {
        &self.entries[self.persons[person].entry_ids[0]].full_name
    }
}

pub fn load(path: &Path) -> Result<Directory> {
    let (rows, (first_row, first_col)) = read_rows(path)?;
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
            let rib = row.get(rib_col)?.trim().to_string();
            if full_name.is_empty() || normalize_rib(&rib).is_empty() {
                return None;
            }
            Some(Entry {
                row: first_row + i as u32,
                full_name,
                rib,
            })
        })
        .collect();

    if entries.is_empty() {
        bail!("no rows with both a full name and a RIB were found");
    }
    let layout = Layout {
        rows,
        first_row,
        first_col,
        header_row: (skip == 1).then_some(first_row),
    };
    Ok(Directory::new(entries, layout))
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
