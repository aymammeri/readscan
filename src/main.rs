mod matcher;
mod scan;
mod sheet;
mod text;

use std::io::{self, BufRead, Write};
use std::path::PathBuf;
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
    if let Err(e) = run() {
        eprintln!("{} {e:#}", "Error:".red().bold());
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let engine = scan::load_engine()?;

    let sheet_path = prompt("Spreadsheet path: ")?;
    let dir = sheet::load(&sheet_path)?;
    println!(
        "Loaded {} entries for {} people.\n",
        dir.entries.len(),
        dir.persons.len()
    );

    let pdf_path = prompt("Scanned PDF path: ")?;
    let scanner = Scanner::new(engine, &pdf_path)?;
    let page_count = scanner.page_count();
    scan::ensure_pairs(page_count)?;

    let doc_count = page_count.div_ceil(2);
    println!("Scanning {doc_count} documents ({page_count} pages)...\n");
    let started = Instant::now();
    let done = AtomicUsize::new(0);

    let results: Vec<DocResult> = (0..doc_count)
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
        .collect();

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

/// Ask for a path. Accepts paths pasted with quotes or drag-and-dropped with escaped spaces.
fn prompt(question: &str) -> Result<PathBuf> {
    loop {
        print!("{question}");
        io::stdout().flush()?;
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line)? == 0 {
            anyhow::bail!("no input");
        }
        let cleaned = line.trim().trim_matches(['"', '\'']).replace("\\ ", " ");
        let path = PathBuf::from(cleaned);
        if path.is_file() {
            return Ok(path);
        }
        eprintln!("File not found: {}", path.display());
    }
}
