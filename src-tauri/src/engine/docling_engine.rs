//! Layout-aware Markdown through IBM's Docling (optional: used when the
//! `docling` command is installed, e.g. `pip install docling`). Docling reads
//! page layout, tables and scanned pages (OCR), so its Markdown is much better
//! than plain text extraction, at the cost of speed.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::{ext_of, find_sidecar, output_path, tool_command, Action, Engine, Progress};

const INPUTS: &[&str] = &[
    "pdf", "docx", "pptx", "xlsx", "html", "htm", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "webp",
];

pub struct DoclingEngine;

fn docling() -> Option<&'static Path> {
    static DOCLING: OnceLock<Option<PathBuf>> = OnceLock::new();
    DOCLING.get_or_init(|| find_sidecar("docling", "--version")).as_deref()
}

impl Engine for DoclingEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        if INPUTS.contains(&ext_of(path).as_str()) && docling().is_some() {
            vec![Action::new("docling:md", "Markdown (Docling)", "Convert")]
        } else {
            Vec::new()
        }
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        (action_id == "docling:md").then(|| "md".into())
    }

    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
        if action_id != "docling:md" {
            return Err(format!("Unknown action '{action_id}'"));
        }
        let exe = docling().ok_or("Docling isn't installed")?;
        let work = std::env::temp_dir().join(format!("ferryfile-docling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;

        let result = tool_command(exe)
            .args(["--to", "md", "--output"])
            .arg(&work)
            .arg(input)
            .output()
            .map_err(|e| format!("Could not start Docling: {e}"))?;
        let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("document");
        let produced = work.join(format!("{stem}.md"));
        let outcome = if result.status.success() && produced.exists() {
            let out = output_path(input, "", "md");
            std::fs::copy(&produced, &out).map(|_| out).map_err(|e| e.to_string())
        } else {
            let msg = String::from_utf8_lossy(&result.stderr);
            Err(format!("Docling failed: {}", msg.lines().last().unwrap_or("unknown error")))
        };
        let _ = std::fs::remove_dir_all(&work);
        progress(1.0);
        outcome
    }
}
