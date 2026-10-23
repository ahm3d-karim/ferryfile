//! PDF -> DOCX: rebuild the text structure with `pdf_layout`, save embedded
//! images next to it, and let the bundled pandoc write the Word file. The
//! result is reflowed (headings, paragraphs, lists, images, page breaks), not a
//! copy of the original layout.

use std::path::{Path, PathBuf};

use pdfium_render::prelude::*;

use super::doc_engine::pandoc;
use super::pdf_engine::pdfium;
use super::pdf_layout::{blocks, page_lines, page_pictures, to_markdown, Page};
use super::{output_path, tool_command, Progress};

/// A hard page break in pandoc's DOCX writer.
const DOCX_PAGE_BREAK: &str = "```{=openxml}\n<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>\n```";

/// Fewer visible characters per page than this means there's no text layer.
const MIN_CHARS_PER_PAGE: usize = 3;

pub(super) const NO_TEXT: &str = "No text found: this looks like a scanned PDF";

/// Reads every page's lines (and, with `media`, its images).
pub(super) fn read_pages(doc: &PdfDocument, media: Option<&Path>, progress: Progress) -> Result<Vec<Page>, String> {
    let count = doc.pages().len() as usize;
    let mut pages = Vec::with_capacity(count);
    for (i, page) in doc.pages().iter().enumerate() {
        let lines = page_lines(&page)?;
        let pictures = media.map(|m| page_pictures(doc, &page, i, m)).unwrap_or_default();
        pages.push(Page { lines, pictures });
        progress((i + 1) as f32 / count.max(1) as f32);
    }
    let chars: usize = pages.iter().flat_map(|p| &p.lines).map(|l| l.text.chars().count()).sum();
    if chars < MIN_CHARS_PER_PAGE * count.max(1) {
        return Err(NO_TEXT.into());
    }
    Ok(pages)
}

/// Reflowed Markdown for the whole PDF (no images), used by "Markdown".
pub(super) fn pdf_markdown(input: &Path) -> Result<String, String> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(|e| format!("PDF error: {e}"))?;
    let pages = read_pages(&doc, None, &|_| {})?;
    Ok(to_markdown(&blocks(&pages), None))
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> Result<TempDir, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("ferryfile-pdf2docx-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(TempDir(dir))
}

pub(super) fn pdf_to_docx(input: &Path, progress: Progress) -> Result<PathBuf, String> {
    let exe = pandoc()?;
    let work = temp_dir()?;
    let markdown = {
        let pdfium = pdfium()?;
        let doc = pdfium.load_pdf_from_file(input, None).map_err(|e| format!("PDF error: {e}"))?;
        let pages = read_pages(&doc, Some(&work.0), &|f| progress(f * 0.8))?;
        to_markdown(&blocks(&pages), Some(DOCX_PAGE_BREAK))
    };
    let md = work.0.join("document.md");
    std::fs::write(&md, markdown).map_err(|e| e.to_string())?;

    let out = std::path::absolute(output_path(input, "", "docx")).map_err(|e| e.to_string())?;
    // Run inside the work folder so image paths in the Markdown resolve.
    let result = tool_command(exe)
        .current_dir(&work.0)
        .args(["--from", "markdown-auto_identifiers", "--to", "docx", "--output"])
        .arg(&out)
        .arg("document.md")
        .output()
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        let _ = std::fs::remove_file(&out);
        let msg = String::from_utf8_lossy(&result.stderr);
        return Err(format!("pandoc failed: {}", msg.lines().last().unwrap_or("unknown error")));
    }
    progress(1.0);
    Ok(out)
}
