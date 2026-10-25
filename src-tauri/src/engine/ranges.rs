//! Page ranges typed by people, like `1-3, 5, 8-`: page numbers start at 1,
//! a range with no end runs to the last page.

use std::ops::RangeInclusive;

/// One range as typed: `start..=end`, with `None` meaning "to the last page".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRange {
    pub start: u32,
    pub end: Option<u32>,
}

impl PageRange {
    /// `1–3`, `5`, `8–end`.
    pub fn label(&self) -> String {
        match self.end {
            Some(e) if e == self.start => self.start.to_string(),
            Some(e) => format!("{}–{e}", self.start),
            None => format!("{}–end", self.start),
        }
    }
}

fn page_number(s: &str, whole: &str) -> Result<u32, String> {
    match s.parse::<u32>() {
        Ok(0) => Err("Pages start at 1".into()),
        Ok(n) => Ok(n),
        Err(_) => Err(format!("'{whole}' isn't a page or a range like 2-5")),
    }
}

/// Parses `1-3, 5, 8-` (commas or semicolons; `-`, `–` or `—` for ranges).
/// Checks the syntax only; see [`resolve`] for checking against a page count.
pub fn parse(spec: &str) -> Result<Vec<PageRange>, String> {
    let mut out = Vec::new();
    for part in spec.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let normalized = part.replace(['–', '—'], "-");
        let range = match normalized.split_once('-') {
            None => {
                let n = page_number(normalized.trim(), part)?;
                PageRange { start: n, end: Some(n) }
            }
            Some((a, b)) => {
                let (a, b) = (a.trim(), b.trim());
                let start = if a.is_empty() { 1 } else { page_number(a, part)? };
                let end = if b.is_empty() { None } else { Some(page_number(b, part)?) };
                if end.is_some_and(|e| e < start) {
                    return Err(format!("'{part}' runs backwards"));
                }
                PageRange { start, end }
            }
        };
        out.push(range);
    }
    if out.is_empty() {
        return Err("Type the pages to keep, like 1-3, 5, 8-".into());
    }
    Ok(out)
}

/// Parses `spec` and checks it against a document with `count` pages.
/// Returns 1-based inclusive ranges with open ends filled in.
pub fn resolve(spec: &str, count: u32) -> Result<Vec<RangeInclusive<u32>>, String> {
    parse(spec)?
        .into_iter()
        .map(|r| {
            let end = r.end.unwrap_or(count);
            let last = end.max(r.start);
            if last > count {
                let pages = if count == 1 { "1 page".to_string() } else { format!("{count} pages") };
                Err(format!("Page {last} is past the end (this PDF has {pages})"))
            } else {
                Ok(r.start..=end)
            }
        })
        .collect()
}

/// The canonical id form of `spec`: `1-3,5,8-` (no spaces, plain dashes).
pub fn canonical(spec: &str) -> Result<String, String> {
    Ok(parse(spec)?
        .iter()
        .map(|r| match r.end {
            Some(e) if e == r.start => r.start.to_string(),
            Some(e) => format!("{}-{e}", r.start),
            None => format!("{}-", r.start),
        })
        .collect::<Vec<_>>()
        .join(","))
}

/// `1–3, 5, 8–end`.
pub fn label(spec: &str) -> Result<String, String> {
    Ok(parse(spec)?.iter().map(PageRange::label).collect::<Vec<_>>().join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges() {
        let r = parse("1-3, 5, 8-").unwrap();
        assert_eq!(
            r,
            vec![
                PageRange { start: 1, end: Some(3) },
                PageRange { start: 5, end: Some(5) },
                PageRange { start: 8, end: None },
            ]
        );
        // Dashes people paste from Word, semicolons, stray spaces and commas.
        assert_eq!(parse(" 2 – 4 ;; 7 ,").unwrap(), vec![PageRange { start: 2, end: Some(4) }, PageRange { start: 7, end: Some(7) }]);
        // "-4" means from the start.
        assert_eq!(parse("-4").unwrap(), vec![PageRange { start: 1, end: Some(4) }]);
        assert_eq!(parse("3-3").unwrap(), vec![PageRange { start: 3, end: Some(3) }]);
    }

    #[test]
    fn rejects_bad_ranges() {
        assert!(parse("").is_err());
        assert!(parse(" , ").is_err());
        assert_eq!(parse("0-2").unwrap_err(), "Pages start at 1");
        assert_eq!(parse("5-3").unwrap_err(), "'5-3' runs backwards");
        assert!(parse("a-b").unwrap_err().contains("isn't a page"));
        assert!(parse("1-2-3").is_err());
        assert!(parse("1.5").is_err());
    }

    #[test]
    fn resolves_against_page_count() {
        assert_eq!(resolve("1-3, 5, 8-", 12).unwrap(), vec![1..=3, 5..=5, 8..=12]);
        assert_eq!(resolve("12-", 12).unwrap(), vec![12..=12]);
        assert_eq!(resolve("4-9", 6).unwrap_err(), "Page 9 is past the end (this PDF has 6 pages)");
        assert_eq!(resolve("2", 1).unwrap_err(), "Page 2 is past the end (this PDF has 1 page)");
        // An open range starting past the end is caught too.
        assert!(resolve("8-", 5).is_err());
    }

    #[test]
    fn canonical_and_label() {
        assert_eq!(canonical(" 1 – 3, 5,8 - ").unwrap(), "1-3,5,8-");
        assert_eq!(label("1-3,5,8-").unwrap(), "1–3, 5, 8–end");
    }
}
