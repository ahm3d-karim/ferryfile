mod drag;
mod dropbridge;
mod engine;
mod recipes;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};

#[tauri::command]
fn list_actions(path: String) -> Vec<engine::Action> {
    engine::list_actions(std::path::Path::new(&path))
}

#[derive(Clone, serde::Serialize)]
struct ProgressPayload {
    path: String,
    fraction: f32,
}

/// Sends progress for the file at `path` to the drawer.
pub(crate) fn emit_progress(app: &tauri::AppHandle, path: &str, fraction: f32) {
    let _ = app.emit_to(
        "drawer",
        "skiff-progress",
        ProgressPayload { path: path.to_string(), fraction },
    );
}

#[tauri::command]
async fn run_action(app: tauri::AppHandle, action_id: String, path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let report = |f: f32| emit_progress(&app, &path, f);
        engine::run_action(&action_id, std::path::Path::new(&path), &report)
            .map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn combine_actions(paths: Vec<String>) -> Vec<engine::Action> {
    let paths: Vec<_> = paths.into_iter().map(std::path::PathBuf::from).collect();
    engine::list_combine_actions(&paths)
}

#[tauri::command]
async fn run_combine(app: tauri::AppHandle, action_id: String, paths: Vec<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let first = paths.first().cloned().unwrap_or_default();
        let report = |f: f32| emit_progress(&app, &first, f);
        let inputs: Vec<_> = paths.iter().map(std::path::PathBuf::from).collect();
        engine::run_combine(&action_id, &inputs, &report).map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Keep the drawer open (cancels the pending auto-hide after a drag ends).
#[tauri::command]
fn pin_drawer() {
    drag::pin();
}

/// Let the drawer take keyboard focus (for typing in search or the editor).
#[tauri::command]
fn focus_drawer(app: tauri::AppHandle) {
    drag::pin();
    if let Some(w) = app.get_webview_window("drawer") {
        drag::set_focusable(&w, true);
        let _ = w.set_focus();
    }
}

#[tauri::command]
fn hide_drawer(app: tauri::AppHandle) {
    drag::hide_now(&app);
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            list_actions,
            run_action,
            combine_actions,
            run_combine,
            pin_drawer,
            focus_drawer,
            hide_drawer,
            recipes::recipes_for,
            recipes::next_actions,
            recipes::save_recipe,
            recipes::delete_recipe,
            recipes::run_recipe,
        ])
        .setup(|app| {
            let quit = MenuItem::with_id(app, "quit", "Quit Skiff", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Skiff")
                .menu(&menu)
                .on_menu_event(|app, ev| {
                    if ev.id.as_ref() == "quit" {
                        app.exit(0);
                    }
                })
                .build(app)?;

            let drawer = app.get_webview_window("drawer").expect("drawer window");
            dropbridge::install(&drawer);
            drag::set_focusable(&drawer, false);
            drag::start(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Skiff");
}
