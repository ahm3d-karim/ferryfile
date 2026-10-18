//! Office files to PDF through an installed Microsoft Office (Word, Excel,
//! PowerPoint) via COM automation. Optional: each family only appears when its
//! Office app is registered. Highest fidelity for Office formats.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::{ext_of, output_path, tool_command, Action, Engine, Progress};

#[derive(Clone, Copy)]
enum App {
    Word,
    Excel,
    PowerPoint,
}

impl App {
    fn for_ext(ext: &str) -> Option<App> {
        match ext {
            "doc" | "docx" | "docm" | "rtf" | "odt" => Some(App::Word),
            "xls" | "xlsx" | "xlsm" | "xlsb" | "ods" | "csv" => Some(App::Excel),
            "ppt" | "pptx" | "pptm" | "odp" => Some(App::PowerPoint),
            _ => None,
        }
    }

    fn prog_id(self) -> &'static str {
        match self {
            App::Word => "Word.Application",
            App::Excel => "Excel.Application",
            App::PowerPoint => "PowerPoint.Application",
        }
    }

    fn installed(self) -> bool {
        static CACHE: OnceLock<[bool; 3]> = OnceLock::new();
        let found = CACHE.get_or_init(|| {
            [App::Word, App::Excel, App::PowerPoint].map(|app| {
                tool_command(Path::new("reg"))
                    .args(["query", &format!("HKCR\\{}\\CLSID", app.prog_id())])
                    .output()
                    .is_ok_and(|o| o.status.success())
            })
        });
        found[self as usize]
    }

    /// PowerShell that opens `input` read-only and exports `out` as PDF.
    fn script(self, input: &str, out: &str) -> String {
        let body = match self {
            App::Word => "$d = $app.Documents.Open($in, $false, $true); $d.ExportAsFixedFormat($out, 17); $d.Close($false)",
            App::Excel => "$app.DisplayAlerts = $false; $d = $app.Workbooks.Open($in, 0, $true); $d.ExportAsFixedFormat(0, $out); $d.Close($false)",
            App::PowerPoint => "$d = $app.Presentations.Open($in, -1, 0, 0); $d.SaveAs($out, 32); $d.Close()",
        };
        format!(
            "$ErrorActionPreference = 'Stop'; $in = {}; $out = {}; \
             $app = New-Object -ComObject {}; \
             try {{ {body} }} finally {{ $app.Quit(); [void][Runtime.InteropServices.Marshal]::ReleaseComObject($app) }}",
            ps_quote(input),
            ps_quote(out),
            self.prog_id(),
        )
    }
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

pub struct MsOfficeEngine;

impl Engine for MsOfficeEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        match App::for_ext(&ext_of(path)) {
            Some(app) if app.installed() => vec![Action::new("convert:pdf", "PDF", "Convert")],
            _ => Vec::new(),
        }
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        (action_id == "convert:pdf").then(|| "pdf".into())
    }

    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
        if action_id != "convert:pdf" {
            return Err(format!("Unknown action '{action_id}'"));
        }
        let app = App::for_ext(&ext_of(input)).ok_or("Not an Office file")?;
        let input = std::path::absolute(input).map_err(|e| e.to_string())?;
        let out = output_path(&input, "", "pdf");
        let result = tool_command(Path::new("powershell"))
            .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"])
            .arg(app.script(&input.to_string_lossy(), &out.to_string_lossy()))
            .output()
            .map_err(|e| format!("Could not start PowerShell: {e}"))?;
        progress(1.0);
        if result.status.success() && out.exists() {
            Ok(out)
        } else {
            let _ = std::fs::remove_file(&out);
            let msg = String::from_utf8_lossy(&result.stderr);
            Err(format!("Office couldn't export this file: {}", msg.lines().find(|l| !l.trim().is_empty()).unwrap_or("unknown error")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_and_scripts() {
        assert_eq!(ps_quote("C:\\it's.docx"), "'C:\\it''s.docx'");
        let s = App::Word.script("C:\\a.docx", "C:\\a.pdf");
        assert!(s.contains("$in = 'C:\\a.docx'") && s.contains("ExportAsFixedFormat($out, 17)"));
        assert!(matches!(App::for_ext("pptx"), Some(App::PowerPoint)));
    }
}
