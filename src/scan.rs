use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem, TextLine};
use rten::Model;

/// 150 DPI is ~44% fewer pixels than 200 and still enough for printed text.
/// Raise it if RIB digits on real checks start being misread.
const RENDER_DPI: f32 = 150.0;
/// Placed between OCR lines that were joined back into one printed row.
const SEGMENT_GAP: &str = "   ";
const DETECTION_MODEL: &str = "text-detection.rten";
const RECOGNITION_MODEL: &str = "text-recognition.rten";

/// A quarter-turn applied to a page image before OCR, clockwise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Turn {
    None,
    Right,
    UpsideDown,
    Left,
}

/// OCR result of one page.
pub struct PageText {
    /// One line per printed row.
    pub text: String,
    /// Median slant of the text lines, in degrees.
    pub tilt: f32,
    /// Most text lines run top to bottom: the page lies on its side.
    pub sideways: bool,
}

/// Renders PDF pages and runs OCR on them. Shared by reference across worker threads.
pub struct Scanner {
    pdf: Pdf,
    engine: OcrEngine,
    interpreter: InterpreterSettings,
}

impl Scanner {
    pub fn new(engine: OcrEngine, pdf_path: &Path) -> Result<Self> {
        let bytes = std::fs::read(pdf_path).context("cannot read pdf")?;
        let pdf = Pdf::new(Arc::new(bytes)).map_err(|e| anyhow!("invalid pdf: {e:?}"))?;
        Ok(Self {
            pdf,
            engine,
            interpreter: InterpreterSettings::default(),
        })
    }

    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// Render one page (0-based), turn it, and OCR it.
    pub fn read_page(&self, index: usize, turn: Turn) -> Result<PageText> {
        let page = &self.pdf.pages()[index];
        let (width, height) = page.render_dimensions();
        let scale = (RENDER_DPI / 72.0).min(u16::MAX as f32 / width.max(height));
        let cache = RenderCache::new();
        let pixmap = hayro::render(
            page,
            &cache,
            &self.interpreter,
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
            },
        );

        let (pixels, width, height) = turned(
            pixmap.data_as_u8_slice(),
            pixmap.width() as usize,
            pixmap.height() as usize,
            turn,
        );
        let source = ImageSource::from_bytes(&pixels, (width as u32, height as u32))?;
        let input = self.engine.prepare_input(source)?;
        let words = self.engine.detect_words(&input)?;
        let boxes: Vec<(f32, f32, f32, f32)> = words
            .iter()
            .map(|w| (w.up_axis().x, w.up_axis().y, w.width(), w.height()))
            .collect();
        let lines = self.engine.find_text_lines(&input, &words);
        let lines: Vec<TextLine> = self
            .engine
            .recognize_text(&input, &lines)?
            .into_iter()
            .flatten()
            .collect();
        Ok(PageText {
            text: join_rows(lines.iter()),
            tilt: tilt(&boxes),
            sideways: sideways(&boxes),
        })
    }
}

/// Rotate an RGBA image clockwise by `turn`.
fn turned(rgba: &[u8], width: usize, height: usize, turn: Turn) -> (Vec<u8>, usize, usize) {
    let (w, h) = match turn {
        Turn::None | Turn::UpsideDown => (width, height),
        Turn::Right | Turn::Left => (height, width),
    };
    if turn == Turn::None {
        return (rgba.to_vec(), w, h);
    }
    let mut out = vec![0u8; rgba.len()];
    for y in 0..height {
        for x in 0..width {
            let (nx, ny) = match turn {
                Turn::Right => (height - 1 - y, x),
                Turn::UpsideDown => (width - 1 - x, height - 1 - y),
                Turn::Left => (y, width - 1 - x),
                Turn::None => (x, y),
            };
            let from = (y * width + x) * 4;
            let to = (ny * w + nx) * 4;
            out[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
        }
    }
    (out, w, h)
}

/// Direction the text of a word box runs in, from its long side, for boxes clearly longer
/// than tall. Works even when the words can't be recognized.
fn text_directions(boxes: &[(f32, f32, f32, f32)]) -> impl Iterator<Item = (f32, f32)> + '_ {
    boxes
        .iter()
        .filter(|(_, _, w, h)| w.max(*h) >= 2.0 * w.min(*h))
        .map(|&(up_x, up_y, w, h)| if w >= h { (-up_y, up_x) } else { (up_x, up_y) })
}

/// Median slant of the words, in degrees (0 to 45). A quarter turn reads as 0; `sideways`
/// catches that.
fn tilt(boxes: &[(f32, f32, f32, f32)]) -> f32 {
    let mut angles: Vec<f32> = text_directions(boxes)
        .map(|(dx, dy)| {
            let a = dy.atan2(dx).to_degrees().abs() % 90.0;
            a.min(90.0 - a)
        })
        .collect();
    if angles.is_empty() {
        return 0.0;
    }
    angles.sort_by(f32::total_cmp);
    angles[angles.len() / 2]
}

/// Most words run top to bottom: the page lies on its side.
fn sideways(boxes: &[(f32, f32, f32, f32)]) -> bool {
    let (vertical, total) = text_directions(boxes).fold((0, 0), |(v, t), (dx, dy)| {
        (v + usize::from(dy.abs() > dx.abs()), t + 1)
    });
    total > 0 && vertical * 2 > total
}

/// OCR splits a row into several lines when the print has big gaps (a RIB printed in segments).
/// Lines that overlap vertically are put back on one row, left to right, with a wide gap
/// between them so the segments stay visible.
fn join_rows<'a>(lines: impl Iterator<Item = &'a TextLine>) -> String {
    struct Segment {
        top: i32,
        bottom: i32,
        left: i32,
        text: String,
    }
    let mut segments: Vec<Segment> = lines
        .map(|line| {
            let rect = line.bounding_rect();
            Segment {
                top: rect.top(),
                bottom: rect.bottom(),
                left: rect.left(),
                text: line.to_string(),
            }
        })
        .filter(|s| !s.text.trim().is_empty())
        .collect();
    segments.sort_by_key(|s| s.top);

    let mut rows: Vec<Vec<Segment>> = Vec::new();
    for segment in segments {
        let same_row = rows.last().is_some_and(|row| {
            let top = row.iter().map(|s| s.top).min().unwrap_or(0);
            let bottom = row.iter().map(|s| s.bottom).max().unwrap_or(0);
            let overlap = bottom.min(segment.bottom) - top.max(segment.top);
            let height = (segment.bottom - segment.top).min(bottom - top).max(1);
            overlap * 2 >= height
        });
        match rows.last_mut() {
            Some(row) if same_row => row.push(segment),
            _ => rows.push(vec![segment]),
        }
    }

    rows.into_iter()
        .map(|mut row| {
            row.sort_by_key(|s| s.left);
            row.into_iter()
                .map(|s| s.text)
                .collect::<Vec<_>>()
                .join(SEGMENT_GAP)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn load_engine() -> Result<OcrEngine> {
    let dir = find_models_dir().ok_or_else(|| {
        anyhow!(
            "OCR models not found. Download them into a `models` folder next to the program:\n  \
             curl -L https://ocrs-models.s3-accelerate.amazonaws.com/{DETECTION_MODEL} -o models/{DETECTION_MODEL}\n  \
             curl -L https://ocrs-models.s3-accelerate.amazonaws.com/{RECOGNITION_MODEL} -o models/{RECOGNITION_MODEL}\n\
             or point READSCAN_MODELS to the folder that contains them."
        )
    })?;
    let detection_model = Model::load_file(dir.join(DETECTION_MODEL))?;
    let recognition_model = Model::load_file(dir.join(RECOGNITION_MODEL))?;
    OcrEngine::new(OcrEngineParams {
        detection_model: Some(detection_model),
        recognition_model: Some(recognition_model),
        ..Default::default()
    })
}

fn find_models_dir() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(dir) = std::env::var("READSCAN_MODELS") {
        candidates.push(PathBuf::from(dir));
    }
    candidates.push(PathBuf::from("models"));
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        candidates.push(exe_dir.join("models"));
    }
    // Where `ocrs-cli` caches its models.
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        candidates.push(PathBuf::from(home).join(".cache").join("ocrs"));
    }
    candidates
        .into_iter()
        .find(|d| d.join(DETECTION_MODEL).is_file() && d.join(RECOGNITION_MODEL).is_file())
}

pub fn ensure_pairs(page_count: usize) -> Result<()> {
    if page_count == 0 {
        bail!("pdf has no pages");
    }
    if !page_count.is_multiple_of(2) {
        eprintln!(
            "Warning: pdf has an odd number of pages ({page_count}); the last document has no check page."
        );
    }
    Ok(())
}
