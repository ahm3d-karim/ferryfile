use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::DynamicImage;

use super::params::{self, CustomForm, Resize};
use super::{ext_of, image_meta, output_path, Action, Engine, Progress};

const INPUTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "heic", "heif", "avif", "svg"];
/// SVGs smaller than this (longest side, px) are scaled up when rasterised,
/// so a 24 px icon doesn't become a 24 px PNG.
const SVG_MIN_SIDE: f32 = 1024.0;
/// "Shrink to" targets for photos, in KB (decimal).
const FIT_KB: &[u64] = &[1000, 500, 200];
/// Photo formats the `image` crate can't read: decoded with ffmpeg and
/// written back as JPG, since nothing here can encode them.
const VIA_FFMPEG: &[&str] = &["heic", "heif", "avif"];
/// Formats offering "Remove location & camera info".
const STRIPPABLE: &[&str] = &["jpg", "jpeg", "png", "webp", "tif", "tiff", "heic", "heif", "avif"];
const TARGETS: &[(&str, &str)] = &[
    ("png", "PNG"),
    ("jpg", "JPG"),
    ("webp", "WebP"),
    ("gif", "GIF"),
    ("bmp", "BMP"),
    ("tiff", "TIFF"),
];

pub struct ImageEngine;

fn same_format(a: &str, b: &str) -> bool {
    let norm = |s: &str| match s {
        "jpeg" => "jpg".to_string(),
        "tif" => "tiff".to_string(),
        o => o.to_string(),
    };
    norm(a) == norm(b)
}

/// Extension for outputs that keep the input's format, e.g. resize.
fn writable_ext(input: &Path) -> String {
    let ext = ext_of(input);
    if VIA_FFMPEG.contains(&ext.as_str()) {
        "jpg".into()
    } else {
        ext
    }
}

/// Opens any supported image, going through ffmpeg for HEIC/HEIF/AVIF and
/// resvg for SVG.
fn open_image(input: &Path) -> Result<DynamicImage, String> {
    if ext_of(input) == "svg" {
        render_svg(input)
    } else if VIA_FFMPEG.contains(&ext_of(input).as_str()) {
        decode_with_ffmpeg(input)
    } else {
        image::open(input).map_err(|e| format!("Could not open image: {e}"))
    }
}

fn svg_fonts() -> std::sync::Arc<resvg::usvg::fontdb::Database> {
    static DB: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> = std::sync::OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        std::sync::Arc::new(db)
    })
    .clone()
}

/// Rasterises an SVG at its own size, scaled up if it's small (icons).
fn render_svg(input: &Path) -> Result<DynamicImage, String> {
    use resvg::{tiny_skia, usvg};
    let data = std::fs::read(input).map_err(|e| e.to_string())?;
    let mut opt = usvg::Options::default();
    opt.resources_dir = input.parent().map(Path::to_path_buf);
    opt.fontdb = svg_fonts();
    let tree = usvg::Tree::from_data(&data, &opt).map_err(|e| format!("Could not read this SVG: {e}"))?;
    let size = tree.size();
    let longest = size.width().max(size.height());
    let scale = if longest < SVG_MIN_SIDE { SVG_MIN_SIDE / longest } else { 1.0 };
    let w = (size.width() * scale).round().max(1.0) as u32;
    let h = (size.height() * scale).round().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("This SVG is too large to render")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    // tiny-skia stores premultiplied alpha; image wants straight alpha.
    let mut rgba = image::RgbaImage::new(w, h);
    for (dst, px) in rgba.pixels_mut().zip(pixmap.pixels()) {
        let c = px.demultiply();
        *dst = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Ok(DynamicImage::ImageRgba8(rgba))
}

/// SVG -> PDF through the built-in Typst typesetter, which keeps it vector
/// (sharp at any zoom, text stays text). The page is exactly the SVG's size.
fn svg_to_pdf(input: &Path) -> Result<PathBuf, String> {
    let dir = input.parent().ok_or("bad path")?;
    let name = input.file_name().and_then(|n| n.to_str()).ok_or("bad file name")?;
    let markup = format!(
        "#set page(width: auto, height: auto, margin: 0pt)\n#image({})",
        super::typeset::typst_str(name)
    );
    let pdf = super::typeset::typst_to_pdf(markup, dir)?;
    let out = output_path(input, "", "pdf");
    std::fs::write(&out, pdf).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Decodes the main picture of a HEIC/AVIF file into a temporary PNG and
/// loads it. ffmpeg applies the stored rotation; the temp file is removed.
fn decode_with_ffmpeg(input: &Path) -> Result<DynamicImage, String> {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let kind = ext_of(input).to_ascii_uppercase();
    let ffmpeg = super::find_sidecar("ffmpeg", "-version")
        .ok_or_else(|| format!("Opening {kind} photos needs ffmpeg, which wasn't found"))?;
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("ferryfile-decode-{}-{n}.png", std::process::id()));
    let result = (|| {
        let output = super::tool_command(&ffmpeg)
            .args(["-v", "error", "-nostdin", "-y", "-i"])
            .arg(input)
            .args(["-frames:v", "1", "-update", "1"])
            .arg(&tmp)
            .output()
            .map_err(|e| format!("Could not run ffmpeg: {e}"))?;
        if !output.status.success() || !tmp.is_file() {
            let err = String::from_utf8_lossy(&output.stderr);
            let detail = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("no picture found");
            return Err(format!("Could not decode this {kind} photo ({})", detail.trim()));
        }
        let img = image::open(&tmp).map_err(|e| format!("Could not read the decoded {kind} photo: {e}"))?;
        // 10-bit photos decode to 16-bit PNGs; most encoders here want 8-bit.
        Ok(if img.color().has_alpha() {
            DynamicImage::ImageRgba8(img.into_rgba8())
        } else {
            DynamicImage::ImageRgb8(img.into_rgb8())
        })
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Writes a copy of `input` without EXIF/XMP/IPTC (GPS, camera, dates).
/// JPEG, PNG and WebP keep their exact pixel data; TIFF is re-encoded
/// losslessly; HEIC/AVIF become a high-quality JPG.
fn strip_metadata(input: &Path) -> Result<PathBuf, String> {
    let ext = ext_of(input);
    let lossless = match ext.as_str() {
        "jpg" | "jpeg" => Some(image_meta::strip_jpeg as fn(&[u8]) -> Result<Vec<u8>, String>),
        "png" => Some(image_meta::strip_png as _),
        "webp" => Some(image_meta::strip_webp as _),
        _ => None,
    };
    if let Some(strip) = lossless {
        let data = std::fs::read(input).map_err(|e| e.to_string())?;
        let out = output_path(input, "-clean", &ext);
        std::fs::write(&out, strip(&data)?).map_err(|e| e.to_string())?;
        return Ok(out);
    }
    let img = open_image(input)?;
    let ext = writable_ext(input);
    let out = output_path(input, "-clean", &ext);
    if ext == "jpg" {
        let file = File::create(&out).map_err(|e| e.to_string())?;
        JpegEncoder::new_with_quality(BufWriter::new(file), 95)
            .encode_image(&img.into_rgb8())
            .map_err(|e| e.to_string())?;
    } else {
        img.save(&out).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// Places a transparent image on white (dropping alpha alone would leave
/// transparent areas whatever colour they happen to store, often black).
pub(super) fn flatten_on_white(img: DynamicImage) -> image::RgbImage {
    if !img.color().has_alpha() {
        return img.into_rgb8();
    }
    let rgba = img.into_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let mix = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32) + 127) / 255) as u8;
        image::Rgb([mix(r), mix(g), mix(b)])
    })
}

fn encode_jpeg(img: &image::RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    JpegEncoder::new_with_quality(&mut buf, quality).encode_image(img).map_err(|e| e.to_string())?;
    Ok(buf)
}

/// The best-looking JPG of `img` that fits in `target` bytes: the highest
/// quality that fits, and if even low quality is too big, a smaller picture.
fn fit_jpeg(img: DynamicImage, target: u64) -> Result<Vec<u8>, String> {
    const MIN_Q: u8 = 40;
    const MAX_Q: u8 = 92;
    let mut rgb = flatten_on_white(img);
    for _ in 0..8 {
        super::jobs::check()?;
        let floor = encode_jpeg(&rgb, MIN_Q)?;
        if floor.len() as u64 <= target {
            // Binary search the highest quality that still fits.
            let (mut lo, mut hi, mut best) = (MIN_Q, MAX_Q, floor);
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                let data = encode_jpeg(&rgb, mid)?;
                if data.len() as u64 <= target {
                    lo = mid;
                    best = data;
                } else {
                    hi = mid - 1;
                }
            }
            return Ok(best);
        }
        // File size scales roughly with pixel count: shrink both sides by
        // the square root of how far over we are, with a margin.
        let factor = ((target as f64 / floor.len() as f64).sqrt() * 0.9).clamp(0.1, 0.9);
        let (w, h) = ((rgb.width() as f64 * factor) as u32, (rgb.height() as f64 * factor) as u32);
        if w < 16 || h < 16 {
            break;
        }
        rgb = image::imageops::resize(&rgb, w, h, FilterType::Lanczos3);
    }
    Err("Couldn't make this image that small".into())
}

/// JPEG has no alpha channel, so flatten before saving.
fn prepare_for(ext: &str, img: DynamicImage) -> DynamicImage {
    if ext == "jpg" || ext == "jpeg" {
        DynamicImage::ImageRgb8(flatten_on_white(img))
    } else {
        img
    }
}

impl Engine for ImageEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        let ext = ext_of(path);
        if !INPUTS.contains(&ext.as_str()) {
            return vec![];
        }
        let mut out = Vec::new();
        for (t, label) in TARGETS {
            if !same_format(&ext, t) {
                out.push(Action::new(&format!("convert:{t}"), label, "Convert"));
            }
        }
        if ext == "svg" {
            // Resizing or compressing a drawing doesn't mean much: just convert.
            out.push(Action::new("convert:pdf", "PDF", "Convert"));
            return out;
        }
        for pct in [75, 50, 25] {
            out.push(Action::new(&format!("resize:{pct}"), &format!("{pct}%"), "Resize"));
        }
        out.push(Action::new("compress:75", "JPEG, quality 75", "Compress"));
        out.push(Action::new("compress:50", "JPEG, quality 50", "Compress"));
        for kb in FIT_KB {
            out.push(Action::new(&format!("fitkb:{kb}"), &params::kb_label(*kb), "Shrink to"));
        }
        if STRIPPABLE.contains(&ext.as_str()) {
            out.push(Action::new("strip:meta", "Remove location & camera info", "Privacy"));
        }
        out
    }

    fn custom_action(&self, action_id: &str, path: &Path) -> Option<Result<Action, String>> {
        let ext = ext_of(path);
        if !INPUTS.contains(&ext.as_str()) || ext == "svg" {
            return None;
        }
        params::image_action(action_id)
    }

    fn custom_forms(&self, path: &Path) -> Vec<CustomForm> {
        let ext = ext_of(path);
        if !INPUTS.contains(&ext.as_str()) || ext == "svg" {
            return vec![];
        }
        vec![
            CustomForm::new("Resize", "image-size"),
            CustomForm::new("Compress", "quality"),
            CustomForm::new("Shrink to", "target-size"),
        ]
    }

    fn output_ext(&self, action_id: &str, input: &Path) -> Option<String> {
        match action_id.split_once(':')? {
            ("convert", ext) => Some(ext.to_string()),
            ("resize", _) | ("strip", _) => Some(writable_ext(input)),
            ("compress" | "fitkb", _) => Some("jpg".into()),
            _ => None,
        }
    }

    fn run(&self, action_id: &str, input: &Path, _progress: Progress) -> Result<PathBuf, String> {
        let (kind, arg) = action_id.split_once(':').ok_or("bad action id")?;
        if kind == "strip" {
            return strip_metadata(input);
        }
        if action_id == "convert:pdf" && ext_of(input) == "svg" {
            return svg_to_pdf(input);
        }
        if kind == "fitkb" {
            let kb = params::target_kb(arg)?;
            let label = params::kb_label(kb);
            let target = kb * 1000;
            let size = std::fs::metadata(input).map_err(|e| e.to_string())?.len();
            if size <= target {
                return Err(format!("Already under {label} — nothing to shrink"));
            }
            let data = fit_jpeg(open_image(input)?, target)?;
            let out = output_path(input, &format!("-{}", label.replace(' ', "")), "jpg");
            std::fs::write(&out, data).map_err(|e| e.to_string())?;
            return Ok(out);
        }
        let img = open_image(input)?;
        match kind {
            "convert" => {
                let out = output_path(input, "", arg);
                prepare_for(arg, img).save(&out).map_err(|e| e.to_string())?;
                Ok(out)
            }
            "resize" => {
                let size = Resize::parse(arg)?;
                let (w, h) = size.apply(img.width(), img.height())?;
                let resized = img.resize_exact(w, h, FilterType::Lanczos3);
                let ext = writable_ext(input);
                let out = output_path(input, &size.suffix(), &ext);
                prepare_for(&ext, resized).save(&out).map_err(|e| e.to_string())?;
                Ok(out)
            }
            "compress" => {
                let quality = params::quality(arg)?;
                let out = output_path(input, "-compressed", "jpg");
                let file = File::create(&out).map_err(|e| e.to_string())?;
                let rgb = flatten_on_white(img);
                JpegEncoder::new_with_quality(BufWriter::new(file), quality)
                    .encode_image(&rgb)
                    .map_err(|e| e.to_string())?;
                Ok(out)
            }
            other => Err(format!("Unknown action kind '{other}'")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    #[test]
    fn convert_resize_compress() {
        let dir = std::env::temp_dir().join(format!("ferryfile-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("sample.png");
        RgbaImage::from_pixel(40, 20, Rgba([10, 200, 180, 128])).save(&input).unwrap();

        let engine = ImageEngine;
        let ids: Vec<String> = engine.actions(&input).into_iter().map(|a| a.id).collect();
        assert!(!ids.contains(&"convert:png".to_string()));

        let jpg = engine.run("convert:jpg", &input, &|_| {}).unwrap();
        assert_eq!(image::open(&jpg).unwrap().width(), 40);

        let half = engine.run("resize:50", &input, &|_| {}).unwrap();
        let img = image::open(&half).unwrap();
        assert_eq!((img.width(), img.height()), (20, 10));

        let small = engine.run("compress:50", &input, &|_| {}).unwrap();
        assert_eq!(ext_of(&small), "jpg");

        // A second conversion must not overwrite the first.
        let jpg2 = engine.run("convert:jpg", &input, &|_| {}).unwrap();
        assert_ne!(jpg, jpg2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferryfile-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ids(path: &str) -> Vec<String> {
        ImageEngine.actions(Path::new(path)).into_iter().map(|a| a.id).collect()
    }

    #[test]
    fn heic_and_strip_actions() {
        for f in ["a.heic", "a.HEIF", "a.avif"] {
            let ids = ids(f);
            assert!(ids.contains(&"convert:jpg".to_string()), "{f}");
            assert!(ids.contains(&"resize:50".to_string()), "{f}");
            assert!(ids.contains(&"strip:meta".to_string()), "{f}");
        }
        for f in ["a.jpg", "a.jpeg", "a.png", "a.webp", "a.tif", "a.tiff"] {
            assert!(ids(f).contains(&"strip:meta".to_string()), "{f}");
        }
        for f in ["a.gif", "a.bmp"] {
            assert!(!ids(f).contains(&"strip:meta".to_string()), "{f}");
        }

        let e = ImageEngine;
        let ext = |a: &str, f: &str| e.output_ext(a, Path::new(f)).unwrap();
        assert_eq!(ext("strip:meta", "a.png"), "png");
        assert_eq!(ext("strip:meta", "a.jpeg"), "jpeg");
        assert_eq!(ext("strip:meta", "a.heic"), "jpg");
        assert_eq!(ext("resize:50", "a.avif"), "jpg");
        assert_eq!(ext("convert:png", "a.heic"), "png");

        // Recipes can chain the new step.
        let steps = ["strip:meta".to_string(), "convert:webp".to_string()];
        let (_, out) = super::super::plan_chain(Path::new("x.heic"), &steps).unwrap();
        assert_eq!(out, "webp");
    }

    #[test]
    fn strips_exif_from_jpeg_losslessly() {
        let dir = temp_dir("strip");
        let mut plain = Vec::new();
        image::RgbImage::from_fn(32, 16, |x, y| image::Rgb([x as u8 * 8, y as u8 * 16, 90]))
            .write_to(&mut std::io::Cursor::new(&mut plain), image::ImageFormat::Jpeg)
            .unwrap();
        // SOI, then the EXIF APP1 (orientation 6 + GPS pointer), then the rest.
        let app1 = super::super::image_meta::tests::exif_app1();
        let mut tagged = plain[..2].to_vec();
        tagged.extend_from_slice(&app1);
        tagged.extend_from_slice(b"\xFF\xFE\x00\x07hello");
        tagged.extend_from_slice(&plain[2..]);
        let input = dir.join("photo.jpg");
        std::fs::write(&input, &tagged).unwrap();

        let out = ImageEngine.run("strip:meta", &input, &|_| {}).unwrap();
        assert_eq!(out.file_name().unwrap(), "photo-clean.jpg");
        let clean = std::fs::read(&out).unwrap();
        // GPS pointer and comment are gone; only a one-tag orientation block remains.
        assert!(!clean.windows(2).any(|w| w == [0x25, 0x88] || w == [0x88, 0x25]));
        assert!(!clean.windows(5).any(|w| w == b"hello"));
        assert!(clean.len() < tagged.len());
        let reader = image::ImageReader::open(&out).unwrap();
        let mut decoder = reader.into_decoder().unwrap();
        use image::ImageDecoder;
        assert_eq!(decoder.orientation().unwrap(), image::metadata::Orientation::Rotate90);
        // Pixels are untouched.
        let a = image::load_from_memory(&tagged).unwrap().into_rgb8();
        let b = image::open(&out).unwrap().into_rgb8();
        assert_eq!(a, b);

        // Without an orientation tag, no EXIF is written at all.
        let input2 = dir.join("plain.jpg");
        let mut no_orient = plain[..2].to_vec();
        let mut seg = app1.clone();
        seg[20] = 0x13; // retag 0x0112 -> 0x0113
        no_orient.extend_from_slice(&seg);
        no_orient.extend_from_slice(&plain[2..]);
        std::fs::write(&input2, &no_orient).unwrap();
        let out2 = ImageEngine.run("strip:meta", &input2, &|_| {}).unwrap();
        let clean2 = std::fs::read(&out2).unwrap();
        assert!(!clean2.windows(4).any(|w| w == b"Exif"));
        assert_eq!(clean2, plain);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shrinks_photo_to_target() {
        let dir = temp_dir("photo-fit");
        // Noisy 1600x1200 picture: a big PNG that also stays big as JPEG.
        let mut seed = 7u32;
        let img = image::RgbImage::from_fn(1600, 1200, |x, y| {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            image::Rgb([(x / 7) as u8, (y / 5) as u8, (seed >> 16) as u8])
        });
        let input = dir.join("photo.png");
        img.save(&input).unwrap();
        assert!(std::fs::metadata(&input).unwrap().len() > 1_000_000);

        for (kb, name) in [(500, "photo-500KB.jpg"), (200, "photo-200KB.jpg")] {
            let out = ImageEngine.run(&format!("fitkb:{kb}"), &input, &|_| {}).unwrap();
            assert_eq!(out.file_name().unwrap(), name);
            let len = std::fs::metadata(&out).unwrap().len();
            assert!(len <= kb * 1000, "{len} > {kb} KB");
            assert!(len > kb * 1000 / 3, "{len}: used too little of {kb} KB");
        }
        // Already small enough: says so, writes nothing.
        let small = dir.join("photo-200KB.jpg");
        let err = ImageEngine.run("fitkb:500", &small, &|_| {}).unwrap_err();
        assert_eq!(err, "Already under 500 KB — nothing to shrink");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn svg_to_raster_and_pdf() {
        let dir = temp_dir("svg");
        let input = dir.join("icon.svg");
        std::fs::write(
            &input,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16" viewBox="0 0 32 16">
                <rect x="0" y="0" width="16" height="16" fill="#ff0000"/>
                <text x="18" y="12" font-size="10">Hi</text></svg>"##,
        )
        .unwrap();

        let ids = ids("a.svg");
        assert!(ids.contains(&"convert:png".to_string()) && ids.contains(&"convert:pdf".to_string()));
        assert!(!ids.iter().any(|i| i.starts_with("resize") || i.starts_with("compress")));

        // Small SVGs are scaled up so the longest side is 1024 px.
        let png = ImageEngine.run("convert:png", &input, &|_| {}).unwrap();
        let img = image::open(&png).unwrap().into_rgba8();
        assert_eq!((img.width(), img.height()), (1024, 512));
        assert_eq!(img.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(1000, 500).0[3], 0); // background stays transparent

        // JPG puts the transparent background on white, not black.
        let jpg = ImageEngine.run("convert:jpg", &input, &|_| {}).unwrap();
        let px = image::open(&jpg).unwrap().into_rgb8().get_pixel(1000, 500).0;
        assert!(px.iter().all(|&c| c > 240), "{px:?}");

        let pdf = ImageEngine.run("convert:pdf", &input, &|_| {}).unwrap();
        assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[ignore = "needs the bundled ffmpeg"]
    fn converts_avif_via_ffmpeg() {
        let ffmpeg = super::super::find_sidecar("ffmpeg", "-version").expect("ffmpeg");
        let dir = temp_dir("avif");
        let input = dir.join("photo.avif");
        // No HEIF muxer in ffmpeg, so AVIF stands in for both (same decode path).
        let status = super::super::tool_command(&ffmpeg)
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=64x48", "-frames:v", "1"])
            .args(["-c:v", "libaom-av1", "-still-picture", "1"])
            .arg(&input)
            .status()
            .unwrap();
        assert!(status.success() && input.is_file(), "ffmpeg couldn't encode AVIF");

        let e = ImageEngine;
        let png = e.run("convert:png", &input, &|_| {}).unwrap();
        let img = image::open(&png).unwrap();
        assert_eq!((img.width(), img.height()), (64, 48));

        let half = e.run("resize:50", &input, &|_| {}).unwrap();
        assert_eq!(ext_of(&half), "jpg");
        assert_eq!(image::open(&half).unwrap().width(), 32);

        let clean = e.run("strip:meta", &input, &|_| {}).unwrap();
        assert_eq!(clean.file_name().unwrap(), "photo-clean.jpg");

        // Garbage gets a clear error, and no temp files are left behind.
        let bad = dir.join("broken.heic");
        std::fs::write(&bad, b"not a photo").unwrap();
        let err = e.run("convert:png", &bad, &|_| {}).unwrap_err();
        assert!(err.starts_with("Could not decode this HEIC photo"), "{err}");
        let leftovers = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter(|f| {
                let name = f.as_ref().unwrap().file_name().to_string_lossy().into_owned();
                name.starts_with(&format!("ferryfile-decode-{}-", std::process::id()))
            })
            .count();
        assert_eq!(leftovers, 0);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
