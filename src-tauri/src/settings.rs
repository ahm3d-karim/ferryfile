//! User settings, stored as JSON in the app config folder. Kept in a global so
//! the mouse-hook thread and the engines can read them without an AppHandle.

use std::path::PathBuf;
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;

use crate::shake::Sensitivity;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    Right,
    Left,
}

/// What makes the drawer slide in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OpenOn {
    /// Only when the user presses the drag hotkey mid-drag (or the drawer
    /// hotkey at any time).
    #[default]
    Hotkey,
    /// As soon as a file drag starts in Explorer.
    Drag,
}

/// Default drawer hotkey (works any time, so it needs modifiers).
pub const DEFAULT_HOTKEY: &str = "Ctrl+Alt+Space";
/// Default drag hotkey. Only caught mid-drag, so a single key is safe.
pub const DEFAULT_DRAG_HOTKEY: &str = "Space";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub open_on: OpenOn,
    /// Pressed while dragging a file, opens the drawer. Ignored (and passed
    /// through) when not dragging. Empty = none.
    pub drag_hotkey: String,
    /// Global shortcut that shows or hides the drawer any time. Empty = none.
    pub hotkey: String,
    /// Screen edge the drawer slides in from.
    pub drawer_side: Side,
    /// Wait this long into a drag before showing the drawer, so quick drags
    /// inside Explorer don't flash it. 0 = immediately.
    pub open_delay_ms: u32,
    /// Where converted files go; `None` = next to the original.
    pub output_dir: Option<String>,
    /// Shaking the cursor mid-drag opens a new shelf at the cursor.
    pub shake_to_shelf: bool,
    pub shake_sensitivity: Sensitivity,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            open_on: OpenOn::Hotkey,
            drag_hotkey: DEFAULT_DRAG_HOTKEY.into(),
            hotkey: DEFAULT_HOTKEY.into(),
            drawer_side: Side::Right,
            open_delay_ms: 0,
            output_dir: None,
            shake_to_shelf: true,
            shake_sensitivity: Sensitivity::Normal,
        }
    }
}

static SETTINGS: RwLock<Option<Settings>> = RwLock::new(None);

pub fn current() -> Settings {
    SETTINGS.read().unwrap().clone().unwrap_or_default()
}

fn apply(settings: Settings) {
    crate::engine::set_output_dir(settings.output_dir.as_deref().map(PathBuf::from));
    crate::drag::configure_shake(settings.shake_to_shelf, settings.shake_sensitivity);
    // The drag hotkey only matters when the drawer waits for it.
    let drag_key = (settings.open_on == OpenOn::Hotkey).then_some(settings.drag_hotkey.as_str());
    if crate::drag::configure_drag_key(drag_key).is_err() {
        let _ = crate::drag::configure_drag_key(Some(DEFAULT_DRAG_HOTKEY));
    }
    *SETTINGS.write().unwrap() = Some(settings);
}

fn store_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("settings.json"))
}

pub fn load(app: &AppHandle) {
    let settings: Settings = store_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // A taken hotkey isn't fatal; Settings shows it as not working.
    let _ = crate::hotkey::register(app, &settings.hotkey);
    apply(settings);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    #[serde(flatten)]
    settings: Settings,
    autostart: bool,
    /// False when the hotkey couldn't be registered (another app owns it).
    hotkey_active: bool,
    version: String,
    config_dir: String,
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> SettingsView {
    SettingsView {
        settings: current(),
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        hotkey_active: crate::hotkey::is_active(&app),
        version: app.package_info().version.to_string(),
        config_dir: app
            .path()
            .app_config_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

#[tauri::command]
pub fn set_settings(app: AppHandle, settings: Settings, autostart: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    if autostart != launcher.is_enabled().unwrap_or(false) {
        let result = if autostart { launcher.enable() } else { launcher.disable() };
        result.map_err(|e| format!("Couldn't change startup setting: {e}"))?;
    }
    if !settings.drag_hotkey.trim().is_empty() {
        crate::drag::parse_combo(&settings.drag_hotkey)?;
    }
    if settings.hotkey != current().hotkey {
        if let Err(e) = crate::hotkey::register(&app, &settings.hotkey) {
            let _ = crate::hotkey::register(&app, &current().hotkey);
            return Err(e);
        }
    }
    let path = store_path(&app).ok_or("No config folder")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, json).map_err(|e| e.to_string())?;
    apply(settings);
    Ok(())
}

#[tauri::command]
pub async fn pick_output_dir(app: AppHandle) -> Option<String> {
    let mut dialog = app.dialog().file().set_title("Save converted files to…");
    if let Some(w) = app.get_webview_window("settings") {
        dialog = dialog.set_parent(&w);
    }
    dialog
        .blocking_pick_folder()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub fn open_settings(app: AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
