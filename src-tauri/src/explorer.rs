//! Explorer integration: the "Convert with Ferryfile" right-click entry, the
//! Send to shortcut, and handing the paths from those launches to the drawer.
//!
//! Both are per-user (HKCU and the user's SendTo folder), so no admin rights.
//! They are classic shell verbs: on Windows 11 they sit under "Show more
//! options". The new top-level menu needs an `IExplorerCommand` inside a
//! signed MSIX (sparse) package, which Ferryfile doesn't ship.
//!
//! Explorer runs `Ferryfile.exe --open <path>` once per selected item (Send to
//! runs it once with every path). The single-instance plugin forwards each
//! launch to the running app, and the [`Inbox`] gathers launches that arrive
//! close together into one drop.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

/// Command-line flag that marks the paths after it as files to open.
pub const OPEN_FLAG: &str = "--open";
/// Tells the drawer page that opened paths are waiting (see `take_opened_paths`).
const OPENED_EVENT: &str = "ferryfile-opened";
/// Launches closer together than this become one drop.
const GATHER: Duration = Duration::from_millis(350);

/// The paths after `--open` in a command line (`args[0]` is the exe).
pub fn paths_from_args(args: &[String]) -> Vec<String> {
    match args.iter().position(|a| a.eq_ignore_ascii_case(OPEN_FLAG)) {
        Some(i) => args[i + 1..].iter().filter(|a| !a.trim().is_empty()).cloned().collect(),
        None => Vec::new(),
    }
}

/// Collects paths from separate launches until they stop arriving.
#[derive(Default)]
pub struct Inbox {
    /// Still arriving.
    pending: Vec<String>,
    /// Gathered and waiting for the drawer page to take them.
    ready: Vec<String>,
    generation: u64,
}

impl Inbox {
    /// Adds paths (skipping repeats) and returns the generation to flush later.
    pub fn add(&mut self, paths: Vec<String>) -> u64 {
        for p in paths {
            let seen = |q: &String| q.eq_ignore_ascii_case(&p);
            if !self.pending.iter().any(seen) && !self.ready.iter().any(seen) {
                self.pending.push(p);
            }
        }
        self.generation += 1;
        self.generation
    }

    /// Moves pending paths to ready if nothing arrived since `generation`.
    pub fn flush(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.pending.is_empty() {
            return false;
        }
        self.ready.append(&mut self.pending);
        true
    }

    pub fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.ready)
    }
}

static INBOX: Mutex<Inbox> = Mutex::new(Inbox { pending: Vec::new(), ready: Vec::new(), generation: 0 });

/// Paths forwarded from another launch (right-click menu or Send to): shows
/// the drawer with them once the burst of launches is over.
pub fn receive(app: &AppHandle, paths: Vec<String>) {
    if queue(paths) {
        show_when_settled(app);
    }
}

/// Queues paths without showing them yet. The app calls this for its own
/// command line before it starts, so launches forwarded while it's still
/// starting up join the same drop. Returns whether there were any.
pub fn queue(paths: Vec<String>) -> bool {
    if paths.is_empty() {
        return false;
    }
    #[cfg(debug_assertions)]
    eprintln!("[ferryfile] received {} path(s) from a launch", paths.len());
    INBOX.lock().unwrap().add(paths);
    true
}

/// Shows the queued paths once no more have arrived for a moment.
pub fn show_when_settled(app: &AppHandle) {
    let generation = INBOX.lock().unwrap().generation;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(GATHER);
        if INBOX.lock().unwrap().flush(generation) {
            crate::drag::show_opened(&app);
            let _ = app.emit_to("drawer", OPENED_EVENT, ());
        }
    });
}

#[derive(Serialize)]
pub struct Opened {
    side: crate::settings::Side,
    paths: Vec<String>,
}

/// The drawer page asks for opened paths when told (and once at load, in case
/// they arrived before the page was listening).
#[tauri::command]
pub fn take_opened_paths() -> Opened {
    let paths = INBOX.lock().unwrap().take();
    #[cfg(debug_assertions)]
    if !paths.is_empty() {
        eprintln!("[ferryfile] drawer took {} opened path(s): {paths:?}", paths.len());
    }
    Opened { side: crate::settings::current().drawer_side, paths }
}

// ---------------------------------------------------------------- registration

/// Right-click verbs, for files and for folders.
const VERB_KEYS: [&str; 2] = [r"Software\Classes\*\shell\Ferryfile", r"Software\Classes\Directory\shell\Ferryfile"];
pub const MENU_LABEL: &str = "Convert with Ferryfile";
/// Remembers that the user turned an entry off, so reinstalling or updating
/// doesn't put it back (the NSIS hooks check these too).
const PREFS_KEY: &str = r"Software\Ferryfile";
const MENU_OFF: &str = "NoContextMenu";
const SEND_TO_OFF: &str = "NoSendTo";
const SEND_TO_NAME: &str = "Ferryfile.lnk";

/// The verb's command line: `"C:\…\Ferryfile.exe" --open "%1"`.
pub fn verb_command(exe: &Path) -> String {
    format!("\"{}\" {OPEN_FLAG} \"%1\"", exe.display())
}

/// The exe a verb command starts, if it parses.
fn command_exe(command: &str) -> Option<PathBuf> {
    let rest = command.strip_prefix('"')?;
    Some(PathBuf::from(&rest[..rest.find('"')?]))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Integration {
    context_menu: bool,
    send_to: bool,
}

#[tauri::command]
pub fn get_shell_integration() -> Integration {
    Integration { context_menu: menu_installed(), send_to: send_to_path().is_some_and(|p| p.exists()) }
}

/// Turns the right-click entry (`kind` = "contextMenu") or the Send to
/// shortcut ("sendTo") on or off.
#[tauri::command]
pub fn set_shell_integration(kind: String, enabled: bool) -> Result<Integration, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    match kind.as_str() {
        "contextMenu" => {
            if enabled {
                install_menu(&exe)?
            } else {
                remove_menu()?
            }
            reg::set_dword(PREFS_KEY, MENU_OFF, (!enabled).into())?;
        }
        "sendTo" => {
            let link = send_to_path().ok_or("Couldn't find the Send to folder")?;
            if enabled {
                shortcut::create(&link, &exe, OPEN_FLAG, MENU_LABEL)?
            } else if link.exists() {
                std::fs::remove_file(&link).map_err(|e| format!("Couldn't remove the Send to shortcut: {e}"))?
            }
            reg::set_dword(PREFS_KEY, SEND_TO_OFF, (!enabled).into())?;
        }
        _ => return Err(format!("Unknown integration \"{kind}\"")),
    }
    Ok(get_shell_integration())
}

fn menu_installed() -> bool {
    reg::read_string(&format!(r"{}\command", VERB_KEYS[0]), "").is_some()
}

fn install_menu(exe: &Path) -> Result<(), String> {
    let icon = format!("\"{}\",0", exe.display());
    for key in VERB_KEYS {
        reg::set_string(key, "", MENU_LABEL)?;
        reg::set_string(key, "Icon", &icon)?;
        // Keep the entry when more than 15 items are selected.
        reg::set_string(key, "MultiSelectModel", "Player")?;
        reg::set_string(&format!(r"{key}\command"), "", &verb_command(exe))?;
    }
    Ok(())
}

fn remove_menu() -> Result<(), String> {
    VERB_KEYS.iter().try_for_each(|k| reg::delete_tree(k))
}

/// At startup: if the right-click entry points at an exe that's gone (a moved
/// portable copy), point it at this one. A working entry is left alone, so a
/// dev build doesn't take over an installed copy's menu.
pub fn repair() {
    let Some(registered) = reg::read_string(&format!(r"{}\command", VERB_KEYS[0]), "").and_then(|c| command_exe(&c))
    else {
        return;
    };
    if !registered.exists() {
        if let Ok(exe) = std::env::current_exe() {
            let _ = install_menu(&exe);
        }
    }
}

fn send_to_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join(r"Microsoft\Windows\SendTo").join(SEND_TO_NAME))
}

/// Small wrappers over the registry API, all under HKEY_CURRENT_USER.
mod reg {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Registry::{
        RegDeleteTreeW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ, REG_VALUE_TYPE,
        RRF_RT_REG_SZ,
    };

    /// `None` = the key's default value.
    fn value_name(name: &HSTRING) -> PCWSTR {
        if name.is_empty() { PCWSTR::null() } else { PCWSTR(name.as_ptr()) }
    }

    /// Writes a value, creating the key if needed.
    fn set(path: &str, name: &str, kind: REG_VALUE_TYPE, data: &[u8]) -> Result<(), String> {
        let name = HSTRING::from(name);
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                &HSTRING::from(path),
                value_name(&name),
                kind.0,
                Some(data.as_ptr().cast()),
                data.len() as u32,
            )
        }
        .ok()
        .map_err(|e| format!("Couldn't write the registry ({path}): {e}"))
    }

    pub fn set_string(path: &str, name: &str, value: &str) -> Result<(), String> {
        let wide: Vec<u16> = value.encode_utf16().chain(Some(0)).collect();
        let bytes: Vec<u8> = wide.iter().flat_map(|c| c.to_le_bytes()).collect();
        set(path, name, REG_SZ, &bytes)
    }

    pub fn set_dword(path: &str, name: &str, value: u32) -> Result<(), String> {
        set(path, name, REG_DWORD, &value.to_le_bytes())
    }

    pub fn read_string(path: &str, name: &str) -> Option<String> {
        let (path, name) = (HSTRING::from(path), HSTRING::from(name));
        let mut buf = vec![0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                &path,
                value_name(&name),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        }
        .ok()
        .ok()?;
        let len = (size as usize / 2).saturating_sub(1);
        Some(String::from_utf16_lossy(&buf[..len]))
    }

    pub fn delete_tree(path: &str) -> Result<(), String> {
        let err = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(path)) };
        if err.is_ok() || err == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("Couldn't remove the registry key ({path}): {}", err.to_hresult().message()))
        }
    }
}

/// Creates a .lnk shortcut through the shell's COM interface.
mod shortcut {
    use std::path::Path;

    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    pub fn create(link: &Path, target: &Path, args: &str, description: &str) -> Result<(), String> {
        let (link, target) = (link.to_path_buf(), target.to_path_buf());
        let (args, description) = (args.to_string(), description.to_string());
        // A thread of our own, so COM setup can't clash with the caller's.
        std::thread::spawn(move || unsafe {
            let inited = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            let result = (|| -> windows::core::Result<()> {
                let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
                sl.SetPath(&HSTRING::from(target.as_os_str()))?;
                sl.SetArguments(&HSTRING::from(args.as_str()))?;
                sl.SetDescription(&HSTRING::from(description.as_str()))?;
                sl.SetIconLocation(&HSTRING::from(target.as_os_str()), 0)?;
                sl.cast::<IPersistFile>()?.Save(&HSTRING::from(link.as_os_str()), true)
            })();
            if inited {
                CoUninitialize();
            }
            result.map_err(|e| format!("Couldn't create the Send to shortcut: {e}"))
        })
        .join()
        .map_err(|_| "Couldn't create the Send to shortcut".to_string())?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reads_paths_after_open_flag() {
        assert_eq!(paths_from_args(&args(&["ferryfile.exe"])), Vec::<String>::new());
        assert_eq!(paths_from_args(&args(&["ferryfile.exe", "--open"])), Vec::<String>::new());
        assert_eq!(
            paths_from_args(&args(&["ferryfile.exe", "--open", r"C:\a b\x.png", r"D:\y"])),
            args(&[r"C:\a b\x.png", r"D:\y"])
        );
        assert_eq!(paths_from_args(&args(&["f.exe", "--OPEN", "x"])), args(&["x"]));
        // A path that happens to look like a flag before --open isn't taken.
        assert_eq!(paths_from_args(&args(&["f.exe", "--autostart"])), Vec::<String>::new());
    }

    #[test]
    fn inbox_gathers_a_burst_into_one_drop() {
        let mut inbox = Inbox::default();
        let first = inbox.add(args(&["a"]));
        let second = inbox.add(args(&["b", "A"])); // "A" repeats "a" (paths ignore case)
        assert!(!inbox.flush(first), "a later launch restarts the wait");
        assert!(inbox.take().is_empty());
        assert!(inbox.flush(second));
        assert_eq!(inbox.take(), args(&["a", "b"]));
        assert!(inbox.take().is_empty());
        assert!(!inbox.flush(second), "nothing left to flush");
    }

    #[test]
    fn inbox_skips_paths_already_waiting_for_the_page() {
        let mut inbox = Inbox::default();
        let g = inbox.add(args(&["a"]));
        inbox.flush(g);
        let g = inbox.add(args(&["a", "c"]));
        assert!(inbox.flush(g));
        assert_eq!(inbox.take(), args(&["a", "c"]));
    }

    #[test]
    fn verb_command_round_trips_the_exe() {
        let exe = Path::new(r"C:\Users\me\AppData\Local\Ferryfile\Ferryfile.exe");
        let cmd = verb_command(exe);
        assert_eq!(cmd, r#""C:\Users\me\AppData\Local\Ferryfile\Ferryfile.exe" --open "%1""#);
        assert_eq!(command_exe(&cmd).as_deref(), Some(exe));
        assert_eq!(command_exe("unquoted.exe %1"), None);
    }

    #[test]
    fn creates_a_shortcut() {
        let dir = std::env::temp_dir().join(format!("ferryfile-lnk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("Ferryfile.lnk");
        let exe = std::env::current_exe().unwrap();
        shortcut::create(&link, &exe, OPEN_FLAG, MENU_LABEL).unwrap();
        let bytes = std::fs::read(&link).unwrap();
        // Shell link header: size 0x4C, then the ShellLink CLSID.
        assert_eq!(&bytes[..4], &[0x4C, 0, 0, 0]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Writes and removes the real HKCU entries under a scratch key.
    #[test]
    fn registry_round_trip() {
        let key = r"Software\Ferryfile-test-registry-round-trip";
        reg::set_string(&format!(r"{key}\command"), "", "hello ✓").unwrap();
        assert_eq!(reg::read_string(&format!(r"{key}\command"), "").as_deref(), Some("hello ✓"));
        reg::delete_tree(key).unwrap();
        assert_eq!(reg::read_string(&format!(r"{key}\command"), ""), None);
        reg::delete_tree(key).unwrap(); // already gone is fine
    }
}
