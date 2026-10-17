//! Format registry and the `Engine` trait. Each engine knows which actions it
//! offers for a file and how to run one.

use std::path::{Path, PathBuf};

use serde::Serialize;

mod image_engine;

#[derive(Debug, Clone, Serialize)]
pub struct Action {
    /// Stable id parsed by the engine, e.g. `convert:png`, `resize:50`.
    pub id: String,
    pub label: String,
    /// UI grouping: "Convert", "Resize", "Compress".
    pub group: String,
}

impl Action {
    pub fn new(id: &str, label: &str, group: &str) -> Self {
        Self { id: id.into(), label: label.into(), group: group.into() }
    }
}

pub trait Engine: Send + Sync {
    fn actions(&self, path: &Path) -> Vec<Action>;
    fn run(&self, action_id: &str, input: &Path) -> Result<PathBuf, String>;
}

fn engines() -> Vec<Box<dyn Engine>> {
    vec![Box::new(image_engine::ImageEngine)]
}

pub fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn list_actions(path: &Path) -> Vec<Action> {
    engines().iter().flat_map(|e| e.actions(path)).collect()
}

pub fn run_action(action_id: &str, input: &Path) -> Result<PathBuf, String> {
    for e in engines() {
        if e.actions(input).iter().any(|a| a.id == action_id) {
            return e.run(action_id, input);
        }
    }
    Err(format!("No engine can run '{action_id}' on this file"))
}

/// `<stem><suffix>.<ext>` next to the input, never overwriting an existing file.
pub fn output_path(input: &Path, suffix: &str, ext: &str) -> PathBuf {
    let dir = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let mut candidate = dir.join(format!("{stem}{suffix}.{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}{suffix} ({n}).{ext}"));
        n += 1;
    }
    candidate
}
