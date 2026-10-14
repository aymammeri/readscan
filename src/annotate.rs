//! The pages of the scanned PDF that need a manual check, each document with a banner on top
//! with the result and the spreadsheet RIB, lined up with what was read on the check. The page
//! is made taller for the banner, so nothing on the scan is covered. Documents that need no
//! check are left out.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use unicode_normalization::UnicodeNormalization;

use crate::report::{DocResult, Status};
use crate::rib::{self, Mark};
use crate::sheet::Directory;

/// Resource name of the banner font (Courier, so RIB digits line up).
const FONT: &str = "ReadscanMono";
/// Courier glyphs are 0.6 em wide.
const CHAR_WIDTH: f32 = 0.6;
/// The font is sized so the longest banner line fills the page width, but never smaller than
/// for lines this long. Longer lines (the details) are wrapped.
const MIN_LINE_CHARS: usize = 50;
const LEADING: f32 = 1.3;
/// Blank space around the banner text, in font sizes.
const PADDING: f32 = 0.8;
/// Width of the labels before the RIB digits, so the digits line up.
const LABEL_WIDTH: usize = 17;

/// Banner for one page.
pub struct Note {
    /// 0-based page.
    page: usize,
    /// Every page (0-based) of the document, kept in the checked PDF.
    keep: Vec<usize>,
    status: Status,
    lines: Vec<String>,
}

/// One banner per document that is not a full MATCH, on its check page (else its QUITTANCE).
pub fn notes(dir: &Directory, docs: &[DocResult]) -> Vec<Note> {
    docs.iter()
        .filter(|d| d.status != Status::Match)
        .map(|d| Note {
            page: d.page,
            keep: d.page_ids.clone(),
            status: d.status,
            lines: note_lines(dir, d),
        })
        .collect()
}

fn note_lines(dir: &Directory, d: &DocResult) -> Vec<String> {
    let pages = if d.pages.contains('-') {
        "pages"
    } else {
        "page"
    };
    let case = d.case.as_deref().unwrap_or("NOT READ, check it");
    let mut lines = vec![
        format!("readscan: {}, {pages} {}", d.status.label(), d.pages),
        d.detail.clone(),
        format!("{:<LABEL_WIDTH$}{case}", "Case number:"),
    ];
    let shown: Vec<usize> = match (
        d.matched_entry.or(d.reading.as_ref().map(|r| r.entry)),
        d.person,
    ) {
        (Some(e), _) => vec![e],
        (None, Some(p)) => dir.persons[p].entry_ids.clone(),
        (None, None) => Vec::new(),
    };
    if shown.is_empty() {
        lines.push("Name not found in the spreadsheet.".to_string());
    }
    for e in shown {
        let entry = &dir.entries[e];
        let rib = if entry.rib_digits.is_empty() {
            "(none)"
        } else {
            &entry.rib_digits
        };
        lines.push(format!(
            "{:<LABEL_WIDTH$}{}, row {}",
            "Name:", entry.full_name, entry.row
        ));
        lines.push(format!("{:<LABEL_WIDTH$}{rib}", "Spreadsheet RIB:"));
        let Some(reading) = d.reading.as_ref().filter(|r| r.entry == e) else {
            continue;
        };
        lines.push(format!(
            "{:<LABEL_WIDTH$}{}",
            "Read on check:", reading.aligned
        ));
        let marks: String = rib::score(&entry.rib_digits, &reading.aligned)
            .iter()
            .map(|m| if *m == Mark::Bull { ' ' } else { '^' })
            .collect();
        lines.push(format!("{:<LABEL_WIDTH$}{marks}", "Differs (^):"));
    }
    lines
}

/// Write `<name>_checked.pdf` in the output folder with only the pages of the noted documents.
/// None when no page needs a check.
pub fn write_checked_pdf(
    pdf_path: &Path,
    out_dir: &Path,
    notes: &[Note],
) -> Result<Option<PathBuf>> {
    if notes.is_empty() {
        return Ok(None);
    }
    let mut doc = Document::load(pdf_path).map_err(|e| anyhow!("cannot open pdf: {e}"))?;
    let pages = doc.get_pages();
    let font = doc.add_object(Dictionary::from_iter([
        ("Type", Object::Name(b"Font".to_vec())),
        ("Subtype", Object::Name(b"Type1".to_vec())),
        ("BaseFont", Object::Name(b"Courier".to_vec())),
        ("Encoding", Object::Name(b"WinAnsiEncoding".to_vec())),
    ]));
    for note in notes {
        let page = *pages
            .get(&(note.page as u32 + 1))
            .ok_or_else(|| anyhow!("page {} not in pdf", note.page + 1))?;
        add_banner(&mut doc, page, font, note)?;
    }
    let keep: HashSet<usize> = notes
        .iter()
        .flat_map(|n| n.keep.iter().copied().chain([n.page]))
        .collect();
    let others: Vec<u32> = pages
        .keys()
        .copied()
        .filter(|&number| !keep.contains(&(number as usize - 1)))
        .collect();
    doc.delete_pages(&others);
    doc.prune_objects();
    let stem = pdf_path.file_stem().unwrap_or_default().to_string_lossy();
    let out = out_dir.join(format!("{stem}_checked.pdf"));
    doc.save(&out)
        .map_err(|e| anyhow!("cannot write {}: {e}", out.display()))?;
    Ok(Some(out))
}

fn add_banner(doc: &mut Document, page: ObjectId, font: ObjectId, note: &Note) -> Result<()> {
    let [x0, y0, x1, y1] = inherited(doc, page, b"CropBox")
        .or_else(|| inherited(doc, page, b"MediaBox"))
        .and_then(|b| rect(doc, b))
        .unwrap_or([0.0, 0.0, 612.0, 792.0]);
    let rotate = inherited(doc, page, b"Rotate")
        .and_then(|r| r.as_i64().ok())
        .map_or(0, |r| r.rem_euclid(360));
    let (w, h) = (x1 - x0, y1 - y0);
    // Size of the page as shown, after its /Rotate.
    let (shown_w, shown_h) = if rotate % 180 == 0 { (w, h) } else { (h, w) };

    // Size the font on the fixed lines (labels and RIBs); the details line wraps to fit.
    let longest = note.lines[2..]
        .iter()
        .map(|l| l.chars().count())
        .max()
        .unwrap_or(0)
        .max(MIN_LINE_CHARS);
    let size = shown_w / (CHAR_WIDTH * (longest as f32 + 2.0 * PADDING / CHAR_WIDTH));
    let chars = longest.max(20);
    let lines: Vec<String> = note.lines.iter().flat_map(|l| wrap(l, chars)).collect();
    let banner = (lines.len() as f32 * LEADING + 2.0 * PADDING) * size;

    // Grow the page on its shown top edge, and map shown coordinates to page space.
    let (new_box, cm) = match rotate {
        90 => ([x0 - banner, y0, x1, y1], [0.0, 1.0, -1.0, 0.0, x1, y0]),
        180 => ([x0, y0 - banner, x1, y1], [-1.0, 0.0, 0.0, -1.0, x1, y1]),
        270 => ([x0, y0, x1 + banner, y1], [0.0, -1.0, 1.0, 0.0, x0, y1]),
        _ => ([x0, y0, x1, y1 + banner], [1.0, 0.0, 0.0, 1.0, x0, y0]),
    };

    let [r, g, b] = fill_rgb(note.status);
    let mut ops = format!(
        "q {} cm\n{r:.3} {g:.3} {b:.3} rg 0 {shown_h:.2} {shown_w:.2} {banner:.2} re f\n\
         0 0 0 RG 0.8 w 0 {shown_h:.2} m {shown_w:.2} {shown_h:.2} l S\n\
         BT /{FONT} {size:.2} Tf 0 0 0 rg {:.2} TL {:.2} {:.2} Td\n",
        cm.map(|v| format!("{v:.3}")).join(" "),
        size * LEADING,
        size * PADDING,
        shown_h + banner - size * (PADDING + 1.0),
    )
    .into_bytes();
    for line in &lines {
        ops.push(b'(');
        ops.extend(pdf_text(line));
        ops.extend(b") Tj T*\n");
    }
    ops.extend(b"ET Q\n");

    let resources = page_resources(doc, page, font);
    let mut contents = vec![Object::Reference(
        doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec())),
    )];
    contents.extend(page_contents(doc, page));
    let mut closing = b"Q\n".to_vec();
    closing.extend(ops);
    contents.push(Object::Reference(
        doc.add_object(Stream::new(Dictionary::new(), closing)),
    ));

    let dict = doc
        .get_dictionary_mut(page)
        .map_err(|e| anyhow!("bad page: {e}"))?;
    let new_box: Vec<Object> = new_box.iter().map(|&v| Object::Real(v)).collect();
    dict.set("MediaBox", new_box.clone());
    dict.set("CropBox", new_box);
    dict.set("Resources", resources);
    dict.set("Contents", contents);
    Ok(())
}

/// A page attribute, looked up through its parents (MediaBox, CropBox, Rotate, Resources).
fn inherited<'a>(doc: &'a Document, page: ObjectId, key: &[u8]) -> Option<&'a Object> {
    let mut id = page;
    for _ in 0..64 {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(resolve(doc, value));
        }
        id = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

fn resolve<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    match object {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(object),
        _ => object,
    }
}

fn rect(doc: &Document, object: &Object) -> Option<[f32; 4]> {
    let values: Vec<f32> = object
        .as_array()
        .ok()?
        .iter()
        .filter_map(|v| match resolve(doc, v) {
            Object::Integer(i) => Some(*i as f32),
            Object::Real(f) => Some(*f),
            _ => None,
        })
        .collect();
    let [a, b, c, d] = values[..] else {
        return None;
    };
    Some([a.min(c), b.min(d), a.max(c), b.max(d)])
}

/// The page's resources with the banner font added, as a new dictionary so resources shared
/// with other pages are left alone.
fn page_resources(doc: &Document, page: ObjectId, font: ObjectId) -> Dictionary {
    let mut resources = inherited(doc, page, b"Resources")
        .and_then(|r| r.as_dict().ok())
        .cloned()
        .unwrap_or_default();
    let mut fonts = resources
        .get(b"Font")
        .ok()
        .and_then(|f| resolve(doc, f).as_dict().ok())
        .cloned()
        .unwrap_or_default();
    fonts.set(FONT, Object::Reference(font));
    resources.set("Font", fonts);
    resources
}

fn page_contents(doc: &Document, page: ObjectId) -> Vec<Object> {
    let Ok(contents) = doc.get_dictionary(page).and_then(|d| d.get(b"Contents")) else {
        return Vec::new();
    };
    match contents {
        Object::Array(items) => items.clone(),
        Object::Reference(id) => match doc.get_object(*id) {
            Ok(Object::Array(items)) => items.clone(),
            _ => vec![Object::Reference(*id)],
        },
        _ => Vec::new(),
    }
}

/// Cut a line at spaces to at most `width` characters.
fn wrap(line: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in line.split(' ') {
        let current = lines.last_mut().expect("never empty");
        if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > width {
            lines.push(word.to_string());
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }
    lines
}

/// A PDF string body in WinAnsi encoding: accents kept where Latin-1 has them, others
/// dropped to their base letter.
fn pdf_text(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for c in text.chars() {
        let byte = match c as u32 {
            0x20..=0x7E | 0xA0..=0xFF => c as u32 as u8,
            _ => c
                .nfd()
                .next()
                .filter(char::is_ascii)
                .map_or(b'?', |base| base as u8),
        };
        if matches!(byte, b'(' | b')' | b'\\') {
            out.push(b'\\');
        }
        out.push(byte);
    }
    out
}

fn fill_rgb(status: Status) -> [f32; 3] {
    let argb = status.fill_argb();
    let channel = |i: usize| u8::from_str_radix(&argb[i..i + 2], 16).unwrap_or(255) as f32 / 255.0;
    [channel(2), channel(4), channel(6)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_spaces() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("short", 20), vec!["short"]);
    }

    #[test]
    fn pdf_text_escapes_and_encodes() {
        assert_eq!(pdf_text("a(b)\\"), b"a\\(b\\)\\\\".to_vec());
        assert_eq!(pdf_text("é°"), vec![0xE9, 0xB0]);
        assert_eq!(pdf_text("ő"), b"o".to_vec());
    }
}
