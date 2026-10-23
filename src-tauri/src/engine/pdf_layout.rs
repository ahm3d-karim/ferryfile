//! Rebuilds document structure from a PDF's positioned characters: lines,
//! paragraphs (from vertical gaps, indents and sentence ends), headings (from
//! font size and weight), list items, and where images sit between them. The
//! result is reflowed Markdown, not a facsimile of the page.

use std::path::Path;

use image::DynamicImage;
use pdfium_render::prelude::*;

/// One extracted line of text.
#[derive(Debug, Clone)]
pub(super) struct Line {
    pub text: String,
    /// Most common font size on the line, in points.
    pub size: f32,
    pub top: f32,
    pub left: f32,
    pub baseline: f32,
    pub bold: bool,
}

/// An image placed on a page, already saved to disk.
#[derive(Debug, Clone)]
pub(super) struct Picture {
    /// File name relative to the Markdown file.
    pub file: String,
    pub top: f32,
    pub width_pt: f32,
}

#[derive(Debug, Default, Clone)]
pub(super) struct Page {
    pub lines: Vec<Line>,
    pub pictures: Vec<Picture>,
}

#[derive(Debug, PartialEq)]
pub(super) enum Block {
    Heading(u8, String),
    Para(String),
    Bold(String),
    Bullet(String),
    Numbered(String, String),
    Image(String, f32),
}

const BULLETS: &[char] = &['•', '●', '▪', '◦', '–', '■', '□', '‣', '▶', '►', '➢', '◆', '✓', '*'];

/// Reads the lines of `page` in pdfium's reading order.
pub(super) fn page_lines(page: &PdfPage) -> Result<Vec<Line>, String> {
    let text = page.text().map_err(|e| format!("PDF error: {e}"))?;
    let mut lines = Vec::new();
    let mut cur = LineBuilder::default();
    // pdfium also reports breaks around super- and subscripts; a break only
    // counts once the next character turns out to sit on another line.
    let mut pending_break = false;
    for ch in text.chars().iter() {
        let Some(c) = ch.unicode_char() else { continue };
        if c == '\r' || c == '\n' {
            pending_break = true;
            continue;
        }
        // Soft-hyphen markers pdfium reports at line ends.
        let c = if c == '\u{2}' || c == '\u{AD}' { '-' } else { c };
        if c.is_control() {
            continue;
        }
        if c.is_whitespace() {
            cur.text.push(' ');
            continue;
        }
        // Sideways text is margin notes and chart axes, not the body.
        if ch.angle_degrees().is_ok_and(|a| a.rem_euclid(360.0).min(360.0 - a.rem_euclid(360.0)) > 1.0) {
            continue;
        }
        let Ok(bounds) = ch.loose_bounds() else {
            cur.text.push(c);
            continue;
        };
        let baseline = ch.origin_y().map(|y| y.value).unwrap_or(bounds.bottom().value);
        let size = ch.scaled_font_size().value;
        let offset = baseline - cur.baseline;
        let same_line = cur.visible > 0
            && offset.abs() < cur.size.min(size) * 0.6
            && (!pending_break || bounds.left().value >= cur.right - cur.size * 0.2);
        if !same_line {
            cur.finish(&mut lines);
        } else if pending_break && bounds.left().value - cur.right > size * 0.15 {
            // Leaving a script with a gap: that was a word break ("R² rewards").
            cur.text.push(' ');
        }
        pending_break = false;
        let bold = ch.font_weight().is_some_and(|w| font_weight_value(w) >= 600)
            || ch.font_name().to_ascii_lowercase().contains("bold");
        cur.push(script(c, size, offset, cur.size, cur.visible), size, bounds, baseline, bold);
    }
    cur.finish(&mut lines);
    Ok(lines)
}

/// Small digits raised or lowered off the line become Unicode scripts (R²).
fn script(c: char, size: f32, offset: f32, line_size: f32, visible: usize) -> char {
    const SUP: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
    const SUB: [char; 10] = ['₀', '₁', '₂', '₃', '₄', '₅', '₆', '₇', '₈', '₉'];
    let Some(d) = c.to_digit(10) else { return c };
    if visible == 0 || size > line_size * 0.85 {
        return c;
    }
    if offset > line_size * 0.15 {
        SUP[d as usize]
    } else if offset < -line_size * 0.1 {
        SUB[d as usize]
    } else {
        c
    }
}

fn font_weight_value(w: PdfFontWeight) -> u32 {
    match w {
        PdfFontWeight::Weight100 => 100,
        PdfFontWeight::Weight200 => 200,
        PdfFontWeight::Weight300 => 300,
        PdfFontWeight::Weight400Normal => 400,
        PdfFontWeight::Weight500 => 500,
        PdfFontWeight::Weight600 => 600,
        PdfFontWeight::Weight700Bold => 700,
        PdfFontWeight::Weight800 => 800,
        PdfFontWeight::Weight900 => 900,
        PdfFontWeight::Custom(v) => v,
    }
}

#[derive(Default)]
struct LineBuilder {
    text: String,
    sizes: Vec<f32>,
    top: f32,
    left: f32,
    right: f32,
    baseline: f32,
    /// Size of the line's first character, the base for scripts.
    size: f32,
    visible: usize,
    bold: usize,
}

impl LineBuilder {
    fn push(&mut self, c: char, size: f32, b: PdfRect, baseline: f32, bold: bool) {
        if self.visible == 0 {
            self.top = b.top().value;
            self.left = b.left().value;
            self.baseline = baseline;
            self.size = size;
        } else {
            self.top = self.top.max(b.top().value);
            self.left = self.left.min(b.left().value);
        }
        self.right = self.right.max(b.right().value);
        self.text.push(c);
        self.sizes.push(size);
        self.visible += 1;
        self.bold += usize::from(bold);
    }

    fn finish(&mut self, out: &mut Vec<Line>) {
        let b = std::mem::take(self);
        let text = b.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if b.visible == 0 || text.is_empty() {
            return;
        }
        out.push(Line {
            text,
            size: mode(&b.sizes),
            top: b.top,
            left: b.left,
            baseline: b.baseline,
            bold: b.bold * 10 >= b.visible * 9,
        });
    }
}

/// The most common value, rounded to half a point.
fn mode(values: &[f32]) -> f32 {
    let mut counts: Vec<(i32, usize)> = Vec::new();
    for v in values {
        let k = (v * 2.0).round() as i32;
        match counts.iter_mut().find(|(c, _)| *c == k) {
            Some((_, n)) => *n += 1,
            None => counts.push((k, 1)),
        }
    }
    counts.into_iter().max_by_key(|&(k, n)| (n, k)).map_or(0.0, |(k, _)| k as f32 / 2.0)
}

/// Body text size: the size most characters in the document are set in.
fn body_size(pages: &[Page]) -> f32 {
    let mut sizes = Vec::new();
    for line in pages.iter().flat_map(|p| &p.lines) {
        sizes.extend(std::iter::repeat_n(line.size, line.text.chars().count()));
    }
    mode(&sizes)
}

/// Heading levels by size: the largest heading size is level 1.
fn heading_sizes(pages: &[Page], body: f32) -> Vec<i32> {
    let mut sizes: Vec<i32> = pages
        .iter()
        .flat_map(|p| &p.lines)
        .filter(|l| l.size >= body * 1.15 && wordy(&l.text))
        .map(|l| (l.size * 2.0).round() as i32)
        .collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.dedup();
    sizes
}

/// Real words, not figure-panel letters ("A B C") or stray glyphs.
fn wordy(text: &str) -> bool {
    let letters = |w: &str| w.chars().filter(|c| c.is_alphabetic()).count();
    letters(text) >= 3 && text.split_whitespace().any(|w| letters(w) >= 2)
}

fn is_page_number(text: &str) -> bool {
    let t = text.trim_matches(|c: char| c == '-' || c == '–' || c.is_whitespace());
    let t = t.strip_prefix("Page ").unwrap_or(t);
    !t.is_empty() && t.len() <= 4 && t.chars().all(|c| c.is_ascii_digit())
}

/// "1. text" / "2) text" -> ("1.", "text").
fn numbered(text: &str) -> Option<(String, String)> {
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 2 {
        return None;
    }
    let rest = &text[digits..];
    let mark = rest.chars().next().filter(|c| *c == '.' || *c == ')')?;
    let body = rest[1..].strip_prefix(' ')?.trim();
    (!body.is_empty()).then(|| (format!("{}{mark}", &text[..digits]), body.to_string()))
}

fn bullet(text: &str) -> Option<&str> {
    let rest = text.strip_prefix(BULLETS)?;
    // "*" or "–" glued to a word isn't a bullet.
    rest.starts_with(' ').then(|| rest.trim())
}

/// Joins a wrapped line onto a paragraph, undoing end-of-line hyphenation.
fn join(para: &mut String, line: &str) {
    if para.is_empty() {
        para.push_str(line);
    } else if para.ends_with('-')
        && para.chars().rev().nth(1).is_some_and(char::is_alphabetic)
        && line.starts_with(|c: char| c.is_lowercase())
    {
        para.pop();
        para.push_str(line);
    } else {
        para.push(' ');
        para.push_str(line);
    }
}

/// Typical distance between baselines of consecutive body lines on a page.
fn typical_leading(lines: &[Line], body: f32) -> f32 {
    let mut gaps: Vec<f32> = lines
        .windows(2)
        .filter(|w| (w[0].size - body).abs() < 0.6 && (w[1].size - body).abs() < 0.6)
        .map(|w| w[0].baseline - w[1].baseline)
        .filter(|g| *g > 0.0)
        .collect();
    if gaps.is_empty() {
        return body * 1.2;
    }
    gaps.sort_by(f32::total_cmp);
    gaps[gaps.len() / 2]
}

#[derive(PartialEq, Clone, Copy)]
enum Kind {
    Heading(u8),
    Para,
    Bold,
    Bullet,
    Numbered,
}

/// A header/footer line with its page numbers and dates' digits taken out.
fn furniture_key(text: &str) -> String {
    text.chars().filter(|c| !c.is_ascii_digit() && !c.is_whitespace()).flat_map(char::to_lowercase).collect()
}

/// Drops page numbers and running headers/footers (the same line at the top
/// or bottom of most pages), and glues drop caps onto their paragraph.
fn strip_furniture(pages: &[Page], body: f32) -> Vec<Page> {
    const EDGE: usize = 2;
    let edges = |p: &Page| {
        let n = p.lines.len();
        (0..n).filter(move |&i| i < EDGE || i + EDGE >= n)
    };
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for page in pages {
        let mut keys: Vec<String> = edges(page).map(|i| furniture_key(&page.lines[i].text)).collect();
        keys.sort();
        keys.dedup();
        for key in keys.into_iter().filter(|k| !k.is_empty()) {
            *counts.entry(key).or_default() += 1;
        }
    }
    // On most pages, or (in small print) on a good share of them: section
    // footers change partway through slide decks.
    let repeated = |line: &Line| {
        let n = counts.get(&furniture_key(&line.text)).copied().unwrap_or(0);
        n >= 3 && (n * 2 >= pages.len() || (line.size < body * 0.9 && n * 5 >= pages.len()))
    };

    pages
        .iter()
        .map(|page| {
            let drop: Vec<usize> = edges(page)
                .filter(|&i| {
                    let line = &page.lines[i];
                    let outermost = i == 0 || i + 1 == page.lines.len();
                    (outermost && is_page_number(&line.text)) || repeated(line)
                })
                .collect();
            let mut lines: Vec<Line> = Vec::with_capacity(page.lines.len());
            let mut glue: Option<String> = None;
            for (i, line) in page.lines.iter().enumerate() {
                if drop.contains(&i) {
                    continue;
                }
                let mut line = line.clone();
                if let Some(cap) = glue.take() {
                    line.text.insert_str(0, &cap);
                }
                let next_lower = page.lines.get(i + 1).is_some_and(|n| n.text.starts_with(char::is_lowercase));
                let is_cap = line.text.chars().count() <= 3
                    && line.text.chars().all(char::is_alphabetic)
                    && line.size >= body * 1.5;
                if is_cap && next_lower {
                    glue = Some(line.text);
                    continue;
                }
                lines.push(line);
            }
            Page { lines, pictures: page.pictures.clone() }
        })
        .collect()
}

/// Turns pages of lines into blocks, document-wide so heading sizes are
/// consistent. Pages are separated by `None`.
pub(super) fn blocks(pages: &[Page]) -> Vec<Option<Block>> {
    let body = body_size(pages);
    let pages = &strip_furniture(pages, body);
    let headings = heading_sizes(pages, body);
    let level_of = |line: &Line| -> Option<u8> {
        if line.size < body * 1.15 || !wordy(&line.text) {
            return None;
        }
        let k = (line.size * 2.0).round() as i32;
        let rank = headings.iter().position(|h| *h == k).unwrap_or(0);
        Some((rank as u8 + 1).min(3))
    };
    let bold_level = (headings.len() as u8 + 1).min(4);

    let mut out = Vec::new();
    for (pi, page) in pages.iter().enumerate() {
        if pi > 0 {
            out.push(None);
        }
        let leading = typical_leading(&page.lines, body);
        let mut pictures: Vec<&Picture> = page.pictures.iter().collect();
        pictures.sort_by(|a, b| b.top.total_cmp(&a.top));
        let mut pictures = pictures.into_iter().peekable();

        let mut kind = Kind::Para;
        let mut text = String::new();
        let mut marker = String::new();
        let mut prev: Option<&Line> = None;
        let flush = |kind: Kind, text: &mut String, marker: &str, out: &mut Vec<Option<Block>>| {
            if text.is_empty() {
                return;
            }
            let t = std::mem::take(text);
            out.push(Some(match kind {
                Kind::Heading(l) => Block::Heading(l, t),
                // A short bold line on its own reads as a sub-heading.
                Kind::Bold if wordy(&t) && t.chars().count() <= 80 && !t.ends_with(['.', ',', ';']) => {
                    Block::Heading(bold_level, t)
                }
                Kind::Bold => Block::Bold(t),
                Kind::Para => Block::Para(t),
                Kind::Bullet => Block::Bullet(t),
                Kind::Numbered => Block::Numbered(marker.to_string(), t),
            }));
        };

        for line in &page.lines {
            while let Some(p) = pictures.next_if(|p| p.top > line.top + 1.0) {
                flush(kind, &mut text, &marker, &mut out);
                out.push(Some(Block::Image(p.file.clone(), p.width_pt)));
                prev = None;
            }

            let (line_kind, content, mark) = if let Some(level) = level_of(line) {
                (Kind::Heading(level), line.text.clone(), String::new())
            } else if let Some(item) = bullet(&line.text) {
                (Kind::Bullet, item.to_string(), String::new())
            } else if let Some((m, item)) = numbered(&line.text) {
                (Kind::Numbered, item, m)
            } else if line.bold {
                (Kind::Bold, line.text.clone(), String::new())
            } else {
                (Kind::Para, line.text.clone(), String::new())
            };

            let new_block = match prev {
                None => true,
                Some(p) => {
                    let gap = p.baseline - line.baseline;
                    let continues_item = matches!(kind, Kind::Bullet | Kind::Numbered)
                        && line_kind == Kind::Para
                        && line.left > p.left - 2.0;
                    let same_kind = line_kind == kind || continues_item;
                    let ended = p.text.ends_with(['.', '!', '?', ':'])
                        && (p.text.chars().count() as f32) < widest(&page.lines) * 0.8;
                    let indented = line_kind == Kind::Para && line.left > p.left + body * 1.2;
                    let new_column = gap < 0.0;
                    !same_kind
                        || matches!(line_kind, Kind::Bullet | Kind::Numbered)
                        || gap > leading * 1.35 * (line.size / body).max(1.0)
                        || new_column
                        || (ended && matches!(kind, Kind::Para | Kind::Bold))
                        || (indented && kind == Kind::Para)
                }
            };
            if new_block {
                flush(kind, &mut text, &marker, &mut out);
                kind = line_kind;
                marker = mark;
            }
            join(&mut text, &content);
            prev = Some(line);
        }
        flush(kind, &mut text, &marker, &mut out);
        for p in pictures {
            out.push(Some(Block::Image(p.file.clone(), p.width_pt)));
        }
    }
    out
}

fn widest(lines: &[Line]) -> f32 {
    lines.iter().map(|l| l.text.chars().count()).max().unwrap_or(0) as f32
}

/// Escapes characters Markdown (CommonMark and pandoc) would treat as markup.
pub(super) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, c) in text.chars().enumerate() {
        let special = matches!(c, '\\' | '*' | '_' | '`' | '[' | ']' | '<' | '>' | '$' | '@' | '^' | '~' | '|' | '#')
            || (i == 0 && matches!(c, '-' | '+' | '='));
        if special {
            out.push('\\');
        }
        out.push(c);
    }
    // "1. " at the start would become a list.
    if let Some((m, _)) = numbered(&out) {
        let digits = m.len() - 1;
        out.insert(digits, '\\');
    }
    out
}

/// Markdown for `blocks`. Page breaks become `page_break` (e.g. a raw OpenXML
/// break for DOCX) when given, and are dropped otherwise.
pub(super) fn to_markdown(blocks: &[Option<Block>], page_break: Option<&str>) -> String {
    // A page break at the very start or end means nothing.
    let start = blocks.iter().position(Option::is_some).unwrap_or(blocks.len());
    let end = blocks.iter().rposition(Option::is_some).map_or(start, |i| i + 1);
    let mut out = String::new();
    let mut prev: Option<&Block> = None;
    for block in &blocks[start..end] {
        // Consecutive items of one list stay a tight list.
        let same_list = matches!(
            (prev, block),
            (Some(Block::Bullet(_)), Some(Block::Bullet(_))) | (Some(Block::Numbered(..)), Some(Block::Numbered(..)))
        );
        if same_list {
            out.pop();
        }
        prev = block.as_ref();
        let piece = match block {
            None => match page_break {
                Some(b) => b.to_string(),
                None => continue,
            },
            Some(Block::Heading(level, t)) => format!("{} {}", "#".repeat(*level as usize), escape(t)),
            Some(Block::Para(t)) => escape(t),
            Some(Block::Bold(t)) => format!("**{}**", escape(t)),
            Some(Block::Bullet(t)) => format!("- {}", escape(t)),
            Some(Block::Numbered(m, t)) => format!("{m} {}", escape(t)),
            Some(Block::Image(file, w)) => format!("![]({file}){{width={:.2}in}}", w / 72.0),
        };
        out.push_str(&piece);
        out.push_str("\n\n");
    }
    out.trim_end().to_string() + "\n"
}

/// Saves the page's images (larger than a thumbnail) into `media` and returns
/// where they sit. Images are named `p<page>-<n>.jpg` (or `.png`).
pub(super) fn page_pictures(doc: &PdfDocument, page: &PdfPage, index: usize, media: &Path) -> Vec<Picture> {
    let mut out = Vec::new();
    for object in page.objects().iter() {
        let Some(image) = object.as_image_object() else { continue };
        let Ok(bounds) = object.bounds() else { continue };
        let rect = bounds.to_rect();
        let (w, h) = (rect.width().value, rect.height().value);
        if w < 24.0 || h < 24.0 {
            continue; // rules, bullets and other decoration
        }
        let Ok(img) = image.get_processed_image(doc).or_else(|_| image.get_raw_image()) else {
            continue;
        };
        // Photos stay compact as JPEG; anything with transparency needs PNG.
        let opaque = !img.color().has_alpha() || img.to_rgba8().pixels().all(|p| p.0[3] == 255);
        let ext = if opaque { "jpg" } else { "png" };
        let img = if opaque { DynamicImage::ImageRgb8(img.into_rgb8()) } else { img };
        let file = format!("p{}-{}.{ext}", index + 1, out.len() + 1);
        if img.save(media.join(&file)).is_ok() {
            out.push(Picture { file, top: rect.top().value, width_pt: w.min(6.5 * 72.0) });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, size: f32, baseline: f32, left: f32) -> Line {
        Line {
            text: text.into(),
            size,
            top: baseline + size * 0.8,
            left,
            baseline,
            bold: false,
        }
    }

    #[test]
    fn finds_headings_paragraphs_and_lists() {
        let lines = vec![
            line("Annual Report", 20.0, 800.0, 72.0),
            line("This is the first paragraph of body text that wraps", 11.0, 770.0, 72.0),
            line("onto a second line of the same para-", 11.0, 756.0, 72.0),
            line("graph.", 11.0, 742.0, 72.0),
            line("Second paragraph after a gap.", 11.0, 720.0, 72.0),
            line("Details", 14.0, 690.0, 72.0),
            line("• first point", 11.0, 670.0, 72.0),
            line("• second point that wraps", 11.0, 656.0, 72.0),
            line("onto another line", 11.0, 642.0, 84.0),
            line("1. numbered item", 11.0, 628.0, 72.0),
            line("12", 11.0, 40.0, 300.0),
        ];
        let pages = [Page { lines, pictures: vec![] }];
        let blocks: Vec<_> = blocks(&pages).into_iter().flatten().collect();
        assert_eq!(
            blocks,
            vec![
                Block::Heading(1, "Annual Report".into()),
                Block::Para(
                    "This is the first paragraph of body text that wraps onto a second line of the same paragraph."
                        .into()
                ),
                Block::Para("Second paragraph after a gap.".into()),
                Block::Heading(2, "Details".into()),
                Block::Bullet("first point".into()),
                Block::Bullet("second point that wraps onto another line".into()),
                Block::Numbered("1.".into(), "numbered item".into()),
            ]
        );
    }

    #[test]
    fn escapes_markup() {
        assert_eq!(escape("# not a heading"), "\\# not a heading");
        assert_eq!(escape("a*b*c [x] <y>"), "a\\*b\\*c \\[x\\] \\<y\\>");
        assert_eq!(escape("2. not a list"), "2\\. not a list");
        assert_eq!(escape("- dash"), "\\- dash");
    }

    #[test]
    fn markdown_with_breaks() {
        let blocks = vec![
            Some(Block::Heading(1, "T".into())),
            None,
            Some(Block::Image("p2-1.png".into(), 144.0)),
            None,
        ];
        let md = to_markdown(&blocks, Some("```{=openxml}\n<w:br/>\n```"));
        assert_eq!(md, "# T\n\n```{=openxml}\n<w:br/>\n```\n\n![](p2-1.png){width=2.00in}\n");
        assert_eq!(to_markdown(&blocks, None), "# T\n\n![](p2-1.png){width=2.00in}\n");
        let list = vec![
            Some(Block::Bullet("a".into())),
            Some(Block::Bullet("b".into())),
            Some(Block::Para("c".into())),
        ];
        assert_eq!(to_markdown(&list, None), "- a\n- b\n\nc\n");
    }
}
