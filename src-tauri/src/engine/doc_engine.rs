//! Text document conversions backed by pandoc (bundled sidecar or PATH).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::{ext_of, find_sidecar, output_path, tool_command, Action, Engine, Progress};

/// Input extension -> pandoc reader.
const INPUTS: &[(&str, &str)] = &[
    ("md", "markdown"),
    ("markdown", "markdown"),
    ("txt", "markdown"),
    ("docx", "docx"),
    ("odt", "odt"),
    ("epub", "epub"),
    ("html", "html"),
    ("htm", "html"),
    ("rtf", "rtf"),
    ("rst", "rst"),
    ("org", "org"),
    ("tex", "latex"),
    ("typ", "typst"),
    ("ipynb", "ipynb"),
    ("pptx", "pptx"),
    ("xlsx", "xlsx"),
    ("csv", "csv"),
    ("tsv", "tsv"),
];

/// Output extension, label, pandoc writer.
const OUTPUTS: &[(&str, &str, &str)] = &[
    ("docx", "DOCX", "docx"),
    ("odt", "ODT", "odt"),
    ("md", "Markdown", "gfm"),
    ("html", "HTML", "html"),
    ("epub", "EPUB", "epub"),
    ("rtf", "RTF", "rtf"),
    ("txt", "Text", "plain"),
    ("tex", "LaTeX", "latex"),
];

pub struct DocEngine;

pub(super) fn pandoc() -> Result<&'static Path, String> {
    static PANDOC: OnceLock<Option<PathBuf>> = OnceLock::new();
    PANDOC
        .get_or_init(|| find_sidecar("pandoc", "--version"))
        .as_deref()
        .ok_or_else(|| "pandoc wasn't found. Run scripts/fetch-doc-tools.ps1 or install pandoc on PATH.".into())
}

pub(super) fn reader_for(ext: &str) -> Option<&'static str> {
    INPUTS.iter().find(|(e, _)| *e == ext).map(|(_, r)| *r)
}

impl Engine for DocEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        let ext = ext_of(path);
        if reader_for(&ext).is_none() {
            return Vec::new();
        }
        // Skip "conversions" into the same format (md -> md, htm -> html, ...).
        let same = match ext.as_str() {
            "markdown" => "md",
            "htm" => "html",
            other => other,
        };
        OUTPUTS
            .iter()
            .filter(|(out, _, _)| *out != same)
            .map(|(out, label, _)| Action::new(&format!("convert:{out}"), label, "Convert"))
            .collect()
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        match action_id.split_once(':')? {
            ("convert", ext) => Some(ext.to_string()),
            _ => None,
        }
    }

    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
        let exe = pandoc()?;
        let reader = reader_for(&ext_of(input)).ok_or("Unsupported input")?;
        let (_, ext) = action_id.split_once(':').ok_or("bad action id")?;
        let (_, _, writer) = OUTPUTS
            .iter()
            .find(|(e, _, _)| *e == ext)
            .ok_or_else(|| format!("Unsupported target '{ext}'"))?;

        let out = output_path(input, "", ext);
        let mut cmd = tool_command(exe);
        cmd.arg("--from").arg(reader).arg("--to").arg(writer).arg("--standalone");
        if matches!(*writer, "gfm" | "html" | "latex") {
            // Keep images from DOCX/ODT/EPUB inputs next to the output.
            let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("doc");
            cmd.arg(format!("--extract-media={stem}-media"));
        }
        // Run from the output folder so extracted media paths stay relative.
        if let Some(dir) = out.parent() {
            cmd.current_dir(dir);
        }
        let result = cmd.arg("--output").arg(&out).arg(input).output().map_err(|e| e.to_string())?;
        if !result.status.success() {
            let _ = std::fs::remove_file(&out);
            let msg = String::from_utf8_lossy(&result.stderr);
            return Err(format!("pandoc failed: {}", msg.lines().last().unwrap_or("unknown error")));
        }
        progress(1.0);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_targets() {
        let ids: Vec<_> = DocEngine.actions(Path::new("notes.md")).into_iter().map(|a| a.id).collect();
        assert!(ids.contains(&"convert:docx".into()));
        assert!(!ids.contains(&"convert:md".into()));
        assert!(DocEngine.actions(Path::new("x.png")).is_empty());
        for input in ["notes.txt", "deck.pptx", "sheet.xlsx", "data.csv", "nb.ipynb"] {
            let ids: Vec<_> = DocEngine.actions(Path::new(input)).into_iter().map(|a| a.id).collect();
            assert!(ids.contains(&"convert:md".into()), "{input} should offer Markdown");
        }
    }

    #[test]
    fn latex_targets() {
        for input in ["notes.md", "notes.txt", "report.docx", "page.html", "doc.odt"] {
            let ids: Vec<_> = DocEngine.actions(Path::new(input)).into_iter().map(|a| a.id).collect();
            assert!(ids.contains(&"convert:tex".into()), "{input} should offer LaTeX");
        }
        let ids: Vec<_> = DocEngine.actions(Path::new("paper.tex")).into_iter().map(|a| a.id).collect();
        assert!(!ids.contains(&"convert:tex".into()));
        assert_eq!(DocEngine.output_ext("convert:tex", Path::new("notes.md")).as_deref(), Some("tex"));
    }

    /// Needs pandoc, so opt-in: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn latex_is_standalone() {
        let dir = std::env::temp_dir().join(format!("ferryfile-tex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let md = dir.join("notes.md");
        std::fs::write(&md, "# Title\n\nSome **bold** text.\n").unwrap();
        let txt = dir.join("plain.txt");
        std::fs::write(&txt, "Just *some* text.\n").unwrap();
        let noop = |_: f32| {};
        for (input, expect) in [(&md, "\\textbf{bold}"), (&txt, "\\emph{some}")] {
            let out = DocEngine.run("convert:tex", input, &noop).unwrap();
            assert_eq!(ext_of(&out), "tex");
            let body = std::fs::read_to_string(&out).unwrap();
            for needle in ["\\documentclass", "\\begin{document}", "\\end{document}", expect] {
                assert!(body.contains(needle), "{input:?} -> tex missing {needle}:\n{body}");
            }
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Needs pandoc, so opt-in: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn markdown_round_trip() {
        let dir = std::env::temp_dir().join(format!("ferryfile-doc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let md = dir.join("notes.md");
        std::fs::write(&md, "# Title\n\nSome **bold** text.\n").unwrap();
        let noop = |_: f32| {};
        for action in DocEngine.actions(&md) {
            let out = DocEngine.run(&action.id, &md, &noop).unwrap_or_else(|e| panic!("{}: {e}", action.id));
            assert!(std::fs::metadata(&out).unwrap().len() > 0, "{} is empty", action.id);
        }
        let docx = dir.join("notes.docx");
        let back = DocEngine.run("convert:md", &docx, &noop).unwrap();
        assert!(std::fs::read_to_string(back).unwrap().contains("**bold**"));

        // New Markdown sources: PPTX (made by pandoc itself), CSV, plain text.
        let pptx = dir.join("deck.pptx");
        let made = tool_command(pandoc().unwrap())
            .arg(&md)
            .arg("-o")
            .arg(&pptx)
            .status()
            .unwrap();
        assert!(made.success());
        let csv = dir.join("data.csv");
        std::fs::write(&csv, "name,qty\napple,3\npear,5\n").unwrap();
        let txt = dir.join("plain.txt");
        std::fs::write(&txt, "Just *some* text.\n").unwrap();
        for (input, expect) in [(&pptx, "Title"), (&csv, "pear"), (&txt, "some")] {
            let out = DocEngine.run("convert:md", input, &noop).unwrap();
            let body = std::fs::read_to_string(&out).unwrap();
            assert!(body.contains(expect), "{input:?} -> md lost content:\n{body}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
