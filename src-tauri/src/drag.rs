//! Global drag detection (Windows low-level mouse and keyboard hooks): shows
//! the drawer when a drag starts in Explorer (or when the drag hotkey is
//! pressed mid-drag), and opens a shelf at the cursor when the user shakes it
//! mid-drag.
//!
//! The hook only decides *when* to show things. Receiving the dropped files is
//! done by the page through WebView2 (see dropbridge.rs).

use std::sync::{
    atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicU8, Ordering},
    mpsc::{channel, Sender},
    Mutex, OnceLock,
};
use std::time::Duration;

use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize};

use crate::settings::{OpenOn, Side};
use crate::shake::{Sensitivity, ShakeDetector};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetClassNameW, GetForegroundWindow, GetMessageW,
    GetWindowLongPtrW, SetWindowLongPtrW, SetWindowsHookExW, TranslateMessage, GWL_EXSTYLE,
    KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_SYSKEYDOWN, WM_SYSKEYUP, WS_EX_NOACTIVATE,
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
    /// The cursor was shaken mid-drag at this screen position.
    Shake(i32, i32),
    /// The drag hotkey was pressed mid-drag.
    DragKey,
}

static TX: OnceLock<Mutex<Sender<Ev>>> = OnceLock::new();
static DOWN: AtomicBool = AtomicBool::new(false);
static DRAGGING: AtomicBool = AtomicBool::new(false);
static START_IN_SHELL: AtomicBool = AtomicBool::new(false);
static START_X: AtomicI32 = AtomicI32::new(0);
static START_Y: AtomicI32 = AtomicI32::new(0);
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The left button is down and has moved past the threshold, in any app.
/// `DRAGGING` is the narrower "a drag that started in Explorer".
static MOVED: AtomicBool = AtomicBool::new(false);

/// Drag hotkey: virtual-key code (0 = off) and modifier bits (`MOD_*`).
static DRAG_KEY_VK: AtomicU32 = AtomicU32::new(0);
static DRAG_KEY_MODS: AtomicU8 = AtomicU8::new(0);
/// The drag hotkey already fired during this drag (ignores key repeat).
static KEY_FIRED: AtomicBool = AtomicBool::new(false);
/// The drag hotkey opened the drawer, so hide it again when this drag ends.
static KEY_OPENED: AtomicBool = AtomicBool::new(false);
/// The key-up of a swallowed key is swallowed too, so no app sees half a press.
static SWALLOWED_VK: AtomicU32 = AtomicU32::new(0);

const MOD_CTRL: u8 = 1;
const MOD_ALT: u8 = 2;
const MOD_SHIFT: u8 = 4;
const MOD_WIN: u8 = 8;

/// Sets the drag hotkey, e.g. "Space" or "Ctrl+D". `None` turns it off.
pub fn configure_drag_key(combo: Option<&str>) -> Result<(), String> {
    let (mods, vk) = match combo.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => parse_combo(c)?,
        None => (0, 0),
    };
    DRAG_KEY_MODS.store(mods, Ordering::Relaxed);
    DRAG_KEY_VK.store(vk, Ordering::Relaxed);
    Ok(())
}

/// Parses "Ctrl+Alt+Space"-style combos (the format Settings writes) into
/// modifier bits and a Windows virtual-key code.
pub fn parse_combo(combo: &str) -> Result<(u8, u32), String> {
    let bad = || format!("\"{combo}\" isn't a key combo Ferryfile understands");
    let mut mods = 0u8;
    let mut vk = None;
    for part in combo.split('+').map(str::trim) {
        match part.to_ascii_uppercase().as_str() {
            "CTRL" | "CONTROL" => mods |= MOD_CTRL,
            "ALT" => mods |= MOD_ALT,
            "SHIFT" => mods |= MOD_SHIFT,
            "WIN" | "SUPER" | "META" => mods |= MOD_WIN,
            key if vk.is_none() => vk = Some(key_vk(key).ok_or_else(bad)?),
            _ => return Err(bad()),
        }
    }
    Ok((mods, vk.ok_or_else(bad)?))
}

fn key_vk(key: &str) -> Option<u32> {
    let b = key.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_uppercase() || b[0].is_ascii_digit()) {
        return Some(b[0] as u32); // VK codes for A-Z and 0-9 are their ASCII codes
    }
    if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then_some(0x70 + n - 1);
    }
    if let Some(n) = key.strip_prefix("NUMPAD").and_then(|n| n.parse::<u32>().ok()) {
        return (n <= 9).then_some(0x60 + n);
    }
    Some(match key {
        "SPACE" => 0x20,
        "TAB" => 0x09,
        "ENTER" => 0x0D,
        "BACKSPACE" => 0x08,
        "INSERT" => 0x2D,
        "DELETE" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "ARROWLEFT" => 0x25,
        "ARROWUP" => 0x26,
        "ARROWRIGHT" => 0x27,
        "ARROWDOWN" => 0x28,
        "BACKQUOTE" => 0xC0,
        "MINUS" => 0xBD,
        "EQUAL" => 0xBB,
        "BRACKETLEFT" => 0xDB,
        "BRACKETRIGHT" => 0xDD,
        "BACKSLASH" => 0xDC,
        "SEMICOLON" => 0xBA,
        "QUOTE" => 0xDE,
        "COMMA" => 0xBC,
        "PERIOD" => 0xBE,
        "SLASH" => 0xBF,
        _ => return None,
    })
}

/// 0 = shake disabled, else 1 + Sensitivity as u8. Read on every mouse move,
/// so it's an atomic rather than the settings lock.
static SHAKE_MODE: AtomicU8 = AtomicU8::new(2);
/// At most one shake-to-shelf per drag.
static SHOOK: AtomicBool = AtomicBool::new(false);
static SHAKE: Mutex<Option<ShakeDetector>> = Mutex::new(None);

pub fn configure_shake(enabled: bool, sensitivity: Sensitivity) {
    SHAKE_MODE.store(if enabled { 1 + sensitivity as u8 } else { 0 }, Ordering::Relaxed);
}

fn shake_sensitivity() -> Option<Sensitivity> {
    match SHAKE_MODE.load(Ordering::Relaxed) {
        1 => Some(Sensitivity::Gentle),
        2 => Some(Sensitivity::Normal),
        3 => Some(Sensitivity::Firm),
        _ => None,
    }
}

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
    let _ = w.emit("ferryfile-drag-end", ());
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
                Ev::Shake(x, y) => shaken(&app, x, y),
                Ev::DragKey => drag_key_pressed(&app),
            }
        }
    });

    // Hook thread: owns the hook and pumps messages.
    std::thread::spawn(|| unsafe {
        let hmod = GetModuleHandleW(None).ok().map(|m| m.into());
        let _hook = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0)
            .expect("failed to install mouse hook");
        let _kb_hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0)
            .expect("failed to install keyboard hook");
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
                MOVED.store(false, Ordering::Relaxed);
            }
            WM_MOUSEMOVE => {
                if DRAGGING.load(Ordering::Relaxed) {
                    track_shake(x, y, info.time);
                } else if DOWN.load(Ordering::Relaxed) && !MOVED.load(Ordering::Relaxed) {
                    let dx = x - START_X.load(Ordering::Relaxed);
                    let dy = y - START_Y.load(Ordering::Relaxed);
                    if dx.abs() >= DRAG_THRESHOLD_PX || dy.abs() >= DRAG_THRESHOLD_PX {
                        MOVED.store(true, Ordering::Relaxed);
                        KEY_FIRED.store(false, Ordering::Relaxed);
                    }
                    if MOVED.load(Ordering::Relaxed) && START_IN_SHELL.load(Ordering::Relaxed) {
                        DRAGGING.store(true, Ordering::Relaxed);
                        SHOOK.store(false, Ordering::Relaxed);
                        if let (Some(sens), Ok(mut d)) = (shake_sensitivity(), SHAKE.lock()) {
                            let mut detector = ShakeDetector::new(sens);
                            detector.reset(x);
                            *d = Some(detector);
                        }
                        send(Ev::DragStart);
                    }
                }
            }
            WM_LBUTTONUP => {
                DOWN.store(false, Ordering::Relaxed);
                let moved = MOVED.swap(false, Ordering::Relaxed);
                let shell_drag = DRAGGING.swap(false, Ordering::Relaxed);
                let key_opened = KEY_OPENED.swap(false, Ordering::Relaxed);
                if shell_drag || (moved && key_opened) {
                    send(Ev::DragEnd);
                }
            }
            _ => {}
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

fn held(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000 != 0 }
}

fn held_mods() -> u8 {
    let mut m = 0;
    if held(VK_CONTROL) {
        m |= MOD_CTRL;
    }
    if held(VK_MENU) {
        m |= MOD_ALT;
    }
    if held(VK_SHIFT) {
        m |= MOD_SHIFT;
    }
    if held(VK_LWIN) || held(VK_RWIN) {
        m |= MOD_WIN;
    }
    m
}

/// Watches for the drag hotkey. It only acts while a drag is in progress;
/// every other key press passes straight through.
unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        match wparam.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = DRAG_KEY_VK.load(Ordering::Relaxed);
                if vk != 0
                    && info.vkCode == vk
                    && DOWN.load(Ordering::Relaxed)
                    && MOVED.load(Ordering::Relaxed)
                    && held_mods() == DRAG_KEY_MODS.load(Ordering::Relaxed)
                {
                    if !KEY_FIRED.swap(true, Ordering::Relaxed) {
                        send(Ev::DragKey);
                    }
                    SWALLOWED_VK.store(vk, Ordering::Relaxed);
                    return LRESULT(1);
                }
            }
            WM_KEYUP | WM_SYSKEYUP => {
                if info.vkCode != 0 && SWALLOWED_VK.load(Ordering::Relaxed) == info.vkCode {
                    SWALLOWED_VK.store(0, Ordering::Relaxed);
                    return LRESULT(1);
                }
            }
            _ => {}
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

/// Feeds the shake detector during a drag (hook thread: keep it cheap).
fn track_shake(x: i32, y: i32, t_ms: u32) {
    if SHOOK.load(Ordering::Relaxed) || shake_sensitivity().is_none() {
        return;
    }
    if let Ok(mut d) = SHAKE.try_lock() {
        if d.as_mut().is_some_and(|d| d.feed(x, t_ms)) {
            SHOOK.store(true, Ordering::Relaxed);
            send(Ev::Shake(x, y));
        }
    }
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
    let _ = w.emit("ferryfile-drag-start", side);
}

/// Shows the drawer after the configured delay, unless the drag ended first,
/// a shake opened a shelf instead, or a shelf is already open (then the shelf
/// is the drop target and the drawer would just get in the way).
fn drag_started(app: &AppHandle) {
    let settings = crate::settings::current();
    if settings.open_on == OpenOn::Hotkey || crate::shelf::any_visible(app) {
        return;
    }
    let delay = settings.open_delay_ms;
    if delay > 0 {
        std::thread::sleep(Duration::from_millis(delay.into()));
        if !DRAGGING.load(Ordering::Relaxed) || SHOOK.load(Ordering::Relaxed) {
            return;
        }
    }
    show_drawer(app);
}

/// The drag hotkey opens the drawer as a drop target for the current drag.
fn drag_key_pressed(app: &AppHandle) {
    let Some(w) = app.get_webview_window("drawer") else {
        return;
    };
    if w.is_visible().unwrap_or(false) {
        pin(); // keep it open: cancels a hide pending from an earlier drag
        return;
    }
    KEY_OPENED.store(true, Ordering::Relaxed);
    show_drawer(app);
}

/// Opens the drawer for the drag in progress, as the drag hotkey would.
pub fn open_for_current_drag(app: &AppHandle) {
    if DOWN.load(Ordering::Relaxed) {
        drag_key_pressed(app);
    }
}

/// Opens the drawer outside a drag, for files sent from Explorer's right-click
/// menu or Send to. It stays open until closed and takes focus so keys work.
pub fn show_opened(app: &AppHandle) {
    show_drawer(app);
    if let Some(w) = app.get_webview_window("drawer") {
        set_focusable(&w, true);
        let _ = w.set_focus();
    }
}

/// The drawer hotkey toggles the drawer. Mid-drag it opens the drawer as a drop
/// target (and the usual hide-after-drop applies); otherwise the drawer stays
/// open until closed, and takes focus so Esc works.
pub fn hotkey_pressed(app: &AppHandle) {
    let Some(w) = app.get_webview_window("drawer") else {
        return;
    };
    let dragging = DOWN.load(Ordering::Relaxed) && MOVED.load(Ordering::Relaxed);
    if w.is_visible().unwrap_or(false) {
        if !dragging {
            hide_now(app);
        }
        return;
    }
    show_drawer(app);
    // Don't grab focus while a mouse button is held: that could be a drag
    // from some other app, which Explorer-only tracking doesn't see.
    if !dragging && !DOWN.load(Ordering::Relaxed) {
        set_focusable(&w, true);
        let _ = w.set_focus();
    }
}

/// A shake opens a shelf at the cursor and puts the drawer away.
fn shaken(app: &AppHandle, x: i32, y: i32) {
    hide_now(app);
    crate::shelf::spawn_at_cursor(app, x, y);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combos() {
        assert_eq!(parse_combo("Space"), Ok((0, 0x20)));
        assert_eq!(parse_combo("Ctrl+Alt+Space"), Ok((MOD_CTRL | MOD_ALT, 0x20)));
        assert_eq!(parse_combo("Shift+F"), Ok((MOD_SHIFT, 0x46)));
        assert_eq!(parse_combo("Super+1"), Ok((MOD_WIN, 0x31)));
        assert_eq!(parse_combo("F12"), Ok((0, 0x7B)));
        assert_eq!(parse_combo("Ctrl+Backquote"), Ok((MOD_CTRL, 0xC0)));
        assert!(parse_combo("Ctrl").is_err());
        assert!(parse_combo("Ctrl+A+B").is_err());
        assert!(parse_combo("Escape").is_err());
    }
}
