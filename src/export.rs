use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use umya_spreadsheet::{RichText, TextElement, Workbook, Worksheet, writer};

use crate::report::{DocResult, ResultRow, Status, count, describe_marks, page_issues};
use crate::rib::Mark;

/// Results sheet columns: header and width.
const RESULT_COLUMNS: [(&str, f64); 7] = [
    ("Pages", 9.0),
    ("Result", 12.0),
    ("Scan details", 70.0),
    ("Scan RIB read", 24.0),
    ("RIB", 28.0),
    ("Full name", 28.0),
    ("Case number", 20.0),
];
const RESULTS_SHEET: &str = "Results";
const SUMMARY_SHEET: &str = "Summary";

pub struct RunInfo<'a> {
    pub sheet_path: &'a Path,
    pub pdf_path: &'a Path,
    pub page_count: usize,
    pub threads: usize,
    pub seconds: f32,
}

pub struct Saved {
    pub path: PathBuf,
    pub note: Option<String>,
}

/// Write the results, in PDF page order, and a summary to a new `<name>_results.xlsx` next to
/// the spreadsheet. The spreadsheet itself is never changed.
pub fn write_results(rows: &[ResultRow], docs: &[DocResult], info: &RunInfo) -> Result<Saved> {
    let mut book = umya_spreadsheet::new_file();
    {
        let sheet = book.sheet_mut(0).map_err(|e| anyhow!("{e:?}"))?;
        sheet.set_name(RESULTS_SHEET);
        write_rows(sheet, rows);
    }
    let summary = book
        .new_sheet(SUMMARY_SHEET)
        .map_err(|e| anyhow!("cannot add sheet {SUMMARY_SHEET}: {e:?}"))?;
    write_summary(summary, rows, docs, info);
    book.set_active_sheet(0);
    let bytes = to_bytes(&book)?;

    let path = sibling(info.sheet_path, "_results");
    match std::fs::write(&path, &bytes) {
        Ok(()) => Ok(Saved { path, note: None }),
        Err(e) => {
            // Usually the previous results file is still open in Excel.
            let stamp = chrono::Local::now().format("_results_%H%M%S");
            let other = sibling(info.sheet_path, &stamp.to_string());
            std::fs::write(&other, &bytes).context("cannot write results file")?;
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            Ok(Saved {
                path: other,
                note: Some(format!(
                    "{name} not replaced ({e}). Close it in Excel next time."
                )),
            })
        }
    }
}

fn write_rows(sheet: &mut Worksheet, rows: &[ResultRow]) {
    let mut out = SheetWriter::new(sheet);
    let headers: Vec<&str> = RESULT_COLUMNS.iter().map(|(title, _)| *title).collect();
    out.bold_row(&headers);
    for r in rows {
        let marks = r.marks();
        let detail = match &marks {
            Some(marks) if r.status != Status::Match => {
                format!("{} Digits: {}.", r.detail, describe_marks(marks))
            }
            _ => r.detail.clone(),
        };
        let read = r.aligned.as_deref().unwrap_or(&r.read);
        let row = out.row;
        out.colored_row(
            &[
                &r.pages,
                r.status.label(),
                &detail,
                read,
                &r.rib,
                &r.full_name,
                &r.case,
            ],
            r.status,
        );
        if let (Some(aligned), Some(marks)) = (&r.aligned, &marks) {
            out.sheet
                .cell_mut((4, row))
                .set_rich_text(marked(aligned, marks));
            out.sheet
                .cell_mut((5, row))
                .set_rich_text(marked_in_place(&r.rib, marks));
        }
    }
    for (col, (_, width)) in RESULT_COLUMNS.iter().enumerate() {
        out.sheet
            .column_dimension_by_number_mut(col as u32 + 1)
            .set_width(*width);
    }
}

/// Excel's dark "Good / Neutral / Bad" text colors, readable on every row fill.
fn mark_argb(mark: Mark) -> &'static str {
    match mark {
        Mark::Bull => "FF006100",
        Mark::Cow => "FFC65911",
        Mark::Miss => "FF9C0006",
        Mark::NotRead => "FF808080",
    }
}

fn run(text: &str, mark: Option<Mark>) -> TextElement {
    let mut element = TextElement::default();
    element.set_text(text);
    if let Some(mark) = mark {
        let font = element.run_properties_mut();
        font.color_mut().set_argb_str(mark_argb(mark));
        font.set_bold(mark != Mark::NotRead);
    }
    element
}

/// One colored character per position of the reading.
fn marked(aligned: &str, marks: &[Mark]) -> RichText {
    let mut text = RichText::default();
    for (c, &mark) in aligned.chars().zip(marks) {
        text.add_rich_text_elements(run(&c.to_string(), Some(mark)));
    }
    text
}

/// Color the RIB digits of a spreadsheet cell in place, keeping its spaces and any prefix.
/// The RIB is the last 20 digits of the cell, so marks are laid from the right.
fn marked_in_place(original: &str, marks: &[Mark]) -> RichText {
    let mut remaining = marks.len();
    let mut runs: Vec<TextElement> = original
        .chars()
        .rev()
        .map(|c| {
            let mark = (c.is_ascii_digit() && remaining > 0).then(|| {
                remaining -= 1;
                marks[remaining]
            });
            run(&c.to_string(), mark)
        })
        .collect();
    runs.reverse();
    let mut text = RichText::default();
    for element in runs {
        text.add_rich_text_elements(element);
    }
    text
}

fn write_summary(sheet: &mut Worksheet, rows: &[ResultRow], docs: &[DocResult], info: &RunInfo) {
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

    out.bold_row(&["Result", "Count", "Meaning"]);
    for status in Status::ALL {
        let n = count(rows, status, |r| r.status).to_string();
        out.colored_row(&[status.label(), &n, status.meaning()], status);
    }
    out.bold_row(&["Total", &rows.len().to_string()]);
    out.skip();

    let issues = page_issues(docs);
    if !issues.is_empty() {
        out.bold_row(&["Page issues", "Pages", "What to do"]);
        for (pages, issue) in issues {
            out.colored_row(&["", pages, issue], Status::Order);
        }
        out.skip();
    }

    out.bold_row(&["RIB digit colors"]);
    for (mark, meaning) in [
        (Mark::Bull, "Right digit, right place"),
        (Mark::Cow, "Right digit, wrong place"),
        (Mark::Miss, "Wrong digit"),
        (Mark::NotRead, "Unread"),
    ] {
        let row = out.row;
        out.row(&["", meaning]);
        let sample = if mark == Mark::NotRead { "?" } else { "0" };
        out.sheet
            .cell_mut((1, row))
            .set_rich_text(marked(sample, &[mark]));
    }
    out.skip();

    let unknown = docs.iter().filter(|d| d.person.is_none()).count();
    if unknown > 0 {
        out.bold_row(&["Notes"]);
        out.row(&[&format!(
            "{unknown} document(s) match no name: \"(not found)\" in {RESULTS_SHEET}."
        )]);
    }

    for (col, width) in [(1, 22.0), (2, 14.0), (3, 90.0)] {
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
