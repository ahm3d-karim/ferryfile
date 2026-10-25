//! Page tools on an existing PDF: rotate, reverse, drop the first/last page,
//! split or extract page ranges, and compress by re-encoding oversized
//! images. Each writes a new file.
//!
//! Passwords aren't offered: pdfium-render 0.9.4 always saves with flags 0, so
//! it can neither write an encrypted PDF nor strip security on save.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::DynamicImage;
use pdfium_render::prelude::*;

use super::{output_dir, output_path, ranges};
use super::pdf_engine::pdfium;
use super::Progress;

/// Images are downsampled to this resolution at their size on the page.
const COMPRESS_DPI: f32 = 150.0;
/// Only images at least this much sharper than `COMPRESS_DPI` are touched.
const COMPRESS_OVER: f32 = 1.25;
const JPEG_QUALITY: u8 = 70;

fn err(e: PdfiumError) -> String {
    format!("PDF error: {e}")
}

fn turned(r: PdfPageRenderRotation, quarter_turns: i32) -> PdfPageRenderRotation {
    let current = match r {
        PdfPageRenderRotation::None => 0,
        PdfPageRenderRotation::Degrees90 => 1,
        PdfPageRenderRotation::Degrees180 => 2,
        PdfPageRenderRotation::Degrees270 => 3,
    };
    match (current + quarter_turns).rem_euclid(4) {
        0 => PdfPageRenderRotation::None,
        1 => PdfPageRenderRotation::Degrees90,
        2 => PdfPageRenderRotation::Degrees180,
        _ => PdfPageRenderRotation::Degrees270,
    }
}

/// Rotates every page clockwise by `quarter_turns` x 90 degrees.
pub(super) fn rotate(input: &Path, quarter_turns: i32, progress: Progress) -> Result<PathBuf, String> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    let count = doc.pages().len();
    for (i, mut page) in doc.pages().iter().enumerate() {
        let r = page.rotation().unwrap_or(PdfPageRenderRotation::None);
        page.set_rotation(turned(r, quarter_turns));
        progress((i + 1) as f32 / count as f32);
    }
    let suffix = if quarter_turns > 0 { "-rotated-right" } else { "-rotated-left" };
    save(&doc, input, suffix)
}

pub(super) fn reverse(input: &Path, progress: Progress) -> Result<PathBuf, String> {
    let pdfium = pdfium()?;
    let src = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    let count = src.pages().len();
    let mut doc = pdfium.create_new_pdf().map_err(err)?;
    for (n, i) in (0..count).rev().enumerate() {
        doc.pages_mut().copy_page_from_document(&src, i, n as PdfPageIndex).map_err(err)?;
        progress((n + 1) as f32 / count as f32);
    }
    save(&doc, input, "-reversed")
}

/// Removes the first (`last == false`) or last page.
pub(super) fn drop_page(input: &Path, last: bool) -> Result<PathBuf, String> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    let count = doc.pages().len();
    if count < 2 {
        return Err("This PDF has only one page".into());
    }
    let index = if last { count - 1 } else { 0 };
    doc.pages().get(index).map_err(err)?.delete().map_err(err)?;
    save(&doc, input, if last { "-no-last-page" } else { "-no-first-page" })
}

/// Number of pages in `input`.
pub(crate) fn page_count(input: &Path) -> Result<u32, String> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    Ok(doc.pages().len() as u32)
}

/// Copies 1-based pages `range` of `src` onto the end of `doc`.
fn copy_range(doc: &mut PdfDocument, src: &PdfDocument, range: &std::ops::RangeInclusive<u32>) -> Result<(), String> {
    for page in range.clone() {
        let at = doc.pages().len();
        doc.pages_mut()
            .copy_page_from_document(src, (page - 1) as PdfPageIndex, at)
            .map_err(err)?;
    }
    Ok(())
}

/// One PDF holding the pages in `spec` (e.g. `1-3,5,8-`), in that order.
pub(super) fn extract_pages(input: &Path, spec: &str, progress: Progress) -> Result<PathBuf, String> {
    let pdfium = pdfium()?;
    let src = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    let wanted = ranges::resolve(spec, src.pages().len() as u32)?;
    let mut doc = pdfium.create_new_pdf().map_err(err)?;
    for (i, range) in wanted.iter().enumerate() {
        copy_range(&mut doc, &src, range)?;
        progress((i + 1) as f32 / wanted.len() as f32);
    }
    save(&doc, input, "-extract")
}

/// A folder with one PDF per range in `spec`, named after the pages it holds.
pub(super) fn split_ranges(input: &Path, spec: &str, progress: Progress) -> Result<PathBuf, String> {
    let pdfium = pdfium()?;
    let src = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    let wanted = ranges::resolve(spec, src.pages().len() as u32)?;
    let dir = output_dir(input, "-split")?;
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("pages");
    let result = (|| {
        for (i, range) in wanted.iter().enumerate() {
            let mut part = pdfium.create_new_pdf().map_err(err)?;
            copy_range(&mut part, &src, range)?;
            let name = if range.start() == range.end() {
                format!("{stem}-p{}.pdf", range.start())
            } else {
                format!("{stem}-p{}-{}.pdf", range.start(), range.end())
            };
            part.save_to_file(&super::unique_in(&dir, &name)).map_err(err)?;
            progress((i + 1) as f32 / wanted.len() as f32);
        }
        Ok(dir.clone())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

fn save(doc: &PdfDocument, input: &Path, suffix: &str) -> Result<PathBuf, String> {
    let out = output_path(input, suffix, "pdf");
    doc.save_to_file(&out).map_err(err)?;
    Ok(out)
}

fn has_transparency(image: &DynamicImage) -> bool {
    image.color().has_alpha() && image.to_rgba8().pixels().any(|p| p.0[3] < 255)
}

/// A downsampled JPEG of `image` if it is opaque and oversized for `width_pt`
/// x `height_pt` on the page; `None` when it should stay as it is.
fn shrunk_jpeg(image: &DynamicImage, width_pt: f32, height_pt: f32) -> Option<Vec<u8>> {
    let target_w = (width_pt / 72.0 * COMPRESS_DPI).round().max(1.0) as u32;
    let target_h = (height_pt / 72.0 * COMPRESS_DPI).round().max(1.0) as u32;
    if (image.width() as f32) < target_w as f32 * COMPRESS_OVER
        && (image.height() as f32) < target_h as f32 * COMPRESS_OVER
    {
        return None;
    }
    // JPEG has no alpha; leave transparent images alone.
    if has_transparency(image) {
        return None;
    }
    let small = image
        .resize(target_w, target_h, image::imageops::FilterType::Triangle)
        .into_rgb8();
    let mut bytes = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, JPEG_QUALITY);
    small.write_with_encoder(encoder).ok()?;
    Some(bytes)
}

/// Re-encodes images that are sharper than `COMPRESS_DPI` as JPEGs. Fails
/// (writing nothing) if that doesn't make the file smaller.
pub(super) fn compress(input: &Path, progress: Progress) -> Result<PathBuf, String> {
    let pdfium = pdfium()?;
    let doc = pdfium.load_pdf_from_file(input, None).map_err(err)?;
    let count = doc.pages().len();
    let mut replaced = 0;
    for (pi, mut page) in doc.pages().iter().enumerate() {
        let images: Vec<_> = page
            .objects()
            .iter()
            .enumerate()
            .filter(|(_, o)| o.object_type() == PdfPageObjectType::Image)
            .map(|(i, _)| i as PdfPageObjectIndex)
            .collect();
        // Back to front so earlier indices stay valid.
        for index in images.into_iter().rev() {
            let Ok(old) = page.objects().get(index) else { continue };
            let Ok(rect) = old.bounds().map(|b| b.to_rect()) else { continue };
            let Some(object) = old.as_image_object() else { continue };
            // The raw image lacks any soft mask; the rendered one shows it.
            if object.get_processed_image(&doc).map_or(true, |p| has_transparency(&p)) {
                continue;
            }
            let Ok(image) = object.get_raw_image() else { continue };
            let Some(jpeg) = shrunk_jpeg(&image, rect.width().value, rect.height().value) else {
                continue;
            };
            let Ok(mut new) = PdfPageImageObject::new_from_jpeg_reader(&doc, Cursor::new(jpeg)) else {
                continue;
            };
            if new.transform_from(&old).is_err() {
                continue;
            }
            drop(old);
            let objects = page.objects_mut();
            if objects.insert_object_at_index(index, PdfPageObject::Image(new)).is_ok() {
                let _ = objects.remove_object_at_index(index + 1);
                replaced += 1;
            }
        }
        progress((pi + 1) as f32 * 0.9 / count as f32);
    }
    let nothing = "Couldn't make this PDF smaller: it has no oversized images";
    if replaced == 0 {
        return Err(nothing.into());
    }
    // pdfium leaves the replaced image streams out when saving.
    let bytes = doc.save_to_bytes().map_err(err)?;
    let before = std::fs::metadata(input).map_err(|e| e.to_string())?.len();
    if bytes.len() as u64 >= before {
        return Err(nothing.into());
    }
    let out = output_path(input, "-compressed", "pdf");
    std::fs::write(&out, bytes).map_err(|e| e.to_string())?;
    progress(1.0);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_wraps() {
        assert_eq!(turned(PdfPageRenderRotation::None, 1), PdfPageRenderRotation::Degrees90);
        assert_eq!(turned(PdfPageRenderRotation::None, -1), PdfPageRenderRotation::Degrees270);
        assert_eq!(turned(PdfPageRenderRotation::Degrees270, 1), PdfPageRenderRotation::None);
    }

    #[test]
    fn only_shrinks_oversized_opaque_images() {
        let big = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(1200, 1200, image::Rgb([9, 99, 199])));
        // 1200 px over 2 inches = 600 DPI -> downsampled.
        assert!(shrunk_jpeg(&big, 144.0, 144.0).is_some());
        // 1200 px over 8 inches = 150 DPI -> left alone.
        assert!(shrunk_jpeg(&big, 576.0, 576.0).is_none());
        let clear = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(1200, 1200, image::Rgba([0, 0, 0, 0])));
        assert!(shrunk_jpeg(&clear, 144.0, 144.0).is_none());
    }
}
