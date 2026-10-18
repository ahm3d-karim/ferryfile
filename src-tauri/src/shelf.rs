//! The shelf: a small floating window that holds files for later. Drop files
//! on it (or "Add to shelf" from the drawer), then drag them out into any app.
//! Items survive restarts; ones that no longer exist are dropped quietly.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State};

const CHANGED_EVENT: &str = "ferryfile-shelf-changed";

pub struct Shelf(Mutex<Vec<String>>);

fn store_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("shelf.json"))
}

pub fn load(app: &AppHandle) -> Shelf {
    let items: Vec<String> = store_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Shelf(Mutex::new(items.into_iter().filter(|p| Path::new(p).exists()).collect()))
}

fn changed(app: &AppHandle, items: &[String]) {
    if let Some(path) = store_path(app) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string(items).unwrap_or_default());
    }
    // Both the shelf and the drawer (its shelf button shows the count) listen.
    let _ = app.emit(CHANGED_EVENT, items);
}

#[tauri::command]
pub fn shelf_items(app: AppHandle, shelf: State<Shelf>) -> Vec<String> {
    let mut items = shelf.0.lock().unwrap();
    let before = items.len();
    items.retain(|p| Path::new(p).exists());
    if items.len() != before {
        changed(&app, &items);
    }
    items.clone()
}

#[tauri::command]
pub fn shelf_add(app: AppHandle, shelf: State<Shelf>, paths: Vec<String>) {
    let mut items = shelf.0.lock().unwrap();
    for p in paths {
        if !items.contains(&p) {
            items.push(p);
        }
    }
    changed(&app, &items);
}

#[tauri::command]
pub fn shelf_remove(app: AppHandle, shelf: State<Shelf>, paths: Vec<String>) {
    let mut items = shelf.0.lock().unwrap();
    items.retain(|p| !paths.contains(p));
    changed(&app, &items);
}

#[tauri::command]
pub fn shelf_clear(app: AppHandle, shelf: State<Shelf>) {
    let mut items = shelf.0.lock().unwrap();
    items.clear();
    changed(&app, &items);
}

/// Shows the shelf without taking focus. The first time, it goes to the
/// bottom of the screen just inside the drawer's edge, so the two never
/// overlap; after that it stays wherever the user dragged it.
#[tauri::command]
pub fn show_shelf(app: AppHandle) {
    static PLACED: std::sync::Once = std::sync::Once::new();
    let Some(w) = app.get_webview_window("shelf") else { return };
    PLACED.call_once(|| {
        if let (Ok(Some(m)), Ok(size)) = (w.primary_monitor(), w.outer_size()) {
            let scale = m.scale_factor();
            let margin = (16.0 * scale) as i32;
            let drawer = (crate::drag::DRAWER_WIDTH_LOGICAL * scale) as i32;
            let (mw, mh) = (m.size().width as i32, m.size().height as i32);
            let x = match crate::settings::current().drawer_side {
                crate::settings::Side::Right => m.position().x + mw - drawer - size.width as i32 - margin,
                crate::settings::Side::Left => m.position().x + drawer + margin,
            };
            // Leave room for the taskbar.
            let y = m.position().y + mh - size.height as i32 - (64.0 * scale) as i32;
            let _ = w.set_position(PhysicalPosition::new(x.max(m.position().x), y));
        }
    });
    let _ = w.show();
}

#[tauri::command]
pub fn toggle_shelf(app: AppHandle) {
    match app.get_webview_window("shelf") {
        Some(w) if w.is_visible().unwrap_or(false) => {
            let _ = w.hide();
        }
        Some(_) => show_shelf(app),
        None => {}
    }
}

#[tauri::command]
pub fn hide_shelf(app: AppHandle) {
    if let Some(w) = app.get_webview_window("shelf") {
        let _ = w.hide();
    }
}

/// Path of a small PNG used as the cursor image when dragging items out.
#[tauri::command]
pub fn drag_icon(app: AppHandle) -> Result<String, String> {
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?;
    let path = dir.join("drag-icon.png");
    if !path.exists() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let size = 48u32;
        let img = image::RgbaImage::from_fn(size, size, |x, y| {
            // Rounded square in Ferryfile's teal.
            let (cx, cy) = (x.min(size - 1 - x), y.min(size - 1 - y));
            let r = 10u32;
            let outside = cx < r && cy < r && (r - cx).pow(2) + (r - cy).pow(2) > r * r;
            if outside { image::Rgba([0, 0, 0, 0]) } else { image::Rgba([45, 212, 191, 230]) }
        });
        img.save(&path).map_err(|e| e.to_string())?;
    }
    Ok(path.to_string_lossy().into_owned())
}
