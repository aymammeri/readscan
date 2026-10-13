use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use ocrs::{ImageSource, OcrEngine, OcrEngineParams};
use rten::Model;

/// Scans are usually 200-300 DPI; rendering at 200 keeps text sharp without huge bitmaps.
const RENDER_DPI: f32 = 200.0;
const DETECTION_MODEL: &str = "text-detection.rten";
const RECOGNITION_MODEL: &str = "text-recognition.rten";

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

    /// Render one page (0-based) and return its OCR text, one line per text line.
    pub fn read_page(&self, index: usize) -> Result<String> {
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

        let source = ImageSource::from_bytes(
            pixmap.data_as_u8_slice(),
            (pixmap.width() as u32, pixmap.height() as u32),
        )?;
        let input = self.engine.prepare_input(source)?;
        self.engine.get_text(&input)
    }
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
