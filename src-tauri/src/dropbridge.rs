//! File drops via WebView2's native drag-and-drop.
//!
//! Tauri's built-in drop handler (wry) disables WebView2's external drops and
//! injects its own OLE drop target into the webview's child windows. Newer
//! WebView2 runtimes host the input window in the browser process, out of reach
//! of that injection, so every drop gets refused. Instead we let WebView2 accept
//! drops, and the page forwards the dropped `File` objects with
//! `chrome.webview.postMessageWithAdditionalObjects`, which lets us read their
//! real paths here.

use tauri::{Emitter, Manager, WebviewWindow};
use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2File, ICoreWebView2WebMessageReceivedEventArgs2,
};
use webview2_com::{take_pwstr, WebMessageReceivedEventHandler};
use windows::core::{Interface, PWSTR};

/// Message the page posts along with the dropped files.
const DROP_MESSAGE: &str = "skiff-drop";
/// Event emitted to the page with the resolved paths.
pub const DROPPED_EVENT: &str = "skiff-dropped";

pub fn install(window: &WebviewWindow) {
    let app = window.app_handle().clone();
    let _ = window.with_webview(move |wv| unsafe {
        let Ok(core) = wv.controller().CoreWebView2() else {
            eprintln!("[skiff] drop bridge: no CoreWebView2");
            return;
        };
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };

            let mut msg = PWSTR::null();
            if args.TryGetWebMessageAsString(&mut msg).is_err() {
                return Ok(()); // JSON message (e.g. Tauri IPC), not ours
            }
            if take_pwstr(msg) != DROP_MESSAGE {
                return Ok(());
            }

            let objects = args
                .cast::<ICoreWebView2WebMessageReceivedEventArgs2>()?
                .AdditionalObjects()?;
            let mut count = 0u32;
            objects.Count(&mut count)?;

            let mut paths = Vec::new();
            for i in 0..count {
                if let Ok(file) = objects.GetValueAtIndex(i)?.cast::<ICoreWebView2File>() {
                    let mut path = PWSTR::null();
                    file.Path(&mut path)?;
                    paths.push(take_pwstr(path));
                }
            }
            if !paths.is_empty() {
                let _ = app.emit_to("drawer", DROPPED_EVENT, paths);
            }
            Ok(())
        }));
        let mut token = 0i64;
        if let Err(e) = core.add_WebMessageReceived(&handler, &mut token) {
            eprintln!("[skiff] drop bridge: failed to register handler: {e}");
        }
    });
}
