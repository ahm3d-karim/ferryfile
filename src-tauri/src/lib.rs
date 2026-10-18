mod drag;
mod dropbridge;
mod engine;
mod files;
mod recipes;
mod shelf;

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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_drag::init())
        .invoke_handler(tauri::generate_handler![
            files::copy_paths,
            files::copy_files,
            files::reveal,
            files::open_terminal,
            files::send_to_folder,
            shelf::shelf_items,
            shelf::shelf_add,
            shelf::shelf_remove,
            shelf::shelf_clear,
            shelf::show_shelf,
            shelf::hide_shelf,
            shelf::toggle_shelf,
            shelf::drag_icon,
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
            app.manage(shelf::load(app.handle()));

            let show_shelf = MenuItem::with_id(app, "shelf", "Show shelf", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Skiff", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_shelf, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Skiff")
                .menu(&menu)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "shelf" => shelf::show_shelf(app.clone()),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            let drawer = app.get_webview_window("drawer").expect("drawer window");
            dropbridge::install(&drawer);
            if let Some(shelf_window) = app.get_webview_window("shelf") {
                dropbridge::install(&shelf_window);
            }
            drag::set_focusable(&drawer, false);
            drag::start(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Skiff");
}
