//! In-process PDF typesetting with Typst (no external tools).
//!
//! Callers hand us Typst markup plus a root folder for any images it
//! references; we compile it with embedded and system fonts and return PDF
//! bytes. Packages (`#import "@preview/..."`) are not available offline.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, LibraryExt, World};
use typst_kit::fonts::FontStore;
use typst_layout::PagedDocument;

fn fonts() -> &'static FontStore {
    static FONTS: OnceLock<FontStore> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut store = FontStore::new();
        store.extend(typst_kit::fonts::embedded());
        store.extend(typst_kit::fonts::system());
        store
    })
}

fn library() -> &'static LazyHash<Library> {
    static LIBRARY: OnceLock<LazyHash<Library>> = OnceLock::new();
    LIBRARY.get_or_init(|| LazyHash::new(Library::default()))
}

struct DocWorld {
    root: PathBuf,
    main: FileId,
    source: Source,
}

impl World for DocWorld {
    fn library(&self) -> &LazyHash<Library> {
        library()
    }

    fn book(&self) -> &LazyHash<FontBook> {
        fonts().book()
    }

    fn main(&self) -> FileId {
        self.main
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main {
            return Ok(self.source.clone());
        }
        let bytes = self.file(id)?;
        let text = String::from_utf8(bytes.to_vec()).map_err(|_| FileError::NotSource)?;
        Ok(Source::new(id, text))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        let rooted = id.get();
        if !matches!(rooted.root(), VirtualRoot::Project) {
            return Err(FileError::Other(Some("packages aren't available in Skiff".into())));
        }
        let path = rooted.vpath().realize(&self.root)?;
        std::fs::read(&path).map(Bytes::new).map_err(|e| FileError::from_io(e, &path))
    }

    fn font(&self, index: usize) -> Option<Font> {
        fonts().font(index)
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        let days = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs()
            / 86_400;
        let (y, m, d) = civil_from_days(days as i64);
        Datetime::from_ymd(y, m, d)
    }
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil_from_days(z: i64) -> (i32, u8, u8) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (y, m, d)
}

/// Compiles Typst `markup` to PDF bytes. Relative paths in the markup (e.g.
/// images) resolve against `root`.
pub fn typst_to_pdf(markup: String, root: &Path) -> Result<Vec<u8>, String> {
    let main = RootedPath::new(VirtualRoot::Project, VirtualPath::new("main.typ").map_err(|e| format!("{e:?}"))?)
        .intern();
    let world = DocWorld { root: root.to_path_buf(), main, source: Source::new(main, markup) };
    let document: PagedDocument = typst::compile(&world)
        .output
        .map_err(|errors| describe(&errors))?;
    typst_pdf::pdf(&document, &typst_pdf::PdfOptions::default()).map_err(|errors| describe(&errors))
}

fn describe(errors: &[typst::diag::SourceDiagnostic]) -> String {
    let first = errors.first().map(|e| e.message.to_string()).unwrap_or_default();
    format!("Typesetting failed: {first}")
}

/// A Typst string literal for arbitrary text.
pub fn typst_str(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_744), (2026, 10, 18));
    }

    #[test]
    fn escapes_strings() {
        assert_eq!(typst_str("a \"b\" \\ c\nd"), r#""a \"b\" \\ c\nd""#);
    }

    #[test]
    fn compiles_pdf() {
        let markup = format!("= Hello\n\n#table(columns: 2, {}, {})", typst_str("a #b"), typst_str("[c]"));
        let pdf = typst_to_pdf(markup, &std::env::temp_dir()).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }
}
