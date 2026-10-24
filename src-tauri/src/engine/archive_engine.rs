//! "Extract here" for archives: ZIP, 7z, TAR, TAR.GZ/TGZ and single-file GZ
//! in Rust; RAR through 7-Zip if it's installed, else the tar.exe that ships
//! with Windows (Windows 11 can read RAR with it).
//!
//! Everything is unpacked into a hidden staging folder next to the output
//! first. If the archive holds a single item (often one top-level folder),
//! that item is moved out directly instead of nesting it in another folder.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use super::{destination, ext_of, jobs, tool_command, unique_in, Action, Engine, Progress, FOLDER_EXT};

pub struct ArchiveEngine;

#[derive(Debug, PartialEq)]
enum Kind {
    Zip,
    SevenZ,
    Rar,
    Tar,
    TarGz,
    /// A single gzipped file, e.g. `data.csv.gz`.
    Gz,
}

fn kind(path: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    Some(match ext_of(path).as_str() {
        "zip" => Kind::Zip,
        "7z" => Kind::SevenZ,
        "rar" => Kind::Rar,
        "tar" => Kind::Tar,
        "tgz" => Kind::TarGz,
        "gz" if name.ends_with(".tar.gz") => Kind::TarGz,
        "gz" => Kind::Gz,
        _ => return None,
    })
}

/// The archive's name without its archive extensions: `photos.tar.gz` -> `photos`.
fn base_name(path: &Path) -> String {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("archive");
    let lower = name.to_ascii_lowercase();
    for ext in [".tar.gz", ".tgz", ".tar", ".zip", ".7z", ".rar", ".gz"] {
        if lower.ends_with(ext) && lower.len() > ext.len() {
            return name[..name.len() - ext.len()].to_string();
        }
    }
    name.to_string()
}

impl Engine for ArchiveEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        match kind(path) {
            Some(_) => vec![Action::new("extract:here", "Extract here", "Archive")],
            None => vec![],
        }
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        // Usually a folder; either way nothing can follow it in a recipe.
        (action_id == "extract:here").then(|| FOLDER_EXT.into())
    }

    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
        if action_id != "extract:here" {
            return Err(format!("Unknown action '{action_id}'"));
        }
        let kind = kind(input).ok_or("Not an archive Ferryfile can open")?;
        let dest = destination(input);
        let staging = staging_dir(&dest)?;
        let result = extract(&kind, input, &staging, progress).and_then(|_| {
            jobs::check()?;
            settle(&staging, &dest, &base_name(input))
        });
        if staging.exists() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result
    }
}

fn staging_dir(dest: &Path) -> Result<PathBuf, String> {
    static N: AtomicU32 = AtomicU32::new(0);
    let dir = dest.join(format!(
        ".ferryfile-extract-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create a folder in {}: {e}", dest.display()))?;
    Ok(dir)
}

/// Moves what was extracted into place: a lone item as itself, several items
/// in a folder named after the archive.
fn settle(staging: &Path, dest: &Path, base: &str) -> Result<PathBuf, String> {
    let entries: Vec<_> = std::fs::read_dir(staging)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    let (from, name) = match entries.as_slice() {
        [] => return Err("This archive is empty".into()),
        [only] => (only.path(), only.file_name().to_string_lossy().into_owned()),
        _ => (staging.to_path_buf(), base.to_string()),
    };
    let to = unique_in(dest, &name);
    std::fs::rename(&from, &to).map_err(|e| format!("Couldn't move the extracted files: {e}"))?;
    Ok(to)
}

fn extract(kind: &Kind, input: &Path, out: &Path, progress: Progress) -> Result<(), String> {
    match kind {
        Kind::Zip => extract_zip(input, out, progress),
        Kind::Tar => extract_tar(input, out, false, progress),
        Kind::TarGz => extract_tar(input, out, true, progress),
        Kind::Gz => extract_gz(input, out, progress),
        Kind::SevenZ => extract_7z(input, out).or_else(|e| {
            // Some 7z methods (e.g. encrypted or BCJ2 archives) need the real thing.
            match seven_zip() {
                Some(exe) => run_external(&exe, &["x", "-y", "-bd"], input, out, true),
                None => Err(e),
            }
        }),
        Kind::Rar => extract_rar(input, out),
    }
}

/// Wraps a reader and counts the bytes read, for progress by file position.
struct Counting<R> {
    inner: R,
    read: Arc<AtomicU64>,
}

impl<R: Read> Read for Counting<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

fn open_counting(input: &Path) -> Result<(Counting<BufReader<File>>, Arc<AtomicU64>, u64), String> {
    let file = File::open(input).map_err(|e| e.to_string())?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0).max(1);
    let read = Arc::new(AtomicU64::new(0));
    Ok((Counting { inner: BufReader::new(file), read: read.clone() }, read, total))
}

fn friendly_zip_error(e: zip::result::ZipError) -> String {
    let text = e.to_string();
    if text.to_ascii_lowercase().contains("password") {
        "Password-protected archives aren't supported yet".into()
    } else {
        format!("Couldn't read this ZIP file ({text})")
    }
}

fn extract_zip(input: &Path, out: &Path, progress: Progress) -> Result<(), String> {
    let file = File::open(input).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(friendly_zip_error)?;
    let n = zip.len().max(1);
    for i in 0..zip.len() {
        jobs::check()?;
        let mut entry = zip.by_index(i).map_err(friendly_zip_error)?;
        // `enclosed_name` drops entries that would land outside `out` ("../x", "C:\x").
        let Some(rel) = entry.enclosed_name() else { continue };
        let path = out.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut f = File::create(&path).map_err(|e| format!("Couldn't write {}: {e}", path.display()))?;
            std::io::copy(&mut entry, &mut f).map_err(|e| format!("Couldn't extract {}: {e}", rel.display()))?;
        }
        progress((i + 1) as f32 / n as f32);
    }
    Ok(())
}

fn extract_tar(input: &Path, out: &Path, gz: bool, progress: Progress) -> Result<(), String> {
    let (reader, read, total) = open_counting(input)?;
    let reader: Box<dyn Read> = if gz { Box::new(flate2::read::GzDecoder::new(reader)) } else { Box::new(reader) };
    let mut archive = tar::Archive::new(reader);
    let entries = archive.entries().map_err(|e| format!("Couldn't read this TAR file ({e})"))?;
    for entry in entries {
        jobs::check()?;
        let mut entry = entry.map_err(|e| format!("Couldn't read this TAR file ({e})"))?;
        // `unpack_in` refuses paths that would escape `out`.
        entry.unpack_in(out).map_err(|e| format!("Couldn't extract a file ({e})"))?;
        progress((read.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32);
    }
    Ok(())
}

fn extract_gz(input: &Path, out: &Path, progress: Progress) -> Result<(), String> {
    let (reader, read, total) = open_counting(input)?;
    let mut decoder = flate2::read::GzDecoder::new(reader);
    let target = out.join(base_name(input));
    let mut f = File::create(&target).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        jobs::check()?;
        let n = decoder.read(&mut buf).map_err(|e| format!("Couldn't read this GZ file ({e})"))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut f, &buf[..n]).map_err(|e| e.to_string())?;
        progress((read.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32);
    }
    Ok(())
}

fn extract_7z(input: &Path, out: &Path) -> Result<(), String> {
    use sevenz_rust2::{default_entry_extract_fn, Error};
    sevenz_rust2::decompress_file_with_extract_fn(input, out, |entry, reader, dest| {
        if jobs::is_cancelled() {
            return Err(Error::from(std::io::Error::other(jobs::CANCELLED)));
        }
        // The crate rejects "..", but an absolute name would replace the root.
        if dest.components().any(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
            && !dest.starts_with(out)
        {
            return Ok(true); // skip it
        }
        default_entry_extract_fn(entry, reader, dest)
    })
    .map_err(|e| {
        let text = e.to_string();
        if text.contains(jobs::CANCELLED) {
            jobs::CANCELLED.to_string()
        } else if text.to_ascii_lowercase().contains("password") {
            "Password-protected archives aren't supported yet".into()
        } else {
            format!("Couldn't read this 7z file ({text})")
        }
    })
}

/// 7-Zip's command-line tool, if 7-Zip is installed.
fn seven_zip() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Ok(dir) = std::env::var(var) {
            candidates.push(Path::new(&dir).join("7-Zip").join("7z.exe"));
        }
    }
    candidates.into_iter().find(|p| p.is_file()).or_else(|| {
        let exe = PathBuf::from("7z");
        tool_command(&exe)
            .arg("i")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
            .then_some(exe)
    })
}

fn extract_rar(input: &Path, out: &Path) -> Result<(), String> {
    if let Some(exe) = seven_zip() {
        return run_external(&exe, &["x", "-y", "-bd"], input, out, true);
    }
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let tar = Path::new(&windir).join("System32").join("tar.exe");
    run_external(&tar, &["-x", "-f"], input, out, false).map_err(|e| {
        if e == jobs::CANCELLED {
            e
        } else {
            "Couldn't open this RAR file. Install 7-Zip (7-zip.org) and Ferryfile will use it for RAR archives."
                .into()
        }
    })
}

/// Runs an extractor: 7-Zip (`x -y -o<out> <file>`) or tar (`-x -f <file> -C <out>`).
fn run_external(exe: &Path, args: &[&str], input: &Path, out: &Path, seven: bool) -> Result<(), String> {
    let mut cmd = tool_command(exe);
    cmd.args(args);
    if seven {
        cmd.arg(format!("-o{}", out.display())).arg(input);
    } else {
        cmd.arg(input).arg("-C").arg(out);
    }
    let mut child = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Couldn't start {}: {e}", exe.display()))?;
    loop {
        if jobs::is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(jobs::CANCELLED.into());
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            if status.success() {
                return Ok(());
            }
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
            return Err(format!("Extracting failed: {}", last.trim()));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ferryfile-arc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn recognises_archives() {
        assert_eq!(kind(Path::new("a.ZIP")), Some(Kind::Zip));
        assert_eq!(kind(Path::new("a.tar.gz")), Some(Kind::TarGz));
        assert_eq!(kind(Path::new("a.tgz")), Some(Kind::TarGz));
        assert_eq!(kind(Path::new("data.csv.gz")), Some(Kind::Gz));
        assert_eq!(kind(Path::new("a.rar")), Some(Kind::Rar));
        assert_eq!(kind(Path::new("a.png")), None);
        assert_eq!(base_name(Path::new("photos.tar.gz")), "photos");
        assert_eq!(base_name(Path::new("data.csv.gz")), "data.csv");
        assert_eq!(base_name(Path::new("Stuff.ZIP")), "Stuff");
    }

    #[test]
    fn extracts_zip_into_folder_and_flattens_single_items() {
        let dir = temp_dir("zip");
        // Two files at the top: they go in a folder named after the archive.
        let two = dir.join("two.zip");
        let mut w = zip::ZipWriter::new(File::create(&two).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        w.start_file("a.txt", opts).unwrap();
        w.write_all(b"hello").unwrap();
        w.start_file("sub/b.txt", opts).unwrap();
        w.write_all(b"world").unwrap();
        w.start_file("../evil.txt", opts).unwrap();
        w.write_all(b"nope").unwrap();
        w.finish().unwrap();

        let out = ArchiveEngine.run("extract:here", &two, &|_| {}).unwrap();
        assert_eq!(out, dir.join("two"));
        assert_eq!(std::fs::read_to_string(out.join("sub/b.txt")).unwrap(), "world");
        assert!(!dir.join("evil.txt").exists() && !out.join("evil.txt").exists());

        // One top-level folder: it comes out as itself, no extra nesting.
        let one = dir.join("one.zip");
        let mut w = zip::ZipWriter::new(File::create(&one).unwrap());
        w.add_directory("project/", opts).unwrap();
        w.start_file("project/readme.md", opts).unwrap();
        w.write_all(b"# hi").unwrap();
        w.finish().unwrap();
        let out = ArchiveEngine.run("extract:here", &one, &|_| {}).unwrap();
        assert_eq!(out, dir.join("project"));
        assert!(out.join("readme.md").is_file());

        // Again: never overwrites, and leaves no staging folder behind.
        let again = ArchiveEngine.run("extract:here", &one, &|_| {}).unwrap();
        assert_eq!(again, dir.join("project (2)"));
        assert_eq!(names(&dir), vec!["one.zip", "project", "project (2)", "two", "two.zip"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extracts_tar_gz_and_gz() {
        let dir = temp_dir("tgz");
        let tgz = dir.join("bundle.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(File::create(&tgz).unwrap(), flate2::Compression::default());
            let mut t = tar::Builder::new(gz);
            for (name, body) in [("x.txt", "1"), ("y.txt", "2")] {
                let mut h = tar::Header::new_gnu();
                h.set_size(body.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                t.append_data(&mut h, name, body.as_bytes()).unwrap();
            }
            t.into_inner().unwrap().finish().unwrap();
        }
        let out = ArchiveEngine.run("extract:here", &tgz, &|_| {}).unwrap();
        assert_eq!(out, dir.join("bundle"));
        assert_eq!(names(&out), vec!["x.txt", "y.txt"]);

        let gz = dir.join("data.csv.gz");
        let mut enc = flate2::write::GzEncoder::new(File::create(&gz).unwrap(), flate2::Compression::default());
        enc.write_all(b"a,b\n1,2\n").unwrap();
        enc.finish().unwrap();
        let out = ArchiveEngine.run("extract:here", &gz, &|_| {}).unwrap();
        assert_eq!(out, dir.join("data.csv"));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "a,b\n1,2\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extracts_7z() {
        let dir = temp_dir("7z");
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("inner")).unwrap();
        std::fs::write(src.join("inner/note.txt"), "seven").unwrap();
        std::fs::write(src.join("top.txt"), "top").unwrap();
        let archive = dir.join("pack.7z");
        sevenz_rust2::compress_to_path(&src, &archive).unwrap();
        let out = ArchiveEngine.run("extract:here", &archive, &|_| {}).unwrap();
        assert_eq!(out, dir.join("pack"));
        assert_eq!(std::fs::read_to_string(out.join("inner/note.txt")).unwrap(), "seven");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn empty_and_broken_archives_fail_cleanly() {
        let dir = temp_dir("bad");
        let bad = dir.join("broken.zip");
        std::fs::write(&bad, b"not a zip").unwrap();
        let err = ArchiveEngine.run("extract:here", &bad, &|_| {}).unwrap_err();
        assert!(err.starts_with("Couldn't read this ZIP file"), "{err}");
        assert_eq!(names(&dir), vec!["broken.zip"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
