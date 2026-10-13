mod export;
mod matcher;
mod report;
mod scan;
mod sheet;
mod text;

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;
use rayon::prelude::*;

use export::RunInfo;
use matcher::{NameMatch, RibMatch};
use report::{DocResult, Status};
use scan::Scanner;
use sheet::Directory;

/// Text shorter than this (in letters/digits) means OCR got nothing usable from the page.
const MIN_READABLE_CHARS: usize = 3;

fn main() {
    let threads = use_all_threads();
    // Older Windows consoles need ANSI colors switched on explicitly.
    #[cfg(windows)]
    let _ = colored::control::set_virtual_terminal(true);

    let outcome = run(threads);
    if let Err(e) = &outcome {
        eprintln!("{} {e:#}", "Error:".red().bold());
    }
    // A double-clicked .exe closes its window on exit, which would hide the report.
    #[cfg(windows)]
    {
        print!("\nPress Enter to exit...");
        let _ = io::stdout().flush();
        let _ = io::stdin().read_line(&mut String::new());
    }
    if outcome.is_err() {
        std::process::exit(1);
    }
}

/// rten (the OCR runtime) sizes its thread pool to physical / performance cores only.
/// Raise it to every logical CPU, unless the user already chose a count with `RTEN_NUM_THREADS`.
fn use_all_threads() -> usize {
    if std::env::var_os("RTEN_NUM_THREADS").is_none() {
        let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
        // SAFETY: called first thing in main, before any other thread exists.
        unsafe { std::env::set_var("RTEN_NUM_THREADS", logical.to_string()) };
    }
    rten::thread_pool().run(rayon::current_num_threads)
}

fn run(threads: usize) -> Result<()> {
    let engine = scan::load_engine()?;

    let sheet_path = prompt(
        "Spreadsheet path: ",
        &["xlsx", "xlsm", "xlsb", "xls", "ods", "csv"],
    )?;
    let dir = sheet::load(&sheet_path)?;
    println!(
        "Loaded {} entries for {} people.\n",
        dir.entries.len(),
        dir.persons.len()
    );

    let pdf_path = prompt("Scanned PDF path: ", &["pdf"])?;
    let scanner = Scanner::new(engine, &pdf_path)?;
    let page_count = scanner.page_count();
    scan::ensure_pairs(page_count)?;

    let doc_count = page_count.div_ceil(2);
    println!("Scanning {doc_count} documents ({page_count} pages) on {threads} threads...\n");
    let started = Instant::now();
    let progress = Mutex::new(Progress::new(page_count));

    // Run inside rten's pool so page rendering and OCR share one set of threads
    // instead of two pools competing for the same cores. `par_bridge` is a FIFO queue:
    // each free thread takes the next page in order (1, 2, 3, ...).
    let results: Vec<DocResult> = rten::thread_pool().run(|| {
        let mut pages: Vec<(usize, Result<String>)> = (0..page_count)
            .par_bridge()
            .map(|page| {
                let text = scanner.read_page(page);
                progress.lock().expect("progress lock").finish(page);
                (page, text)
            })
            .collect();
        pages.sort_by_key(|(page, _)| *page);
        let texts: Vec<Result<String>> = pages.into_iter().map(|(_, text)| text).collect();

        texts
            .par_chunks(2)
            .enumerate()
            .map(|(doc, sides)| process_document(&dir, doc * 2, &sides[0], sides.get(1)))
            .collect()
    });

    let seconds = started.elapsed().as_secs_f32();
    println!("\nFinished in {seconds:.1}s.\n");
    let reports = report::entry_reports(&dir, &results);
    report::print_documents(&dir, &results);
    report::print_entries(&dir, &reports);

    let info = RunInfo {
        sheet_path: &sheet_path,
        pdf_path: &pdf_path,
        page_count,
        threads,
        seconds,
    };
    let saved = export::write_results(&dir, &results, &reports, &info)?;
    println!(
        "\n{} Results written to {}",
        "i".cyan().bold(),
        saved.path.display()
    );
    if let Some(backup) = saved.backup {
        println!("  Original spreadsheet backed up as {}", backup.display());
    }
    if let Some(note) = saved.note {
        println!("  {note}");
    }
    Ok(())
}

/// Prints "pages X-Y read" per document in page order, holding back pages that
/// finish before an earlier one.
struct Progress {
    page_count: usize,
    /// Every page before this one has been read.
    next: usize,
    finished: BTreeSet<usize>,
}

impl Progress {
    fn new(page_count: usize) -> Self {
        Self {
            page_count,
            next: 0,
            finished: BTreeSet::new(),
        }
    }

    fn finish(&mut self, page: usize) {
        self.finished.insert(page);
        while self.finished.remove(&self.next) {
            self.next += 1;
            // A document is complete once its check page, or the very last page, is read.
            if self.next.is_multiple_of(2) || self.next == self.page_count {
                let name_page = (self.next - 1) / 2 * 2;
                eprintln!(
                    "  [{}/{}] pages {} read",
                    name_page / 2 + 1,
                    self.page_count.div_ceil(2),
                    page_range(name_page, self.next - 1 - name_page)
                );
            }
        }
    }
}

/// 1-based label for the document starting at 0-based `name_page`.
fn page_range(name_page: usize, extra_pages: usize) -> String {
    if extra_pages > 0 {
        format!("{}-{}", name_page + 1, name_page + 2)
    } else {
        format!("{}", name_page + 1)
    }
}

/// Match one document from the OCR text of its two sides. The check is only
/// interpreted once the name is known.
fn process_document(
    dir: &Directory,
    name_page: usize,
    name_text: &Result<String>,
    check_text: Option<&Result<String>>,
) -> DocResult {
    let mut result = DocResult {
        pages: page_range(name_page, usize::from(check_text.is_some())),
        person: None,
        matched_entry: None,
        status: Status::Yellow,
        rib_read: String::new(),
        detail: String::new(),
    };

    let name_text = match name_text {
        Ok(t) if is_readable(t) => t,
        Ok(_) => return result.yellow("name page unreadable"),
        Err(e) => return result.yellow(&format!("name page OCR failed: {e}")),
    };

    let person = match matcher::find_person(dir, name_text) {
        NameMatch::Found(p) => p,
        NameMatch::Ambiguous(ids) => {
            let names: Vec<&str> = ids.iter().map(|&p| dir.person_name(p)).collect();
            return result.yellow(&format!("name is ambiguous: {}", names.join(" / ")));
        }
        NameMatch::NotFound => {
            let excerpt: String = name_text
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(80)
                .collect();
            return result.yellow(&format!(
                "no spreadsheet name found (OCR: \"{excerpt}...\")"
            ));
        }
    };
    result.person = Some(person);

    let check_text = match check_text {
        Some(Ok(t)) if is_readable(t) => t,
        Some(Ok(_)) => return result.yellow("check page unreadable"),
        Some(Err(e)) => return result.yellow(&format!("check page OCR failed: {e}")),
        None => return result.yellow("check page missing"),
    };

    match matcher::check_rib(dir, &dir.persons[person].entry_ids, check_text) {
        RibMatch::Matched(entry) => {
            result.matched_entry = Some(entry);
            result.status = Status::Green;
            result.rib_read = dir.entries[entry].rib.clone();
            result.detail = format!("RIB {} OK", dir.entries[entry].rib);
        }
        RibMatch::Mismatch { seen } => {
            result.status = Status::Red;
            result.rib_read = seen.join(" | ");
            result.detail = format!("RIB mismatch, check shows: {}", result.rib_read);
        }
        RibMatch::Unreadable => return result.yellow("no RIB readable on check"),
    }
    result
}

impl DocResult {
    fn yellow(mut self, detail: &str) -> Self {
        self.status = Status::Yellow;
        self.detail = detail.to_string();
        self
    }
}

fn is_readable(text: &str) -> bool {
    text.chars().filter(|c| c.is_alphanumeric()).count() >= MIN_READABLE_CHARS
}

/// Ask for a path. Accepts paths pasted with quotes ("Copy as path" on Windows)
/// or drag-and-dropped with escaped spaces (macOS / Linux terminals).
fn prompt(question: &str, extensions: &[&str]) -> Result<PathBuf> {
    loop {
        print!("{question}");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            anyhow::bail!("no input");
        }
        let cleaned = line.trim().trim_matches(['"', '\'']);
        // On Windows a backslash is the path separator, never an escape.
        let path = if cfg!(windows) {
            PathBuf::from(cleaned)
        } else {
            PathBuf::from(cleaned.replace("\\ ", " "))
        };
        match check_file(&path, extensions) {
            Ok(()) => return Ok(path),
            Err(reason) => eprintln!("{} {reason}", "!".yellow().bold()),
        }
    }
}

#[cfg(target_os = "macos")]
const PERMISSION_HINT: &str = "macOS blocked access to this folder. Allow your terminal app in System Settings > \
     Privacy & Security > Files and Folders, or move the file elsewhere (e.g. the project folder).";
#[cfg(windows)]
const PERMISSION_HINT: &str = "Access denied. Close the file if it is open in Excel, or check that \
     Windows Security (Controlled folder access) is not blocking this folder.";
#[cfg(not(any(target_os = "macos", windows)))]
const PERMISSION_HINT: &str = "Permission denied. Check the file's read permissions.";

fn check_file(path: &Path, extensions: &[&str]) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("File not found: {}", path.display()));
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if ext == "numbers" {
        return Err(
            "Apple Numbers files can't be read. In Numbers use File > Export To > Excel, \
             then give the .xlsx path."
                .to_string(),
        );
    }
    if !extensions.contains(&ext.as_str()) {
        return Err(format!(
            "Unsupported file type, expected: {}",
            extensions.join(", ")
        ));
    }
    match File::open(path) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(PERMISSION_HINT.to_string()),
        Err(e) => Err(format!("Cannot open file: {e}")),
        Ok(_) => Ok(()),
    }
}
