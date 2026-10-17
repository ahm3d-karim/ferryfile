mod drag;
mod dropbridge;
mod engine;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};

#[tauri::command]
fn list_actions(path: String) -> Vec<engine::Action> {
    engine::list_actions(std::path::Path::new(&path))
}

#[tauri::command]
async fn run_action(action_id: String, path: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        engine::run_action(&action_id, std::path::Path::new(&path))
            .map(|p| p.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Keep the drawer open (cancels the pending auto-hide after a drag ends).
#[tauri::command]
fn pin_drawer() {
    drag::pin();
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
            pin_drawer,
            hide_drawer
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
            drag::prepare_window(&drawer);
            drag::start(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Skiff");
}
