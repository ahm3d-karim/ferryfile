//! Office conversions through an installed LibreOffice (optional, not bundled).
//! Actions only appear when `soffice.exe` is found.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::{ext_of, output_path, tool_command, Action, Engine, Progress};

/// Input extensions and the formats each family can be saved as.
const FAMILIES: &[(&[&str], &[(&str, &str)])] = &[
    (
        &["doc", "docx", "odt", "rtf"],
        &[("pdf", "PDF"), ("docx", "DOCX"), ("odt", "ODT")],
    ),
    (&["ppt", "pptx", "odp"], &[("pdf", "PDF"), ("pptx", "PPTX")]),
    (
        &["xls", "xlsx", "ods", "csv"],
        &[("pdf", "PDF"), ("xlsx", "XLSX"), ("csv", "CSV")],
    ),
];

pub struct OfficeEngine;

fn soffice() -> Option<&'static Path> {
    static SOFFICE: OnceLock<Option<PathBuf>> = OnceLock::new();
    SOFFICE
        .get_or_init(|| {
            let mut candidates: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)"]
                .iter()
                .filter_map(|v| std::env::var_os(v))
                .map(|dir| PathBuf::from(dir).join("LibreOffice/program/soffice.exe"))
                .collect();
            if let Some(path) = std::env::var_os("PATH") {
                candidates.extend(std::env::split_paths(&path).map(|d| d.join("soffice.exe")));
            }
            candidates.into_iter().find(|p| p.is_file())
        })
        .as_deref()
}

fn targets_for(ext: &str) -> &'static [(&'static str, &'static str)] {
    FAMILIES
        .iter()
        .find(|(inputs, _)| inputs.contains(&ext))
        .map(|(_, targets)| *targets)
        .unwrap_or(&[])
}

impl Engine for OfficeEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        if soffice().is_none() {
            return Vec::new();
        }
        let ext = ext_of(path);
        targets_for(&ext)
            .iter()
            .filter(|(t, _)| *t != ext)
            .map(|(t, label)| Action::new(&format!("convert:{t}"), label, "Convert"))
            .collect()
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        match action_id.split_once(':')? {
            ("convert", ext) => Some(ext.to_string()),
            _ => None,
        }
    }

    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
        let exe = soffice().ok_or("LibreOffice isn't installed")?;
        let (_, ext) = action_id.split_once(':').ok_or("bad action id")?;

        // A private profile and output folder: no clash with a running
        // LibreOffice window, and we know exactly which file it wrote.
        let work = std::env::temp_dir().join(format!("ferryfile-office-{}-{}", std::process::id(), rand_suffix()));
        let profile = work.join("profile");
        let outdir = work.join("out");
        std::fs::create_dir_all(&outdir).map_err(|e| e.to_string())?;
        let profile_url = format!("file:///{}", profile.display().to_string().replace('\\', "/"));

        let result = tool_command(exe)
            .arg(format!("-env:UserInstallation={profile_url}"))
            .args(["--headless", "--norestore", "--convert-to", ext, "--outdir"])
            .arg(&outdir)
            .arg(input)
            .output();

        let produced = std::fs::read_dir(&outdir)
            .ok()
            .and_then(|mut d| d.next())
            .and_then(|e| e.ok())
            .map(|e| e.path());
        let outcome = match (result, produced) {
            (Ok(r), Some(file)) if r.status.success() => {
                let out = output_path(input, "", ext);
                std::fs::rename(&file, &out)
                    .or_else(|_| std::fs::copy(&file, &out).map(|_| ()))
                    .map_err(|e| e.to_string())
                    .map(|_| out)
            }
            (Ok(r), _) => Err(format!(
                "LibreOffice couldn't convert this file: {}",
                String::from_utf8_lossy(&r.stderr).lines().last().unwrap_or("no output")
            )),
            (Err(e), _) => Err(format!("Could not start LibreOffice: {e}")),
        };
        let _ = std::fs::remove_dir_all(&work);
        progress(1.0);
        outcome
    }
}

fn rand_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families() {
        assert_eq!(targets_for("pptx").len(), 2);
        assert!(targets_for("csv").iter().any(|(t, _)| *t == "xlsx"));
        assert!(targets_for("png").is_empty());
    }
}
