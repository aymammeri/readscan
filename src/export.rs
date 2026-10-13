use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use umya_spreadsheet::{Workbook, Worksheet, reader, writer};

use crate::report::{DocResult, EntryReport, Status, count};
use crate::sheet::{Directory, Layout};

/// Columns appended to the first sheet: header and width. Found again by header on re-runs.
const RESULT_COLUMNS: [(&str, f64); 4] = [
    ("Scan result", 14.0),
    ("Scan pages", 12.0),
    ("Scan RIB read", 28.0),
    ("Scan details", 70.0),
];
const SUMMARY_SHEET: &str = "Scan summary";
const DOCUMENTS_SHEET: &str = "Scan documents";
/// Above this many digits a number is a RIB, not an amount, and must stay text.
const MAX_NUMERIC_DIGITS: usize = 15;

pub struct RunInfo<'a> {
    pub sheet_path: &'a Path,
    pub pdf_path: &'a Path,
    pub page_count: usize,
    pub threads: usize,
    pub seconds: f32,
}

pub struct Saved {
    pub path: PathBuf,
    pub backup: Option<PathBuf>,
    pub note: Option<String>,
}

/// Write per-row results, a summary sheet and a documents sheet into the input workbook.
/// `.xlsx` files are updated in place after a backup copy; other formats get a new `_results.xlsx`.
pub fn write_results(
    dir: &Directory,
    docs: &[DocResult],
    reports: &[EntryReport],
    info: &RunInfo,
) -> Result<Saved> {
    let path = info.sheet_path;
    let in_place = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("xlsx"));
    let results_copy = sibling(path, "_results");
    let backup = sibling(path, ".backup");

    let mut book = if in_place {
        reader::xlsx::read(path).map_err(|e| anyhow!("cannot reopen spreadsheet: {e:?}"))?
    } else {
        copy_of(&dir.layout)
    };

    let data_sheet = {
        let sheet = book.sheet_mut(0).map_err(|e| anyhow!("{e:?}"))?;
        annotate_rows(sheet, dir, reports);
        sheet.name().to_string()
    };
    let saved_as = if in_place { &backup } else { &results_copy };
    let notes = notes(docs, reports, &data_sheet, in_place.then_some(saved_as));
    write_summary(
        new_sheet(&mut book, SUMMARY_SHEET)?,
        docs,
        reports,
        info,
        &notes,
    );
    write_documents(new_sheet(&mut book, DOCUMENTS_SHEET)?, dir, docs);
    book.set_active_sheet(0);

    let bytes = to_bytes(&book)?;
    if !in_place {
        std::fs::write(&results_copy, bytes).context("cannot write results file")?;
        return Ok(Saved {
            path: results_copy,
            backup: None,
            note: Some(
                "This format can't be edited in place, results were saved as a new .xlsx file."
                    .into(),
            ),
        });
    }

    std::fs::copy(path, &backup).context("cannot create backup copy")?;
    // Write next to the original then swap, so a failed write never leaves a half-written file.
    let tmp = sibling(path, ".tmp");
    let replaced = std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, path));
    match replaced {
        Ok(()) => Ok(Saved {
            path: path.to_path_buf(),
            backup: Some(backup),
            note: None,
        }),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            std::fs::write(&results_copy, &bytes).context("cannot write results file")?;
            Ok(Saved {
                path: results_copy,
                backup: None,
                note: Some(format!(
                    "Could not overwrite the spreadsheet ({e}). Is it open in Excel? \
                     Results were saved to a new file instead."
                )),
            })
        }
    }
}

fn annotate_rows(sheet: &mut Worksheet, dir: &Directory, reports: &[EntryReport]) {
    let layout = &dir.layout;
    let data_width = layout.rows.iter().map(Vec::len).max().unwrap_or(0) as u32;
    let last_data_col = layout.first_col + data_width.saturating_sub(1);

    let existing = layout.header_row.and_then(|row| {
        (1..=sheet.highest_column()).find(|&col| sheet.value((col, row)) == RESULT_COLUMNS[0].0)
    });
    let first = existing.unwrap_or(last_data_col.max(sheet.highest_column()) + 1);

    for (i, (title, width)) in RESULT_COLUMNS.iter().enumerate() {
        let col = first + i as u32;
        sheet.column_dimension_by_number_mut(col).set_width(*width);
        if let Some(row) = layout.header_row {
            sheet.cell_mut((col, row)).set_value_string(*title);
            sheet.style_mut((col, row)).font_mut().set_bold(true);
        }
    }

    let last = first + RESULT_COLUMNS.len() as u32 - 1;
    for (entry, report) in dir.entries.iter().zip(reports) {
        let values = [
            report.status.label(),
            &report.pages,
            &report.rib_read,
            &report.detail,
        ];
        for (i, value) in values.iter().enumerate() {
            sheet
                .cell_mut((first + i as u32, entry.row))
                .set_value_string(*value);
        }
        for col in layout.first_col..=last {
            sheet
                .style_mut((col, entry.row))
                .set_background_color(report.status.fill_argb());
        }
    }
}

fn write_summary(
    sheet: &mut Worksheet,
    docs: &[DocResult],
    reports: &[EntryReport],
    info: &RunInfo,
    notes: &[String],
) {
    let mut out = SheetWriter::new(sheet);
    out.bold_row(&["Scan summary"]);
    out.skip();
    let file_name = |p: &Path| {
        p.file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned())
    };
    out.row(&["Spreadsheet", &file_name(info.sheet_path)]);
    out.row(&["PDF", &file_name(info.pdf_path)]);
    out.row(&[
        "Date",
        &chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
    ]);
    out.row(&["Pages scanned", &info.page_count.to_string()]);
    out.row(&["Documents", &docs.len().to_string()]);
    out.row(&["Threads", &info.threads.to_string()]);
    out.row(&["Duration", &format!("{:.1} s", info.seconds)]);
    out.skip();

    out.bold_row(&["Spreadsheet rows", "Count", "Meaning"]);
    for status in Status::ALL {
        let n = count(reports, status, |r| r.status).to_string();
        out.colored_row(&[status.label(), &n, status.meaning()], status);
    }
    out.bold_row(&["Total", &reports.len().to_string()]);
    out.skip();

    out.bold_row(&["Scanned documents", "Count"]);
    for status in [Status::Green, Status::Red, Status::Yellow] {
        let n = count(docs, status, |d| d.status).to_string();
        out.colored_row(&[status.label(), &n], status);
    }
    let unknown = docs.iter().filter(|d| d.person.is_none()).count();
    out.row(&["Name not found", &unknown.to_string()]);
    out.bold_row(&["Total", &docs.len().to_string()]);
    out.skip();

    out.bold_row(&["Notes"]);
    for note in notes {
        out.row(&[note]);
    }

    for (col, width) in [(1, 22.0), (2, 14.0), (3, 90.0)] {
        out.sheet
            .column_dimension_by_number_mut(col)
            .set_width(width);
    }
}

fn notes(
    docs: &[DocResult],
    reports: &[EntryReport],
    data_sheet: &str,
    backup: Option<&PathBuf>,
) -> Vec<String> {
    let mut notes = vec![format!(
        "Rows are colored in sheet \"{data_sheet}\". Every scanned document is listed in \"{DOCUMENTS_SHEET}\"."
    )];
    let red = count(reports, Status::Red, |r| r.status);
    if red > 0 {
        notes.push(format!(
            "{red} row(s) have a RIB that does not match the check (red)."
        ));
    }
    let yellow = count(reports, Status::Yellow, |r| r.status);
    if yellow > 0 {
        notes.push(format!(
            "{yellow} row(s) could not be verified and need a manual check (yellow)."
        ));
    }
    let unknown = docs.iter().filter(|d| d.person.is_none()).count();
    if unknown > 0 {
        notes.push(format!(
            "{unknown} scanned document(s) could not be linked to any name in the spreadsheet, see \"{DOCUMENTS_SHEET}\"."
        ));
    }
    if let Some(backup) = backup.and_then(|b| b.file_name()) {
        notes.push(format!(
            "The spreadsheet as it was before this scan is saved as \"{}\".",
            backup.to_string_lossy()
        ));
    }
    notes
}

fn write_documents(sheet: &mut Worksheet, dir: &Directory, docs: &[DocResult]) {
    let mut out = SheetWriter::new(sheet);
    out.bold_row(&["Pages", "Name", "Result", "RIB read on check", "Details"]);
    for d in docs {
        let name = d.person.map_or("(not found)", |_| d.person_name(dir));
        out.colored_row(
            &[&d.pages, name, d.status.label(), &d.rib_read, &d.detail],
            d.status,
        );
    }
    for (col, width) in [(1, 10.0), (2, 30.0), (3, 14.0), (4, 28.0), (5, 90.0)] {
        out.sheet
            .column_dimension_by_number_mut(col)
            .set_width(width);
    }
}

/// Appends rows top to bottom, starting at A1.
struct SheetWriter<'a> {
    sheet: &'a mut Worksheet,
    row: u32,
}

impl<'a> SheetWriter<'a> {
    fn new(sheet: &'a mut Worksheet) -> Self {
        Self { sheet, row: 1 }
    }

    fn row(&mut self, values: &[&str]) {
        for (i, value) in values.iter().enumerate() {
            self.sheet
                .cell_mut((i as u32 + 1, self.row))
                .set_value_string(*value);
        }
        self.row += 1;
    }

    fn bold_row(&mut self, values: &[&str]) {
        for col in 1..=values.len() as u32 {
            self.sheet
                .style_mut((col, self.row))
                .font_mut()
                .set_bold(true);
        }
        self.row(values);
    }

    fn colored_row(&mut self, values: &[&str], status: Status) {
        for col in 1..=values.len() as u32 {
            self.sheet
                .style_mut((col, self.row))
                .set_background_color(status.fill_argb());
        }
        self.row(values);
    }

    fn skip(&mut self) {
        self.row += 1;
    }
}

/// Start a fresh copy of `name`, dropping the one left by a previous run.
fn new_sheet<'a>(book: &'a mut Workbook, name: &str) -> Result<&'a mut Worksheet> {
    let _ = book.remove_sheet_by_name(name);
    book.new_sheet(name)
        .map_err(|e| anyhow!("cannot add sheet {name}: {e:?}"))
}

/// Rebuild the first sheet of a csv / xls / ods file as a new xlsx workbook.
fn copy_of(layout: &Layout) -> Workbook {
    let mut book = umya_spreadsheet::new_file();
    let sheet = book.sheet_mut(0).expect("new workbook has a sheet");
    for (r, row) in layout.rows.iter().enumerate() {
        for (c, value) in row.iter().enumerate() {
            if value.is_empty() {
                continue;
            }
            let cell = sheet.cell_mut((layout.first_col + c as u32, layout.first_row + r as u32));
            let digits = value.chars().filter(char::is_ascii_digit).count();
            match value.parse::<f64>() {
                Ok(n) if digits <= MAX_NUMERIC_DIGITS => cell.set_value_number(n),
                _ => cell.set_value_string(value),
            };
        }
    }
    book
}

fn to_bytes(book: &Workbook) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    writer::xlsx::write_writer(book, &mut bytes)
        .map_err(|e| anyhow!("cannot build xlsx: {e:?}"))?;
    Ok(bytes)
}

/// `dir/name.xlsx` -> `dir/name{suffix}.xlsx`
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!("{stem}{suffix}.xlsx"))
}
