//! The global hotkey that shows or hides the drawer (see drag::hotkey_pressed).

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub fn plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri_plugin_global_shortcut::Builder::new()
        .with_handler(|app, _shortcut, ev| {
            if ev.state == ShortcutState::Pressed {
                let app = app.clone();
                // Window work off the handler, like the tray menu does.
                tauri::async_runtime::spawn(async move { crate::drag::hotkey_pressed(&app) });
            }
        })
        .build()
}

/// Replaces the registered hotkey with `hotkey` (empty = none).
pub fn register(app: &AppHandle, hotkey: &str) -> Result<(), String> {
    let shortcuts = app.global_shortcut();
    let _ = shortcuts.unregister_all();
    let hotkey = hotkey.trim();
    if hotkey.is_empty() {
        return Ok(());
    }
    shortcuts
        .register(hotkey)
        .map_err(|e| format!("Couldn't use {hotkey} as the hotkey. Another app may already use it. ({e})"))
}

pub fn is_active(app: &AppHandle) -> bool {
    let hotkey = crate::settings::current().hotkey;
    !hotkey.trim().is_empty() && app.global_shortcut().is_registered(hotkey.trim())
}

/// Settings turns the hotkey off while recording a new one, so pressing the
/// current combo reaches the page instead of toggling the drawer.
#[tauri::command]
pub fn pause_hotkey(app: AppHandle, paused: bool) -> Result<(), String> {
    if paused {
        let _ = app.global_shortcut().unregister_all();
        Ok(())
    } else {
        register(&app, &crate::settings::current().hotkey)
    }
}
