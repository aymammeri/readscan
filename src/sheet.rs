use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use calamine::{Data, Reader, open_workbook_auto};

use crate::text::{normalize_name, normalize_rib};

/// One spreadsheet row we care about.
#[derive(Debug, Clone)]
pub struct Entry {
    /// 1-based row number as shown in Excel.
    pub row: usize,
    pub full_name: String,
    pub rib: String,
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
}

impl Directory {
    fn new(entries: Vec<Entry>) -> Self {
        let mut by_key: HashMap<String, usize> = HashMap::new();
        let mut persons: Vec<Person> = Vec::new();
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
        }
        Self { entries, persons }
    }
}

pub fn load(path: &Path) -> Result<Directory> {
    let rows = read_rows(path)?;
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
                row: i + 1,
                full_name,
                rib,
            })
        })
        .collect();

    if entries.is_empty() {
        bail!("no rows with both a full name and a RIB were found");
    }
    Ok(Directory::new(entries))
}

fn read_rows(path: &Path) -> Result<Vec<Vec<String>>> {
    let is_csv = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("csv"));
    if is_csv {
        return read_csv(path);
    }

    let mut workbook = open_workbook_auto(path).context("cannot open spreadsheet")?;
    let range = workbook
        .worksheet_range_at(0)
        .context("spreadsheet has no sheets")?
        .context("cannot read first sheet")?;

    let mut warned = false;
    Ok(range
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
                        format!("{f:.0}")
                    }
                    other => other.to_string(),
                })
                .collect()
        })
        .collect())
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
