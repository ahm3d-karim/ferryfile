//! Command line: `ferryfile convert photo.heic --to jpg`, `ferryfile recipe
//! web-image *.png --json` and so on. It runs the same engine and recipes as
//! the drawer, without starting the tray app.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{engine, recipes};

const HELP: &str = "\
Ferryfile: offline file converter. With no arguments it starts the tray app.

Usage:
  ferryfile actions <file>                    list the actions that apply to a file
  ferryfile convert <files...> --to <format>  convert, e.g. --to webp, --to mp3, --to pdf
  ferryfile run-action <id> <files...>        run any action id from `actions`
  ferryfile recipes                           list built-in and saved recipes
  ferryfile recipe <name|id> <files...>       run a recipe on each file

Options:
  -o, --out <dir>   save results in <dir> (created if needed). Default: next to each file
      --json        print machine-readable JSON instead of text
  -h, --help        show this help
  -V, --version     print the version

Exit code: 0 when every file worked, 1 when any failed, 2 for a usage error.";

/// Same identifier as `tauri.conf.json`, so the CLI reads the app's saved recipes.
const APP_ID: &str = "app.ferryfile.desktop";

/// Job id the CLI runs under, so Ctrl+C can cancel it (and stop ffmpeg).
const CLI_JOB: &str = "cli";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    Action(String),
    /// `convert --to <format>`: resolved per file to a matching action.
    Convert(String),
    Recipe(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Actions(PathBuf),
    Recipes,
    Run { job: Job, files: Vec<PathBuf> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub json: bool,
    pub out: Option<PathBuf>,
}

/// Arguments the tray app itself understands. Anything else is a CLI call.
/// `--open` comes from the Explorer right-click entry and Send to.
const GUI_ARGS: &[&str] = &[crate::explorer::OPEN_FLAG];

/// Reads `args` (without the program name). `Ok(None)` means: start the app.
pub fn parse(args: &[String]) -> Result<Option<Invocation>, String> {
    let Some(first) = args.first() else { return Ok(None) };
    // Hand-offs to the app: its own flags, or a file path (e.g. "Open with").
    if GUI_ARGS.contains(&first.as_str()) || (!first.starts_with('-') && Path::new(first).exists()) {
        return Ok(None);
    }

    let mut json = false;
    let mut out = None;
    let mut to = None;
    let mut help = false;
    let mut version = false;
    let mut positional = Vec::new();
    let mut it = args.iter();
    let mut only_positional = false;
    while let Some(arg) = it.next() {
        if only_positional || !arg.starts_with('-') || arg == "-" {
            positional.push(arg.clone());
            continue;
        }
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag {
            "--" => only_positional = true,
            "--json" => json = true,
            "-o" | "--out" => out = Some(PathBuf::from(value("--out")?)),
            "-t" | "--to" => to = Some(value("--to")?),
            "-h" | "--help" => help = true,
            "-V" | "--version" => version = true,
            _ => return Err(format!("Unknown option '{arg}'")),
        }
    }

    let invocation = |command| Ok(Some(Invocation { command, json, out: out.clone() }));
    if help {
        return invocation(Command::Help);
    }
    if version {
        return invocation(Command::Version);
    }
    let (name, rest) = positional.split_first().ok_or("Missing a command. See `ferryfile --help`")?;
    let files = |from: &[String]| -> Result<Vec<PathBuf>, String> {
        if from.is_empty() {
            return Err(format!("`{name}` needs at least one file"));
        }
        Ok(from.iter().map(PathBuf::from).collect())
    };
    if to.is_some() && name != "convert" {
        return Err("--to only works with `convert`".into());
    }
    let command = match name.as_str() {
        "help" => Command::Help,
        "version" => Command::Version,
        "recipes" => {
            if !rest.is_empty() {
                return Err("`recipes` takes no files. To run one: ferryfile recipe <name> <files...>".into());
            }
            Command::Recipes
        }
        "actions" => match rest {
            [file] => Command::Actions(PathBuf::from(file)),
            [] => return Err("`actions` needs a file".into()),
            _ => return Err("`actions` takes one file".into()),
        },
        "convert" => {
            let fmt = to.ok_or("`convert` needs --to <format>, e.g. --to png")?;
            Command::Run { job: Job::Convert(normalize_format(&fmt)), files: files(rest)? }
        }
        "run-action" | "run" => {
            let (id, rest) = rest.split_first().ok_or("`run-action` needs an action id and files")?;
            Command::Run { job: Job::Action(id.clone()), files: files(rest)? }
        }
        "recipe" => {
            let (id, rest) = rest.split_first().ok_or("`recipe` needs a recipe name and files")?;
            Command::Run { job: Job::Recipe(id.clone()), files: files(rest)? }
        }
        other => return Err(format!("Unknown command '{other}'. See `ferryfile --help`")),
    };
    invocation(command)
}

/// `.JPEG` -> `jpg`, `markdown` -> `md`.
fn normalize_format(fmt: &str) -> String {
    let f = fmt.trim().trim_start_matches('.').to_ascii_lowercase();
    match f.as_str() {
        "jpeg" => "jpg".into(),
        "tif" => "tiff".into(),
        "markdown" => "md".into(),
        "htm" => "html".into(),
        _ => f,
    }
}

/// The action that turns `file` into `fmt`: `convert:<fmt>` if offered, else
/// another convert-style action that produces it (e.g. PDF -> PNG pages).
fn convert_action(file: &Path, fmt: &str) -> Result<String, String> {
    // Actions that keep the format (resize, compress…) aren't conversions.
    if engine::ext_of(file) == fmt {
        return Err(format!("It's already a .{fmt} file"));
    }
    let actions = engine::list_actions(file);
    let direct = format!("convert:{fmt}");
    if actions.iter().any(|a| a.id == direct) {
        return Ok(direct);
    }
    let produces = |a: &&engine::Action| engine::action_output_ext(&a.id, file).as_deref() == Some(fmt);
    actions
        .iter()
        .filter(produces)
        .find(|a| a.group == "Convert")
        .or_else(|| actions.iter().find(produces))
        .map(|a| a.id.clone())
        .ok_or_else(|| {
            let ext = engine::ext_of(file);
            format!("Can't convert a .{ext} file to {fmt}. `ferryfile actions <file>` lists what works")
        })
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub input: String,
    pub output: Option<String>,
    pub status: &'static str,
    pub error: Option<String>,
}

fn config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join(APP_ID))
}

fn run_one(job: &Job, input: &Path, recipe_steps: Option<&[String]>, progress: engine::Progress) -> Result<PathBuf, String> {
    if !input.is_file() {
        return Err(if input.exists() { "Not a file".into() } else { "File not found".into() });
    }
    match job {
        Job::Action(id) => engine::run_action(id, input, progress),
        Job::Convert(fmt) => engine::run_action(&convert_action(input, fmt)?, input, progress),
        Job::Recipe(_) => engine::run_chain(input, recipe_steps.unwrap_or_default(), progress),
    }
}

fn display(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Runs the command. Returns the process exit code.
pub fn execute(inv: Invocation) -> i32 {
    let mut stdout = std::io::stdout().lock();
    match inv.command {
        Command::Help => {
            let _ = writeln!(stdout, "{HELP}");
            0
        }
        Command::Version => {
            let _ = writeln!(stdout, "ferryfile {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::Actions(file) => {
            if !file.is_file() {
                return usage(&format!("File not found: {}", display(&file)));
            }
            let actions = engine::list_actions(&file);
            if inv.json {
                print_json(&mut stdout, &actions);
            } else if actions.is_empty() {
                let _ = writeln!(stdout, "No actions for .{} files", engine::ext_of(&file));
            } else {
                let width = actions.iter().map(|a| a.id.len()).max().unwrap_or(0);
                for a in &actions {
                    let _ = writeln!(stdout, "{:width$}  {}", a.id, a.title());
                }
            }
            0
        }
        Command::Recipes => {
            let list = config_dir().map(|d| recipes::all_in(&d)).unwrap_or_else(|| recipes::all_in(Path::new("")));
            if inv.json {
                #[derive(Serialize)]
                struct Row<'a> {
                    id: &'a str,
                    name: &'a str,
                    steps: &'a [String],
                    builtin: bool,
                }
                let rows: Vec<_> = list
                    .iter()
                    .map(|r| Row { id: &r.id, name: &r.name, steps: &r.steps, builtin: recipes::is_builtin(&r.id) })
                    .collect();
                print_json(&mut stdout, &rows);
            } else {
                let width = list.iter().map(|r| r.id.len()).max().unwrap_or(0);
                for r in &list {
                    let _ = writeln!(stdout, "{:width$}  {}  ({})", r.id, r.name, r.steps.join(" -> "));
                }
            }
            0
        }
        Command::Run { job, files } => {
            let steps = match &job {
                Job::Recipe(q) => {
                    let list = config_dir().map(|d| recipes::all_in(&d)).unwrap_or_default();
                    match recipes::lookup(list, q) {
                        Some(r) => Some(r.steps),
                        None => return usage(&format!("No recipe called '{q}'. `ferryfile recipes` lists them")),
                    }
                }
                _ => None,
            };
            let out_dir = match &inv.out {
                Some(dir) => match std::fs::create_dir_all(dir).and_then(|_| std::path::absolute(dir)) {
                    Ok(d) => Some(d),
                    Err(e) => return usage(&format!("Can't use output folder {}: {e}", display(dir))),
                },
                None => None,
            };
            drop(stdout);
            let outcomes = run_files(&job, &files, steps.as_deref(), out_dir.as_deref(), !inv.json);
            let mut stdout = std::io::stdout().lock();
            if inv.json {
                print_json(&mut stdout, &outcomes);
            }
            if outcomes.iter().all(|o| o.error.is_none()) {
                0
            } else {
                1
            }
        }
    }
}

fn run_files(job: &Job, files: &[PathBuf], steps: Option<&[String]>, out_dir: Option<&Path>, text: bool) -> Vec<Outcome> {
    let _job = engine::jobs::begin(Some(CLI_JOB));
    console::on_ctrl_c(|| {
        engine::jobs::cancel(CLI_JOB);
    });
    let show_progress = text && std::io::stderr().is_terminal();
    let mut outcomes = Vec::with_capacity(files.len());
    for file in files {
        let input = std::path::absolute(file).unwrap_or_else(|_| file.clone());
        let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| display(file));
        let report = |f: f32| {
            if show_progress {
                eprint!("\r  {name}  {:>3}%", (f.clamp(0.0, 1.0) * 100.0) as u32);
            }
        };
        let run = || run_one(job, &input, steps, &report);
        let result = if engine::jobs::is_cancelled() {
            Err(engine::jobs::CANCELLED.to_string())
        } else {
            match out_dir {
                Some(dir) => engine::with_output_dir(dir, run),
                None => run(),
            }
        };
        if show_progress {
            eprint!("\r\x1b[2K");
        }
        let outcome = match result {
            Ok(out) => Outcome { input: display(&input), output: Some(display(&out)), status: "ok", error: None },
            Err(e) => Outcome { input: display(&input), output: None, status: "error", error: Some(e) },
        };
        if text {
            match (&outcome.output, &outcome.error) {
                (Some(out), _) => println!("ok     {} -> {out}", display(file)),
                (_, Some(err)) => eprintln!("error  {}: {err}", display(file)),
                _ => {}
            }
        }
        outcomes.push(outcome);
    }
    outcomes
}

fn usage(msg: &str) -> i32 {
    eprintln!("ferryfile: {msg}");
    2
}

/// Entry point used by `main`: `Some(exit code)` if this was a CLI call,
/// `None` to start the tray app.
pub fn run_from_env() -> Option<i32> {
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    let parsed = parse(&args);
    if matches!(parsed, Ok(None)) {
        return None;
    }
    console::attach();
    Some(match parsed {
        Ok(Some(inv)) => execute(inv),
        Ok(None) => unreachable!(),
        Err(msg) => usage(&msg),
    })
}

fn print_json<T: Serialize + ?Sized>(out: &mut impl Write, value: &T) {
    let json = serde_json::to_string_pretty(value).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"));
    let _ = writeln!(out, "{json}");
}

/// Release builds are GUI-subsystem exes, which start without a console.
/// When run from a terminal, borrow the parent's console so output shows up.
#[cfg(windows)]
mod console {
    use std::sync::OnceLock;

    use windows::core::{BOOL, PCWSTR};
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, GetFileType, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK,
        FILE_TYPE_PIPE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        AttachConsole, GetConsoleMode, GetStdHandle, SetConsoleCtrlHandler, SetStdHandle, ATTACH_PARENT_PROCESS,
        CONSOLE_MODE, STD_ERROR_HANDLE, STD_HANDLE, STD_OUTPUT_HANDLE,
    };

    /// True if `h` already goes somewhere useful: a pipe, a file, or a console.
    fn usable(h: HANDLE) -> bool {
        if h.is_invalid() || h.0.is_null() {
            return false;
        }
        unsafe {
            let kind = GetFileType(h);
            if kind == FILE_TYPE_PIPE || kind == FILE_TYPE_DISK {
                return true;
            }
            let mut mode = CONSOLE_MODE::default();
            GetConsoleMode(h, &mut mode).is_ok()
        }
    }

    fn std_handle(which: STD_HANDLE) -> HANDLE {
        unsafe { GetStdHandle(which).unwrap_or_default() }
    }

    pub fn attach() {
        let outputs = [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE];
        if outputs.iter().all(|&w| usable(std_handle(w))) {
            return; // redirected, or already a console app (debug builds)
        }
        unsafe {
            if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
                return; // started from Explorer or a shortcut: nowhere to print
            }
            let name: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
            let Ok(conout) = CreateFileW(
                PCWSTR(name.as_ptr()),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            ) else {
                return;
            };
            for w in outputs {
                if !usable(std_handle(w)) {
                    let _ = SetStdHandle(w, conout);
                }
            }
        }
        // The shell already printed its prompt; start on a fresh line.
        eprintln!();
    }

    static ON_CTRL_C: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

    unsafe extern "system" fn handler(_ctrl: u32) -> BOOL {
        match ON_CTRL_C.get() {
            Some(f) => {
                f();
                BOOL(1) // handled: let the job stop and clean up
            }
            None => BOOL(0),
        }
    }

    pub fn on_ctrl_c(f: impl Fn() + Send + Sync + 'static) {
        if ON_CTRL_C.set(Box::new(f)).is_ok() {
            unsafe {
                let _ = SetConsoleCtrlHandler(Some(handler), true);
            }
        }
    }
}

#[cfg(not(windows))]
mod console {
    pub fn attach() {}
    pub fn on_ctrl_c(_f: impl Fn() + Send + Sync + 'static) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Option<Invocation>, String> {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn explorer_open_flag_starts_the_app() {
        // Even when the file is gone by the time Ferryfile starts.
        assert!(p(&["--open", r"C:\missing\x.png"]).unwrap().is_none());
        assert!(p(&["--open"]).unwrap().is_none());
    }
    fn cmd(args: &[&str]) -> Invocation {
        p(args).unwrap().expect("a CLI call")
    }

    #[test]
    fn no_args_starts_the_app() {
        assert_eq!(p(&[]), Ok(None));
        // A path on its own is a hand-off to the app, not a command.
        let here = std::env::current_dir().unwrap();
        assert_eq!(p(&[here.to_str().unwrap()]), Ok(None));
    }

    #[test]
    fn parses_convert() {
        let inv = cmd(&["convert", "a.png", "b.png", "--to", ".JPEG", "--out", "out dir", "--json"]);
        assert!(inv.json);
        assert_eq!(inv.out, Some(PathBuf::from("out dir")));
        assert_eq!(
            inv.command,
            Command::Run { job: Job::Convert("jpg".into()), files: vec!["a.png".into(), "b.png".into()] }
        );
        // --flag=value and options before the command
        let inv = cmd(&["--to=webp", "-o", "x", "convert", "a.png"]);
        assert_eq!(inv.command, Command::Run { job: Job::Convert("webp".into()), files: vec!["a.png".into()] });
        assert!(!inv.json);
    }

    #[test]
    fn parses_other_commands() {
        assert_eq!(cmd(&["actions", "a.pdf"]).command, Command::Actions("a.pdf".into()));
        assert_eq!(cmd(&["recipes", "--json"]).command, Command::Recipes);
        assert_eq!(
            cmd(&["run-action", "resize:50", "a.png"]).command,
            Command::Run { job: Job::Action("resize:50".into()), files: vec!["a.png".into()] }
        );
        assert_eq!(
            cmd(&["recipe", "Web-ready image", "a.png", "--", "--odd-name.png"]).command,
            Command::Run {
                job: Job::Recipe("Web-ready image".into()),
                files: vec!["a.png".into(), "--odd-name.png".into()]
            }
        );
        assert_eq!(cmd(&["--help"]).command, Command::Help);
        assert_eq!(cmd(&["convert", "-h"]).command, Command::Help);
        assert_eq!(cmd(&["-V"]).command, Command::Version);
    }

    #[test]
    fn rejects_bad_usage() {
        assert!(p(&["convert", "a.png"]).unwrap_err().contains("--to"));
        assert!(p(&["convert", "--to", "png"]).unwrap_err().contains("at least one file"));
        assert!(p(&["convert", "a.png", "--to"]).unwrap_err().contains("needs a value"));
        assert!(p(&["recipe", "web-image"]).unwrap_err().contains("at least one file"));
        assert!(p(&["actions"]).is_err());
        assert!(p(&["actions", "a", "b"]).is_err());
        assert!(p(&["frobnicate", "a.png"]).unwrap_err().contains("Unknown command"));
        assert!(p(&["convert", "a.png", "--to", "png", "--fast"]).unwrap_err().contains("Unknown option"));
        assert!(p(&["recipe", "x", "a.png", "--to", "png"]).is_err());
        assert!(p(&["--json"]).unwrap_err().contains("Missing a command"));
    }

    #[test]
    fn convert_picks_a_matching_action() {
        assert_eq!(convert_action(Path::new("a.png"), "webp").unwrap(), "convert:webp");
        // No convert:mp3 for video; the action that produces MP3 is used.
        assert_eq!(convert_action(Path::new("a.mov"), "mp3").unwrap(), "audio:mp3");
        assert!(convert_action(Path::new("a.png"), "png").is_err());
        assert!(convert_action(Path::new("a.png"), "mp3").is_err());
    }

    #[test]
    fn runs_files_and_reports_each() {
        let dir = std::env::temp_dir().join(format!("ferryfile-cli-{}", std::process::id()));
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        let input = dir.join("pic.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([9, 9, 9, 255])).save(&input).unwrap();
        let missing = dir.join("missing.png");

        let results =
            run_files(&Job::Convert("webp".into()), &[input.clone(), missing], None, Some(&out), false);
        assert_eq!(results[0].status, "ok");
        assert_eq!(results[0].output.as_deref().map(Path::new), Some(out.join("pic.webp").as_path()));
        assert_eq!(results[1].status, "error");
        assert_eq!(results[1].error.as_deref(), Some("File not found"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
