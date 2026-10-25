//! OCR actions: read the text in images (to a .txt, or to the clipboard via
//! [`recognized_text`]) and make searchable PDFs from scans and images.

use std::path::{Path, PathBuf};

use super::ocr::{self, Recognizer};
use super::pdf_ocr;
use super::{ext_of, output_path, Action, Engine, Progress};

const IMAGE_IN: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];
pub const NOTHING_RECOGNIZED: &str = "No text was recognized in this image";

pub struct OcrEngine;

fn is_image(path: &Path) -> bool {
    IMAGE_IN.contains(&ext_of(path).as_str())
}

/// The text in an image (OCR) or a PDF (its text layer, OCR for scans).
pub fn recognized_text(path: &Path) -> Result<String, String> {
    if ext_of(path) == "pdf" {
        let text = pdf_ocr::pdf_plain_text(path, &|_| {})?;
        return if text.trim().is_empty() { Err("No text found in this PDF".into()) } else { Ok(text) };
    }
    let img = image::open(path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let text = ocr::plain_text(&Recognizer::new()?.recognize(&img)?);
    if text.trim().is_empty() {
        Err(NOTHING_RECOGNIZED.into())
    } else {
        Ok(text)
    }
}

fn image_to_searchable_pdf(input: &Path, progress: Progress) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let tmp = std::env::temp_dir().join(format!("ferryfile-ocr-{}-{nanos}.pdf", std::process::id()));
    let out = output_path(input, "", "pdf");
    let result = super::pdf_engine::combine_to_pdf(&[input.to_path_buf()], &tmp, &|_| {})
        .and_then(|_| pdf_ocr::make_searchable(&tmp, &out, progress));
    let _ = std::fs::remove_file(&tmp);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&out);
        return Err(if e == "No text was recognized in this PDF" { NOTHING_RECOGNIZED.into() } else { e });
    }
    Ok(out)
}

impl Engine for OcrEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        if is_image(path) {
            vec![
                Action::new("ocr:txt", "Extract text", "OCR"),
                Action::new("ocr:pdf", "Searchable PDF", "OCR"),
            ]
        } else if ext_of(path) == "pdf" {
            vec![Action::new("ocr:pdf", "Searchable PDF", "OCR")]
        } else {
            Vec::new()
        }
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        match action_id {
            "ocr:txt" => Some("txt".into()),
            "ocr:pdf" => Some("pdf".into()),
            _ => None,
        }
    }

    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
        match action_id {
            "ocr:txt" => {
                let text = recognized_text(input)?;
                let out = output_path(input, "", "txt");
                std::fs::write(&out, text).map_err(|e| e.to_string())?;
                Ok(out)
            }
            "ocr:pdf" if ext_of(input) == "pdf" => pdf_ocr::searchable_pdf(input, progress),
            "ocr:pdf" => image_to_searchable_pdf(input, progress),
            _ => Err(format!("Unknown action '{action_id}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_ocr_for_images_and_pdfs() {
        let ids = |p: &str| OcrEngine.actions(Path::new(p)).into_iter().map(|a| a.id).collect::<Vec<_>>();
        assert_eq!(ids("shot.png"), ["ocr:txt", "ocr:pdf"]);
        assert_eq!(ids("scan.pdf"), ["ocr:pdf"]);
        assert!(ids("song.mp3").is_empty());
        // OCR steps chain like any other action.
        let steps = vec!["resize:50".to_string(), "ocr:txt".to_string()];
        assert_eq!(crate::engine::plan_chain(Path::new("a.png"), &steps).unwrap().1, "txt");
    }
}
