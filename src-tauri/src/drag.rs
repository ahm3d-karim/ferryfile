//! Global drag detection (Windows low-level mouse hook) and drawer show/hide.
//!
//! The hook only decides *when* to show the drawer. Receiving the dropped
//! files is handled by Tauri's built-in drag-drop support, which registers the
//! window as an OLE drop target.

use std::sync::{
    atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
    mpsc::{channel, Sender},
    Mutex, OnceLock,
};
use std::time::Duration;

use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize};

use crate::settings::Side;

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetClassNameW, GetForegroundWindow, GetMessageW,
    GetWindowLongPtrW, SetWindowLongPtrW, SetWindowsHookExW, TranslateMessage, GWL_EXSTYLE, MSG,
    MSLLHOOKSTRUCT, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW,
};

const DRAG_THRESHOLD_PX: i32 = 12;
pub(crate) const DRAWER_WIDTH_LOGICAL: f64 = 340.0;
/// Only react to drags that start in the shell (Explorer / desktop).
const SHELL_CLASSES: &[&str] = &["CabinetWClass", "WorkerW", "Progman", "SHELLDLL_DefView"];

#[derive(Debug, Clone, Copy)]
enum Ev {
    DragStart,
    DragEnd,
}

static TX: OnceLock<Mutex<Sender<Ev>>> = OnceLock::new();
static DOWN: AtomicBool = AtomicBool::new(false);
static DRAGGING: AtomicBool = AtomicBool::new(false);
static START_IN_SHELL: AtomicBool = AtomicBool::new(false);
static START_X: AtomicI32 = AtomicI32::new(0);
static START_Y: AtomicI32 = AtomicI32::new(0);
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// By default the drawer never takes activation, so dragging from Explorer
/// isn't disturbed. It becomes focusable only while the user types in it.
pub fn set_focusable(w: &tauri::WebviewWindow, focusable: bool) {
    if let Ok(hwnd) = w.hwnd() {
        unsafe {
            let mut ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) | WS_EX_TOOLWINDOW.0 as isize;
            if focusable {
                ex &= !(WS_EX_NOACTIVATE.0 as isize);
            } else {
                ex |= WS_EX_NOACTIVATE.0 as isize;
            }
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);
        }
    }
}

fn hide(w: &tauri::WebviewWindow) {
    let _ = w.emit("skiff-drag-end", ());
    let _ = w.hide();
    set_focusable(w, false);
}

pub fn start(app: AppHandle) {
    let (tx, rx) = channel::<Ev>();
    let _ = TX.set(Mutex::new(tx));

    // Worker: reacts to hook events off the hook thread (hook callbacks must be fast).
    std::thread::spawn(move || {
        while let Ok(ev) = rx.recv() {
            match ev {
                Ev::DragStart => drag_started(&app),
                Ev::DragEnd => schedule_hide(&app),
            }
        }
    });

    // Hook thread: owns the hook and pumps messages.
    std::thread::spawn(|| unsafe {
        let hmod = GetModuleHandleW(None).ok().map(|m| m.into());
        let _hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0)
            .expect("failed to install mouse hook");
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}

fn send(ev: Ev) {
    if let Some(tx) = TX.get() {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(ev);
        }
    }
}

unsafe fn foreground_in_shell() -> bool {
    let hwnd = GetForegroundWindow();
    let mut buf = [0u16; 64];
    let n = GetClassNameW(hwnd, &mut buf).max(0) as usize;
    let class = String::from_utf16_lossy(&buf[..n]);
    SHELL_CLASSES.contains(&class.as_str())
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        let (x, y) = (info.pt.x, info.pt.y);
        match wparam.0 as u32 {
            WM_LBUTTONDOWN => {
                DOWN.store(true, Ordering::Relaxed);
                START_X.store(x, Ordering::Relaxed);
                START_Y.store(y, Ordering::Relaxed);
                START_IN_SHELL.store(foreground_in_shell(), Ordering::Relaxed);
            }
            WM_MOUSEMOVE => {
                if DOWN.load(Ordering::Relaxed)
                    && START_IN_SHELL.load(Ordering::Relaxed)
                    && !DRAGGING.load(Ordering::Relaxed)
                {
                    let dx = x - START_X.load(Ordering::Relaxed);
                    let dy = y - START_Y.load(Ordering::Relaxed);
                    if dx.abs() >= DRAG_THRESHOLD_PX || dy.abs() >= DRAG_THRESHOLD_PX {
                        DRAGGING.store(true, Ordering::Relaxed);
                        send(Ev::DragStart);
                    }
                }
            }
            WM_LBUTTONUP => {
                DOWN.store(false, Ordering::Relaxed);
                if DRAGGING.swap(false, Ordering::Relaxed) {
                    send(Ev::DragEnd);
                }
            }
            _ => {}
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

fn show_drawer(app: &AppHandle) {
    GENERATION.fetch_add(1, Ordering::SeqCst); // cancels any pending hide
    let Some(w) = app.get_webview_window("drawer") else {
        return;
    };

    // Dock to the configured edge of the monitor under the cursor (falls back to primary).
    let side = crate::settings::current().drawer_side;
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    if let Some(m) = monitor {
        let width = (DRAWER_WIDTH_LOGICAL * m.scale_factor()) as u32;
        let size = m.size();
        let pos = m.position();
        let x = match side {
            Side::Right => pos.x + size.width as i32 - width as i32,
            Side::Left => pos.x,
        };
        let _ = w.set_size(PhysicalSize::new(width, size.height));
        let _ = w.set_position(PhysicalPosition::new(x, pos.y));
    } else {
        let _ = w.set_size(LogicalSize::new(DRAWER_WIDTH_LOGICAL, 800.0));
    }
    let _ = w.show();
    let _ = w.emit("skiff-drag-start", side);
}

/// Shows the drawer after the configured delay, unless the drag ended first.
fn drag_started(app: &AppHandle) {
    let delay = crate::settings::current().open_delay_ms;
    if delay > 0 {
        std::thread::sleep(Duration::from_millis(delay.into()));
        if !DRAGGING.load(Ordering::Relaxed) {
            return;
        }
    }
    show_drawer(app);
}

pub fn pin() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

pub fn hide_now(app: &AppHandle) {
    pin();
    if let Some(w) = app.get_webview_window("drawer") {
        hide(&w);
    }
}

fn schedule_hide(app: &AppHandle) {
    let gen = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        // Grace period so the drop event can be delivered and displayed.
        std::thread::sleep(Duration::from_millis(1800));
        if GENERATION.load(Ordering::SeqCst) == gen {
            if let Some(w) = app.get_webview_window("drawer") {
                hide(&w);
            }
        }
    });
}
