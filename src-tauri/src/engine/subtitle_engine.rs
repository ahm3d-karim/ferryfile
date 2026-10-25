//! Subtitles: SRT <-> WebVTT and either to plain text, in pure Rust.
//! (Pulling subtitles out of a video lives in the ffmpeg engine.)

use std::path::{Path, PathBuf};

use super::{ext_of, output_path, Action, Engine, Progress};

pub struct SubtitleEngine;

#[derive(Debug, Clone, PartialEq)]
struct Cue {
    start_ms: u64,
    end_ms: u64,
    text: String,
}

impl Engine for SubtitleEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        let ext = ext_of(path);
        if ext != "srt" && ext != "vtt" {
            return vec![];
        }
        let mut out = Vec::new();
        if ext == "srt" {
            out.push(Action::new("convert:vtt", "VTT", "Convert"));
        } else {
            out.push(Action::new("convert:srt", "SRT", "Convert"));
        }
        out.push(Action::new("convert:txt", "Plain text", "Convert"));
        out
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        action_id.strip_prefix("convert:").map(str::to_string)
    }

    fn run(&self, action_id: &str, input: &Path, _progress: Progress) -> Result<PathBuf, String> {
        let bytes = std::fs::read(input).map_err(|e| e.to_string())?;
        let cues = parse(&decode(&bytes));
        if cues.is_empty() {
            return Err("No subtitles found in this file".into());
        }
        let (text, ext) = match action_id {
            "convert:srt" => (to_srt(&cues), "srt"),
            "convert:vtt" => (to_vtt(&cues), "vtt"),
            "convert:txt" => (to_txt(&cues), "txt"),
            other => return Err(format!("Unknown action '{other}'")),
        };
        let out = output_path(input, "", ext);
        std::fs::write(&out, text).map_err(|e| e.to_string())?;
        Ok(out)
    }
}

/// Subtitle files come in all encodings: UTF-8 (with or without BOM),
/// UTF-16 from some Windows tools, and Windows-1252 for older downloads.
fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, true);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, false);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| cp1252(b)).collect(),
    }
}

fn cp1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8D}', 'Ž', '\u{8F}', '\u{90}',
        '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9D}', 'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9F => HIGH[(b - 0x80) as usize],
        _ => b as char, // Latin-1 matches Unicode for everything else
    }
}

/// Parses SRT or WebVTT. Blocks without a `-->` line (the WEBVTT header,
/// NOTE, STYLE, REGION) are skipped; cue settings after the end time are dropped.
fn parse(text: &str) -> Vec<Cue> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = Vec::new();
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        let Some(i) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let (a, b) = lines[i].split_once("-->").unwrap();
        let end = b.split_whitespace().next().unwrap_or("");
        let (Some(start_ms), Some(end_ms)) = (parse_time(a.trim()), parse_time(end)) else { continue };
        let body = lines[i + 1..].join("\n").trim_end().to_string();
        cues.push(Cue { start_ms, end_ms, text: body });
    }
    cues
}

/// `hh:mm:ss,mmm`, `hh:mm:ss.mmm` or `mm:ss.mmm` to milliseconds.
fn parse_time(s: &str) -> Option<u64> {
    let s = s.replace(',', ".");
    let (hms, frac) = s.split_once('.').unwrap_or((&s, "0"));
    let parts: Vec<u64> = hms.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    let secs = match parts.as_slice() {
        [h, m, s] => h * 3600 + m * 60 + s,
        [m, s] => m * 60 + s,
        _ => return None,
    };
    // "5" -> 500 ms, "05" -> 50 ms, "005" -> 5 ms
    let digits: String = frac.chars().take(3).collect();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let ms = format!("{digits:0<3}").parse::<u64>().ok()?;
    Some(secs * 1000 + ms)
}

fn fmt_time(ms: u64, sep: char) -> String {
    format!("{:02}:{:02}:{:02}{sep}{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

/// Removes markup between `open` and `close` unless `keep` says to keep it.
fn strip_between(text: &str, open: char, close: char, keep: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i]);
        match rest[i..].find(close) {
            Some(j) => {
                let tag = &rest[i..i + j + close.len_utf8()];
                if keep(tag) {
                    out.push_str(tag);
                }
                rest = &rest[i + j + close.len_utf8()..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// `<i>`, `<b>`, `<u>` work in both formats; anything else is dropped.
fn is_basic_style(tag: &str) -> bool {
    matches!(tag.to_ascii_lowercase().as_str(), "<i>" | "</i>" | "<b>" | "</b>" | "<u>" | "</u>")
}

/// Escapes every "<" that doesn't start an `<i>`/`<b>`/`<u>` tag.
fn escape_vtt(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, ch) in s.char_indices() {
        if ch == '<' {
            let rest = s[i..].to_ascii_lowercase();
            let tag = ["<i>", "</i>", "<b>", "</b>", "<u>", "</u>"].iter().any(|t| rest.starts_with(t));
            out.push_str(if tag { "<" } else { "&lt;" });
        } else {
            out.push(ch);
        }
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").replace("&amp;", "&")
}

fn to_srt(cues: &[Cue]) -> String {
    let mut out = String::new();
    for (n, c) in cues.iter().enumerate() {
        let text = decode_entities(&strip_between(&c.text, '<', '>', is_basic_style));
        out.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            n + 1,
            fmt_time(c.start_ms, ','),
            fmt_time(c.end_ms, ','),
            text.trim_end()
        ));
    }
    out
}

fn to_vtt(cues: &[Cue]) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for c in cues {
        // Drop ASS-style overrides ({\an8}) and SRT-only tags like <font>.
        let text = strip_between(&c.text, '{', '}', |t| !t.starts_with("{\\"));
        let text = strip_between(&text, '<', '>', is_basic_style);
        // In VTT a bare "&" or "<" would be read as markup.
        let text = escape_vtt(&text.replace('&', "&amp;"));
        let text = text.replace("-->", "->"); // a cue can't contain the arrow
        out.push_str(&format!(
            "{} --> {}\n{}\n\n",
            fmt_time(c.start_ms, '.'),
            fmt_time(c.end_ms, '.'),
            text.trim_end()
        ));
    }
    out
}

/// Just the words, one line per subtitle line. Repeated lines (rolling
/// auto-captions repeat each line in the next cue) are written once.
fn to_txt(cues: &[Cue]) -> String {
    let mut out = String::new();
    let mut last = String::new();
    for c in cues {
        let text = strip_between(&c.text, '{', '}', |_| false);
        let text = decode_entities(&strip_between(&text, '<', '>', |_| false));
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if line != last {
                out.push_str(line);
                out.push('\n');
                last = line.to_string();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\r\n00:00:01,000 --> 00:00:02,500\r\n<i>Hello</i> {\\an8}there\r\n\r\n2\r\n00:00:03,000 --> 00:00:04,000\r\nTom & Jerry\r\nsecond line\r\n";
    const VTT: &str = "WEBVTT\n\nNOTE a comment\n\nintro\n00:01.000 --> 00:02.500 align:start\n<v Ann>Hello</v> <c.yellow>there</c>\n\n00:00:03.5 --> 00:00:04.000\n&lt;3 you\n";

    #[test]
    fn parses_both_formats() {
        let srt = parse(SRT);
        assert_eq!(srt.len(), 2);
        assert_eq!((srt[0].start_ms, srt[0].end_ms), (1000, 2500));
        assert_eq!(srt[1].text, "Tom & Jerry\nsecond line");

        let vtt = parse(VTT);
        assert_eq!(vtt.len(), 2);
        assert_eq!((vtt[0].start_ms, vtt[0].end_ms), (1000, 2500));
        assert_eq!(vtt[1].start_ms, 3500);
    }

    #[test]
    fn converts() {
        let vtt = to_vtt(&parse(SRT));
        assert!(vtt.starts_with("WEBVTT\n\n00:00:01.000 --> 00:00:02.500\n<i>Hello</i> there\n"), "{vtt}");
        assert!(vtt.contains("Tom &amp; Jerry"));

        let srt = to_srt(&parse(VTT));
        assert!(srt.starts_with("1\n00:00:01,000 --> 00:00:02,500\nHello there\n\n2\n00:00:03,500"), "{srt}");
        assert!(srt.contains("<3 you"));

        assert_eq!(to_txt(&parse(SRT)), "Hello there\nTom & Jerry\nsecond line\n");
        // Round trip keeps timing and words.
        assert_eq!(parse(&to_srt(&parse(&to_vtt(&parse(SRT)))))[1].text, "Tom & Jerry\nsecond line");
    }

    #[test]
    fn dedupes_rolling_captions() {
        let rolling = "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\none\n\n00:00:02.000 --> 00:00:03.000\none\ntwo\n";
        assert_eq!(to_txt(&parse(rolling)), "one\ntwo\n");
    }

    #[test]
    fn decodes_legacy_encodings() {
        assert_eq!(decode(b"caf\xe9 \x93hi\x94"), "café “hi”");
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'o', b'k']), "ok");
        assert_eq!(decode(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
    }

    #[test]
    fn runs_on_files() {
        let dir = std::env::temp_dir().join(format!("ferryfile-subs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("movie.srt");
        std::fs::write(&input, SRT).unwrap();
        let vtt = SubtitleEngine.run("convert:vtt", &input, &|_| {}).unwrap();
        assert_eq!(vtt.file_name().unwrap(), "movie.vtt");
        let txt = SubtitleEngine.run("convert:txt", &vtt, &|_| {}).unwrap();
        assert_eq!(std::fs::read_to_string(txt).unwrap(), "Hello there\nTom & Jerry\nsecond line\n");

        let empty = dir.join("empty.srt");
        std::fs::write(&empty, "nothing here").unwrap();
        assert!(SubtitleEngine.run("convert:vtt", &empty, &|_| {}).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
