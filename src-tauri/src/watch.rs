//! Watch folders: files that land in a watched folder run through a recipe or
//! action on their own. A file counts as landed once it stops changing (no
//! events for a moment, same size and time on two checks, and not locked by
//! the app writing it). Each watch is non-recursive, and every output we
//! write is remembered and ignored, so results never trigger another run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::{engine, recipes};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    #[default]
    Recipe,
    Action,
}

/// Where results go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WatchOutput {
    /// A `converted` folder inside the watched folder.
    #[default]
    Subfolder,
    /// Next to the new file.
    Beside,
    /// `output_dir`.
    Folder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WatchFolder {
    pub id: String,
    pub folder: String,
    pub kind: TaskKind,
    /// A recipe id or an action id, per `kind`.
    pub target: String,
    pub output: WatchOutput,
    pub output_dir: Option<String>,
    pub enabled: bool,
}

pub const SUBFOLDER: &str = "converted";
/// Results are written in a hidden work folder first, then moved into place.
const WORK_PREFIX: &str = ".ferryfile-work-";

// ---- when is a new file ready?

/// No events for this long before a file is checked.
const QUIET: Duration = Duration::from_millis(1000);
/// Size and modified time must hold for this long.
const STABLE: Duration = Duration::from_millis(1000);
/// Give up on files that never settle (or stay empty) after this long.
const GIVE_UP: Duration = Duration::from_secs(30 * 60);
/// How long outputs and handled inputs are remembered.
const REMEMBER: Duration = Duration::from_secs(6 * 60 * 60);
const POLL: Duration = Duration::from_millis(400);

/// What a check of a waiting file saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Another program still has it open for writing.
    pub locked: bool,
}

#[derive(Debug)]
struct Pending {
    path: PathBuf,
    first_event: Instant,
    last_event: Instant,
    /// Last observation and when it was first seen unchanged.
    seen: Option<(Observation, Instant)>,
}

/// Decides which new files are ready, and which paths to ignore. Pure logic
/// (time and the file system are passed in) so it can be tested.
#[derive(Debug, Default)]
pub struct Tracker {
    pending: HashMap<PathBuf, Pending>,
    /// Outputs Ferryfile wrote: their events are ignored.
    produced: HashMap<PathBuf, Instant>,
    /// Inputs already handled: later edits to them don't re-run.
    handled: HashMap<PathBuf, Instant>,
}

/// Case-insensitive key, since Windows paths are.
fn key(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

/// Files that are clearly not finished downloads or documents.
pub fn is_temp_name(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name.is_empty() || name.starts_with('.') || name.starts_with('~') {
        return true;
    }
    if path.components().any(|c| c.as_os_str().to_string_lossy().starts_with(WORK_PREFIX)) {
        return true;
    }
    const TEMP_EXTS: &[&str] =
        &["tmp", "temp", "part", "partial", "crdownload", "download", "opdownload", "!ut", "swp", "lock", "lnk"];
    TEMP_EXTS.contains(&engine::ext_of(path).as_str())
}

impl Tracker {
    fn ignored(&self, path: &Path) -> bool {
        let k = key(path);
        is_temp_name(path) || self.produced.contains_key(&k) || self.handled.contains_key(&k)
    }

    /// A file appeared (created, moved in, or renamed to this name).
    pub fn on_created(&mut self, path: &Path, now: Instant) {
        if self.ignored(path) {
            return;
        }
        let p = self.pending.entry(key(path)).or_insert_with(|| Pending {
            path: path.to_path_buf(),
            first_event: now,
            last_event: now,
            seen: None,
        });
        p.last_event = now;
    }

    /// A file changed. Only matters while it's still being written.
    pub fn on_modified(&mut self, path: &Path, now: Instant) {
        if let Some(p) = self.pending.get_mut(&key(path)) {
            p.last_event = now;
            p.seen = None;
        }
    }

    /// A file was deleted or renamed away: a new file by that name is new again.
    pub fn on_removed(&mut self, path: &Path) {
        let k = key(path);
        self.pending.remove(&k);
        self.handled.remove(&k);
        self.produced.remove(&k);
    }

    /// An output is about to be written at `path`; never treat it as new.
    pub fn mark_produced(&mut self, path: &Path, now: Instant) {
        let k = key(path);
        self.pending.remove(&k);
        self.produced.insert(k, now);
    }

    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Files that have settled. `observe` returns `None` for files that are
    /// gone. Ready files are marked handled.
    pub fn poll(&mut self, now: Instant, observe: impl Fn(&Path) -> Option<Observation>) -> Vec<PathBuf> {
        self.produced.retain(|_, t| now.duration_since(*t) < REMEMBER);
        self.handled.retain(|_, t| now.duration_since(*t) < REMEMBER);
        let mut ready = Vec::new();
        self.pending.retain(|_, p| {
            if now.duration_since(p.first_event) > GIVE_UP {
                return false;
            }
            if now.duration_since(p.last_event) < QUIET {
                return true;
            }
            let Some(obs) = observe(&p.path) else { return false };
            if obs.locked || obs.size == 0 {
                p.seen = None;
                return true;
            }
            match p.seen {
                Some((prev, since)) if prev == obs => {
                    if now.duration_since(since) >= STABLE {
                        ready.push(p.path.clone());
                        return false;
                    }
                }
                _ => p.seen = Some((obs, now)),
            }
            true
        });
        for path in &ready {
            self.handled.insert(key(path), now);
        }
        ready.sort();
        ready
    }
}

/// Looks at a waiting file on disk.
fn observe(path: &Path) -> Option<Observation> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    Some(Observation { size: meta.len(), modified: meta.modified().ok(), locked: is_locked(path) })
}

/// True while another program has the file open for writing.
#[cfg(windows)]
fn is_locked(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    // Asking to share only reading fails if someone holds it for writing.
    std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(path).is_err()
}

#[cfg(not(windows))]
fn is_locked(_path: &Path) -> bool {
    false
}

// ---- running

impl WatchFolder {
    /// The steps to run, or why there are none.
    fn steps(&self, all_recipes: &[recipes::Recipe]) -> Result<Vec<String>, String> {
        match self.kind {
            TaskKind::Action if !self.target.is_empty() => Ok(vec![self.target.clone()]),
            TaskKind::Recipe => all_recipes
                .iter()
                .find(|r| r.id == self.target)
                .map(|r| r.steps.clone())
                .ok_or_else(|| "Its recipe no longer exists".into()),
            _ => Err("Pick something to run".into()),
        }
    }

    fn destination(&self, input: &Path) -> Result<PathBuf, String> {
        let here = input.parent().unwrap_or(Path::new(".")).to_path_buf();
        let dir = match self.output {
            WatchOutput::Beside => here,
            WatchOutput::Subfolder => here.join(SUBFOLDER),
            WatchOutput::Folder => match self.output_dir.as_deref().filter(|d| !d.is_empty()) {
                Some(d) => PathBuf::from(d),
                None => return Err("Choose a folder to save to".into()),
            },
        };
        std::fs::create_dir_all(&dir).map_err(|e| format!("Can't write to {}: {e}", dir.display()))?;
        Ok(dir)
    }
}

/// Runs `steps` on `input`, writing the result into `dest`. `before_move` gets
/// the final path before anything appears there.
fn process(input: &Path, steps: &[String], dest: &Path, before_move: &mut dyn FnMut(&Path)) -> Result<PathBuf, String> {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let work = dest.join(format!("{WORK_PREFIX}{}-{n}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let _job = engine::jobs::begin(None);
        let out = engine::with_output_dir(&work, || engine::run_chain(input, steps, &|_| {}))?;
        let name = out.file_name().ok_or("Output has no name")?.to_string_lossy().into_owned();
        let target = engine::unique_in(dest, &name);
        before_move(&target);
        std::fs::rename(&out, &target).map_err(|e| e.to_string())?;
        Ok(target)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Which watch a path belongs to (its parent is the watched folder).
fn owner<'a>(folders: &'a [WatchFolder], path: &Path) -> Option<&'a WatchFolder> {
    let parent = key(path.parent()?);
    folders.iter().find(|f| key(Path::new(&f.folder)) == parent)
}

// ---- notifications

pub struct Done {
    pub input: PathBuf,
    pub result: Result<PathBuf, String>,
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

/// Title and body for one notification covering `done`.
pub fn summary(done: &[Done]) -> Option<(String, String)> {
    let failed: Vec<_> = done.iter().filter(|d| d.result.is_err()).collect();
    match done {
        [] => None,
        [one] => Some(match &one.result {
            Ok(out) => (format!("Converted {}", file_name(&one.input)), format!("Saved {}", file_name(out))),
            Err(e) => (format!("Couldn't convert {}", file_name(&one.input)), e.clone()),
        }),
        many => {
            let ok = many.len() - failed.len();
            let title = if failed.is_empty() {
                format!("Converted {ok} files")
            } else {
                format!("Converted {ok} of {} files", many.len())
            };
            let body = match failed.first() {
                None => "From your watch folders".into(),
                Some(f) => format!(
                    "{} failed. {}: {}",
                    failed.len(),
                    file_name(&f.input),
                    f.result.as_ref().err().map(String::as_str).unwrap_or_default()
                ),
            };
            Some((title, body))
        }
    }
}

fn show(app: &AppHandle, done: &[Done]) {
    use tauri_plugin_notification::NotificationExt;
    if let Some((title, body)) = summary(done) {
        let _ = app.notification().builder().title(title).body(body).show();
    }
}

// ---- the watcher thread

struct State {
    folders: Vec<WatchFolder>,
    watcher: Option<notify::RecommendedWatcher>,
    /// Watch id -> why it isn't running.
    errors: HashMap<String, String>,
    tx: Option<mpsc::Sender<notify::Event>>,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State { folders: vec![], watcher: None, errors: HashMap::new(), tx: None }))
}

/// Starts, stops or changes watches to match `folders`.
pub fn configure(app: &AppHandle, folders: &[WatchFolder]) {
    use notify::Watcher;
    let mut st = state().lock().unwrap();
    st.watcher = None; // drops the old watches
    st.errors.clear();
    st.folders = folders.iter().filter(|f| f.enabled).cloned().collect();
    if st.folders.is_empty() {
        return;
    }
    let tx = match &st.tx {
        Some(tx) => tx.clone(),
        None => {
            let (tx, rx) = mpsc::channel();
            let app = app.clone();
            std::thread::Builder::new().name("watch-folders".into()).spawn(move || worker(app, rx)).ok();
            st.tx = Some(tx.clone());
            tx
        }
    };
    let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            let _ = tx.send(ev);
        }
    });
    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => {
            for f in folders {
                st.errors.insert(f.id.clone(), e.to_string());
            }
            return;
        }
    };
    let mut errors = HashMap::new();
    for f in &st.folders {
        let path = Path::new(&f.folder);
        let err = if !path.is_dir() {
            Some("Folder not found".to_string())
        } else {
            watcher.watch(path, notify::RecursiveMode::NonRecursive).err().map(|e| e.to_string())
        };
        if let Some(e) = err {
            errors.insert(f.id.clone(), e);
        }
    }
    st.errors = errors;
    st.watcher = Some(watcher);
}

fn worker(app: AppHandle, rx: mpsc::Receiver<notify::Event>) {
    run_loop(
        rx,
        || state().lock().unwrap().folders.clone(),
        || recipes::all(&app),
        |done| show(&app, done),
    );
}

/// Turns file events into runs until the event channel closes. Watches and
/// recipes are read fresh for each batch, so Settings changes apply at once.
fn run_loop(
    rx: mpsc::Receiver<notify::Event>,
    folders: impl Fn() -> Vec<WatchFolder>,
    all_recipes: impl Fn() -> Vec<recipes::Recipe>,
    mut on_done: impl FnMut(&[Done]),
) {
    use notify::event::{EventKind, ModifyKind, RenameMode};
    let mut tracker = Tracker::default();
    loop {
        let first = if tracker.is_pending() {
            match rx.recv_timeout(POLL) {
                Ok(ev) => Some(ev),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(ev) => Some(ev),
                Err(_) => return,
            }
        };
        let now = Instant::now();
        for ev in first.into_iter().chain(rx.try_iter()) {
            for path in &ev.paths {
                match ev.kind {
                    EventKind::Create(_)
                    | EventKind::Modify(ModifyKind::Name(RenameMode::To | RenameMode::Both | RenameMode::Any)) => {
                        tracker.on_created(path, now)
                    }
                    EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                        tracker.on_removed(path)
                    }
                    EventKind::Modify(_) => tracker.on_modified(path, now),
                    _ => {}
                }
            }
        }

        let ready = tracker.poll(Instant::now(), observe);
        if ready.is_empty() {
            continue;
        }
        let folders = folders();
        let all_recipes = all_recipes();
        let mut done = Vec::new();
        for input in ready {
            let Some(watch) = owner(&folders, &input) else { continue };
            let Ok(steps) = watch.steps(&all_recipes) else { continue };
            // Files the recipe can't take (other formats, our own outputs) are left alone.
            if engine::plan_chain(&input, &steps).is_err() {
                continue;
            }
            let result = watch
                .destination(&input)
                .and_then(|dest| process(&input, &steps, &dest, &mut |p| tracker.mark_produced(p, Instant::now())));
            done.push(Done { input, result });
        }
        on_done(&done);
    }
}

// ---- commands for Settings

/// Watch id -> why it isn't running, for watches that failed to start.
#[tauri::command]
pub fn watch_errors() -> HashMap<String, String> {
    state().lock().unwrap().errors.clone()
}

#[derive(Serialize)]
pub struct Choice {
    id: String,
    title: String,
    group: String,
}

#[derive(Serialize)]
pub struct Choices {
    recipes: Vec<Choice>,
    actions: Vec<Choice>,
}

/// Recipes and actions a watch folder can run. Actions come from a spread of
/// common file types, since the folder's files aren't known yet.
#[tauri::command]
pub fn watch_choices(app: AppHandle) -> Choices {
    const SAMPLES: &[&str] = &[
        "png", "jpg", "heic", "webp", "gif", "svg", "mp4", "mov", "mkv", "wav", "mp3", "flac", "pdf", "docx",
        "md", "html", "xlsx", "csv", "srt", "zip",
    ];
    let mut seen = std::collections::HashSet::new();
    let mut actions = Vec::new();
    for ext in SAMPLES {
        for a in engine::list_actions(Path::new(&format!("file.{ext}"))) {
            if seen.insert(a.id.clone()) {
                actions.push(Choice { title: a.title(), id: a.id, group: a.group });
            }
        }
    }
    let recipes = recipes::all(&app)
        .into_iter()
        .map(|r| Choice { title: r.name, group: if recipes::is_builtin(&r.id) { "Built-in" } else { "Saved" }.into(), id: r.id })
        .collect();
    Choices { recipes, actions }
}

#[tauri::command]
pub async fn pick_watch_folder(app: AppHandle, title: String) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let mut dialog = app.dialog().file().set_title(title);
    if let Some(w) = app.get_webview_window("settings") {
        dialog = dialog.set_parent(&w);
    }
    dialog.blocking_pick_folder().and_then(|f| f.into_path().ok()).map(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(size: u64, locked: bool) -> Option<Observation> {
        Some(Observation { size, modified: None, locked })
    }
    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn waits_for_quiet_then_stable_size() {
        let t0 = Instant::now();
        let file = Path::new(r"C:\In\Photo.PNG");
        let mut tr = Tracker::default();
        tr.on_created(file, t0);

        // Still getting events: not even checked.
        tr.on_modified(file, t0 + MS(800));
        assert!(tr.poll(t0 + MS(1200), |_| panic!("checked too early")).is_empty());
        // Quiet now; first look records the size.
        assert!(tr.poll(t0 + MS(1900), |_| obs(10, false)).is_empty());
        // Size grew: start over.
        assert!(tr.poll(t0 + MS(2500), |_| obs(20, false)).is_empty());
        // Same size, but not for long enough yet.
        assert!(tr.poll(t0 + MS(3000), |_| obs(20, false)).is_empty());
        let ready = tr.poll(t0 + MS(3600), |_| obs(20, false));
        assert_eq!(ready, vec![file.to_path_buf()], "original case is kept");
        assert!(!tr.is_pending());
    }

    #[test]
    fn locked_or_empty_files_keep_waiting() {
        let t0 = Instant::now();
        let file = Path::new(r"C:\in\big.mov");
        let mut tr = Tracker::default();
        tr.on_created(file, t0);
        for i in 0..5 {
            assert!(tr.poll(t0 + MS(2000 + i * 1000), |_| obs(50, true)).is_empty());
        }
        assert!(tr.poll(t0 + MS(8000), |_| obs(0, false)).is_empty());
        assert!(tr.poll(t0 + MS(9000), |_| obs(50, false)).is_empty());
        assert_eq!(tr.poll(t0 + MS(10_100), |_| obs(50, false)).len(), 1);
    }

    #[test]
    fn deleted_and_stuck_files_are_dropped() {
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        tr.on_created(Path::new(r"C:\in\gone.png"), t0);
        assert!(tr.poll(t0 + MS(1500), |_| None).is_empty());
        assert!(!tr.is_pending());

        tr.on_created(Path::new(r"C:\in\stuck.png"), t0);
        assert!(tr.poll(t0 + GIVE_UP + MS(1), |_| obs(1, true)).is_empty());
        assert!(!tr.is_pending());
    }

    #[test]
    fn outputs_and_handled_files_never_rerun() {
        let t0 = Instant::now();
        let input = Path::new(r"C:\in\photo.png");
        let mut tr = Tracker::default();
        tr.on_created(input, t0);
        assert!(tr.poll(t0 + MS(1100), |_| obs(5, false)).is_empty());
        assert_eq!(tr.poll(t0 + MS(2200), |_| obs(5, false)).len(), 1);

        // Our output appears next to it (any case), and the input is touched again.
        let output = Path::new(r"C:\in\photo-50pct.png");
        tr.mark_produced(output, t0 + MS(2300));
        tr.on_created(Path::new(r"C:\IN\Photo-50pct.PNG"), t0 + MS(2400));
        tr.on_created(input, t0 + MS(2500));
        tr.on_modified(input, t0 + MS(2500));
        assert!(!tr.is_pending());

        // Long after, they're forgotten (memory stays bounded).
        tr.poll(t0 + REMEMBER + MS(3000), |_| None);
        tr.on_created(output, t0 + REMEMBER + MS(3000));
        assert!(tr.is_pending());

        // A handled file that's deleted and dropped in again is new.
        tr.on_removed(input);
        tr.on_created(input, t0 + REMEMBER + MS(4000));
        assert_eq!(tr.pending.len(), 2);
    }

    #[test]
    fn skips_temp_and_work_files() {
        for name in [
            r"C:\in\video.mp4.crdownload",
            r"C:\in\~$report.docx",
            r"C:\in\.hidden.png",
            r"C:\in\x.tmp",
            r"C:\in\.ferryfile-work-1-0\photo.webp",
        ] {
            assert!(is_temp_name(Path::new(name)), "{name}");
        }
        assert!(!is_temp_name(Path::new(r"C:\in\photo.png")));
        let mut tr = Tracker::default();
        tr.on_created(Path::new(r"C:\in\file.part"), Instant::now());
        assert!(!tr.is_pending());
    }

    #[test]
    fn finds_the_owning_watch() {
        let w = |id: &str, folder: &str| WatchFolder { id: id.into(), folder: folder.into(), ..Default::default() };
        let folders = vec![w("a", r"C:\Inbox"), w("b", r"D:\Scans")];
        assert_eq!(owner(&folders, Path::new(r"c:\inbox\x.png")).map(|f| f.id.as_str()), Some("a"));
        // Not recursive: files in subfolders (like `converted`) aren't owned.
        assert!(owner(&folders, Path::new(r"C:\Inbox\converted\x.png")).is_none());
    }

    #[test]
    fn processes_into_destination_and_reports_output_first() {
        let dir = std::env::temp_dir().join(format!("ferryfile-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("pic.png");
        image::RgbaImage::from_pixel(20, 20, image::Rgba([1, 2, 3, 255])).save(&input).unwrap();
        let watch = WatchFolder {
            folder: dir.to_string_lossy().into(),
            kind: TaskKind::Action,
            target: "convert:webp".into(),
            output: WatchOutput::Subfolder,
            enabled: true,
            ..Default::default()
        };
        let dest = watch.destination(&input).unwrap();
        assert_eq!(dest, dir.join(SUBFOLDER));
        let mut announced = Vec::new();
        let out = process(&input, &watch.steps(&[]).unwrap(), &dest, &mut |p| announced.push(p.to_path_buf())).unwrap();
        assert_eq!(out, dest.join("pic.webp"));
        assert_eq!(announced, vec![out.clone()], "output is marked before it appears");
        // The hidden work folder is cleaned up.
        let left: Vec<_> = std::fs::read_dir(&dest).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("pic.webp")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Real file events: a file written in slow chunks runs once, after it's
    /// complete, and its output (next to it, and itself a PNG that "Resize
    /// 50%" would take) never runs again.
    #[test]
    fn end_to_end_with_real_events() {
        use notify::Watcher;
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("ferryfile-watch-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let watch = WatchFolder {
            id: "w".into(),
            folder: dir.to_string_lossy().into(),
            kind: TaskKind::Action,
            target: "resize:50".into(),
            output: WatchOutput::Beside,
            enabled: true,
            ..Default::default()
        };

        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let _ = tx.send(ev);
            }
        })
        .unwrap();
        watcher.watch(&dir, notify::RecursiveMode::NonRecursive).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let folders = vec![watch];
        let worker = std::thread::spawn(move || {
            run_loop(rx, move || folders.clone(), Vec::new, |done| {
                for d in done {
                    done_tx.send((d.input.clone(), d.result.clone())).unwrap();
                }
            })
        });

        // Encode a PNG, then write it in pieces with pauses, like a slow copy.
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 10, 10, 255]))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let input = dir.join("Shot.png");
        let mut f = std::fs::File::create(&input).unwrap();
        for chunk in png.chunks(png.len() / 3 + 1) {
            f.write_all(chunk).unwrap();
            f.flush().unwrap();
            std::thread::sleep(Duration::from_millis(700));
        }
        drop(f);

        let (got_input, result) = done_rx.recv_timeout(Duration::from_secs(20)).expect("file was processed");
        assert_eq!(got_input, input, "original name and case");
        let out = result.expect("resize worked");
        assert_eq!(out, dir.join("Shot-50pct.png"));
        assert_eq!(image::open(&out).unwrap().width(), 32, "ran on the complete file");

        // Nothing else runs: not the output, not the input again.
        assert!(done_rx.recv_timeout(Duration::from_secs(5)).is_err(), "output triggered a re-run");
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["Shot-50pct.png", "Shot.png"], "no work folders left");

        drop(watcher); // closes the channel, ending the loop
        worker.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn summarises_results() {
        let ok = |n: &str| Done { input: n.into(), result: Ok(format!("{n}.webp").into()) };
        let bad = |n: &str| Done { input: n.into(), result: Err("Broken file".into()) };
        assert_eq!(summary(&[]), None);
        assert_eq!(summary(&[ok("a.png")]).unwrap().0, "Converted a.png");
        assert_eq!(summary(&[bad("a.png")]).unwrap(), ("Couldn't convert a.png".into(), "Broken file".into()));
        let (title, body) = summary(&[ok("a"), bad("b"), ok("c")]).unwrap();
        assert_eq!(title, "Converted 2 of 3 files");
        assert_eq!(body, "1 failed. b: Broken file");
    }
}
