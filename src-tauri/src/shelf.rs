//! Shelves: small floating windows that hold files for later. Shake the cursor
//! while dragging to open one under it, drop files on it, run actions on its
//! contents, and drag everything back out into any app.
//!
//! Each shelf is its own window (`shelf-<id>`). Shelves that hold items survive
//! restarts; items that no longer exist are dropped quietly. An unpinned shelf
//! closes itself once you've dragged everything out of it.
//!
//! Text, links and images (dropped or pasted) are saved as files in a
//! "Shelf items" folder first, so shelves only ever hold paths.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::content::{self, Content};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const CHANGED_EVENT: &str = "ferryfile-shelf-changed";
const COUNT_EVENT: &str = "ferryfile-shelves-count";
const WIDTH: f64 = 260.0;
const HEIGHT: f64 = 360.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShelfData {
    pub id: String,
    pub items: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
}

#[derive(Default)]
pub struct Shelves {
    shelves: Mutex<Vec<ShelfData>>,
    /// The shelf "Add to shelf" puts things on.
    last_used: Mutex<Option<String>>,
}

fn label(id: &str) -> String {
    format!("shelf-{id}")
}

fn new_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    format!("{:x}", nanos)
}

fn store_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join("shelves.json"))
}

pub fn load(app: &AppHandle) -> Shelves {
    let dir = app.path().app_data_dir().ok();
    let mut shelves: Vec<ShelfData> = store_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // Migrate the single shelf of earlier versions.
    if let Some(old) = dir.map(|d| d.join("shelf.json")) {
        if let Some(items) = std::fs::read_to_string(&old).ok().and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok()) {
            if !items.is_empty() {
                shelves.push(ShelfData { id: new_id(), items, pinned: true });
            }
            let _ = std::fs::remove_file(old);
        }
    }
    for s in &mut shelves {
        s.items.retain(|p| Path::new(p).exists());
    }
    shelves.retain(|s| !s.items.is_empty() || s.pinned);
    clean_content(app, &shelves);
    Shelves { shelves: Mutex::new(shelves), last_used: Mutex::new(None) }
}

fn save(app: &AppHandle, shelves: &[ShelfData]) {
    if let Some(path) = store_path(app) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, serde_json::to_string(shelves).unwrap_or_default());
    }
}

/// Persists and tells the shelf window (and the drawer's counter) about it.
fn changed(app: &AppHandle, shelves: &[ShelfData], id: &str) {
    save(app, shelves);
    if let Some(s) = shelves.iter().find(|s| s.id == id) {
        let _ = app.emit_to(label(id).as_str(), CHANGED_EVENT, s);
    }
    let total: usize = shelves.iter().map(|s| s.items.len()).sum();
    let _ = app.emit(COUNT_EVENT, total);
}

/// Opens (or shows) the window for shelf `id`. Must not run on the main thread
/// inside a command or menu handler: creating a window there deadlocks on Windows.
fn open_window(app: &AppHandle, id: &str, at: Option<PhysicalPosition<i32>>) -> Option<WebviewWindow> {
    let label = label(id);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.show();
        return Some(w);
    }
    let w = WebviewWindowBuilder::new(app, &label, WebviewUrl::App("shelf.html".into()))
        .title("Ferryfile Shelf")
        .inner_size(WIDTH, HEIGHT)
        .transparent(true)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .shadow(false)
        .visible(false)
        .focused(false)
        .disable_drag_drop_handler()
        .build()
        .ok()?;
    crate::dropbridge::install(&w);
    // Like the drawer: never steal focus from the app you're dragging from.
    crate::drag::set_focusable(&w, false);
    let pos = at.unwrap_or_else(|| default_position(app, &w));
    let _ = w.set_position(pos);
    let _ = w.show();
    Some(w)
}

/// True if any shelf window is currently on screen.
pub fn any_visible(app: &AppHandle) -> bool {
    app.webview_windows()
        .iter()
        .any(|(l, w)| l.starts_with("shelf-") && w.is_visible().unwrap_or(false))
}

fn open_shelf_count(app: &AppHandle) -> usize {
    app.webview_windows().keys().filter(|l| l.starts_with("shelf-")).count()
}

/// Bottom of the screen just inside the drawer's edge, staggered per shelf.
fn default_position(app: &AppHandle, w: &WebviewWindow) -> PhysicalPosition<i32> {
    let (Ok(Some(m)), Ok(size)) = (w.primary_monitor(), w.outer_size()) else {
        return PhysicalPosition::new(100, 100);
    };
    let scale = m.scale_factor();
    let margin = (16.0 * scale) as i32;
    let stagger = (24.0 * scale) as i32 * open_shelf_count(app).saturating_sub(1) as i32;
    let drawer = (crate::drag::DRAWER_WIDTH_LOGICAL * scale) as i32;
    let (mw, mh) = (m.size().width as i32, m.size().height as i32);
    let x = match crate::settings::current().drawer_side {
        crate::settings::Side::Right => m.position().x + mw - drawer - size.width as i32 - margin - stagger,
        crate::settings::Side::Left => m.position().x + drawer + margin + stagger,
    };
    let y = m.position().y + mh - size.height as i32 - (64.0 * scale) as i32 - stagger;
    PhysicalPosition::new(x.max(m.position().x), y.max(m.position().y))
}

/// Shake handler (runs on the hook's worker thread): a new, empty shelf with
/// the cursor over its top so the file can be dropped right away.
pub fn spawn_at_cursor(app: &AppHandle, x: i32, y: i32) {
    let shelves = app.state::<Shelves>();
    let id = new_id();
    {
        let mut list = shelves.shelves.lock().unwrap();
        list.push(ShelfData { id: id.clone(), items: Vec::new(), pinned: false });
        changed(app, &list, &id);
    }
    *shelves.last_used.lock().unwrap() = Some(id.clone());

    let scale = app
        .monitor_from_point(x as f64, y as f64)
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or(1.0);
    let (w, h) = ((WIDTH * scale) as i32, (HEIGHT * scale) as i32);
    // Center horizontally; cursor about a third of the way down.
    let mut pos = PhysicalPosition::new(x - w / 2, y - h / 3);
    if let Some(m) = app.monitor_from_point(x as f64, y as f64).ok().flatten() {
        let (mx, my) = (m.position().x, m.position().y);
        let (mw, mh) = (m.size().width as i32, m.size().height as i32);
        pos.x = pos.x.clamp(mx, mx + mw - w);
        pos.y = pos.y.clamp(my, my + mh - h);
    }
    open_window(app, &id, Some(pos));
}

#[tauri::command]
pub fn shelf_state(app: AppHandle, shelves: State<Shelves>, id: String) -> Option<ShelfData> {
    let mut list = shelves.shelves.lock().unwrap();
    let shelf = list.iter_mut().find(|s| s.id == id)?;
    let before = shelf.items.len();
    shelf.items.retain(|p| Path::new(p).exists());
    let (removed, snapshot) = (shelf.items.len() != before, shelf.clone());
    if removed {
        changed(&app, &list, &id);
    }
    Some(snapshot)
}

/// Adds items to shelf `id`, or to the last-used shelf (creating and showing
/// one if needed) when `id` is None. Returns the shelf's id.
pub fn add(app: &AppHandle, id: Option<String>, paths: Vec<String>) -> String {
    let shelves = app.state::<Shelves>();
    let (id, need_window) = {
        let mut list = shelves.shelves.lock().unwrap();
        let wanted = id.or_else(|| shelves.last_used.lock().unwrap().clone());
        let id = match wanted.filter(|w| list.iter().any(|s| &s.id == w)) {
            Some(id) => id,
            None => {
                let id = new_id();
                list.push(ShelfData { id: id.clone(), items: Vec::new(), pinned: false });
                id
            }
        };
        let shelf = list.iter_mut().find(|s| s.id == id).unwrap();
        for p in paths {
            if !shelf.items.contains(&p) {
                shelf.items.push(p);
            }
        }
        changed(app, &list, &id);
        let need_window = app.get_webview_window(&label(&id)).is_none();
        (id, need_window)
    };
    *shelves.last_used.lock().unwrap() = Some(id.clone());
    if need_window {
        open_window(app, &id, None);
    }
    id
}

#[tauri::command]
pub async fn shelf_add(app: AppHandle, id: Option<String>, paths: Vec<String>) -> Result<String, String> {
    Ok(add(&app, id, paths))
}

// ---------------------------------------------------------------- content

/// Where text, links and images put on a shelf are saved as files.
fn content_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_local_data_dir().map(|d| d.join("Shelf items")).map_err(|e| e.to_string())
}

/// Saves `content` as files and puts them on shelf `id` (or the last-used
/// one). Returns the files' paths.
async fn add_content(app: AppHandle, id: Option<String>, content: Content) -> Result<Vec<String>, String> {
    let dir = content_dir(&app)?;
    let paths = tauri::async_runtime::spawn_blocking(move || content::save(content, &dir, &content::stamp()))
        .await
        .map_err(|e| e.to_string())??;
    let paths: Vec<String> = paths.into_iter().map(|p| p.to_string_lossy().into_owned()).collect();
    add(&app, id, paths.clone());
    Ok(paths)
}

/// Text dropped from another app becomes a .txt (a lone link, a .url).
#[tauri::command]
pub async fn shelf_add_text(app: AppHandle, id: String, text: String) -> Result<Vec<String>, String> {
    add_content(app, Some(id), Content::Text(text)).await
}

/// A link dropped from a browser becomes an Internet Shortcut (.url).
#[tauri::command]
pub async fn shelf_add_url(app: AppHandle, id: String, url: String, title: Option<String>) -> Result<Vec<String>, String> {
    add_content(app, Some(id), Content::Url { url, title }).await
}

/// File data with no path on disk, like an image dragged out of a browser or
/// an Outlook attachment (raw request body; the shelf id and a URL-encoded
/// file name come as `x-shelf-id` / `x-name` headers). Images not in a common
/// format become PNGs; anything else keeps its name.
#[tauri::command]
pub async fn shelf_add_bytes(app: AppHandle, request: tauri::ipc::Request<'_>) -> Result<Vec<String>, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("Expected image data".into());
    };
    let header = |k: &str| request.headers().get(k).and_then(|v| v.to_str().ok()).map(percent_decode);
    let id = header("x-shelf-id");
    let name = header("x-name").filter(|n| !n.is_empty());
    add_content(app, id, Content::Image { bytes: bytes.clone(), name }).await
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (b[i], b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(h), Some(l)) => {
                out.push((h * 16 + l) as u8);
                i += 3;
            }
            (c, _, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Puts the clipboard on a shelf: images become PNGs, text a .txt, copied
/// files are added as they are. `id` None: the last-used shelf (or a new one).
#[tauri::command]
pub async fn shelf_paste(app: AppHandle, id: Option<String>) -> Result<Vec<String>, String> {
    let content = tauri::async_runtime::spawn_blocking(read_clipboard).await.map_err(|e| e.to_string())??;
    add_content(app, id, content).await
}

#[cfg(windows)]
fn read_clipboard() -> Result<Content, String> {
    content::read_clipboard()
}
#[cfg(not(windows))]
fn read_clipboard() -> Result<Content, String> {
    Err("Pasting needs Windows".into())
}

/// Tray item: paste onto the last-used shelf, telling you if nothing could be pasted.
pub fn paste_from_tray(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = shelf_paste(app.clone(), None).await {
            use tauri_plugin_notification::NotificationExt;
            let _ = app.notification().builder().title("Nothing to paste").body(e).show();
        }
    });
}

/// Shelves don't take focus (so drags in never steal it); clicking one lets
/// it, so Ctrl+V works. It gives focus up again when it loses it.
#[tauri::command]
pub fn shelf_focus(app: AppHandle, id: String, focus: bool) {
    if let Some(w) = app.get_webview_window(&label(&id)) {
        crate::drag::set_focusable(&w, focus);
        if focus {
            let _ = w.set_focus();
        }
    }
}

/// Saved content nobody's shelf holds any more, kept a day in case an app
/// you dragged it into still reads it.
fn clean_content(app: &AppHandle, shelves: &[ShelfData]) {
    let Ok(dir) = content_dir(app) else { return };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let day = std::time::Duration::from_secs(24 * 3600);
    for entry in entries.flatten() {
        let path = entry.path();
        let held = shelves.iter().any(|s| s.items.iter().any(|i| Path::new(i) == path));
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > day);
        if !held && old {
            let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        }
    }
}

/// Removes items. With `dragged_out`, an unpinned shelf that ends up empty closes.
#[tauri::command]
pub fn shelf_remove(app: AppHandle, shelves: State<Shelves>, id: String, paths: Vec<String>, dragged_out: bool) {
    let mut list = shelves.shelves.lock().unwrap();
    let Some(shelf) = list.iter_mut().find(|s| s.id == id) else { return };
    shelf.items.retain(|p| !paths.contains(p));
    let close = dragged_out && shelf.items.is_empty() && !shelf.pinned;
    if close {
        list.retain(|s| s.id != id);
        save(&app, &list);
        let total: usize = list.iter().map(|s| s.items.len()).sum();
        let _ = app.emit(COUNT_EVENT, total);
        if let Some(w) = app.get_webview_window(&label(&id)) {
            let _ = w.close();
        }
    } else {
        changed(&app, &list, &id);
    }
}

/// Swaps old paths for new ones (after Move to…), keeping their order.
#[tauri::command]
pub fn shelf_replace(app: AppHandle, shelves: State<Shelves>, id: String, old: Vec<String>, new: Vec<String>) {
    let mut list = shelves.shelves.lock().unwrap();
    let Some(shelf) = list.iter_mut().find(|s| s.id == id) else { return };
    for (o, n) in old.iter().zip(new) {
        if let Some(slot) = shelf.items.iter_mut().find(|p| *p == o) {
            *slot = n;
        }
    }
    changed(&app, &list, &id);
}

#[tauri::command]
pub fn shelf_set_pinned(app: AppHandle, shelves: State<Shelves>, id: String, pinned: bool) {
    let mut list = shelves.shelves.lock().unwrap();
    if let Some(shelf) = list.iter_mut().find(|s| s.id == id) {
        shelf.pinned = pinned;
    }
    changed(&app, &list, &id);
}

/// Closes a shelf and forgets its items (the files themselves are untouched).
#[tauri::command]
pub fn shelf_close(app: AppHandle, shelves: State<Shelves>, id: String) {
    let mut list = shelves.shelves.lock().unwrap();
    list.retain(|s| s.id != id);
    save(&app, &list);
    let total: usize = list.iter().map(|s| s.items.len()).sum();
    let _ = app.emit(COUNT_EVENT, total);
    let mut last = shelves.last_used.lock().unwrap();
    if last.as_deref() == Some(id.as_str()) {
        *last = None;
    }
    drop(last);
    if let Some(w) = app.get_webview_window(&label(&id)) {
        let _ = w.close();
    }
}

#[tauri::command]
pub fn shelf_hide(app: AppHandle, id: String) {
    if let Some(w) = app.get_webview_window(&label(&id)) {
        let _ = w.hide();
    }
}

#[tauri::command]
pub fn shelves_count(shelves: State<Shelves>) -> usize {
    shelves.shelves.lock().unwrap().iter().map(|s| s.items.len()).sum()
}

/// Shows every saved shelf, or opens a new one if there are none.
pub fn show_all(app: &AppHandle) {
    let ids: Vec<String> = app.state::<Shelves>().shelves.lock().unwrap().iter().map(|s| s.id.clone()).collect();
    if ids.is_empty() {
        new_shelf(app);
    }
    for id in ids {
        open_window(app, &id, None);
    }
}

pub fn new_shelf(app: &AppHandle) {
    let shelves = app.state::<Shelves>();
    let id = new_id();
    {
        let mut list = shelves.shelves.lock().unwrap();
        list.push(ShelfData { id: id.clone(), items: Vec::new(), pinned: true });
        changed(app, &list, &id);
    }
    *shelves.last_used.lock().unwrap() = Some(id.clone());
    open_window(app, &id, None);
}

/// Drawer button: hide all shelves if any are showing, else show them.
#[tauri::command]
pub async fn toggle_shelves(app: AppHandle) {
    let visible: Vec<WebviewWindow> = app
        .webview_windows()
        .into_iter()
        .filter(|(l, w)| l.starts_with("shelf-") && w.is_visible().unwrap_or(false))
        .map(|(_, w)| w)
        .collect();
    if visible.is_empty() {
        show_all(&app);
    } else {
        for w in visible {
            let _ = w.hide();
        }
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
