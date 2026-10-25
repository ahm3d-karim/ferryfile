//! What finished files are called, and "Replace original".
//!
//! Engines save outputs under their usual names (`photo-50pct.png`). Once a
//! whole action or recipe is done, [`finish`] renames the result by the
//! user's pattern and, if asked, sends the original to the Recycle Bin.
//!
//! Pattern tokens:
//! - `{name}` the original name. It keeps Ferryfile's short tag (`photo-50pct`)
//!   unless the pattern places `{action}` itself.
//! - `{action}` that tag on its own (`50pct`, `compressed`); empty for plain
//!   conversions, along with the separator before it.
//! - `{ext}` the new extension, `{date}` today as `2026-10-25`.
//! - `{n}` the lowest number from 1 that makes the name unique. Without it,
//!   a taken name gets " (2)", " (3)"… Nothing is ever overwritten.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub const DEFAULT_PATTERN: &str = "{name}.{ext}";
const TOKENS: &[&str] = &["name", "ext", "action", "date", "n"];
const MAX_LEN: usize = 120;

struct Rules {
    pattern: String,
    replace_original: bool,
}

static RULES: RwLock<Option<Rules>> = RwLock::new(None);

pub fn set_rules(pattern: &str, replace_original: bool) {
    let pattern = if validate(pattern).is_ok() { pattern } else { DEFAULT_PATTERN };
    *RULES.write().unwrap() = Some(Rules { pattern: pattern.into(), replace_original });
}

fn pattern() -> String {
    RULES.read().unwrap().as_ref().map(|r| r.pattern.clone()).unwrap_or_else(|| DEFAULT_PATTERN.into())
}

fn replace_original_on() -> bool {
    RULES.read().unwrap().as_ref().is_some_and(|r| r.replace_original)
}

enum Piece<'a> {
    Text(&'a str),
    Token(&'a str),
}

fn pieces(pattern: &str) -> Result<Vec<Piece<'_>>, String> {
    let mut out = Vec::new();
    let mut rest = pattern;
    while !rest.is_empty() {
        match rest.find(['{', '}']) {
            Some(i) if rest[i..].starts_with('}') => return Err("A } has no matching {".into()),
            Some(i) => {
                if i > 0 {
                    out.push(Piece::Text(&rest[..i]));
                }
                let end = rest[i..].find('}').ok_or("A { has no matching }")? + i;
                let token = &rest[i + 1..end];
                if !TOKENS.contains(&token) {
                    return Err(format!("{{{token}}} isn't a token. Use {{name}}, {{ext}}, {{action}}, {{date}} or {{n}}"));
                }
                out.push(Piece::Token(token));
                rest = &rest[end + 1..];
            }
            None => {
                out.push(Piece::Text(rest));
                rest = "";
            }
        }
    }
    Ok(out)
}

/// Checks a naming pattern; the error says what to fix.
pub fn validate(pattern: &str) -> Result<(), String> {
    if pattern.trim().is_empty() {
        return Err("The name pattern is empty".into());
    }
    if pattern.chars().count() > MAX_LEN {
        return Err(format!("Keep the name pattern under {MAX_LEN} characters"));
    }
    let parts = pieces(pattern)?;
    for p in &parts {
        if let Piece::Text(t) = p {
            if let Some(c) = t.chars().find(|c| r#"\/:*?"<>|"#.contains(*c) || c.is_control()) {
                return Err(format!("File names can't contain {c}"));
            }
        }
    }
    if !parts.iter().any(|p| matches!(p, Piece::Token("ext"))) {
        return Err("Include {ext} so files keep their type".into());
    }
    Ok(())
}

/// The values a pattern is filled with.
pub struct Parts<'a> {
    /// The original file's name without extension.
    pub name: &'a str,
    /// Ferryfile's tag for the action, with its leading dash: `-50pct` or "".
    pub tag: &'a str,
    pub ext: &'a str,
    pub date: &'a str,
}

/// Fills `pattern` (assumed valid). `n` stands in for `{n}`.
pub fn expand(pattern: &str, parts: &Parts, n: u32) -> String {
    let Ok(pieces) = pieces(pattern) else {
        return format!("{}{}.{}", parts.name, parts.tag, parts.ext);
    };
    let places_action = pieces.iter().any(|p| matches!(p, Piece::Token("action")));
    let no_action = places_action && parts.tag.trim_start_matches('-').is_empty();
    let mut out = String::new();
    for p in pieces {
        match p {
            Piece::Text(t) => out.push_str(t),
            Piece::Token("name") => {
                out.push_str(parts.name);
                if !places_action {
                    out.push_str(parts.tag);
                }
            }
            Piece::Token("action") => {
                let action = parts.tag.trim_start_matches('-');
                if action.is_empty() {
                    // "{name}-{action}.{ext}" on a conversion: no dangling dash.
                    if out.ends_with(['-', '_', ' ']) {
                        out.pop();
                    }
                } else {
                    out.push_str(action);
                }
            }
            Piece::Token("ext") => out.push_str(parts.ext),
            Piece::Token("date") => out.push_str(parts.date),
            Piece::Token(_) => out.push_str(&n.to_string()),
        }
    }
    if no_action {
        // "{name} ({action})" on a conversion: drop the empty brackets too.
        for empty in [" ()", "()", " []", "[]"] {
            out = out.replace(empty, "");
        }
    }
    // Windows drops trailing dots and spaces; a name can't be only those.
    let trimmed = out.trim_end_matches(['.', ' ']).trim_start();
    if trimmed.is_empty() {
        format!("{}{}.{}", parts.name, parts.tag, parts.ext)
    } else {
        trimmed.to_string()
    }
}

/// `photo (2)` -> `photo`: the marker `unique_in`/`output_path` add.
fn strip_counter(stem: &str) -> &str {
    if let Some(open) = stem.rfind(" (") {
        let inner = &stem[open + 2..];
        if inner.len() > 1 && inner.ends_with(')') && inner[..inner.len() - 1].bytes().all(|b| b.is_ascii_digit()) {
            return &stem[..open];
        }
    }
    stem
}

/// Splits a produced stem into the original name and Ferryfile's tag.
fn split_tag<'a>(original: &'a str, produced: &'a str) -> (&'a str, &'a str) {
    match produced.strip_prefix(original) {
        Some(rest) => (original, strip_counter(rest)),
        None => (strip_counter(produced), ""),
    }
}

/// The name `pattern` gives a file `produced` from `input`, with `{n}` = `n`.
pub fn name_for(pattern: &str, input: &Path, produced: &Path, date: &str, n: u32) -> String {
    let stem = |p: &Path| p.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
    let (orig, made) = (stem(input), stem(produced));
    let ext = produced.extension().and_then(|e| e.to_str()).unwrap_or("");
    let (name, tag) = split_tag(&orig, &made);
    expand(pattern, &Parts { name, tag, ext, date }, n)
}

/// Today in the local time zone, as `2026-10-25`.
pub fn today() -> String {
    #[cfg(windows)]
    {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay)
    }
    #[cfg(not(windows))]
    {
        // UTC civil date (Howard Hinnant's algorithm); Ferryfile ships for Windows.
        let days = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() / 86_400)
            .unwrap_or(0) as i64;
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02}")
    }
}

/// Moves `from` to `to` only if nothing is at `to` (never replaces a file).
fn move_if_free(from: &Path, to: &Path) -> std::io::Result<()> {
    match std::fs::hard_link(from, to) {
        Ok(()) => std::fs::remove_file(from),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        // Some drives can't hard-link: fall back to a checked rename.
        Err(_) if !to.exists() => std::fs::rename(from, to),
        Err(e) => Err(e),
    }
}

/// Renames `produced` by `pattern`, next to where it was saved.
fn rename_by_pattern(pattern: &str, input: &Path, produced: PathBuf) -> Result<PathBuf, String> {
    if pattern == DEFAULT_PATTERN {
        return Ok(produced); // engines already use these names
    }
    let dir = produced.parent().map(Path::to_path_buf).unwrap_or_default();
    let date = today();
    let uses_n = pattern.contains("{n}");
    let first = name_for(pattern, input, &produced, &date, 1);
    if !uses_n && produced.file_name().and_then(|f| f.to_str()) == Some(first.as_str()) {
        return Ok(produced);
    }
    for i in 1..10_000u32 {
        let name = if uses_n {
            name_for(pattern, input, &produced, &date, i)
        } else if i == 1 {
            first.clone()
        } else {
            let p = Path::new(&first);
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(&first);
            match p.extension().and_then(|e| e.to_str()) {
                Some(ext) => format!("{stem} ({i}).{ext}"),
                None => format!("{stem} ({i})"),
            }
        };
        let target = dir.join(&name);
        if target == produced {
            return Ok(produced);
        }
        match move_if_free(&produced, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists || target.exists() => continue,
            Err(e) => return Err(format!("Saved {}, but couldn't rename it: {e}", produced.display())),
        }
    }
    Ok(produced)
}

/// Is `out` a complete file we'd trust in place of the original?
fn check_output(out: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(out).map_err(|e| format!("Couldn't read the new file: {e}"))?;
    if !meta.is_file() || meta.len() == 0 {
        return Err("The new file is empty".into());
    }
    let ext = super::ext_of(out);
    if ["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"].contains(&ext.as_str()) {
        image::image_dimensions(out).map_err(|e| format!("The new image doesn't open: {e}"))?;
    }
    Ok(())
}

/// Sends `path` to the Recycle Bin. Windows asks first if it can't recycle
/// it (e.g. a network drive), so this never deletes for good on its own.
#[cfg(windows)]
pub fn recycle(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING,
        FO_DELETE, SHFILEOPSTRUCTW,
    };
    let abs = std::path::absolute(path).map_err(|e| e.to_string())?;
    // The shell wants a double-NUL-terminated list.
    let mut from: Vec<u16> = abs.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI | FOF_WANTNUKEWARNING).0 as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut op) };
    if op.fAnyOperationsAborted.as_bool() {
        return Err("you chose to keep it".into());
    }
    if code != 0 || path.exists() {
        return Err(format!("Windows said no (code {code})"));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn recycle(_path: &Path) -> Result<(), String> {
    Err("the Recycle Bin is Windows-only".into())
}

/// Puts `out` in place of `input`: checks `out`, recycles `input`, and gives
/// `out` the original name when it's the same type in the same folder.
fn replace_original(input: &Path, out: PathBuf) -> Result<PathBuf, String> {
    if !input.is_file() || same_file(input, &out) {
        return Ok(out);
    }
    let keep = |why: String| format!("Saved {}, but kept the original: {why}", file_name(&out));
    check_output(&out).map_err(keep)?;
    recycle(input).map_err(|e| keep(format!("couldn't move it to the Recycle Bin ({e})")))?;
    let same_place = out.parent() == input.parent() && super::ext_of(&out) == super::ext_of(input);
    if same_place && move_if_free(&out, input).is_ok() {
        return Ok(input.to_path_buf());
    }
    Ok(out)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Final touches once a whole action or recipe has produced `produced` from
/// `input`: the naming pattern, then "Replace original" if it's on and
/// `may_replace` (combining several files never replaces them). Folders keep
/// their names.
pub fn finish(input: &Path, produced: PathBuf, may_replace: bool) -> Result<PathBuf, String> {
    if produced.is_dir() {
        return Ok(produced);
    }
    let out = rename_by_pattern(&pattern(), input, produced)?;
    if may_replace && replace_original_on() {
        return replace_original(input, out);
    }
    Ok(out)
}

/// Example names for the Settings preview: (what was done, resulting name).
pub fn preview(pattern: &str) -> Result<Vec<(String, String)>, String> {
    validate(pattern)?;
    let date = today();
    let input = Path::new("Holiday photo.png");
    Ok([("Convert to WebP", "Holiday photo.webp"), ("Resize 50%", "Holiday photo-50pct.png")]
        .into_iter()
        .map(|(what, made)| (what.to_string(), name_for(pattern, input, Path::new(made), &date, 1)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(pattern: &str, produced: &str) -> String {
        name_for(pattern, Path::new("photo.png"), Path::new(produced), "2026-10-25", 3)
    }

    #[test]
    fn expands_tokens() {
        // The default keeps Ferryfile's usual names.
        assert_eq!(named(DEFAULT_PATTERN, "photo.webp"), "photo.webp");
        assert_eq!(named(DEFAULT_PATTERN, "photo-50pct.png"), "photo-50pct.png");
        assert_eq!(named("{name}-{date}.{ext}", "photo-50pct.png"), "photo-50pct-2026-10-25.png");
        // Placing {action} moves the tag there.
        assert_eq!(named("{action}_{name}.{ext}", "photo-50pct.png"), "50pct_photo.png");
        assert_eq!(named("{name} [{action}].{ext}", "photo-compressed.jpg"), "photo [compressed].jpg");
        // No tag: the separator before {action} goes too.
        assert_eq!(named("{name}-{action}.{ext}", "photo.webp"), "photo.webp");
        assert_eq!(named("{date} {name} {n}.{ext}", "photo.webp"), "2026-10-25 photo 3.webp");
        // A collision counter isn't part of the tag.
        assert_eq!(named("{name}-{action}.{ext}", "photo-50pct (2).png"), "photo-50pct.png");
        assert_eq!(named("{name}_{date}.{ext}", "photo (4).webp"), "photo_2026-10-25.webp");
        // Original names with brackets or a counter of their own survive.
        let n = name_for("{name}-{action}.{ext}", Path::new("a (3).png"), Path::new("a (3)-w800.png"), "d", 1);
        assert_eq!(n, "a (3)-w800.png");
        // Trailing dots or spaces can't end a Windows file name.
        assert_eq!(named("{name}.{ext}. ", "photo.webp"), "photo.webp");
    }

    #[test]
    fn validates_patterns() {
        for ok in [DEFAULT_PATTERN, "{date} {name}.{ext}", "{name}-{n}.{ext}", "out.{ext}"] {
            assert!(validate(ok).is_ok(), "{ok}");
        }
        for bad in ["", "  ", "{name}", "{name}.{type}", "{name.{ext}", "{name}}.{ext}", "a/{name}.{ext}", "{name}:{ext}", "{name}?.{ext}"] {
            assert!(validate(bad).is_err(), "{bad:?} should be rejected");
        }
        assert_eq!(validate("{nmae}.{ext}").unwrap_err(), "{nmae} isn't a token. Use {name}, {ext}, {action}, {date} or {n}");
    }

    #[test]
    fn preview_shows_two_examples() {
        let p = preview("{name} ({action}).{ext}").unwrap();
        assert_eq!(p[0].1, "Holiday photo.webp");
        assert_eq!(p[1].1, "Holiday photo (50pct).png");
        assert!(preview("{name}").is_err());
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferryfile-naming-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn renames_without_overwriting() {
        let dir = temp_dir("rename");
        let input = dir.join("pic.png");
        std::fs::write(&input, b"in").unwrap();
        let made = |name: &str| {
            let p = dir.join(name);
            std::fs::write(&p, name).unwrap();
            p
        };
        let date = today();

        let out = rename_by_pattern("{name}_{action}.{ext}", &input, made("pic-50pct.png")).unwrap();
        assert_eq!(file_name(&out), "pic_50pct.png");
        // Same name again: " (2)", and the first file is untouched.
        let out2 = rename_by_pattern("{name}_{action}.{ext}", &input, made("pic-50pct.png")).unwrap();
        assert_eq!(file_name(&out2), "pic_50pct (2).png");
        assert_eq!(std::fs::read(&out).unwrap(), b"pic-50pct.png");

        // {n} counts up instead.
        for want in ["pic 1.webp", "pic 2.webp"] {
            let out = rename_by_pattern("{name} {n}.{ext}", &input, made("pic.webp")).unwrap();
            assert_eq!(file_name(&out), want);
        }
        let out = rename_by_pattern("{date}-{name}.{ext}", &input, made("pic.webp")).unwrap();
        assert_eq!(file_name(&out), format!("{date}-pic.webp"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "puts small test files in the Recycle Bin"]
    fn replace_original_recycles_after_checking() {
        let dir = temp_dir("replace");
        let input = dir.join("pic.png");
        image::RgbaImage::new(4, 4).save(&input).unwrap();

        // A broken output never costs the original.
        let broken = dir.join("pic-50pct.png");
        std::fs::write(&broken, b"not a png").unwrap();
        let err = replace_original(&input, broken.clone()).unwrap_err();
        assert!(err.starts_with("Saved pic-50pct.png, but kept the original"), "{err}");
        assert!(input.exists());
        std::fs::remove_file(&broken).unwrap();

        // A good one of the same type takes the original's place.
        let good = dir.join("pic-50pct.png");
        image::RgbaImage::new(2, 2).save(&good).unwrap();
        let out = replace_original(&input, good).unwrap();
        assert_eq!(out, input);
        assert_eq!(image::image_dimensions(&input).unwrap(), (2, 2));
        assert!(!dir.join("pic-50pct.png").exists());

        // A different type keeps its own name; the original is recycled.
        let webp = dir.join("pic.webp");
        image::RgbaImage::new(2, 2).save(&webp).unwrap();
        let out = replace_original(&input, webp.clone()).unwrap();
        assert_eq!(out, webp);
        assert!(!input.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
