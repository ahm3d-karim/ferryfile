//! Commands behind the drawer's editing views (Trim, Crop, Merge, Split
//! pages). The edits themselves are ordinary actions (`trim:…`, `crop:…`,
//! `split:…`, `extract:…`, `combine:video`) run through `run_action` and
//! `run_combine`; these commands only feed the views: previews, page counts
//! and merge plans.
//!
//! Previews load through Tauri's asset protocol. Its scope starts empty and
//! each file is allowed only when its view opens, so the page can't read
//! anything else on disk.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::engine::{self, image_crop, media_edit, pdf_tools, ranges};

fn allow(app: &AppHandle, path: &Path) -> Result<String, String> {
    if !path.is_file() {
        return Err(format!("{} no longer exists", path.display()));
    }
    app.asset_protocol_scope().allow_file(path).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

/// Lets the Trim view play `path` directly.
#[tauri::command]
pub fn media_source(app: AppHandle, path: String) -> Result<String, String> {
    allow(&app, Path::new(&path))
}

/// A small converted copy of `path` for formats the view can't play (AVI,
/// WMV, HEVC, WMA…). Cancellable through `cancel_job`.
#[tauri::command]
pub async fn media_preview(app: AppHandle, path: String, job_id: Option<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _job = engine::jobs::begin(job_id.as_deref());
        let preview = media_edit::make_preview(Path::new(&path), &|_| {})?;
        allow(&app, &preview)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// A picture the Crop view can show, plus the real size crops refer to.
#[tauri::command]
pub async fn crop_source(app: AppHandle, path: String) -> Result<image_crop::CropSource, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let src = image_crop::crop_source(Path::new(&path))?;
        allow(&app, Path::new(&src.path))?;
        Ok(src)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageRanges {
    /// Pages in the PDF.
    pub count: u32,
    /// The ranges as they go into an action id, e.g. `1-3,5,8-`.
    pub spec: String,
    /// 1-based, inclusive, open ends filled in.
    pub ranges: Vec<[u32; 2]>,
}

/// Checks typed page ranges against the PDF, for the Split pages view.
/// With empty `ranges` it just reports the page count.
#[tauri::command]
pub async fn pdf_ranges(path: String, ranges: String) -> Result<PageRanges, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let count = pdf_tools::page_count(Path::new(&path))?;
        if ranges.trim().is_empty() {
            return Ok(PageRanges { count, spec: String::new(), ranges: Vec::new() });
        }
        let resolved = ranges::resolve(&ranges, count)?;
        Ok(PageRanges {
            count,
            spec: ranges::canonical(&ranges)?,
            ranges: resolved.into_iter().map(|r| [*r.start(), *r.end()]).collect(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Whether `paths` merge without re-encoding, for the Merge view.
#[tauri::command]
pub async fn merge_plan(paths: Vec<String>) -> Result<media_edit::MergePlan, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        media_edit::merge_plan(&inputs)
    })
    .await
    .map_err(|e| e.to_string())?
}
