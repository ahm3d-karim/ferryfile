//! First-run walkthrough: a small window with a sample file to drag, teaching
//! the drag hotkey. Shown once (a marker file remembers it), and again from
//! the tray's "Show tutorial".

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

const LABEL: &str = "welcome";
const SEEN_MARKER: &str = "tutorial-seen";
const SAMPLE_NAME: &str = "Ferryfile sample.png";
/// The app icon makes a fine sample picture.
const SAMPLE_PNG: &[u8] = include_bytes!("../icons/128x128@2x.png");

fn marker(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join(SEEN_MARKER))
}

/// On the very first launch, opens the walkthrough.
pub fn show_on_first_run(app: &AppHandle) {
    if marker(app).is_some_and(|m| !m.exists()) {
        show(app);
    }
}

pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("welcome.html".into()))
        .title("Welcome to Ferryfile")
        .inner_size(420.0, 470.0)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .center()
        .disable_drag_drop_handler()
        .build();
    let Ok(w) = built else { return };
    // However it closes (Skip, Done, the X), don't show it on launch again.
    let app = app.clone();
    w.on_window_event(move |ev| {
        if let tauri::WindowEvent::Destroyed = ev {
            if let Some(m) = marker(&app) {
                if let Some(dir) = m.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(m, "");
            }
        }
    });
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TutorialInfo {
    /// The sample file to drag.
    sample: String,
    /// The key to press mid-drag, or `None` when the drawer opens on any drag.
    drag_hotkey: Option<String>,
    /// The screen edge the drawer slides in from.
    side: crate::settings::Side,
}

#[tauri::command]
pub fn tutorial_info(app: AppHandle) -> Result<TutorialInfo, String> {
    let dir = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("tutorial");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let sample = dir.join(SAMPLE_NAME);
    if !sample.exists() {
        std::fs::write(&sample, SAMPLE_PNG).map_err(|e| e.to_string())?;
    }
    let settings = crate::settings::current();
    let key = settings.drag_hotkey.trim();
    let drag_hotkey = (settings.open_on == crate::settings::OpenOn::Hotkey && !key.is_empty()).then(|| key.to_string());
    Ok(TutorialInfo { sample: sample.to_string_lossy().into_owned(), drag_hotkey, side: settings.drawer_side })
}

/// With "Opens when: Any drag", only drags from Explorer open the drawer, so
/// the tutorial opens it for the sample drag itself.
#[tauri::command]
pub fn tutorial_drag_started(app: AppHandle) {
    if crate::settings::current().open_on == crate::settings::OpenOn::Drag {
        crate::drag::open_for_current_drag(&app);
    }
}

#[tauri::command]
pub fn close_tutorial(app: AppHandle) {
    if let Some(w) = app.get_webview_window(LABEL) {
        let _ = w.close();
    }
}
