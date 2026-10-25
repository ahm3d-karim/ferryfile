pub mod cli;
mod drag;
mod dropbridge;
mod engine;
mod files;
mod hotkey;
mod recipes;
mod settings;
mod shake;
mod shelf;
mod watch;

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

/// Sends progress for the file at `path` to the window that started the job.
pub(crate) fn emit_progress(app: &tauri::AppHandle, window: &str, path: &str, fraction: f32) {
    let _ = app.emit_to(
        window,
        "ferryfile-progress",
        ProgressPayload { path: path.to_string(), fraction },
    );
}

#[tauri::command]
async fn run_action(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    action_id: String,
    path: String,
    job_id: Option<String>,
) -> Result<String, String> {
    let label = window.label().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let _job = engine::jobs::begin(job_id.as_deref());
        let report = |f: f32| emit_progress(&app, &label, &path, f);
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
async fn run_combine(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    action_id: String,
    paths: Vec<String>,
    job_id: Option<String>,
) -> Result<String, String> {
    let label = window.label().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let _job = engine::jobs::begin(job_id.as_deref());
        let first = paths.first().cloned().unwrap_or_default();
        let report = |f: f32| emit_progress(&app, &label, &first, f);
        let inputs: Vec<_> = paths.iter().map(std::path::PathBuf::from).collect();
        engine::run_combine(&action_id, &inputs, &report).map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Stops a running job started with this `job_id`.
#[tauri::command]
fn cancel_job(job_id: String) -> bool {
    engine::jobs::cancel(&job_id)
}

/// A Windows notification, for jobs that finish while you're elsewhere.
#[tauri::command]
fn notify(app: tauri::AppHandle, title: String, body: String) {
    use tauri_plugin_notification::NotificationExt;
    let _ = app.notification().builder().title(title).body(body).show();
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
        // Must come first: a second launch just opens settings in the running app.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            settings::open_settings(app.clone());
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_drag::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(hotkey::plugin())
        .invoke_handler(tauri::generate_handler![
            settings::get_settings,
            settings::set_settings,
            settings::pick_output_dir,
            settings::open_settings,
            hotkey::pause_hotkey,
            files::copy_paths,
            files::copy_files,
            files::reveal,
            files::open_terminal,
            files::send_to_folder,
            files::zip_items,
            files::file_info,
            shelf::shelf_state,
            shelf::shelf_add,
            shelf::shelf_remove,
            shelf::shelf_replace,
            shelf::shelf_set_pinned,
            shelf::shelf_close,
            shelf::shelf_hide,
            shelf::shelves_count,
            shelf::toggle_shelves,
            shelf::drag_icon,
            list_actions,
            run_action,
            combine_actions,
            run_combine,
            cancel_job,
            notify,
            pin_drawer,
            focus_drawer,
            hide_drawer,
            recipes::recipes_for,
            recipes::next_actions,
            recipes::save_recipe,
            recipes::delete_recipe,
            recipes::run_recipe,
            watch::watch_errors,
            watch::watch_choices,
            watch::pick_watch_folder,
        ])
        .setup(|app| {
            settings::load(app.handle());
            app.manage(shelf::load(app.handle()));

            let show_shelves = MenuItem::with_id(app, "shelves", "Show shelves", true, None::<&str>)?;
            let new_shelf = MenuItem::with_id(app, "new-shelf", "New shelf", true, None::<&str>)?;
            let open_settings = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Ferryfile", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_shelves, &new_shelf, &open_settings, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Ferryfile")
                .menu(&menu)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    // Creating windows in a menu handler deadlocks on Windows; hop threads.
                    "shelves" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move { shelf::show_all(&app) });
                    }
                    "new-shelf" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move { shelf::new_shelf(&app) });
                    }
                    "settings" => settings::open_settings(app.clone()),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            // Closing settings just hides it; the app lives in the tray.
            if let Some(w) = app.get_webview_window("settings") {
                let hide = w.clone();
                w.on_window_event(move |ev| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = ev {
                        api.prevent_close();
                        let _ = hide.hide();
                    }
                });
            }

            let drawer = app.get_webview_window("drawer").expect("drawer window");
            dropbridge::install(&drawer);
            drag::set_focusable(&drawer, false);
            drag::start(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Ferryfile");
}
