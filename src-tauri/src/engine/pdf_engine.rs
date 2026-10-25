//! PDF actions backed by pdfium (pdfium.dll, shipped next to the app):
//! render pages to images, extract text, convert to DOCX, rotate/reorder/
//! compress pages, split into pages, turn images into PDFs, and combine
//! several PDFs/images into one PDF.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use image::DynamicImage;
use pdfium_render::prelude::*;

use super::pdf_docx::{pdf_markdown, pdf_to_docx};
use super::pdf_ocr::pdf_plain_text;
use super::pdf_tools;
use super::{bundled_candidates, ext_of, output_dir, output_path, Action, Engine, Progress, FOLDER_EXT};

const IMAGE_IN: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];
const RENDER_DPI: f32 = 150.0;
/// Images become pages that fit an A4 sheet (in points) in their orientation.
const A4_LONG: f32 = 842.0;
const A4_SHORT: f32 = 595.0;

pub struct PdfEngine;

/// pdfium's bindings are process-wide; bind once, then reuse them.
pub(super) fn pdfium() -> Result<Pdfium, String> {
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

/// PDF text as plain text (`txt`) or reflowed Markdown (`md`).
fn extract_text(input: &Path, fmt: &str, progress: Progress) -> Result<PathBuf, String> {
    let body = if fmt == "md" {
        let md = pdf_markdown(input, progress)?;
        if md.starts_with('#') {
            md
        } else {
            let title = input.file_stem().and_then(|s| s.to_str()).unwrap_or("Document");
            format!("# {title}\n\n{md}")
        }
    } else {
        pdf_plain_text(input, progress)?
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
pub(super) fn combine_to_pdf(inputs: &[PathBuf], out: &Path, progress: Progress) -> Result<(), String> {
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
                Action::new("convert:docx", "DOCX", "Convert"),
                Action::new("rotate:right", "Right", "Rotate"),
                Action::new("rotate:left", "Left", "Rotate"),
                Action::new("reorder:reverse", "Reverse", "Pages"),
                Action::new("drop:first", "Drop 1st", "Pages"),
                Action::new("drop:last", "Drop last", "Pages"),
                Action::new("compress:pdf", "Smaller PDF", "Compress"),
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
            ("convert", "pdf") | ("rotate" | "reorder" | "drop" | "compress", _) => Some("pdf".into()),
            ("convert", "docx") => Some("docx".into()),
            _ => None,
        }
    }

    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
        match action_id.split_once(':').ok_or("bad action id")? {
            ("pages", fmt) => render_pages(input, fmt, progress),
            ("text", fmt) => extract_text(input, fmt, progress),
            ("split", _) => split_pages(input, progress),
            ("convert", "docx") => pdf_to_docx(input, progress),
            ("rotate", "right") => pdf_tools::rotate(input, 1, progress),
            ("rotate", "left") => pdf_tools::rotate(input, -1, progress),
            ("reorder", "reverse") => pdf_tools::reverse(input, progress),
            ("drop", which @ ("first" | "last")) => pdf_tools::drop_page(input, which == "last"),
            ("compress", "pdf") => pdf_tools::compress(input, progress),
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
    fn lists_pdf_tools() {
        let pdf = Path::new("report.pdf");
        let actions = PdfEngine.actions(pdf);
        for id in [
            "convert:docx",
            "rotate:right",
            "rotate:left",
            "reorder:reverse",
            "drop:first",
            "drop:last",
            "compress:pdf",
        ] {
            assert!(actions.iter().any(|a| a.id == id), "{id} missing");
        }
        // Small groups with short labels render as segmented controls.
        for group in ["Rotate", "Pages"] {
            let labels: Vec<_> = actions.iter().filter(|a| a.group == group).map(|a| &a.label).collect();
            assert!((2..=4).contains(&labels.len()), "{group}: {labels:?}");
            assert!(labels.iter().all(|l| l.chars().count() <= 9), "{group}: {labels:?}");
        }
        assert_eq!(PdfEngine.output_ext("convert:docx", pdf).as_deref(), Some("docx"));
        for id in ["rotate:right", "rotate:left", "reorder:reverse", "drop:first", "drop:last", "compress:pdf"] {
            assert_eq!(PdfEngine.output_ext(id, pdf).as_deref(), Some("pdf"), "{id}");
        }
        assert!(PdfEngine.actions(Path::new("a.png")).iter().all(|a| a.id == "convert:pdf"));

        // Tools chain: rotate, drop a page, compress, then Word.
        let steps: Vec<String> = ["rotate:right", "drop:first", "compress:pdf", "convert:docx"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (_, ext) = crate::engine::plan_chain(pdf, &steps).unwrap();
        assert_eq!(ext, "docx");
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

        // Image-only pages have no text to reflow.
        assert_eq!(e.run("text:md", &three, &noop).unwrap_err(), super::super::pdf_docx::NO_TEXT);
        assert_eq!(e.run("convert:docx", &three, &noop).unwrap_err(), super::super::pdf_docx::NO_TEXT);

        let text = e.run("text:txt", &three, &noop).unwrap();
        assert!(text.exists());

        drop(pdfium);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A noisy photo-like PNG that doesn't compress well losslessly.
    fn noise_png(path: &Path, w: u32, h: u32) {
        let mut seed: u32 = 12345;
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let n = (seed >> 24) as u8 / 4;
            image::Rgb([(x / 7) as u8 ^ n, (y / 5) as u8 ^ n, n.wrapping_mul(3)])
        });
        img.save(path).unwrap();
    }

    fn docx_xml(docx: &Path) -> (String, Vec<String>) {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(docx).unwrap()).unwrap();
        let names: Vec<String> = zip.file_names().filter_map(|n| n.ok().map(|s| s.into_owned())).collect();
        let mut xml = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("word/document.xml").unwrap(), &mut xml).unwrap();
        (xml, names)
    }

    fn page_text(pdfium: &Pdfium, pdf: &Path, index: PdfPageIndex) -> String {
        let doc = pdfium.load_pdf_from_file(pdf, None).unwrap();
        let text = doc.pages().get(index).unwrap().text().unwrap().all();
        text
    }

    /// Needs pdfium.dll and pandoc, so opt-in: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn pdf_to_docx_and_page_tools() {
        let dir = std::env::temp_dir().join(format!("ferryfile-pdftools-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        noise_png(&dir.join("photo.png"), 1600, 1200);
        let markup = "= Quarterly Report\n\n\
            This is the first paragraph of the report. It is long enough to wrap across more \
            than one line on the page, so the reflow has to join the wrapped lines back into \
            a single paragraph again.\n\n\
            == Details\n\n\
            - first point\n\
            - second point\n\n\
            #image(\"photo.png\", width: 6cm)\n\n\
            #pagebreak()\n\n\
            Second page text ends here.\n";
        let pdf = dir.join("report.pdf");
        std::fs::write(&pdf, super::super::typeset::typst_to_pdf(markup.into(), &dir).unwrap()).unwrap();

        let e = PdfEngine;
        let noop = |_: f32| {};
        let pdfium = pdfium().unwrap();
        let pages = |p: &Path| pdfium.load_pdf_from_file(p, None).unwrap().pages().len();

        // PDF -> DOCX keeps text, structure and the image.
        let docx = e.run("convert:docx", &pdf, &noop).unwrap();
        assert_eq!(ext_of(&docx), "docx");
        let (xml, names) = docx_xml(&docx);
        for text in ["Quarterly Report", "first paragraph of the report", "a single paragraph again", "second point", "Second page text"] {
            assert!(xml.contains(text), "DOCX lost {text:?}");
        }
        assert!(xml.contains("Heading1") && xml.contains("Heading2"), "headings weren't detected");
        assert!(xml.contains("w:type=\"page\""), "page break missing");
        assert!(names.iter().any(|n| n.starts_with("word/media/")), "image missing: {names:?}");

        // Markdown uses the same structure.
        let md = std::fs::read_to_string(e.run("text:md", &pdf, &noop).unwrap()).unwrap();
        assert!(md.starts_with("# Quarterly Report\n"), "{md}");
        assert!(md.contains("\n## Details\n") && md.contains("\n- first point\n"), "{md}");

        // DOCX -> PDF through the regular engine chain (built-in Typst here).
        let back = crate::engine::run_action("convert:pdf", &docx, &noop).unwrap();
        assert!(page_text(&pdfium, &back, 0).contains("Quarterly Report"));

        // Rotate.
        let right = e.run("rotate:right", &pdf, &noop).unwrap();
        let left = e.run("rotate:left", &pdf, &noop).unwrap();
        for (out, want) in [(&right, PdfPageRenderRotation::Degrees90), (&left, PdfPageRenderRotation::Degrees270)] {
            let doc = pdfium.load_pdf_from_file(out, None).unwrap();
            assert_eq!(doc.pages().len(), 2);
            assert!(doc.pages().iter().all(|p| p.rotation().unwrap() == want));
        }

        // Reorder and drop pages.
        let reversed = e.run("reorder:reverse", &pdf, &noop).unwrap();
        assert_eq!(pages(&reversed), 2);
        assert!(page_text(&pdfium, &reversed, 0).contains("Second page text"));
        let no_first = e.run("drop:first", &pdf, &noop).unwrap();
        assert_eq!(pages(&no_first), 1);
        assert!(page_text(&pdfium, &no_first, 0).contains("Second page text"));
        let no_last = e.run("drop:last", &pdf, &noop).unwrap();
        assert!(page_text(&pdfium, &no_last, 0).contains("Quarterly Report"));
        assert!(e.run("drop:last", &no_first, &noop).is_err(), "can't drop the only page");

        // Compress: the 1600 px photo shown 6 cm wide gets downsampled.
        let compressed = e.run("compress:pdf", &pdf, &noop).unwrap();
        let (before, after) = (std::fs::metadata(&pdf).unwrap().len(), std::fs::metadata(&compressed).unwrap().len());
        eprintln!("compress: {before} -> {after} bytes");
        assert!(after * 2 < before, "{before} -> {after}");
        assert_eq!(pages(&compressed), 2);
        assert!(page_text(&pdfium, &compressed, 0).contains("Quarterly Report"));
        // ...and still looks the same.
        let render = |p: &Path| {
            let doc = pdfium.load_pdf_from_file(p, None).unwrap();
            let config = PdfRenderConfig::new().set_target_width(400);
            doc.pages().get(0).unwrap().render_with_config(&config).unwrap().as_image().unwrap().into_rgb8()
        };
        let (a, b) = (render(&pdf), render(&compressed));
        let diff: u64 = a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| x.abs_diff(*y) as u64).sum();
        let mean = diff as f64 / a.as_raw().len() as f64;
        assert!(mean < 4.0, "compressed page looks different (mean diff {mean:.1})");
        // A text-only PDF can't shrink, and says so instead of writing a copy.
        let err = e.run("compress:pdf", &no_first, &noop).unwrap_err();
        assert!(err.starts_with("Couldn't make this PDF smaller"), "{err}");

        // Nothing was overwritten: every action wrote a new file.
        assert!(std::fs::read_dir(&dir).unwrap().count() >= 11);
        drop(pdfium);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
