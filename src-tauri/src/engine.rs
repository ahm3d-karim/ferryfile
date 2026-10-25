//! Format registry and the `Engine` trait. Each engine knows which actions it
//! offers for a file, what format an action produces, and how to run one.
//! Actions only look at the file extension, so chains can be validated on a
//! virtual path without touching the disk.

use std::path::{Path, PathBuf};

use serde::Serialize;

mod archive_engine;
mod builtin_engine;
mod doc_engine;
mod docling_engine;
mod ffmpeg_engine;
mod image_engine;
pub(crate) mod image_crop;
mod image_meta;
pub(crate) mod media_edit;
pub mod jobs;
mod msoffice_engine;
pub mod naming;
mod ocr;
mod ocr_engine;
mod office_engine;
pub mod params;
mod pdf_docx;
mod pdf_engine;
mod pdf_layout;
mod pdf_ocr;
pub(crate) mod pdf_tools;
pub(crate) mod ranges;
mod subtitle_engine;
mod typeset;

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

/// Extension used for actions that write a folder of files (e.g. PDF pages).
/// No engine accepts it, so such a step can only end a chain.
pub const FOLDER_EXT: &str = "folder";

pub trait Engine: Send + Sync {
    fn actions(&self, path: &Path) -> Vec<Action>;
    /// Extension of the file `action_id` produces from `input`.
    fn output_ext(&self, action_id: &str, input: &Path) -> Option<String>;
    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String>;

    /// An action with a value someone typed in (e.g. `resize:w1280`) that
    /// `actions` doesn't list. `None` = not this engine's; `Some(Err)` = its
    /// kind, but the value is out of range.
    fn custom_action(&self, _action_id: &str, _path: &Path) -> Option<Result<Action, String>> {
        None
    }
    /// "Custom…" entries the drawer can offer for `path`.
    fn custom_forms(&self, _path: &Path) -> Vec<params::CustomForm> {
        Vec::new()
    }
    /// Actions whose id carries values set in an editing view, e.g.
    /// `trim:1.5-10` or `crop:0,0,640,480`, so `actions` can't list them.
    /// Returns the action when `action_id` is valid for `path`.
    fn parse_action(&self, _action_id: &str, _path: &Path) -> Option<Action> {
        None
    }
    /// How many files a batch may run through this action at once. Engines
    /// that drive a single-instance tool (pdfium, Office) keep the default.
    fn max_parallel(&self, _action_id: &str, _input: &Path) -> usize {
        1
    }

    /// Actions that take several files at once and produce one output.
    fn combine_actions(&self, _inputs: &[PathBuf]) -> Vec<Action> {
        Vec::new()
    }
    fn combine(&self, action_id: &str, _inputs: &[PathBuf], _progress: Progress) -> Result<PathBuf, String> {
        Err(format!("'{action_id}' isn't a combine action"))
    }
}

/// Engines in priority order: when two offer the same action id for a file,
/// the first one wins.
fn engines() -> Vec<Box<dyn Engine>> {
    vec![
        Box::new(image_engine::ImageEngine),
        Box::new(ffmpeg_engine::FfmpegEngine),
        Box::new(pdf_engine::PdfEngine),
        Box::new(msoffice_engine::MsOfficeEngine),
        Box::new(office_engine::OfficeEngine),
        Box::new(builtin_engine::BuiltinEngine),
        Box::new(doc_engine::DocEngine),
        Box::new(docling_engine::DoclingEngine),
        Box::new(archive_engine::ArchiveEngine),
        Box::new(subtitle_engine::SubtitleEngine),
        Box::new(ocr_engine::OcrEngine),
    ]
}

pub use ocr_engine::recognized_text;

pub fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub fn list_actions(path: &Path) -> Vec<Action> {
    let mut seen = std::collections::HashSet::new();
    engines()
        .iter()
        .flat_map(|e| e.actions(path))
        .filter(|a| seen.insert(a.id.clone()))
        .collect()
}

pub fn list_combine_actions(inputs: &[PathBuf]) -> Vec<Action> {
    let mut seen = std::collections::HashSet::new();
    engines()
        .iter()
        .flat_map(|e| e.combine_actions(inputs))
        .filter(|a| seen.insert(a.id.clone()))
        .collect()
}

pub fn run_combine(action_id: &str, inputs: &[PathBuf], progress: Progress) -> Result<PathBuf, String> {
    let engine = engines()
        .into_iter()
        .find(|e| e.combine_actions(inputs).iter().any(|a| a.id == action_id))
        .ok_or_else(|| format!("No engine can run '{action_id}' on these files"))?;
    engine.combine(action_id, inputs, progress)
}

/// The engine offering `action_id` for `input`, plus the action itself.
fn find(action_id: &str, input: &Path) -> Result<(Box<dyn Engine>, Action), String> {
    let mut invalid = None;
    for e in engines() {
        let listed = e.actions(input).into_iter().find(|a| a.id == action_id);
        // Values from an editing view (trim, crop, split…).
        if let Some(action) = listed.or_else(|| e.parse_action(action_id, input)) {
            return Ok((e, action));
        }
        // Values typed into a "Custom…" form (resize:w1280, compress:q62…).
        match e.custom_action(action_id, input) {
            Some(Ok(action)) => return Ok((e, action)),
            Some(Err(why)) => invalid = invalid.or(Some(why)),
            None => {}
        }
    }
    Err(invalid.unwrap_or_else(|| format!("No engine can run '{action_id}' on a .{} file", ext_of(input))))
}

/// "Custom…" entries for the groups of `path`'s actions.
pub fn custom_forms(path: &Path) -> Vec<params::CustomForm> {
    let mut seen = std::collections::HashSet::new();
    engines()
        .iter()
        .flat_map(|e| e.custom_forms(path))
        .filter(|f| seen.insert(f.group.clone()))
        .collect()
}

pub fn run_action(action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
    let (engine, _) = find(action_id, input)?;
    engine.run(action_id, input, progress)
}

/// Half the logical CPUs (at least one): for engines that use one core per file.
pub fn half_cpus() -> usize {
    std::thread::available_parallelism().map(|n| n.get() / 2).unwrap_or(1).max(1)
}

/// How many files a batch can run through `steps` at once: the lowest limit
/// of any step, following the format through the chain.
pub fn parallel_limit(input: &Path, steps: &[String]) -> usize {
    let mut current = PathBuf::from(format!("file.{}", ext_of(input)));
    let mut limit = usize::MAX;
    for step in steps {
        let Ok((engine, _)) = find(step, &current) else { return 1 };
        limit = limit.min(engine.max_parallel(step, &current).max(1));
        match engine.output_ext(step, &current) {
            Some(ext) => current.set_extension(ext),
            None => break,
        };
    }
    if limit == usize::MAX { 1 } else { limit }
}

/// Checks that `steps` can run one after another starting from `input`'s
/// format. Returns each step's action and the final extension.
pub fn plan_chain(input: &Path, steps: &[String]) -> Result<(Vec<Action>, String), String> {
    let mut current = PathBuf::from(format!("file.{}", ext_of(input)));
    let mut actions = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let (engine, action) = find(step, &current).map_err(|why| {
            if why.starts_with("No engine") {
                format!("Step {} ({step}) can't run on a .{} file", i + 1, ext_of(&current))
            } else {
                format!("Step {}: {why}", i + 1)
            }
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

/// "Custom…" entries for a step after `steps`.
pub fn next_custom_forms(input: &Path, steps: &[String]) -> Result<Vec<params::CustomForm>, String> {
    let (_, ext) = plan_chain(input, steps)?;
    Ok(custom_forms(Path::new(&format!("file.{ext}"))))
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
            jobs::check()?;
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
    let dir = destination(input);
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let mut candidate = dir.join(format!("{stem}{suffix}.{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}{suffix} ({n}).{ext}"));
        n += 1;
    }
    candidate
}

static OUTPUT_DIR: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

thread_local! {
    /// A recipe's own output folder, for the job running on this thread.
    static OUTPUT_OVERRIDE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Sends this thread's outputs to `dir` (if it exists) instead of the global
/// folder, until the guard is dropped.
pub fn override_output_dir(dir: Option<PathBuf>) -> impl Drop {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            OUTPUT_OVERRIDE.with(|o| *o.borrow_mut() = None);
        }
    }
    OUTPUT_OVERRIDE.with(|o| *o.borrow_mut() = dir);
    Reset
}

/// Where outputs go: `None` means next to each input file.
pub fn set_output_dir(dir: Option<PathBuf>) {
    *OUTPUT_DIR.write().unwrap() = dir;
}

/// `dir/name`, or `dir/<stem> (2).<ext>` and so on if that's taken.
pub fn unique_in(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let p = Path::new(name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = p.extension().and_then(|e| e.to_str()).map(|e| format!(".{e}")).unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|c| !c.exists())
        .unwrap()
}

thread_local! {
    static THREAD_OUTPUT_DIR: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Runs `f` with outputs on this thread going to `dir`, whatever the global
/// setting says. The CLI (`--out`) and watch folders use this.
pub fn with_output_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<PathBuf>);
    impl Drop for Restore {
        fn drop(&mut self) {
            THREAD_OUTPUT_DIR.with(|d| *d.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(THREAD_OUTPUT_DIR.with(|d| d.borrow_mut().replace(dir.to_path_buf())));
    f()
}

/// Extension `action_id` produces from `input`, if some engine offers it.
pub fn action_output_ext(action_id: &str, input: &Path) -> Option<String> {
    let (engine, _) = find(action_id, input).ok()?;
    engine.output_ext(action_id, input)
}

/// The configured output folder if it still exists, else the input's folder.
pub fn destination(input: &Path) -> PathBuf {
    // The CLI's --out and watch folders win over a recipe's own folder.
    if let Some(dir) = THREAD_OUTPUT_DIR.with(|d| d.borrow().clone()) {
        return dir;
    }
    if let Some(dir) = OUTPUT_OVERRIDE.with(|o| o.borrow().clone()).filter(|d| d.is_dir()) {
        return dir;
    }
    match OUTPUT_DIR.read().unwrap().as_ref() {
        Some(dir) if dir.is_dir() => dir.clone(),
        _ => input.parent().unwrap_or_else(|| Path::new(".")).to_path_buf(),
    }
}

/// Creates a new, uniquely named `<stem><suffix>` folder next to the input
/// (or in the configured output folder).
pub fn output_dir(input: &Path, suffix: &str) -> Result<PathBuf, String> {
    let dir = destination(input);
    let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let mut candidate = dir.join(format!("{stem}{suffix}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("{stem}{suffix} ({n})"));
        n += 1;
    }
    std::fs::create_dir(&candidate).map_err(|e| e.to_string())?;
    Ok(candidate)
}

/// A `Command` for a helper tool that never flashes a console window.
pub fn tool_command(exe: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Where to look for a bundled file: next to the app (where Tauri puts
/// sidecars and resources), then the repo's `binaries/` folder in dev builds.
pub fn bundled_candidates(installed_name: &str, dev_name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        out.push(dir.join(installed_name));
    }
    out.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("binaries").join(dev_name));
    out
}

/// Sidecar executable `name` (bundled, else on PATH) that runs with `probe_arg`.
pub fn find_sidecar(name: &str, probe_arg: &str) -> Option<PathBuf> {
    let mut candidates = bundled_candidates(
        &format!("{name}.exe"),
        &format!("{name}-x86_64-pc-windows-msvc.exe"),
    );
    candidates.push(PathBuf::from(name));
    candidates.into_iter().find(|exe| {
        tool_command(exe)
            .arg(probe_arg)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
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

        // LaTeX output works as a recipe step, including after another document step.
        let (_, ext) = plan_chain(Path::new("notes.txt"), &ids(&["convert:docx", "convert:tex"])).unwrap();
        assert_eq!(ext, "tex");
    }

    #[test]
    fn edit_steps_validate_in_chains() {
        // Values travel in the id; engines check them through parse_action.
        let (actions, ext) = plan_chain(Path::new("clip.avi"), &ids(&["trim:5-12.5:precise", "scale:720"])).unwrap();
        assert_eq!(ext, "mp4");
        assert_eq!(actions[0].title(), "Trim 0:05.0–0:12.5 (precise)");
        assert_eq!(plan_chain(Path::new("a.wav"), &ids(&["trim:1-", "convert:mp3"])).unwrap().1, "mp3");
        let (actions, ext) = plan_chain(Path::new("a.jpg"), &ids(&["crop:16x9", "resize:50"])).unwrap();
        assert_eq!((actions[0].title().as_str(), ext.as_str()), ("Crop 16:9, centred", "jpg"));
        assert!(plan_chain(Path::new("a.png"), &ids(&["crop:0,0,640,480", "convert:webp"])).is_ok());
        let (actions, ext) = plan_chain(Path::new("r.pdf"), &ids(&["extract:1-3,5", "rotate:right"])).unwrap();
        assert_eq!((actions[0].title().as_str(), ext.as_str()), ("Extract pages 1–3, 5", "pdf"));
        assert_eq!(plan_chain(Path::new("r.pdf"), &ids(&["split:1-3,8-"])).unwrap().1, FOLDER_EXT);

        // Bad values, or the wrong kind of file, are refused.
        for (file, step) in [
            ("a.mp4", "trim:9-3"),
            ("a.png", "trim:1-2"),
            ("a.svg", "crop:1x1"),
            ("a.png", "crop:0,0,0,5"),
            ("a.mp4", "crop:1x1"),
            ("r.pdf", "extract:3-1"),
            ("r.pdf", "extract:1 - 3"), // ids use the canonical form only
            ("a.png", "extract:1-3"),
        ] {
            assert!(plan_chain(Path::new(file), &ids(&[step])).is_err(), "{step} on {file}");
        }
        // Editing steps aren't listed as drawer buttons.
        assert!(!list_actions(Path::new("a.mp4")).iter().any(|a| a.id.starts_with("trim:")));
    }

    #[test]
    fn batch_limits_follow_the_engine() {
        assert_eq!(parallel_limit(Path::new("a.png"), &ids(&["convert:webp"])), half_cpus());
        assert_eq!(parallel_limit(Path::new("a.mov"), &ids(&["convert:mp4"])), 1);
        assert_eq!(parallel_limit(Path::new("a.wav"), &ids(&["convert:mp3"])), 2);
        // A chain is as limited as its slowest step: video → audio is still video.
        assert_eq!(parallel_limit(Path::new("a.mov"), &ids(&["audio:mp3", "convert:flac"])), 1);
        assert_eq!(parallel_limit(Path::new("a.png"), &ids(&["nope"])), 1);
    }

    #[test]
    fn chain_keeps_only_final_file() {
        let dir = std::env::temp_dir().join(format!("ferryfile-chain-{}", std::process::id()));
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
