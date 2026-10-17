use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::DynamicImage;

use super::{ext_of, output_path, Action, Engine};

const INPUTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"];
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

/// JPEG has no alpha channel, so flatten before saving.
fn prepare_for(ext: &str, img: DynamicImage) -> DynamicImage {
    if ext == "jpg" || ext == "jpeg" {
        DynamicImage::ImageRgb8(img.into_rgb8())
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
        for pct in [75, 50, 25] {
            out.push(Action::new(&format!("resize:{pct}"), &format!("{pct}%"), "Resize"));
        }
        out.push(Action::new("compress:75", "JPEG, quality 75", "Compress"));
        out.push(Action::new("compress:50", "JPEG, quality 50", "Compress"));
        out
    }

    fn run(&self, action_id: &str, input: &Path) -> Result<PathBuf, String> {
        let (kind, arg) = action_id.split_once(':').ok_or("bad action id")?;
        let img = image::open(input).map_err(|e| format!("Could not open image: {e}"))?;
        match kind {
            "convert" => {
                let out = output_path(input, "", arg);
                prepare_for(arg, img).save(&out).map_err(|e| e.to_string())?;
                Ok(out)
            }
            "resize" => {
                let pct: u32 = arg.parse().map_err(|_| "bad percentage")?;
                let w = (img.width() * pct / 100).max(1);
                let h = (img.height() * pct / 100).max(1);
                let resized = img.resize_exact(w, h, FilterType::Lanczos3);
                let ext = ext_of(input);
                let out = output_path(input, &format!("-{pct}pct"), &ext);
                prepare_for(&ext, resized).save(&out).map_err(|e| e.to_string())?;
                Ok(out)
            }
            "compress" => {
                let quality: u8 = arg.parse().map_err(|_| "bad quality")?;
                let out = output_path(input, "-compressed", "jpg");
                let file = File::create(&out).map_err(|e| e.to_string())?;
                let rgb = img.into_rgb8();
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
        let dir = std::env::temp_dir().join(format!("skiff-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("sample.png");
        RgbaImage::from_pixel(40, 20, Rgba([10, 200, 180, 128])).save(&input).unwrap();

        let engine = ImageEngine;
        let ids: Vec<String> = engine.actions(&input).into_iter().map(|a| a.id).collect();
        assert!(!ids.contains(&"convert:png".to_string()));

        let jpg = engine.run("convert:jpg", &input).unwrap();
        assert_eq!(image::open(&jpg).unwrap().width(), 40);

        let half = engine.run("resize:50", &input).unwrap();
        let img = image::open(&half).unwrap();
        assert_eq!((img.width(), img.height()), (20, 10));

        let small = engine.run("compress:50", &input).unwrap();
        assert_eq!(ext_of(&small), "jpg");

        // A second conversion must not overwrite the first.
        let jpg2 = engine.run("convert:jpg", &input).unwrap();
        assert_ne!(jpg, jpg2);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
