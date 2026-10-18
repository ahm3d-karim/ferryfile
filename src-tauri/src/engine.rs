//! Format registry and the `Engine` trait. Each engine knows which actions it
//! offers for a file, what format an action produces, and how to run one.
//! Actions only look at the file extension, so chains can be validated on a
//! virtual path without touching the disk.

use std::path::{Path, PathBuf};

use serde::Serialize;

mod ffmpeg_engine;
mod image_engine;

/// Reports progress of a running action as a fraction in `0.0..=1.0`.
pub type Progress<'a> = &'a dyn Fn(f32);

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

    /// Human-readable step name, e.g. "Resize 50%".
    pub fn title(&self) -> String {
        format!("{} {}", self.group, self.label)
    }
}

pub trait Engine: Send + Sync {
    fn actions(&self, path: &Path) -> Vec<Action>;
    /// Extension of the file `action_id` produces from `input`.
    fn output_ext(&self, action_id: &str, input: &Path) -> Option<String>;
    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String>;
}

fn engines() -> Vec<Box<dyn Engine>> {
    vec![
        Box::new(image_engine::ImageEngine),
        Box::new(ffmpeg_engine::FfmpegEngine),
    ]
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

/// The engine offering `action_id` for `input`, plus the action itself.
fn find(action_id: &str, input: &Path) -> Option<(Box<dyn Engine>, Action)> {
    engines().into_iter().find_map(|e| {
        let action = e.actions(input).into_iter().find(|a| a.id == action_id)?;
        Some((e, action))
    })
}

pub fn run_action(action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
    let (engine, _) = find(action_id, input)
        .ok_or_else(|| format!("No engine can run '{action_id}' on this file"))?;
    engine.run(action_id, input, progress)
}

/// Checks that `steps` can run one after another starting from `input`'s
/// format. Returns each step's action and the final extension.
pub fn plan_chain(input: &Path, steps: &[String]) -> Result<(Vec<Action>, String), String> {
    let mut current = PathBuf::from(format!("file.{}", ext_of(input)));
    let mut actions = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let (engine, action) = find(step, &current).ok_or_else(|| {
            format!("Step {} ({step}) can't run on a .{} file", i + 1, ext_of(&current))
        })?;
        let ext = engine
            .output_ext(step, &current)
            .ok_or_else(|| format!("Unknown output of step {}", i + 1))?;
        current.set_extension(ext);
        actions.push(action);
    }
    Ok((actions, ext_of(&current)))
}

/// Actions that could follow `steps` when starting from `input`.
pub fn next_actions(input: &Path, steps: &[String]) -> Result<Vec<Action>, String> {
    let (_, ext) = plan_chain(input, steps)?;
    Ok(list_actions(Path::new(&format!("file.{ext}"))))
}

/// Runs `steps` in order, feeding each output into the next step. Only the
/// final file is kept; intermediate files are removed, also on failure.
pub fn run_chain(input: &Path, steps: &[String], progress: Progress) -> Result<PathBuf, String> {
    if steps.is_empty() {
        return Err("This recipe has no steps".into());
    }
    plan_chain(input, steps)?;

    let n = steps.len() as f32;
    let mut intermediates = Vec::new();
    let mut current = input.to_path_buf();
    let result = (|| {
        for (i, step) in steps.iter().enumerate() {
            let report = |f: f32| progress((i as f32 + f) / n);
            let out = run_action(step, &current, &report)?;
            report(1.0); // not every engine reports progress
            if i + 1 < steps.len() {
                intermediates.push(out.clone());
            }
            current = out;
        }
        Ok(current.clone())
    })();
    for path in &intermediates {
        let _ = std::fs::remove_file(path);
    }
    result
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn plans_chains_by_format() {
        let png = Path::new("photo.png");
        let (actions, ext) = plan_chain(png, &ids(&["resize:50", "convert:webp"])).unwrap();
        assert_eq!(ext, "webp");
        assert_eq!(actions[0].title(), "Resize 50%");

        // compress produces a JPG, and a JPG can't be "converted" to JPG.
        assert!(plan_chain(png, &ids(&["compress:75", "convert:jpg"])).is_err());
        // Video actions don't apply to images.
        assert!(plan_chain(png, &ids(&["scale:720"])).is_err());

        let next = next_actions(Path::new("clip.mov"), &ids(&["audio:mp3"])).unwrap();
        assert!(next.iter().any(|a| a.id == "convert:flac"));
    }

    #[test]
    fn chain_keeps_only_final_file() {
        let dir = std::env::temp_dir().join(format!("skiff-chain-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("pic.png");
        image::RgbaImage::from_pixel(40, 40, image::Rgba([1, 2, 3, 255])).save(&input).unwrap();

        let fractions = std::cell::RefCell::new(Vec::new());
        let out = run_chain(&input, &ids(&["resize:50", "convert:webp"]), &|f| {
            fractions.borrow_mut().push(f)
        })
        .unwrap();

        assert_eq!(ext_of(&out), "webp");
        assert_eq!(image::open(&out).unwrap().width(), 20);
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        files.sort();
        assert_eq!(files, vec!["pic-50pct.webp", "pic.png"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
