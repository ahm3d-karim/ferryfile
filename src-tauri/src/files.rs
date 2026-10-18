//! File actions that work on any dropped items, folders included: clipboard,
//! Explorer, Terminal, and move/copy to a chosen folder. They never overwrite:
//! a clash gets a " (2)" style name.

use std::path::{Path, PathBuf};

use tauri::AppHandle;
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
pub async fn send_to_folder(
    app: AppHandle,
    window: tauri::WebviewWindow,
    paths: Vec<String>,
    copy: bool,
) -> Result<Vec<String>, String> {
    // Parent the picker to whichever window asked (drawer or a shelf).
    let dialog = app
        .dialog()
        .file()
        .set_title(if copy { "Copy to…" } else { "Move to…" })
        .set_parent(&window);
    let Some(folder) = dialog.blocking_pick_folder() else {
        return Ok(Vec::new());
    };
    let dir = folder.into_path().map_err(|e| e.to_string())?;
    paths
        .iter()
        .map(|p| transfer(Path::new(p), &dir, copy).map(|d| d.to_string_lossy().into_owned()))
        .collect()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    /// Human kind, e.g. "PNG image", "Folder".
    kind: String,
    size: u64,
    is_dir: bool,
    width: Option<u32>,
    height: Option<u32>,
    /// Number of entries, for folders.
    items: Option<usize>,
}

fn kind_of(ext: &str) -> &'static str {
    match ext {
        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff" | "heic" | "avif" | "svg" => "image",
        "mp3" | "m4a" | "aac" | "wav" | "flac" | "ogg" | "opus" | "aiff" | "aif" | "wma" => "audio",
        "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "wmv" => "video",
        "xlsx" | "xlsm" | "xlsb" | "xls" | "ods" | "csv" | "tsv" => "spreadsheet",
        "pptx" | "ppt" | "odp" => "presentation",
        "zip" | "7z" | "rar" | "tar" | "gz" => "archive",
        _ => "document",
    }
}

/// What the drawer's file card shows: kind, size, and image dimensions
/// (read from the header only, so it's fast even for large files).
#[tauri::command]
pub fn file_info(path: String) -> Result<FileInfo, String> {
    let p = Path::new(&path);
    let meta = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if meta.is_dir() {
        let items = std::fs::read_dir(p).map(|d| d.count()).ok();
        return Ok(FileInfo { kind: "Folder".into(), size: 0, is_dir: true, width: None, height: None, items });
    }
    let ext = crate::engine::ext_of(p);
    let kind = if ext.is_empty() { "File".to_string() } else { format!("{} {}", ext.to_uppercase(), kind_of(&ext)) };
    let (width, height) = match image::image_dimensions(p) {
        Ok((w, h)) => (Some(w), Some(h)),
        Err(_) => (None, None),
    };
    Ok(FileInfo { kind, size: meta.len(), is_dir: false, width, height, items: None })
}

/// Adds `path` to the archive under `name`, recursing into folders.
fn zip_add<W: std::io::Write + std::io::Seek>(
    zip: &mut zip::ZipWriter<W>,
    path: &Path,
    name: &str,
) -> Result<(), String> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let err = |e: &dyn std::fmt::Display| format!("{}: {e}", path.display());
    if path.is_dir() {
        zip.add_directory(format!("{name}/"), options).map_err(|e| err(&e))?;
        for entry in std::fs::read_dir(path).map_err(|e| err(&e))? {
            let entry = entry.map_err(|e| err(&e))?;
            let child = format!("{name}/{}", entry.file_name().to_string_lossy());
            zip_add(zip, &entry.path(), &child)?;
        }
    } else {
        zip.start_file(name, options).map_err(|e| err(&e))?;
        let mut file = std::fs::File::open(path).map_err(|e| err(&e))?;
        std::io::copy(&mut file, zip).map_err(|e| err(&e))?;
    }
    Ok(())
}

/// Zips the items into one archive next to the first one (or in the
/// configured output folder): `<name>.zip` for one item, `Archive.zip` for
/// several. Never overwrites.
pub fn zip_to(paths: &[String]) -> Result<PathBuf, String> {
    let first = Path::new(paths.first().ok_or("Nothing to zip")?);
    let stem_source = if paths.len() == 1 {
        first.to_path_buf()
    } else {
        first.with_file_name("Archive")
    };
    // Folders have no extension to replace, so give output_path a stand-in.
    let out = crate::engine::output_path(&stem_source.with_extension("zip"), "", "zip");
    let file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let result = paths.iter().try_for_each(|p| {
        let p = Path::new(p);
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        zip_add(&mut zip, p, &name)
    });
    let result = result.and_then(|_| zip.finish().map(|_| ()).map_err(|e| e.to_string()));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&out);
        return Err(e);
    }
    Ok(out)
}

#[tauri::command]
pub async fn zip_items(paths: Vec<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || zip_to(&paths).map(|p| p.to_string_lossy().into_owned()))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zips_files_and_folders() {
        let root = std::env::temp_dir().join(format!("ferryfile-zip-{}", std::process::id()));
        std::fs::create_dir_all(root.join("photos/2026")).unwrap();
        std::fs::write(root.join("notes.txt"), "hello").unwrap();
        std::fs::write(root.join("photos/2026/a.txt"), "a").unwrap();
        let items = [root.join("notes.txt"), root.join("photos")].map(|p| p.to_string_lossy().into_owned());

        let out = zip_to(&items).unwrap();
        assert_eq!(out, root.join("Archive.zip"));
        let again = zip_to(&items).unwrap();
        assert_eq!(again, root.join("Archive (2).zip"), "must not overwrite");

        let mut archive = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut names: Vec<_> = (0..archive.len()).map(|i| archive.by_index(i).unwrap().name().unwrap().to_string()).collect();
        names.sort();
        assert_eq!(names, ["notes.txt", "photos/", "photos/2026/", "photos/2026/a.txt"]);

        let single = zip_to(&items[..1]).unwrap();
        assert_eq!(single, root.join("notes.zip"));
        std::fs::remove_dir_all(&root).unwrap();
    }

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
