//! PDF actions backed by pdfium (pdfium.dll, shipped next to the app):
//! render pages to images, extract text, split into pages, turn images into
//! PDFs, and combine several PDFs/images into one PDF.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use image::DynamicImage;
use pdfium_render::prelude::*;

use super::{bundled_candidates, ext_of, output_dir, output_path, Action, Engine, Progress, FOLDER_EXT};

const IMAGE_IN: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];
const RENDER_DPI: f32 = 150.0;
/// Images become pages that fit an A4 sheet (in points) in their orientation.
const A4_LONG: f32 = 842.0;
const A4_SHORT: f32 = 595.0;

pub struct PdfEngine;

/// pdfium's bindings are process-wide; bind once, then reuse them.
fn pdfium() -> Result<Pdfium, String> {
    static BOUND: OnceLock<Result<(), String>> = OnceLock::new();
    BOUND
        .get_or_init(|| {
            for path in bundled_candidates("pdfium.dll", "pdfium.dll") {
                if let Ok(bindings) = Pdfium::bind_to_library(&path) {
                    let _ = Pdfium::new(bindings);
                    return Ok(());
                }
            }
            Err("pdfium.dll wasn't found. Run scripts/fetch-doc-tools.ps1.".into())
        })
        .clone()?;
    // With bindings in place, `default()` reuses them instead of loading again.
    Ok(Pdfium::default())
}

fn is_image(path: &Path) -> bool {
    IMAGE_IN.contains(&ext_of(path).as_str())
}

fn err(e: PdfiumError) -> String {
    format!("PDF error: {e}")
}

/// Appends `img` as a page sized to fit A4 in the image's orientation.
fn add_image_page(doc: &mut PdfDocument, img: &DynamicImage) -> Result<(), String> {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (long, short) = if w >= h { (w, h) } else { (h, w) };
    // Never enlarge past 96 DPI (0.75 pt per pixel).
    let scale = (A4_LONG / long).min(A4_SHORT / short).min(0.75);
    let (pw, ph) = (PdfPoints::new(w * scale), PdfPoints::new(h * scale));
    let mut page = doc
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::from_points(pw, ph))
        .map_err(err)?;
    page.objects_mut()
        .create_image_object(PdfPoints::ZERO, PdfPoints::ZERO, img, Some(pw), Some(ph))
        .map_err(err)?;
    Ok(())
}

fn open_image(path: &Path) -> Result<DynamicImage, String> {
    image::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))
}

fn render_pages(input: &Path, fmt: &str, progress: Progress) -> Result<PathBuf, String> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    let pages = doc.pages();
    let count = pages.len();
    let dir = output_dir(input, "-pages")?;
    let config = PdfRenderConfig::new().scale_page_by_factor(RENDER_DPI / 72.0);
    for (i, page) in pages.iter().enumerate() {
        let img = page.render_with_config(&config).map_err(err)?.as_image().map_err(err)?;
        let img = if fmt == "jpg" { DynamicImage::ImageRgb8(img.into_rgb8()) } else { img };
        img.save(dir.join(format!("page-{:03}.{fmt}", i + 1)))
            .map_err(|e| e.to_string())?;
        progress((i + 1) as f32 / count as f32);
    }
    Ok(dir)
}

/// Reflows one page of extracted text into Markdown: wrapped lines are joined
/// into paragraphs, bullet glyphs become list items.
fn page_to_markdown(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let widest = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    let mut para = String::new();
    let flush = |para: &mut String, out: &mut String| {
        if !para.is_empty() {
            out.push_str(para);
            out.push_str("\n\n");
            para.clear();
        }
    };
    for line in lines {
        if line.is_empty() {
            flush(&mut para, &mut out);
            continue;
        }
        if let Some(item) = line.strip_prefix(['•', '●', '▪', '◦', '–']) {
            flush(&mut para, &mut out);
            para.push_str("- ");
            para.push_str(item.trim_start());
        } else {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(line);
        }
        // A short line ending a sentence is the last line of its paragraph.
        let short = line.chars().count() * 10 < widest * 8;
        if short && line.ends_with(['.', '!', '?', ':']) {
            flush(&mut para, &mut out);
        }
    }
    flush(&mut para, &mut out);
    out.trim_end().to_string()
}

/// PDF text as plain text (`txt`) or reflowed Markdown (`md`).
fn extract_text(input: &Path, fmt: &str) -> Result<PathBuf, String> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    let mut pages = Vec::new();
    for page in doc.pages().iter() {
        pages.push(page.text().map_err(err)?.all());
    }
    let body = if fmt == "md" {
        let title = input.file_stem().and_then(|s| s.to_str()).unwrap_or("Document");
        let pages: Vec<_> = pages.iter().map(|p| page_to_markdown(p)).collect();
        format!("# {title}\n\n{}\n", pages.join("\n\n---\n\n"))
    } else {
        pages.join("\n\n")
    };
    let out = output_path(input, "", fmt);
    std::fs::write(&out, body.trim_end()).map_err(|e| e.to_string())?;
    Ok(out)
}

fn split_pages(input: &Path, progress: Progress) -> Result<PathBuf, String> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    let count = doc.pages().len();
    let dir = output_dir(input, "-pages")?;
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("page");
    for i in 0..count {
        let mut single = pdfium.create_new_pdf().map_err(err)?;
        single.pages_mut().copy_page_from_document(&doc, i, 0).map_err(err)?;
        single
            .save_to_file(&dir.join(format!("{stem}-p{:03}.pdf", i + 1)))
            .map_err(err)?;
        progress((i + 1) as f32 / count as f32);
    }
    Ok(dir)
}

/// One PDF from PDFs and/or images, in the given order.
fn combine_to_pdf(inputs: &[PathBuf], out: &Path, progress: Progress) -> Result<(), String> {
    let pdfium = pdfium()?;
    let mut doc = pdfium.create_new_pdf().map_err(err)?;
    for (i, input) in inputs.iter().enumerate() {
        if is_image(input) {
            add_image_page(&mut doc, &open_image(input)?)?;
        } else {
            let src = pdfium.load_pdf_from_file(input, None).map_err(err)?;
            doc.pages_mut().append(&src).map_err(err)?;
        }
        progress((i + 1) as f32 / inputs.len() as f32);
    }
    doc.save_to_file(out).map_err(err)
}

impl Engine for PdfEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        if ext_of(path) == "pdf" {
            vec![
                Action::new("pages:png", "PNG pages", "Convert"),
                Action::new("pages:jpg", "JPG pages", "Convert"),
                Action::new("text:txt", "Text", "Convert"),
                Action::new("text:md", "Markdown", "Convert"),
                Action::new("split:pages", "One PDF per page", "Split"),
            ]
        } else if is_image(path) {
            vec![Action::new("convert:pdf", "PDF", "Convert")]
        } else {
            Vec::new()
        }
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        match action_id.split_once(':')? {
            ("pages" | "split", _) => Some(FOLDER_EXT.into()),
            ("text", fmt) => Some(fmt.to_string()),
            ("convert", "pdf") => Some("pdf".into()),
            _ => None,
        }
    }

    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
        match action_id.split_once(':').ok_or("bad action id")? {
            ("pages", fmt) => render_pages(input, fmt, progress),
            ("text", fmt) => extract_text(input, fmt),
            ("split", _) => split_pages(input, progress),
            ("convert", "pdf") => {
                let out = output_path(input, "", "pdf");
                combine_to_pdf(&[input.to_path_buf()], &out, progress)?;
                Ok(out)
            }
            (kind, _) => Err(format!("Unknown action kind '{kind}'")),
        }
    }

    fn combine_actions(&self, inputs: &[PathBuf]) -> Vec<Action> {
        let all_fit = inputs.iter().all(|p| ext_of(p) == "pdf" || is_image(p));
        if inputs.len() >= 2 && all_fit {
            vec![Action::new("combine:pdf", "One PDF", "Combine")]
        } else {
            Vec::new()
        }
    }

    fn combine(&self, action_id: &str, inputs: &[PathBuf], progress: Progress) -> Result<PathBuf, String> {
        if action_id != "combine:pdf" {
            return Err(format!("Unknown action '{action_id}'"));
        }
        let out = output_path(&inputs[0], "-combined", "pdf");
        combine_to_pdf(inputs, &out, progress)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflows_markdown() {
        let page = "Introduction to the topic that wraps across\n\
                    several lines of the page.\n\
                    • first point\n\
                    • second point\n\
                    \n\
                    Next paragraph.";
        assert_eq!(
            page_to_markdown(page),
            "Introduction to the topic that wraps across several lines of the page.\n\n\
             - first point\n\n- second point\n\nNext paragraph."
        );
    }

    /// Needs pdfium.dll, so opt-in: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn pdf_round_trip() {
        let dir = std::env::temp_dir().join(format!("ferryfile-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.png");
        let b = dir.join("b.jpg");
        image::RgbaImage::from_pixel(800, 600, image::Rgba([200, 30, 30, 255])).save(&a).unwrap();
        image::RgbImage::from_pixel(300, 900, image::Rgb([30, 30, 200])).save(&b).unwrap();

        let e = PdfEngine;
        let noop = |_: f32| {};
        let combined = e.combine("combine:pdf", &[a.clone(), b.clone()], &noop).unwrap();
        let single = e.run("convert:pdf", &a, &noop).unwrap();
        // PDF + image -> 3 pages.
        let three = e.combine("combine:pdf", &[combined.clone(), single.clone()], &noop).unwrap();

        let pdfium = pdfium().unwrap();
        assert_eq!(pdfium.load_pdf_from_file(&three, None).unwrap().pages().len(), 3);

        let pages = e.run("pages:png", &three, &noop).unwrap();
        assert_eq!(std::fs::read_dir(&pages).unwrap().count(), 3);
        let first = image::open(pages.join("page-001.png")).unwrap();
        assert!(first.width() > first.height(), "landscape image stays landscape");

        let split = e.run("split:pages", &three, &noop).unwrap();
        assert_eq!(std::fs::read_dir(&split).unwrap().count(), 3);

        let md = e.run("text:md", &three, &noop).unwrap();
        assert!(std::fs::read_to_string(&md).unwrap().starts_with("# a-combined"));

        let text = e.run("text:txt", &three, &noop).unwrap();
        assert!(text.exists());

        drop(pdfium);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
