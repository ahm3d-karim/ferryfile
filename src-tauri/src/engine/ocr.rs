//! Offline OCR with the recognizer built into Windows (`Windows.Media.Ocr`).
//! It uses the OCR languages the user has installed; nothing leaves the
//! machine and nothing extra ships with the app.

use image::DynamicImage;

/// Shown when Windows has no OCR language to recognize with.
pub const NO_LANGUAGES: &str = "Windows has no text recognition (OCR) language installed. \
     Add one in Settings › Time & language › Language & region, then try again.";

/// Small images (screenshots) recognize better enlarged to about this size.
const UPSCALE_TO: u32 = 1800;

/// A recognized word; the box is in pixels of the input image, y down.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A recognized line, in reading order.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub words: Vec<Word>,
}

impl Line {
    pub fn top(&self) -> f32 {
        self.words.iter().map(|w| w.y).fold(f32::INFINITY, f32::min)
    }
    pub fn bottom(&self) -> f32 {
        self.words.iter().map(|w| w.y + w.h).fold(0.0, f32::max)
    }
    pub fn left(&self) -> f32 {
        self.words.iter().map(|w| w.x).fold(f32::INFINITY, f32::min)
    }
    /// Tallest word: close to the font's full height (ascenders to descenders).
    pub fn height(&self) -> f32 {
        self.words.iter().map(|w| w.h).fold(0.0, f32::max)
    }
}

/// Plain text from recognized lines: one line per line, with a blank line
/// where the vertical gap says a new paragraph starts.
pub fn plain_text(lines: &[Line]) -> String {
    let mut heights: Vec<f32> = lines.iter().map(Line::height).filter(|h| *h > 0.0).collect();
    heights.sort_by(f32::total_cmp);
    let typical = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    let mut out = String::new();
    let mut prev_bottom: Option<f32> = None;
    for line in lines {
        if let Some(bottom) = prev_bottom {
            out.push('\n');
            if line.top() - bottom > typical * 0.9 {
                out.push('\n');
            }
        }
        out.push_str(&line.text);
        prev_bottom = Some(line.bottom());
    }
    out
}

/// A ready recognizer. Make one per document and reuse it for every page.
pub struct Recognizer {
    #[cfg(windows)]
    engine: windows::Media::Ocr::OcrEngine,
    max_side: u32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub language: String,
}

#[cfg(windows)]
impl Recognizer {
    /// Uses the first of the user's preferred languages that has OCR, else
    /// any installed OCR language.
    pub fn new() -> Result<Self, String> {
        use windows::Media::Ocr::OcrEngine;
        // WinRT needs the thread in an apartment; worker threads start without one.
        // An "already initialized" result is fine.
        unsafe {
            let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
        }
        let engine = OcrEngine::TryCreateFromUserProfileLanguages().ok().or_else(|| {
            let langs = OcrEngine::AvailableRecognizerLanguages().ok()?;
            langs.into_iter().find_map(|l| OcrEngine::TryCreateFromLanguage(&l).ok())
        });
        let engine = engine.ok_or(NO_LANGUAGES)?;
        let language = engine
            .RecognizerLanguage()
            .and_then(|l| l.LanguageTag())
            .map(|t| t.to_string())
            .unwrap_or_default();
        let max_side = OcrEngine::MaxImageDimension().unwrap_or(4096).max(1024);
        Ok(Self { engine, max_side, language })
    }

    /// Recognizes the text in `img`. Word boxes are in `img`'s pixels.
    pub fn recognize(&self, img: &DynamicImage) -> Result<Vec<Line>, String> {
        use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
        use windows::Storage::Streams::DataWriter;

        let fail = |e: windows::core::Error| format!("Text recognition failed: {}", e.message());
        let (w, h) = (img.width().max(1), img.height().max(1));
        let scale = scale_for(w, h, self.max_side);
        let scaled;
        let img = if (scale - 1.0).abs() > 0.01 {
            let (sw, sh) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
            scaled = img.resize_exact(sw, sh, image::imageops::FilterType::CatmullRom);
            &scaled
        } else {
            img
        };
        let rgba = img.to_rgba8();
        let (bw, bh) = rgba.dimensions();
        let mut bgra = rgba.into_raw();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        let writer = DataWriter::new().map_err(fail)?;
        writer.WriteBytes(&bgra).map_err(fail)?;
        let buffer = writer.DetachBuffer().map_err(fail)?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(&buffer, BitmapPixelFormat::Bgra8, bw as i32, bh as i32)
            .map_err(fail)?;
        let result = self.engine.RecognizeAsync(&bitmap).map_err(fail)?.join().map_err(fail)?;

        let mut lines = Vec::new();
        for line in result.Lines().map_err(fail)? {
            let mut words = Vec::new();
            for word in line.Words().map_err(fail)? {
                let r = word.BoundingRect().map_err(fail)?;
                words.push(Word {
                    text: word.Text().map_err(fail)?.to_string(),
                    x: r.X / scale,
                    y: r.Y / scale,
                    w: r.Width / scale,
                    h: r.Height / scale,
                });
            }
            let text = line.Text().map_err(fail)?.to_string();
            if !words.is_empty() && !text.trim().is_empty() {
                lines.push(Line { text, words });
            }
        }
        Ok(lines)
    }
}

#[cfg(not(windows))]
impl Recognizer {
    pub fn new() -> Result<Self, String> {
        Err("Text recognition needs Windows".into())
    }
    pub fn recognize(&self, _img: &DynamicImage) -> Result<Vec<Line>, String> {
        Err("Text recognition needs Windows".into())
    }
}

/// How much to scale an image before OCR: small ones up (to at most 2x),
/// anything over the recognizer's limit down.
fn scale_for(w: u32, h: u32, max_side: u32) -> f32 {
    let long = w.max(h) as f32;
    let up = (UPSCALE_TO as f32 / long).clamp(1.0, 2.0);
    up.min(max_side as f32 / long)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, y: f32, h: f32) -> Line {
        Line { text: text.into(), words: vec![Word { text: text.into(), x: 10.0, y, w: 100.0, h }] }
    }

    #[test]
    fn plain_text_breaks_paragraphs_on_gaps() {
        let lines = [line("First line", 0.0, 20.0), line("wraps here.", 26.0, 20.0), line("New paragraph", 80.0, 20.0)];
        assert_eq!(plain_text(&lines), "First line\nwraps here.\n\nNew paragraph");
        assert_eq!(plain_text(&[]), "");
    }

    #[test]
    fn scales_small_images_up_and_huge_ones_down() {
        assert_eq!(scale_for(800, 600, 10_000), 2.0);
        assert_eq!(scale_for(1800, 1000, 10_000), 1.0);
        assert!((scale_for(3000, 2000, 10_000) - 1.0).abs() < 1e-6);
        assert!((scale_for(20_000, 100, 10_000) - 0.5).abs() < 1e-6);
    }

    /// Real OCR with Windows' recognizer: `cargo test -- --ignored ocr_reads`.
    /// Reads a page it renders itself (Typst + pdfium), so it runs in CI.
    /// Set FERRYFILE_OCR_SAMPLE to an image path to try your own instead.
    #[test]
    #[ignore]
    fn ocr_reads_rendered_text() {
        let (img, expected): (DynamicImage, &[&str]) = match std::env::var("FERRYFILE_OCR_SAMPLE") {
            Ok(path) => (image::open(path).unwrap(), &[]),
            Err(_) => (rendered_sample(), &["Ferryfile", "converts", "offline"]),
        };
        let rec = Recognizer::new().unwrap();
        let lines = rec.recognize(&img).unwrap();
        let text = plain_text(&lines);
        eprintln!("language {}\n{text}", rec.language);
        assert!(!lines.is_empty());
        for w in expected {
            assert!(text.contains(w), "OCR missed {w:?}");
        }
    }

    /// A short paragraph typeset with Typst and rendered at 200 DPI.
    fn rendered_sample() -> DynamicImage {
        use pdfium_render::prelude::PdfRenderConfig;
        let dir = std::env::temp_dir().join(format!("ferryfile-ocr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let markup = "#set page(width: 16cm, height: 6cm, margin: 1cm)\n#set text(size: 14pt)\n\
            Ferryfile converts files offline. Invoice 4821 ships on Monday.\n";
        let pdf = dir.join("sample.pdf");
        std::fs::write(&pdf, crate::engine::typeset::typst_to_pdf(markup.into(), &dir).unwrap()).unwrap();
        let pdfium = crate::engine::pdf_engine::pdfium().unwrap();
        let doc = pdfium.load_pdf_from_file(&pdf, None).unwrap();
        let page = doc.pages().get(0).unwrap();
        let img = page.render_with_config(&PdfRenderConfig::new().scale_page_by_factor(200.0 / 72.0)).unwrap().as_image().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        img
    }
}
