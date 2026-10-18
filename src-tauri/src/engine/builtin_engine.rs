//! Ferryfile's own converters, no LibreOffice needed:
//! - spreadsheets (XLSX/XLSM/XLSB/XLS/ODS via calamine, CSV/TSV) to CSV,
//!   Markdown, HTML and PDF;
//! - documents to PDF: pandoc writes Typst markup, which we typeset in-process;
//! - Typst sources (.typ) straight to PDF.

use std::path::{Path, PathBuf};

use calamine::{open_workbook_auto, Data, Reader};

use super::doc_engine::{pandoc, reader_for};
use super::typeset::{typst_str, typst_to_pdf};
use super::{ext_of, output_dir, output_path, tool_command, Action, Engine, Progress, FOLDER_EXT};

const WORKBOOKS: &[&str] = &["xlsx", "xlsm", "xlsb", "xls", "ods"];
const DELIMITED: &[&str] = &["csv", "tsv"];

pub struct BuiltinEngine;

struct Sheet {
    name: String,
    rows: Vec<Vec<String>>,
}

fn cell_text(cell: &Data) -> String {
    match cell {
        Data::Empty => String::new(),
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(t) if t.format("%H:%M:%S").to_string() == "00:00:00" => t.format("%Y-%m-%d").to_string(),
            Some(t) => t.format("%Y-%m-%d %H:%M").to_string(),
            None => cell.to_string(),
        },
        other => other.to_string(),
    }
}

/// Minimal RFC 4180 reader: quoted fields, doubled quotes, embedded newlines.
fn parse_delimited(text: &str, sep: char) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.trim_start_matches('\u{feff}').chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', true) => quoted = false,
            ('"', false) if field.is_empty() => quoted = true,
            (c, false) if c == sep => row.push(std::mem::take(&mut field)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (c, _) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

fn read_sheets(path: &Path) -> Result<Vec<Sheet>, String> {
    let ext = ext_of(path);
    if DELIMITED.contains(&ext.as_str()) {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let sep = if ext == "tsv" { '\t' } else { ',' };
        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Sheet").to_string();
        return Ok(vec![Sheet { name, rows: parse_delimited(&text, sep) }]);
    }
    let mut book = open_workbook_auto(path).map_err(|e| format!("Could not open spreadsheet: {e}"))?;
    let mut sheets = Vec::new();
    for name in book.sheet_names() {
        let range = book.worksheet_range(&name).map_err(|e| format!("Sheet '{name}': {e}"))?;
        let rows = range.rows().map(|r| r.iter().map(cell_text).collect()).collect();
        sheets.push(Sheet { name, rows });
    }
    Ok(sheets)
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn to_csv(rows: &[Vec<String>]) -> String {
    rows.iter()
        .map(|r| r.iter().map(|c| csv_field(c)).collect::<Vec<_>>().join(","))
        .collect::<Vec<_>>()
        .join("\r\n")
}

fn width(rows: &[Vec<String>]) -> usize {
    rows.iter().map(Vec::len).max().unwrap_or(0)
}

fn to_markdown(sheets: &[Sheet]) -> String {
    let md_cell = |s: &str| s.replace('|', "\\|").replace(['\r', '\n'], " ");
    let mut out = String::new();
    for sheet in sheets.iter().filter(|s| !s.rows.is_empty()) {
        let n = width(&sheet.rows);
        out.push_str(&format!("## {}\n\n", sheet.name));
        for (i, row) in sheet.rows.iter().enumerate() {
            let cells: Vec<String> = (0..n).map(|j| md_cell(row.get(j).map_or("", String::as_str))).collect();
            out.push_str(&format!("| {} |\n", cells.join(" | ")));
            if i == 0 {
                out.push_str(&format!("|{}\n", " --- |".repeat(n)));
            }
        }
        out.push('\n');
    }
    out.trim_end().to_string() + "\n"
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn to_html(title: &str, sheets: &[Sheet]) -> String {
    let mut body = String::new();
    for sheet in sheets.iter().filter(|s| !s.rows.is_empty()) {
        body.push_str(&format!("<h2>{}</h2>\n<table>\n", html_escape(&sheet.name)));
        for (i, row) in sheet.rows.iter().enumerate() {
            let tag = if i == 0 { "th" } else { "td" };
            let cells: String = row.iter().map(|c| format!("<{tag}>{}</{tag}>", html_escape(c))).collect();
            body.push_str(&format!("<tr>{cells}</tr>\n"));
        }
        body.push_str("</table>\n");
    }
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>{}</title>\n\
         <style>body{{font-family:system-ui,sans-serif;margin:2rem}}table{{border-collapse:collapse;margin-bottom:2rem}}\
         th,td{{border:1px solid #ccc;padding:4px 8px;text-align:left}}th{{background:#f0f0f0}}</style>\n\
         </head><body>\n{body}</body></html>\n",
        html_escape(title)
    )
}

fn to_typst(sheets: &[Sheet]) -> String {
    let mut out = String::from(
        "#set page(paper: \"a4\", flipped: true, margin: 1.5cm)\n#set text(size: 8pt)\n\
         #set table(stroke: 0.5pt + luma(180), inset: 4pt, fill: (_, y) => if y == 0 { luma(235) })\n",
    );
    let sheets: Vec<_> = sheets.iter().filter(|s| !s.rows.is_empty()).collect();
    for (i, sheet) in sheets.iter().enumerate() {
        if i > 0 {
            out.push_str("#pagebreak()\n");
        }
        let n = width(&sheet.rows).max(1);
        out.push_str(&format!("#heading({})\n#table(columns: (1fr,) * {n},\n", typst_str(&sheet.name)));
        for row in &sheet.rows {
            let cells: Vec<String> = (0..n).map(|j| typst_str(row.get(j).map_or("", String::as_str))).collect();
            out.push_str(&format!("  {},\n", cells.join(", ")));
        }
        out.push_str(")\n");
    }
    out
}

fn write_pdf(markup: String, root: &Path, input: &Path) -> Result<PathBuf, String> {
    let pdf = typst_to_pdf(markup, root)?;
    let out = output_path(input, "", "pdf");
    std::fs::write(&out, pdf).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Document -> Typst markup (pandoc) -> PDF (in-process).
fn document_to_pdf(input: &Path, reader: &str) -> Result<PathBuf, String> {
    let work = std::env::temp_dir().join(format!(
        "ferryfile-typeset-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let result = (|| {
        let out = tool_command(pandoc()?)
            .current_dir(&work)
            .args(["--from", reader, "--to", "typst", "--standalone", "--extract-media=media", "--output", "main.typ"])
            .arg(input)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            let msg = String::from_utf8_lossy(&out.stderr);
            return Err(format!("pandoc failed: {}", msg.lines().last().unwrap_or("unknown error")));
        }
        let markup = std::fs::read_to_string(work.join("main.typ")).map_err(|e| e.to_string())?;
        write_pdf(markup, &work, input)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

impl Engine for BuiltinEngine {
    fn actions(&self, path: &Path) -> Vec<Action> {
        let ext = ext_of(path);
        let mut out = Vec::new();
        if WORKBOOKS.contains(&ext.as_str()) {
            out.push(Action::new("convert:csv", "CSV", "Convert"));
            out.push(Action::new("convert:md", "Markdown", "Convert"));
            out.push(Action::new("convert:html", "HTML", "Convert"));
            out.push(Action::new("convert:pdf", "PDF", "Convert"));
            out.push(Action::new("sheets:csv", "CSV per sheet", "Split"));
        } else if DELIMITED.contains(&ext.as_str()) || ext == "typ" || reader_for(&ext).is_some() {
            out.push(Action::new("convert:pdf", "PDF", "Convert"));
        }
        out
    }

    fn output_ext(&self, action_id: &str, _input: &Path) -> Option<String> {
        match action_id.split_once(':')? {
            ("convert", ext) => Some(ext.to_string()),
            ("sheets", _) => Some(FOLDER_EXT.into()),
            _ => None,
        }
    }

    fn run(&self, action_id: &str, input: &Path, progress: Progress) -> Result<PathBuf, String> {
        let ext = ext_of(input);
        let is_sheet = WORKBOOKS.contains(&ext.as_str()) || DELIMITED.contains(&ext.as_str());
        let stem = input.file_stem().and_then(|s| s.to_str()).unwrap_or("sheet").to_string();
        let result = match action_id.split_once(':').ok_or("bad action id")? {
            ("convert", "pdf") if is_sheet => {
                let root = input.parent().unwrap_or(Path::new("."));
                write_pdf(to_typst(&read_sheets(input)?), root, input)
            }
            ("convert", "pdf") if ext == "typ" => {
                let markup = std::fs::read_to_string(input).map_err(|e| e.to_string())?;
                write_pdf(markup, input.parent().unwrap_or(Path::new(".")), input)
            }
            ("convert", "pdf") => {
                let reader = reader_for(&ext).ok_or("Unsupported document")?;
                document_to_pdf(input, reader)
            }
            ("convert", fmt) => {
                let sheets = read_sheets(input)?;
                let body = match fmt {
                    "csv" => to_csv(sheets.first().map_or(&[][..], |s| &s.rows)),
                    "md" => to_markdown(&sheets),
                    "html" => to_html(&stem, &sheets),
                    other => return Err(format!("Unsupported target '{other}'")),
                };
                let out = output_path(input, "", fmt);
                std::fs::write(&out, body).map_err(|e| e.to_string())?;
                Ok(out)
            }
            ("sheets", _) => {
                let dir = output_dir(input, "-sheets")?;
                for sheet in read_sheets(input)? {
                    let safe: String = sheet
                        .name
                        .chars()
                        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
                        .collect();
                    std::fs::write(dir.join(format!("{safe}.csv")), to_csv(&sheet.rows))
                        .map_err(|e| e.to_string())?;
                }
                Ok(dir)
            }
            (kind, _) => Err(format!("Unknown action kind '{kind}'")),
        };
        progress(1.0);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_csv() {
        let rows = parse_delimited("a,\"b,1\",\"say \"\"hi\"\"\"\r\nx,\"multi\nline\",\n", ',');
        assert_eq!(rows, vec![vec!["a", "b,1", "say \"hi\""], vec!["x", "multi\nline", ""]]);
        assert_eq!(to_csv(&rows), "a,\"b,1\",\"say \"\"hi\"\"\"\r\nx,\"multi\nline\",");
    }

    #[test]
    fn sheet_outputs() {
        let sheets = vec![Sheet {
            name: "Fruit".into(),
            rows: vec![vec!["name".into(), "qty".into()], vec!["a|b".into(), "3".into()]],
        }];
        assert_eq!(to_markdown(&sheets), "## Fruit\n\n| name | qty |\n| --- | --- |\n| a\\|b | 3 |\n");
        assert!(to_html("t", &sheets).contains("<th>name</th>"));
        let pdf = typst_to_pdf(to_typst(&sheets), &std::env::temp_dir()).unwrap();
        assert!(pdf.starts_with(b"%PDF"));
    }

    #[test]
    fn workbook_actions() {
        let dir = std::env::temp_dir().join(format!("ferryfile-xlsx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let xlsx = dir.join("book.xlsx");
        let mut wb = rust_xlsxwriter::Workbook::new();
        let fruit = wb.add_worksheet().set_name("Fruit").unwrap();
        fruit.write_string(0, 0, "name").unwrap();
        fruit.write_string(0, 1, "qty").unwrap();
        fruit.write_string(1, 0, "pear, ripe").unwrap();
        fruit.write_number(1, 1, 3.0).unwrap();
        let other = wb.add_worksheet().set_name("Q<2>").unwrap();
        other.write_string(0, 0, "total").unwrap();
        wb.save(&xlsx).unwrap();

        let noop = |_: f32| {};
        let e = BuiltinEngine;
        let csv = std::fs::read_to_string(e.run("convert:csv", &xlsx, &noop).unwrap()).unwrap();
        assert_eq!(csv, "name,qty\r\n\"pear, ripe\",3");
        let md = std::fs::read_to_string(e.run("convert:md", &xlsx, &noop).unwrap()).unwrap();
        assert!(md.contains("## Fruit") && md.contains("## Q<2>"));
        let pdf = e.run("convert:pdf", &xlsx, &noop).unwrap();
        assert!(std::fs::read(pdf).unwrap().starts_with(b"%PDF"));
        let sheets = e.run("sheets:csv", &xlsx, &noop).unwrap();
        assert!(sheets.join("Q_2_.csv").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Needs pandoc, so opt-in: `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn documents_to_pdf() {
        let dir = std::env::temp_dir().join(format!("ferryfile-builtin-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let md = dir.join("report.md");
        std::fs::write(&md, "# Report\n\nSome *styled* text.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n").unwrap();
        let noop = |_: f32| {};
        let docx = super::super::doc_engine::DocEngine.run("convert:docx", &md, &noop).unwrap();
        let pptx = dir.join("deck.pptx");
        assert!(tool_command(pandoc().unwrap()).arg(&md).arg("-o").arg(&pptx).status().unwrap().success());
        let csv = dir.join("data.csv");
        std::fs::write(&csv, "x,y\n1,2\n").unwrap();
        for input in [&md, &docx, &pptx, &csv] {
            let pdf = BuiltinEngine.run("convert:pdf", input, &noop).unwrap_or_else(|e| panic!("{input:?}: {e}"));
            assert!(std::fs::read(&pdf).unwrap().starts_with(b"%PDF"), "{input:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
