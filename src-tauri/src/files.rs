//! File actions that work on any dropped items, folders included: clipboard,
//! Explorer, Terminal, and move/copy to a chosen folder. They never overwrite:
//! a clash gets a " (2)" style name.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

/// Copies the paths as text, one per line.
#[tauri::command]
pub fn copy_paths(paths: Vec<String>) -> Result<(), String> {
    clipboard_win::set_clipboard_string(&paths.join("\r\n")).map_err(|e| e.to_string())
}

/// Puts the items on the clipboard as files, ready to paste in Explorer.
#[tauri::command]
pub fn copy_files(paths: Vec<String>) -> Result<(), String> {
    let _open = clipboard_win::Clipboard::new_attempts(10).map_err(|e| e.to_string())?;
    clipboard_win::raw::empty().map_err(|e| e.to_string())?;
    clipboard_win::raw::set_file_list(&paths).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reveal(path: String) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    // explorer.exe wants `/select,"C:\path with spaces"` as a single raw token.
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{path}\""))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Opens a terminal in the folder (or the file's folder): Windows Terminal if
/// present, else PowerShell.
#[tauri::command]
pub fn open_terminal(path: String) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let path = Path::new(&path);
    let dir = if path.is_dir() { path } else { path.parent().ok_or("No parent folder")? };
    if std::process::Command::new("wt.exe").arg("-d").arg(dir).spawn().is_ok() {
        return Ok(());
    }
    let quoted = format!("'{}'", dir.display().to_string().replace('\'', "''"));
    std::process::Command::new("powershell.exe")
        .args(["-NoExit", "-Command", &format!("Set-Location -LiteralPath {quoted}")])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `dir/name`, or `dir/stem (n).ext` if that's taken.
fn free_name(dir: &Path, source: &Path) -> PathBuf {
    let name = source.file_name().unwrap_or_default();
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("item");
    let ext = source.extension().and_then(|e| e.to_str());
    (2..)
        .map(|n| match ext {
            Some(ext) if source.is_file() => dir.join(format!("{stem} ({n}).{ext}")),
            _ => dir.join(format!("{} ({n})", name.to_string_lossy())),
        })
        .find(|p| !p.exists())
        .unwrap()
}

fn copy_recursive(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ())
    }
}

fn transfer(source: &Path, dir: &Path, keep_source: bool) -> Result<PathBuf, String> {
    if dir.starts_with(source) {
        return Err(format!("Can't put {} inside itself", source.display()));
    }
    let dest = free_name(dir, source);
    let err = |e: std::io::Error| format!("{}: {e}", source.display());
    if keep_source {
        copy_recursive(source, &dest).map_err(err)?;
    } else if std::fs::rename(source, &dest).is_err() {
        // Different drive: copy, then remove the original.
        copy_recursive(source, &dest).map_err(err)?;
        if source.is_dir() {
            std::fs::remove_dir_all(source).map_err(err)?;
        } else {
            std::fs::remove_file(source).map_err(err)?;
        }
    }
    Ok(dest)
}

/// Asks for a folder, then moves (or copies) the items there. Returns the new
/// paths, or an empty list if the user cancelled.
#[tauri::command]
pub async fn send_to_folder(app: AppHandle, paths: Vec<String>, copy: bool) -> Result<Vec<String>, String> {
    let mut dialog = app.dialog().file().set_title(if copy { "Copy to…" } else { "Move to…" });
    if let Some(w) = app.get_webview_window("drawer") {
        dialog = dialog.set_parent(&w);
    }
    let Some(folder) = dialog.blocking_pick_folder() else {
        return Ok(Vec::new());
    };
    let dir = folder.into_path().map_err(|e| e.to_string())?;
    paths
        .iter()
        .map(|p| transfer(Path::new(p), &dir, copy).map(|d| d.to_string_lossy().into_owned()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfers_without_overwriting() {
        let root = std::env::temp_dir().join(format!("ferryfile-files-{}", std::process::id()));
        let (a, b) = (root.join("a"), root.join("b"));
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("note.txt"), "hi").unwrap();
        std::fs::write(a.join("sub/deep.txt"), "x").unwrap();
        std::fs::write(b.join("note.txt"), "existing").unwrap();

        let copied = transfer(&a.join("note.txt"), &b, true).unwrap();
        assert_eq!(copied, b.join("note (2).txt"));
        assert_eq!(std::fs::read_to_string(b.join("note.txt")).unwrap(), "existing");

        let moved = transfer(&a.join("sub"), &b, false).unwrap();
        assert!(moved.join("deep.txt").exists() && !a.join("sub").exists());

        assert!(transfer(&a, &a.join("inner"), true).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
