//! OCR for PDFs: pages without a text layer (scans) are rendered with pdfium
//! and read by Windows' recognizer. Used as the automatic fallback for
//! "Text", "Markdown" and "DOCX", and by "Searchable PDF", which lays an
//! invisible, selectable text layer over the scanned page images.

use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;

use super::ocr::{self, Recognizer};
use super::pdf_engine::pdfium;
use super::pdf_layout::Line as LayoutLine;
use super::{jobs, output_path, Progress};

/// A page with fewer visible characters than this has no usable text layer.
pub(super) const MIN_CHARS_PER_PAGE: usize = 3;

/// Render scans at this resolution for OCR (capped by [`MAX_RENDER_SIDE`]).
const OCR_DPI: f32 = 300.0;
const MAX_RENDER_SIDE: f32 = 6000.0;

pub(super) const ALREADY_SEARCHABLE: &str = "This PDF already has selectable text on every page";

/// Which pages to OCR, given each page's count of text-layer characters.
///
/// Pages without a text layer are OCR'd. When OCR can't run (`ocr_error`),
/// a PDF that has text elsewhere still converts from that text; one with no
/// text at all fails with the reason OCR couldn't run.
pub(super) fn ocr_plan(char_counts: &[usize], ocr_error: Option<&str>) -> Result<Vec<usize>, String> {
    let pages: Vec<usize> = (0..char_counts.len()).filter(|&i| char_counts[i] < MIN_CHARS_PER_PAGE).collect();
    let total: usize = char_counts.iter().sum();
    match ocr_error {
        _ if pages.is_empty() => Ok(pages),
        None => Ok(pages),
        Some(_) if total >= MIN_CHARS_PER_PAGE * char_counts.len().max(1) => Ok(Vec::new()),
        Some(e) => Err(e.to_string()),
    }
}

/// Recognized lines of one rendered page, with what's needed to place them
/// back on the page.
pub(super) struct PageOcr {
    pub lines: Vec<ocr::Line>,
    /// Points per rendered pixel.
    pub scale: f32,
    /// Page size as displayed (after its /Rotate), in points.
    pub view_w: f32,
    pub view_h: f32,
}

pub(super) fn ocr_page(rec: &Recognizer, page: &PdfPage) -> Result<PageOcr, String> {
    let (view_w, view_h) = (page.width().value, page.height().value);
    let factor = (OCR_DPI / 72.0).min(MAX_RENDER_SIDE / view_w.max(view_h).max(1.0));
    let config = PdfRenderConfig::new().scale_page_by_factor(factor);
    let img = page
        .render_with_config(&config)
        .and_then(|b| b.as_image())
        .map_err(|e| format!("PDF error: {e}"))?;
    let scale = view_w / img.width().max(1) as f32;
    Ok(PageOcr { lines: rec.recognize(&img)?, scale, view_w, view_h })
}

impl PageOcr {
    /// The lines in `pdf_layout`'s terms (points, y up), so scans get the same
    /// headings/paragraphs/lists treatment as text PDFs.
    pub fn layout_lines(&self) -> Vec<LayoutLine> {
        self.lines
            .iter()
            .map(|l| {
                let size = line_font_size(l) * self.scale;
                LayoutLine {
                    text: l.text.clone(),
                    size,
                    top: self.view_h - l.top() * self.scale,
                    left: l.left() * self.scale,
                    baseline: self.view_h - (l.bottom() * self.scale - size * 0.2),
                    bold: false,
                }
            })
            .collect()
    }
}

/// A word's box spans different parts of the type depending on its letters
/// ("Contents" has no descenders, "only" no capitals). Estimates the font size
/// from the box height and which letters stick up or down.
fn word_font_size(text: &str, height: f32) -> f32 {
    if !text.chars().all(|c| c.is_ascii()) {
        return height / 0.9;
    }
    let up = text.chars().any(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || "bdfhiklt()[]{}/|'\"!?".contains(c));
    let down = text.chars().any(|c| "gjpqy,;()[]{}".contains(c));
    let ratio = match (up, down) {
        (true, true) => 0.92,
        (true, false) => 0.72,
        (false, true) => 0.70,
        (false, false) => 0.5,
    };
    height / ratio
}

/// Median of the line's per-word font size estimates.
fn line_font_size(line: &ocr::Line) -> f32 {
    let mut sizes: Vec<f32> = line.words.iter().map(|w| word_font_size(&w.text, w.h)).collect();
    sizes.sort_by(f32::total_cmp);
    sizes.get(sizes.len() / 2).copied().unwrap_or_else(|| line.height())
}

/// OCR'd sizes are rough. Lines near the body size are snapped to it, and
/// short lines clearly larger than body text are lifted to where the layout
/// pass reads them as headings (it wants 15% larger; OCR blurs that).
pub(super) fn snap_sizes(pages: &mut [super::pdf_layout::Page], which: &[usize]) {
    let mut sizes: Vec<f32> = which.iter().flat_map(|&i| pages[i].lines.iter().map(|l| l.size)).collect();
    if sizes.is_empty() {
        return;
    }
    sizes.sort_by(f32::total_cmp);
    let body = sizes[sizes.len() / 2];
    for &i in which {
        for l in &mut pages[i].lines {
            if (l.size - body).abs() <= body * 0.06 {
                l.size = body;
            } else if l.size >= body * 1.08 && l.text.split_whitespace().count() <= 10 {
                l.size = l.size.max(body * 1.16);
            }
        }
    }
}

/// Maps a point on the displayed page (points, y down from the top-left) to
/// PDF user space for a page with the given /Rotate and crop box
/// `(left, bottom, right, top)`. Also returns the text direction there.
fn to_user_space(rotation: u16, crop: (f32, f32, f32, f32), vx: f32, vy: f32) -> ((f32, f32), (f32, f32)) {
    let (l, b, r, t) = crop;
    match rotation {
        90 => ((l + vy, b + vx), (0.0, 1.0)),
        180 => ((r - vx, b + vy), (-1.0, 0.0)),
        270 => ((r - vy, t - vx), (0.0, -1.0)),
        _ => ((l + vx, t - vy), (1.0, 0.0)),
    }
}

fn rotation_degrees(page: &PdfPage) -> u16 {
    match page.rotation() {
        Ok(PdfPageRenderRotation::Degrees90) => 90,
        Ok(PdfPageRenderRotation::Degrees180) => 180,
        Ok(PdfPageRenderRotation::Degrees270) => 270,
        _ => 0,
    }
}

/// Characters the standard Helvetica font can carry (WinAnsi); anything else
/// needs an embedded Unicode font.
fn win_ansi(c: char) -> bool {
    matches!(c, ' '..='~' | '\u{A0}'..='\u{FF}')
        || "€‚ƒ„…†‡ˆ‰Š‹ŒŽ‘’“”•–—˜™š›œžŸ".contains(c)
}

/// A Unicode font from Windows for non-Latin text layers.
fn unicode_font(doc: &mut PdfDocument) -> Option<PdfFontToken> {
    let fonts = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| "C:\\Windows".into()).join("Fonts");
    ["arial.ttf", "segoeui.ttf"]
        .iter()
        .find_map(|f| doc.fonts_mut().load_true_type_from_file(&fonts.join(f), true).ok())
}

/// Adds one invisible text object per recognized word, sized and stretched
/// to cover the word on the page image.
fn add_text_layer<'a>(doc: &PdfDocument<'a>, page: &mut PdfPage<'a>, ocr: &PageOcr, font: PdfFontToken) -> Result<usize, String> {
    let err = |e: PdfiumError| format!("PDF error: {e}");
    let rotation = rotation_degrees(page);
    let crop = page
        .boundaries()
        .crop()
        .or_else(|_| page.boundaries().media())
        .map(|b| (b.bounds.left().value, b.bounds.bottom().value, b.bounds.right().value, b.bounds.top().value))
        .unwrap_or((0.0, 0.0, ocr.view_w, ocr.view_h));
    page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
    let mut added = 0;
    let words = ocr.lines.iter().flat_map(|l| l.words.iter().enumerate().map(move |(i, w)| (w, i + 1 == l.words.len())));
    for (word, last_in_line) in words {
        let text = word.text.trim();
        if text.is_empty() {
            continue;
        }
        let (w, h) = (word.w * ocr.scale, word.h * ocr.scale);
        if w < 0.5 || h < 0.5 {
            continue;
        }
        let size = h * 0.8;
        let mut obj = PdfPageTextObject::new(doc, text, font, PdfPoints::new(size)).map_err(err)?;
        obj.set_render_mode(PdfPageTextRenderMode::Invisible).map_err(err)?;
        let natural = obj.width().map(|p| p.value).unwrap_or(0.0);
        // A trailing space (stretched into the gap) keeps words apart when
        // the text is copied or searched; the box is fitted to the word alone.
        if !last_in_line {
            obj.set_text(format!("{text} ")).map_err(err)?;
        }
        let stretch = if natural > 0.01 { w / natural } else { 1.0 };
        // Baseline sits a little above the box's bottom (descenders hang below).
        let (vx, vy) = (word.x * ocr.scale, (word.y + word.h) * ocr.scale - h * 0.2);
        let ((x, y), (dx, dy)) = to_user_space(rotation, crop, vx, vy);
        obj.apply_matrix(PdfMatrix::new(dx * stretch, dy * stretch, -dy, dx, x, y)).map_err(err)?;
        page.objects_mut().add_text_object(obj).map_err(err)?;
        added += 1;
    }
    page.regenerate_content().map_err(err)?;
    Ok(added)
}

/// Counts the text-layer characters on each page.
pub(super) fn text_chars(doc: &PdfDocument) -> Result<Vec<usize>, String> {
    doc.pages()
        .iter()
        .map(|p| {
            let text = p.text().map_err(|e| format!("PDF error: {e}"))?.all();
            Ok(text.chars().filter(|c| !c.is_whitespace()).count())
        })
        .collect()
}

/// Opens `input` and adds an OCR text layer to its scanned pages, saving to `out`.
pub(super) fn make_searchable(input: &Path, out: &Path, progress: Progress) -> Result<(), String> {
    let err = |e: PdfiumError| format!("PDF error: {e}");
    let pdfium = pdfium()?;
    let mut doc = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    let counts = text_chars(&doc)?;
    let todo = ocr_plan(&counts, None)?;
    if todo.is_empty() {
        return Err(ALREADY_SEARCHABLE.into());
    }
    let rec = Recognizer::new()?;

    // OCR first (needs shared borrows of the pages), then write the layers.
    let mut results = Vec::with_capacity(todo.len());
    for (n, &i) in todo.iter().enumerate() {
        jobs::check()?;
        let page = doc.pages().get(i as PdfPageIndex).map_err(err)?;
        results.push((i, ocr_page(&rec, &page)?));
        progress((n + 1) as f32 / todo.len() as f32 * 0.9);
    }
    let needs_unicode = results
        .iter()
        .flat_map(|(_, r)| &r.lines)
        .any(|l| !l.text.chars().all(win_ansi));
    let font = match needs_unicode.then(|| unicode_font(&mut doc)).flatten() {
        Some(f) => f,
        None => doc.fonts_mut().helvetica(),
    };
    let mut words = 0;
    for (i, result) in &results {
        let mut page = doc.pages().get(*i as PdfPageIndex).map_err(err)?;
        words += add_text_layer(&doc, &mut page, result, font)?;
    }
    if words == 0 {
        return Err("No text was recognized in this PDF".into());
    }
    doc.save_to_file(out).map_err(err)?;
    progress(1.0);
    Ok(())
}

pub(super) fn searchable_pdf(input: &Path, progress: Progress) -> Result<PathBuf, String> {
    let out = output_path(input, "-searchable", "pdf");
    make_searchable(input, &out, progress).inspect_err(|_| {
        let _ = std::fs::remove_file(&out);
    })?;
    Ok(out)
}

/// Plain text of every page: the text layer where there is one, OCR elsewhere.
pub(super) fn pdf_plain_text(input: &Path, progress: Progress) -> Result<String, String> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(|e| format!("PDF error: {e}"))?;
    let mut texts: Vec<String> = doc
        .pages()
        .iter()
        .map(|p| p.text().map(|t| t.all()).map_err(|e| format!("PDF error: {e}")))
        .collect::<Result<_, _>>()?;
    let counts: Vec<usize> = texts.iter().map(|t| t.chars().filter(|c| !c.is_whitespace()).count()).collect();
    let rec = Recognizer::new();
    let todo = ocr_plan(&counts, rec.as_ref().err().map(String::as_str))?;
    if let (Ok(rec), false) = (&rec, todo.is_empty()) {
        for (n, &i) in todo.iter().enumerate() {
            jobs::check()?;
            let page = doc.pages().get(i as PdfPageIndex).map_err(|e| format!("PDF error: {e}"))?;
            texts[i] = ocr::plain_text(&ocr_page(rec, &page)?.lines);
            progress((n + 1) as f32 / todo.len() as f32);
        }
    }
    Ok(texts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_only_pages_without_text() {
        // A normal text PDF: nothing to OCR.
        assert_eq!(ocr_plan(&[900, 1200], None), Ok(vec![]));
        // A scan: every page.
        assert_eq!(ocr_plan(&[0, 0, 1], None), Ok(vec![0, 1, 2]));
        // Mixed: a scanned page inside a text PDF.
        assert_eq!(ocr_plan(&[800, 0, 650], None), Ok(vec![1]));
    }

    #[test]
    fn missing_ocr_only_fails_pdfs_without_any_text() {
        let no_langs = Some(ocr::NO_LANGUAGES);
        // Scans can't convert without OCR: say why.
        assert_eq!(ocr_plan(&[0, 0], no_langs), Err(ocr::NO_LANGUAGES.to_string()));
        // Text elsewhere: convert what's there instead of failing.
        assert_eq!(ocr_plan(&[800, 0], no_langs), Ok(vec![]));
        // Nothing to OCR: OCR's state doesn't matter.
        assert_eq!(ocr_plan(&[800, 700], no_langs), Ok(vec![]));
    }

    #[test]
    fn maps_displayed_points_into_rotated_pages() {
        // A 600 x 800 page (crop box at the origin).
        let crop = (0.0, 0.0, 600.0, 800.0);
        assert_eq!(to_user_space(0, crop, 10.0, 20.0), ((10.0, 780.0), (1.0, 0.0)));
        // Rotated 90° clockwise: displayed top-left is the page's bottom-left.
        assert_eq!(to_user_space(90, crop, 10.0, 20.0), ((20.0, 10.0), (0.0, 1.0)));
        assert_eq!(to_user_space(180, crop, 10.0, 20.0), ((590.0, 20.0), (-1.0, 0.0)));
        assert_eq!(to_user_space(270, crop, 10.0, 20.0), ((580.0, 790.0), (0.0, -1.0)));
        // An offset crop box shifts everything.
        assert_eq!(to_user_space(0, (50.0, 40.0, 650.0, 840.0), 0.0, 0.0).0, (50.0, 840.0));
    }

    #[test]
    fn estimates_font_size_from_letter_shapes() {
        // At 20 pt: caps/ascenders reach ~0.72 of it, with descenders ~0.92.
        assert!((word_font_size("Contents", 14.4) - 20.0).abs() < 0.5);
        assert!((word_font_size("jumping", 18.4) - 20.0).abs() < 0.5);
        assert!((word_font_size("use", 10.0) - 20.0).abs() < 0.5);
    }

    #[test]
    fn picks_a_unicode_font_only_when_needed() {
        assert!("Invoice № 12".chars().any(|c| !win_ansi(c)));
        assert!("Café “déjà vu” – 50 €".chars().all(win_ansi));
        assert!(!"Привет".chars().all(win_ansi));
    }
}

/// Real OCR on a "scanned" PDF (text rendered to a picture, then made into an
/// image-only PDF). Needs pdfium.dll, pandoc and an OCR language:
/// `cargo test -- --ignored scanned_pdf`
#[cfg(test)]
mod scan_tests {
    use super::*;
    use crate::engine::{ext_of, run_action};

    #[test]
    #[ignore]
    fn scanned_pdf_converts_through_ocr() {
        let dir = std::env::temp_dir().join(format!("ferryfile-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let markup = "#set page(paper: \"a4\")\n#set text(size: 12pt)\n\
            = Shipping Notice\n\n\
            The quick brown fox jumps over the lazy dog. Invoice number 4821 is due on the \
            first of March, and the parcel ships from Rotterdam.\n\n\
            == Contents\n\n\
            Two boxes of ceramic tiles and one crate of garden tools.\n";
        let text_pdf = dir.join("typed.pdf");
        std::fs::write(&text_pdf, crate::engine::typeset::typst_to_pdf(markup.into(), &dir).unwrap()).unwrap();

        // "Scan" it: render at 200 DPI, grey and a little noisy, then wrap the
        // picture in a PDF with no text layer.
        let pdfium = pdfium().unwrap();
        let page_img = {
            let doc = pdfium.load_pdf_from_file(&text_pdf, None).unwrap();
            let page = doc.pages().get(0).unwrap();
            let img = page.render_with_config(&PdfRenderConfig::new().scale_page_by_factor(200.0 / 72.0)).unwrap().as_image().unwrap();
            let mut gray = img.into_luma8();
            let mut seed: u32 = 7;
            for p in gray.pixels_mut() {
                seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                p.0[0] = p.0[0].saturating_sub((seed >> 27) as u8 * 3).max(25);
            }
            gray
        };
        let scan_png = dir.join("scan.png");
        page_img.save(&scan_png).unwrap();
        let scan = dir.join("scan.pdf");
        crate::engine::pdf_engine::combine_to_pdf(&[scan_png.clone()], &scan, &|_| {}).unwrap();
        assert_eq!(text_chars(&pdfium.load_pdf_from_file(&scan, None).unwrap()).unwrap(), vec![0], "scan has no text layer");

        let noop = |_: f32| {};
        let txt = std::fs::read_to_string(run_action("text:txt", &scan, &noop).unwrap()).unwrap();
        eprintln!("--- text ---\n{txt}");
        for w in ["Shipping Notice", "quick brown fox", "4821", "parcel ships", "garden tools"] {
            assert!(txt.contains(w), "text missing {w:?}");
        }
        let md = std::fs::read_to_string(run_action("text:md", &scan, &noop).unwrap()).unwrap();
        eprintln!("--- markdown ---\n{md}");
        assert!(md.contains("# Shipping Notice"), "heading lost: {md}");
        assert!(md.contains("
## Contents
"), "sub-heading lost: {md}");
        let docx = run_action("convert:docx", &scan, &noop).unwrap();
        assert_eq!(ext_of(&docx), "docx");

        // Searchable PDF: same page image, plus invisible text pdfium can find.
        let searchable = run_action("ocr:pdf", &scan, &noop).unwrap();
        let doc = pdfium.load_pdf_from_file(&searchable, None).unwrap();
        let page = doc.pages().get(0).unwrap();
        let layer = page.text().unwrap().all();
        eprintln!("--- text layer ---\n{layer}");
        assert!(layer.contains("the parcel ships") && layer.contains("The quick brown"), "{layer}");
        // Invisible: renders the same as the scan.
        let render = |p: &PdfPage| p.render_with_config(&PdfRenderConfig::new().set_target_width(500)).unwrap().as_image().unwrap().into_luma8();
        let scan_doc = pdfium.load_pdf_from_file(&scan, None).unwrap();
        let (a, b) = (render(&scan_doc.pages().get(0).unwrap()), render(&page));
        assert_eq!(a.as_raw(), b.as_raw(), "text layer changed how the page looks");
        // The word "parcel" sits where it's printed (its box overlaps the OCR'd box).
        let page_text = page.text().unwrap();
        let search = page_text.search("parcel", &PdfSearchOptions::new()).unwrap();
        let found = search.find_next().unwrap();
        let r = found.iter().next().unwrap().bounds();
        eprintln!("\"parcel\" at {r:?}");
        assert!(r.left().value > 20.0 && r.top().value < page.height().value && r.bottom().value > 0.0);
        drop(search);
        drop(page_text);
        drop(page);
        // A second pass has nothing left to do.
        assert_eq!(run_action("ocr:pdf", &searchable, &noop).unwrap_err(), ALREADY_SEARCHABLE);

        // Images: extract text, and a searchable PDF from a picture.
        let img_txt = std::fs::read_to_string(run_action("ocr:txt", &scan_png, &noop).unwrap()).unwrap();
        assert!(img_txt.contains("ceramic tiles"), "{img_txt}");
        let from_img = run_action("ocr:pdf", &scan_png, &noop).unwrap();
        let t = pdfium.load_pdf_from_file(&from_img, None).unwrap().pages().get(0).unwrap().text().unwrap().all();
        assert!(t.contains("Shipping"), "{t}");

        // Screenshot, if one is given (FERRYFILE_OCR_SCREENSHOT=path.png).
        if let Ok(shot) = std::env::var("FERRYFILE_OCR_SCREENSHOT") {
            let text = crate::engine::recognized_text(Path::new(&shot)).unwrap();
            eprintln!("--- screenshot ---\n{text}");
            assert!(!text.trim().is_empty());
        }
        drop(doc);
        drop(scan_doc);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
