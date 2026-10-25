//! Non-file content as files: text, links and images dragged from a browser or
//! another app, and whatever is on the clipboard. Each piece is saved as a real
//! file (`.txt`, `.url`, `.png`) in Ferryfile's shelf-items folder so every
//! action works on it, with a readable name taken from the content.

use std::path::{Path, PathBuf};

/// Something to turn into shelf items.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text(String),
    Url { url: String, title: Option<String> },
    /// Encoded image bytes (PNG, JPEG, BMP...). `name` is a suggested file name.
    Image { bytes: Vec<u8>, name: Option<String> },
    /// Files that already exist (copied in Explorer): added as they are.
    Files(Vec<PathBuf>),
}

const MAX_STEM: usize = 48;
const TEXT_WORDS: usize = 6;

/// A file name stem that Windows accepts: no reserved characters, no
/// trailing dots or spaces, no device names, at most `MAX_STEM` characters
/// (cut at a word break when possible).
pub fn sanitize(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c })
        .collect();
    let mut s = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > MAX_STEM {
        let cut: String = s.chars().take(MAX_STEM).collect();
        s = match cut.rfind(' ') {
            Some(i) if i > MAX_STEM / 2 => cut[..i].to_string(),
            _ => cut,
        };
    }
    let s = s.trim_matches(|c: char| c == '.' || c == ' ').to_string();
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(&s)) {
        format!("{s}_")
    } else {
        s
    }
}

/// Name for a text snippet: its first few words.
pub fn text_stem(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().take(TEXT_WORDS).collect();
    let stem = sanitize(&words.join(" "));
    if stem.is_empty() { "Text".into() } else { stem }
}

/// Host of an http(s)/ftp URL, without `www.`.
pub fn host_of(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?; // port (IPv6 hosts are rare enough to ignore)
    let host = host.strip_prefix("www.").unwrap_or(host);
    (!host.is_empty()).then_some(host)
}

/// Name for a link: the page title when we have one, else the host.
pub fn url_stem(url: &str, title: Option<&str>) -> String {
    let title = title.map(str::trim).filter(|t| !t.is_empty() && *t != url.trim());
    let stem = match title {
        Some(t) => sanitize(t),
        None => host_of(url).map(sanitize).unwrap_or_default(),
    };
    if stem.is_empty() { "Link".into() } else { stem }
}

/// True for a single http(s) URL with nothing around it.
pub fn looks_like_url(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with("http://") || t.starts_with("https://")) && !t.contains(char::is_whitespace) && host_of(t).is_some()
}

/// An Internet Shortcut: double-click opens the page; nothing is downloaded.
pub fn url_shortcut(url: &str) -> String {
    format!("[InternetShortcut]\r\nURL={}\r\n", url.trim())
}

/// Turns a CF_DIB clipboard block (a bitmap without its file header) into a
/// complete .bmp file the image crate can read.
pub fn dib_to_bmp(dib: &[u8]) -> Result<Vec<u8>, String> {
    let u32_at = |i: usize| dib.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let u16_at = |i: usize| dib.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let header = u32_at(0).ok_or("The clipboard image is empty")? as usize;
    let bits = u16_at(14).ok_or("The clipboard image is damaged")?;
    let compression = u32_at(16).unwrap_or(0);
    let used = if header >= 40 { u32_at(32).unwrap_or(0) as usize } else { 0 };
    let palette = if bits <= 8 { if used > 0 { used } else { 1 << bits } } else { used } * 4;
    // BI_BITFIELDS / BI_ALPHABITFIELDS masks follow a plain BITMAPINFOHEADER.
    let masks = match (header, compression) {
        (40, 3) => 12,
        (40, 6) => 16,
        _ => 0,
    };
    let offset = 14 + header + masks + palette;
    if dib.len() + 14 < offset {
        return Err("The clipboard image is damaged".into());
    }
    let mut out = Vec::with_capacity(dib.len() + 14);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    out.extend_from_slice(dib);
    Ok(out)
}

/// Re-encodes any image the image crate reads as PNG. Images whose alpha is
/// all zero (common for 32-bit clipboard bitmaps) come out opaque.
pub fn to_png(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("Couldn't read the image: {e}"))?;
    let mut rgba = img.to_rgba8();
    if rgba.pixels().all(|p| p[3] == 0) {
        rgba.pixels_mut().for_each(|p| p[3] = 255);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    rgba.write_to(&mut out, image::ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

fn write_new(dir: &Path, stem: &str, ext: &str, data: &[u8]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = crate::engine::unique_in(dir, &format!("{stem}.{ext}"));
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Saves `content` as files in `dir`. `stamp` (e.g. "2026-10-25 143012")
/// names images that come without a name, like screenshots.
pub fn save(content: Content, dir: &Path, stamp: &str) -> Result<Vec<PathBuf>, String> {
    match content {
        Content::Files(paths) => {
            let paths: Vec<_> = paths.into_iter().filter(|p| p.exists()).collect();
            if paths.is_empty() {
                return Err("The copied files no longer exist".into());
            }
            Ok(paths)
        }
        Content::Text(text) if looks_like_url(&text) => save(Content::Url { url: text.trim().into(), title: None }, dir, stamp),
        Content::Text(text) => {
            if text.trim().is_empty() {
                return Err("There's no text to save".into());
            }
            Ok(vec![write_new(dir, &text_stem(&text), "txt", text.as_bytes())?])
        }
        Content::Url { url, title } => {
            if host_of(&url).is_none() {
                return Err(format!("{url} isn't a web link"));
            }
            Ok(vec![write_new(dir, &url_stem(&url, title.as_deref()), "url", url_shortcut(&url).as_bytes())?])
        }
        Content::Image { bytes, name } => {
            let given = name.as_deref().map(Path::new);
            let stem = given
                .and_then(|p| p.file_stem())
                .and_then(|s| s.to_str())
                .map(sanitize)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("Screenshot {stamp}"));
            // Formats every app reads are kept as they are; anything else becomes PNG.
            let kept = match image::guess_format(&bytes) {
                Ok(image::ImageFormat::Png) => Some("png"),
                Ok(image::ImageFormat::Jpeg) => Some("jpg"),
                Ok(image::ImageFormat::Gif) => Some("gif"),
                Ok(image::ImageFormat::WebP) => Some("webp"),
                _ => None,
            };
            if let Some(ext) = kept {
                return Ok(vec![write_new(dir, &stem, ext, &bytes)?]);
            }
            match to_png(&bytes) {
                Ok(png) => Ok(vec![write_new(dir, &stem, "png", &png)?]),
                // Not a raster image (an SVG, say): keep it under its own name.
                Err(e) => match given.and_then(|p| p.extension()).and_then(|e| e.to_str()) {
                    Some(ext) => Ok(vec![write_new(dir, &stem, &sanitize(ext), &bytes)?]),
                    None => Err(e),
                },
            }
        }
    }
}

/// Local time as "YYYY-MM-DD HHMMSS", like Windows names its screenshots.
pub fn stamp() -> String {
    #[cfg(windows)]
    {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        format!("{:04}-{:02}-{:02} {:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        secs.to_string()
    }
}

/// What's on the clipboard, best guess first: copied files, then text (a lone
/// link next to an image counts as the image's address, so the image wins),
/// then an image.
#[cfg(windows)]
pub fn read_clipboard() -> Result<Content, String> {
    use clipboard_win::{formats, raw};
    let _open = clipboard_win::Clipboard::new_attempts(10).map_err(|e| format!("Couldn't open the clipboard: {e}"))?;

    if raw::is_format_avail(formats::CF_HDROP) {
        let mut files = Vec::new();
        if raw::get_file_list_path(&mut files).is_ok() && !files.is_empty() {
            return Ok(Content::Files(files));
        }
    }

    let image = || -> Option<Vec<u8>> {
        let mut buf = Vec::new();
        if let Some(png) = raw::register_format("PNG") {
            if raw::is_format_avail(png.get()) && raw::get_vec(png.get(), &mut buf).is_ok() && !buf.is_empty() {
                return Some(buf);
            }
        }
        buf.clear();
        if raw::is_format_avail(formats::CF_DIB) && raw::get_vec(formats::CF_DIB, &mut buf).is_ok() {
            return dib_to_bmp(&buf).ok();
        }
        None
    };

    let mut text = None;
    if raw::is_format_avail(formats::CF_UNICODETEXT) {
        let mut buf = Vec::new();
        if raw::get_string(&mut buf).is_ok() {
            text = Some(String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string());
        }
    }
    let text = text.filter(|t| !t.trim().is_empty());

    match text {
        Some(t) if !looks_like_url(&t) => Ok(Content::Text(t)),
        t => match image() {
            Some(bytes) => Ok(Content::Image { bytes, name: None }),
            None => t.map(Content::Text).ok_or_else(|| "The clipboard has no text, image or files".into()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferryfile-content-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn names_text_by_its_first_words() {
        assert_eq!(text_stem("  Meeting notes:\nship the shelf queue on Friday, then rest"), "Meeting notes ship the shelf queue");
        assert_eq!(text_stem("a/b\\c?"), "a b c");
        assert_eq!(text_stem("   \n\t"), "Text");
        assert_eq!(text_stem("CON"), "CON_");
        let long = "Supercalifragilisticexpialidocious".repeat(3);
        assert_eq!(text_stem(&long).chars().count(), MAX_STEM);
        assert_eq!(sanitize("trailing dots... "), "trailing dots");
    }

    #[test]
    fn names_links_by_title_or_host() {
        assert_eq!(host_of("https://www.example.com:8080/a?b#c"), Some("example.com"));
        assert_eq!(host_of("https://user@docs.rs/image"), Some("docs.rs"));
        assert_eq!(host_of("not a url"), None);
        assert_eq!(url_stem("https://www.rust-lang.org/learn", None), "rust-lang.org");
        assert_eq!(url_stem("https://x.com", Some("Rust: Learn | Fast?")), "Rust Learn Fast");
        // A "title" that is just the URL again isn't a title.
        assert_eq!(url_stem("https://github.com/a/b", Some("https://github.com/a/b")), "github.com");
        assert!(looks_like_url(" https://example.com/x \n"));
        assert!(!looks_like_url("see https://example.com"));
        assert!(url_shortcut("https://example.com").contains("URL=https://example.com\r\n"));
    }

    #[test]
    fn saves_text_and_links_as_files() {
        let dir = temp_dir("text");
        let a = save(Content::Text("Hello there, shelf".into()), &dir, "s").unwrap();
        assert_eq!(a[0].file_name().unwrap(), "Hello there, shelf.txt");
        assert_eq!(std::fs::read_to_string(&a[0]).unwrap(), "Hello there, shelf");
        // Same text again doesn't overwrite.
        let b = save(Content::Text("Hello there, shelf".into()), &dir, "s").unwrap();
        assert_eq!(b[0].file_name().unwrap(), "Hello there, shelf (2).txt");

        let link = save(Content::Text("https://www.example.com/page".into()), &dir, "s").unwrap();
        assert_eq!(link[0].file_name().unwrap(), "example.com.url");
        assert!(std::fs::read_to_string(&link[0]).unwrap().starts_with("[InternetShortcut]"));
        let titled = save(Content::Url { url: "https://example.com".into(), title: Some("Example Domain".into()) }, &dir, "s").unwrap();
        assert_eq!(titled[0].file_name().unwrap(), "Example Domain.url");

        assert!(save(Content::Text(" ".into()), &dir, "s").is_err());
        assert!(save(Content::Url { url: "javascript:alert(1)".into(), title: None }, &dir, "s").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn clipboard_bitmaps_become_pngs() {
        let dir = temp_dir("image");
        // A clipboard DIB is a .bmp without its 14-byte file header.
        let img = image::RgbImage::from_fn(5, 3, |x, y| image::Rgb([x as u8 * 40, y as u8 * 80, 7]));
        let mut bmp = std::io::Cursor::new(Vec::new());
        img.write_to(&mut bmp, image::ImageFormat::Bmp).unwrap();
        let dib = &bmp.get_ref()[14..];

        let rebuilt = dib_to_bmp(dib).unwrap();
        assert_eq!(rebuilt, *bmp.get_ref());

        let out = save(Content::Image { bytes: rebuilt, name: None }, &dir, "2026-10-25 143012").unwrap();
        assert_eq!(out[0].file_name().unwrap(), "Screenshot 2026-10-25 143012.png");
        let back = image::open(&out[0]).unwrap().to_rgb8();
        assert_eq!(back, img);

        // PNG data (what Win+Shift+S also puts on the clipboard) is kept byte for byte.
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let kept = save(Content::Image { bytes: png.get_ref().clone(), name: Some("cat photo.png".into()) }, &dir, "x").unwrap();
        assert_eq!(kept[0].file_name().unwrap(), "cat photo.png");
        assert_eq!(std::fs::read(&kept[0]).unwrap(), *png.get_ref());

        // Fully transparent 32-bit bitmaps come out opaque.
        let clear = image::RgbaImage::from_pixel(2, 2, image::Rgba([9, 9, 9, 0]));
        let mut tiff = std::io::Cursor::new(Vec::new());
        clear.write_to(&mut tiff, image::ImageFormat::Tiff).unwrap();
        let opaque = image::load_from_memory(&to_png(tiff.get_ref()).unwrap()).unwrap().to_rgba8();
        assert!(opaque.pixels().all(|p| p[3] == 255));

        assert!(dib_to_bmp(&[1, 2]).is_err());
        assert!(save(Content::Image { bytes: vec![1, 2, 3], name: None }, &dir, "x").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn copied_files_pass_through() {
        let dir = temp_dir("files");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let out = save(Content::Files(vec![f.clone(), dir.join("gone.txt")]), &dir, "x").unwrap();
        assert_eq!(out, vec![f]);
        assert!(save(Content::Files(vec![dir.join("gone.txt")]), &dir, "x").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
