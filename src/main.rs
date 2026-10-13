mod matcher;
mod scan;
mod sheet;
mod text;

use std::fs::File;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::Result;
use colored::Colorize;
use rayon::prelude::*;

use matcher::{NameMatch, RibMatch};
use scan::Scanner;
use sheet::Directory;

/// Text shorter than this (in letters/digits) means OCR got nothing usable from the page.
const MIN_READABLE_CHARS: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    NotScanned,
    Yellow,
    Red,
    Green,
}

/// Outcome for one two-sided document (name page + check page).
struct DocResult {
    name_page: usize,
    person: Option<usize>,
    matched_entry: Option<usize>,
    status: Status,
    detail: String,
}

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
    let done = AtomicUsize::new(0);

    // Run inside rten's pool so page rendering and OCR share one set of threads
    // instead of two pools competing for the same cores.
    let results: Vec<DocResult> = rten::thread_pool().run(|| {
        (0..doc_count)
            .into_par_iter()
            .map(|doc| {
                let result = process_document(&scanner, &dir, doc * 2, page_count);
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                eprintln!(
                    "  [{n}/{doc_count}] pages {} done",
                    page_label(result.name_page, page_count)
                );
                result
            })
            .collect()
    });

    println!("\nFinished in {:.1}s.\n", started.elapsed().as_secs_f32());
    print_documents(&dir, &results, page_count);
    print_entries(&dir, &results);
    Ok(())
}

fn process_document(
    scanner: &Scanner,
    dir: &Directory,
    name_page: usize,
    page_count: usize,
) -> DocResult {
    let check_page = (name_page + 1 < page_count).then_some(name_page + 1);
    let mut result = DocResult {
        name_page,
        person: None,
        matched_entry: None,
        status: Status::Yellow,
        detail: String::new(),
    };

    // Both sides are OCR'd at the same time; the check is only interpreted once the name is known.
    let (name_text, check_text) = rayon::join(
        || scanner.read_page(name_page),
        || check_page.map(|p| scanner.read_page(p)),
    );

    let name_text = match name_text {
        Ok(t) if is_readable(&t) => t,
        Ok(_) => return result.yellow("name page unreadable"),
        Err(e) => return result.yellow(&format!("name page OCR failed: {e}")),
    };

    let person = match matcher::find_person(dir, &name_text) {
        NameMatch::Found(p) => p,
        NameMatch::Ambiguous(ids) => {
            let names: Vec<&str> = ids.iter().map(|&p| person_name(dir, p)).collect();
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
        Some(Ok(t)) if is_readable(&t) => t,
        Some(Ok(_)) => return result.yellow("check page unreadable"),
        Some(Err(e)) => return result.yellow(&format!("check page OCR failed: {e}")),
        None => return result.yellow("check page missing"),
    };

    match matcher::check_rib(dir, &dir.persons[person].entry_ids, &check_text) {
        RibMatch::Matched(entry) => {
            result.matched_entry = Some(entry);
            result.status = Status::Green;
            result.detail = format!("RIB {} OK", dir.entries[entry].rib);
        }
        RibMatch::Mismatch { seen } => {
            result.status = Status::Red;
            result.detail = format!("RIB mismatch, check shows: {}", seen.join(" | "));
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

fn person_name(dir: &Directory, person: usize) -> &str {
    &dir.entries[dir.persons[person].entry_ids[0]].full_name
}

fn page_label(name_page: usize, page_count: usize) -> String {
    if name_page + 1 < page_count {
        format!("{}-{}", name_page + 1, name_page + 2)
    } else {
        format!("{}", name_page + 1)
    }
}

fn paint(status: Status, text: &str) -> colored::ColoredString {
    match status {
        Status::Green => text.green(),
        Status::Red => text.red(),
        Status::Yellow => text.yellow(),
        Status::NotScanned => text.dimmed(),
    }
}

fn print_documents(dir: &Directory, results: &[DocResult], page_count: usize) {
    println!("{}", "Documents".bold());
    for r in results {
        let name = r.person.map_or("?", |p| person_name(dir, p));
        let line = format!(
            "  pages {:<7} {:<30} {}",
            page_label(r.name_page, page_count),
            name,
            r.detail
        );
        println!("{}", paint(r.status, &line));
    }
    println!();
}

/// Roll document results up to spreadsheet rows. A row is green once any check proves its RIB;
/// otherwise it takes the worst outcome among the documents of the same person.
fn print_entries(dir: &Directory, results: &[DocResult]) {
    let mut statuses = vec![Status::NotScanned; dir.entries.len()];
    for r in results {
        if let Some(entry) = r.matched_entry {
            statuses[entry] = Status::Green;
        }
    }
    for r in results.iter().filter(|r| r.status != Status::Green) {
        let Some(person) = r.person else { continue };
        for &entry in &dir.persons[person].entry_ids {
            statuses[entry] = statuses[entry].max(r.status);
        }
    }

    println!("{}", "Spreadsheet entries".bold());
    for (entry, status) in dir.entries.iter().zip(&statuses) {
        let label = match status {
            Status::Green => "MATCH",
            Status::Red => "MISMATCH",
            Status::Yellow => "UNREADABLE",
            Status::NotScanned => "NOT IN PDF",
        };
        let line = format!(
            "  row {:<5} {:<30} {:<26} {}",
            entry.row, entry.full_name, entry.rib, label
        );
        println!("{}", paint(*status, &line));
    }

    let count = |s: Status| statuses.iter().filter(|&&x| x == s).count();
    println!(
        "\n{}  {}  {}  {}",
        format!("{} match", count(Status::Green)).green(),
        format!("{} mismatch", count(Status::Red)).red(),
        format!("{} unreadable", count(Status::Yellow)).yellow(),
        format!("{} not in pdf", count(Status::NotScanned)).dimmed(),
    );
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
