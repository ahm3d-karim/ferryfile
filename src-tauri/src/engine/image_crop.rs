//! Crop for images: a pixel rectangle picked in the Crop view
//! (`crop:x,y,w,h`), or a centred aspect ratio for recipes that should work on
//! any picture size (`crop:16x9`).
//!
//! Coordinates are in the picture as it's displayed, i.e. after the EXIF
//! orientation is applied, since that is what the preview shows.

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, ImageDecoder, ImageReader};

use super::image_engine::{flatten_on_white, open_image};
use super::{ext_of, output_path};

/// Formats the drawer's `<img>` shows directly; others get a JPEG preview.
const BROWSER_SHOWS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];
/// Longest side of generated previews.
const PREVIEW_SIDE: u32 = 1600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crop {
    /// Pixels, from the top-left corner.
    Rect { x: u32, y: u32, w: u32, h: u32 },
    /// The largest centred area with this width:height ratio.
    Aspect { w: u32, h: u32 },
}

pub fn parse_crop(arg: &str) -> Result<Crop, String> {
    let bad = || format!("'{arg}' isn't a crop like 0,0,640,480 or 16x9");
    let num = |s: &str| s.trim().parse::<u32>().map_err(|_| bad());
    if let Some((w, h)) = arg.split_once('x') {
        let (w, h) = (num(w)?, num(h)?);
        if w == 0 || h == 0 || w > 100 || h > 100 {
            return Err(bad());
        }
        return Ok(Crop::Aspect { w, h });
    }
    let parts: Vec<_> = arg.split(',').collect();
    let [x, y, w, h] = parts.as_slice() else { return Err(bad()) };
    let (x, y, w, h) = (num(x)?, num(y)?, num(w)?, num(h)?);
    if w == 0 || h == 0 {
        return Err("The crop area is empty".into());
    }
    Ok(Crop::Rect { x, y, w, h })
}

impl Crop {
    pub fn id(&self) -> String {
        match self {
            Crop::Rect { x, y, w, h } => format!("crop:{x},{y},{w},{h}"),
            Crop::Aspect { w, h } => format!("crop:{w}x{h}"),
        }
    }

    /// Step title after "Crop": `640×480 at 10, 20` or `16:9, centred`.
    pub fn label(&self) -> String {
        match self {
            Crop::Rect { x, y, w, h } => format!("{w}×{h} at {x}, {y}"),
            Crop::Aspect { w, h } => format!("{w}:{h}, centred"),
        }
    }

    /// The area to keep in a `width` x `height` picture. A saved rectangle
    /// that runs past the edge (a recipe on a smaller picture) is clipped.
    pub fn area(&self, width: u32, height: u32) -> Result<(u32, u32, u32, u32), String> {
        match *self {
            Crop::Rect { x, y, w, h } => {
                if x >= width || y >= height {
                    return Err(format!("The crop starts outside this {width}×{height} picture"));
                }
                Ok((x, y, w.min(width - x), h.min(height - y)))
            }
            Crop::Aspect { w, h } => {
                // Widest area that fits: limited by the width or the height.
                let (cw, ch) = if width as u64 * h as u64 > height as u64 * w as u64 {
                    (((height as u64 * w as u64) / h as u64).max(1) as u32, height)
                } else {
                    (width, ((width as u64 * h as u64) / w as u64).max(1) as u32)
                };
                Ok(((width - cw) / 2, (height - ch) / 2, cw, ch))
            }
        }
    }
}

/// Opens `input` turned the way it's meant to be seen.
fn open_upright(input: &Path) -> Result<DynamicImage, String> {
    let via_image_crate = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"].contains(&ext_of(input).as_str());
    if !via_image_crate {
        // HEIC/AVIF: ffmpeg already applies the stored rotation.
        return open_image(input);
    }
    let open = || -> image::ImageResult<DynamicImage> {
        let mut decoder = ImageReader::open(input)?.with_guessed_format()?.into_decoder()?;
        let orientation = decoder.orientation()?;
        let mut img = DynamicImage::from_decoder(decoder)?;
        img.apply_orientation(orientation);
        Ok(img)
    };
    open().map_err(|e| format!("Could not open image: {e}"))
}

/// Writes `img` as `ext` (JPEGs at high quality, flattened on white).
fn save_as(img: DynamicImage, ext: &str, out: &Path) -> Result<(), String> {
    if ext == "jpg" || ext == "jpeg" {
        let file = File::create(out).map_err(|e| e.to_string())?;
        JpegEncoder::new_with_quality(BufWriter::new(file), 92)
            .encode_image(&flatten_on_white(img))
            .map_err(|e| e.to_string())
    } else {
        img.save(out).map_err(|e| e.to_string())
    }
}

/// Crops `input` and writes `<stem>-cropped.<out_ext>`.
pub fn crop(input: &Path, arg: &str, out_ext: &str) -> Result<PathBuf, String> {
    let spec = parse_crop(arg)?;
    let img = open_upright(input)?;
    let (x, y, w, h) = spec.area(img.width(), img.height())?;
    let out = output_path(input, "-cropped", out_ext);
    save_as(img.crop_imm(x, y, w, h), out_ext, &out)?;
    Ok(out)
}

/// What the Crop view shows: a picture the drawer can display, and the real
/// (upright) size of `input` that crop coordinates refer to.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CropSource {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

pub fn crop_source(input: &Path) -> Result<CropSource, String> {
    let ext = ext_of(input);
    if BROWSER_SHOWS.contains(&ext.as_str()) {
        // Only the header: fast even for huge photos.
        let read = || -> image::ImageResult<(u32, u32)> {
            let mut decoder = ImageReader::open(input)?.with_guessed_format()?.into_decoder()?;
            let (w, h) = decoder.dimensions();
            use image::metadata::Orientation::*;
            Ok(match decoder.orientation()? {
                Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH => (h, w),
                _ => (w, h),
            })
        };
        let (width, height) = read().map_err(|e| format!("Could not open image: {e}"))?;
        return Ok(CropSource { path: input.to_string_lossy().into_owned(), width, height });
    }
    let img = open_upright(input)?;
    let (width, height) = (img.width(), img.height());
    let small = img.thumbnail(PREVIEW_SIDE, PREVIEW_SIDE);
    let dir = std::env::temp_dir().join("ferryfile-preview");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join("crop-preview.jpg");
    save_as(small, "jpg", &out)?;
    Ok(CropSource { path: out.to_string_lossy().into_owned(), width, height })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_crops() {
        assert_eq!(parse_crop("10,20,300,200").unwrap(), Crop::Rect { x: 10, y: 20, w: 300, h: 200 });
        assert_eq!(parse_crop("16x9").unwrap(), Crop::Aspect { w: 16, h: 9 });
        assert!(parse_crop("0,0,0,10").is_err());
        assert!(parse_crop("1,2,3").is_err());
        assert!(parse_crop("-1,0,5,5").is_err());
        assert!(parse_crop("0x9").is_err());
        assert_eq!(Crop::Rect { x: 1, y: 2, w: 3, h: 4 }.id(), "crop:1,2,3,4");
        assert_eq!(Crop::Aspect { w: 4, h: 3 }.label(), "4:3, centred");
    }

    #[test]
    fn crop_areas() {
        // Centred aspect crops in landscape and portrait pictures.
        assert_eq!(Crop::Aspect { w: 1, h: 1 }.area(400, 300).unwrap(), (50, 0, 300, 300));
        assert_eq!(Crop::Aspect { w: 16, h: 9 }.area(1000, 1000).unwrap(), (0, 219, 1000, 562));
        assert_eq!(Crop::Aspect { w: 9, h: 16 }.area(1920, 1080).unwrap(), (656, 0, 607, 1080));
        // Rectangles are clipped to the picture, or refused when outside it.
        assert_eq!(Crop::Rect { x: 100, y: 50, w: 500, h: 500 }.area(400, 300).unwrap(), (100, 50, 300, 250));
        assert!(Crop::Rect { x: 400, y: 0, w: 5, h: 5 }.area(400, 300).is_err());
    }

    #[test]
    fn crops_upright_pixels() {
        let dir = std::env::temp_dir().join(format!("ferryfile-crop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("pic.png");
        // Left half red, right half blue.
        image::RgbImage::from_fn(200, 100, |x, _| if x < 100 { image::Rgb([255, 0, 0]) } else { image::Rgb([0, 0, 255]) })
            .save(&input)
            .unwrap();
        let out = crop(&input, "120,10,50,40", "png").unwrap();
        let img = image::open(&out).unwrap().into_rgb8();
        assert_eq!(img.dimensions(), (50, 40));
        assert_eq!(img.get_pixel(0, 0).0, [0, 0, 255]);
        let src = crop_source(&input).unwrap();
        assert_eq!((src.width, src.height), (200, 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
