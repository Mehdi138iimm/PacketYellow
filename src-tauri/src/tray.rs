//! Windows tray icon that shows the VPN state at a glance (like v2rayN):
//! blue = not connected · yellow = connecting · red = connected.
//!
//! v0.5.3:
//! - the window's close button (and Alt+F4) no longer quits: the app hides next to the clock and keeps running.
//! - right-click on the tray icon opens a custom PacketYellow popup (ui/tray.html) instead of the plain
//!   Windows menu: live VPN state, one-click connect / disconnect, quick tools and «خروج».
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::image::Image;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder, Window, WindowEvent};
use tauri_plugin_notification::NotificationExt;

const TRAY_ID: &str = "py-tray";
pub const MENU: &str = "traymenu";
/// popup size in logical px (must match ui/tray.html)
const MENU_W: f64 = 320.0;
const MENU_H: f64 = 520.0;
// 32x32 raw RGBA, made from icons/icon.png (no PNG decoder needed at runtime)
const OFF: &[u8] = include_bytes!("../icons/tray/off.rgba");
const BUSY: &[u8] = include_bytes!("../icons/tray/busy.rgba");
const ON: &[u8] = include_bytes!("../icons/tray/on.rgba");

/// when the popup last hid itself because it lost focus (ms). Clicking the tray icon while the popup is
/// open first blurs it, so without this the same click would open it again right away.
static LAST_BLUR: AtomicU64 = AtomicU64::new(0);
static HINT_SHOWN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Vpn {
    Off,
    Busy,
    On,
}

#[derive(Serialize, Clone)]
struct TrayState {
    state: &'static str,
    tip: String,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn icon(s: Vpn) -> Image<'static> {
    Image::new(
        match s {
            Vpn::Off => OFF,
            Vpn::Busy => BUSY,
            Vpn::On => ON,
        },
        32,
        32,
    )
}

pub fn show_main(app: &AppHandle) {
    hide_menu(app);
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn hide_menu(app: &AppHandle) {
    if let Some(m) = app.get_webview_window(MENU) {
        let _ = m.hide();
    }
}

/// The popup window: frameless, transparent, always on top, never in the taskbar. Created hidden once
/// at startup so it opens instantly.
fn build_menu(app: &AppHandle) -> tauri::Result<()> {
    WebviewWindowBuilder::new(app, MENU, WebviewUrl::App("tray.html".into()))
        .title("PacketYellow")
        .inner_size(MENU_W, MENU_H)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .build()?;
    Ok(())
}

/// Open the popup right above the cursor (or below it when the taskbar is at the top), kept on screen.
fn open_menu(app: &AppHandle, cursor: PhysicalPosition<f64>) {
    let Some(m) = app.get_webview_window(MENU) else { return };
    if m.is_visible().unwrap_or(false) {
        let _ = m.hide();
        return;
    }
    if now_ms().saturating_sub(LAST_BLUR.load(Ordering::Relaxed)) < 250 {
        return; // this click is the one that just closed it
    }
    let (cx, cy) = (cursor.x as i32, cursor.y as i32);
    // monitor under the cursor (falls back to the primary one)
    let mon = app
        .available_monitors()
        .unwrap_or_default()
        .into_iter()
        .find(|mo| {
            let (p, s) = (mo.position(), mo.size());
            cx >= p.x && cx < p.x + s.width as i32 && cy >= p.y && cy < p.y + s.height as i32
        })
        .or_else(|| app.primary_monitor().ok().flatten());
    let scale = mon.as_ref().map(|mo| mo.scale_factor()).unwrap_or(1.0);
    let (w, h) = ((MENU_W * scale) as i32, (MENU_H * scale) as i32);
    let gap = (8.0 * scale) as i32;
    let (mut x, mut y) = (cx - w + gap, cy - h - gap);
    if let Some(mo) = &mon {
        let (p, s) = (mo.position(), mo.size());
        let (right, bottom) = (p.x + s.width as i32, p.y + s.height as i32);
        if y < p.y {
            y = cy + gap; // taskbar at the top
        }
        if x < p.x {
            x = cx + gap; // taskbar on the left
        }
        x = x.clamp(p.x, (right - w).max(p.x));
        y = y.clamp(p.y, (bottom - h).max(p.y));
    }
    let _ = m.set_size(LogicalSize::new(MENU_W, MENU_H));
    let _ = m.set_position(PhysicalPosition::new(x, y));
    let _ = m.show();
    let _ = m.set_focus();
    let _ = m.emit_to(MENU, "traymenu://open", ());
}

pub fn init(app: &AppHandle) -> tauri::Result<()> {
    if let Err(e) = build_menu(app) {
        crate::applog::warn("app", format!("منوی کنار ساعت ساخته نشد: {e}"));
    }
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon(Vpn::Off))
        .tooltip("PacketYellow · VPN وصل نیست")
        .on_tray_icon_event(|tray, ev| match ev {
            TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => {
                show_main(tray.app_handle());
            }
            TrayIconEvent::Click { button: MouseButton::Right, button_state: MouseButtonState::Up, position, .. } => {
                open_menu(tray.app_handle(), position);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// Window events for every window (hooked in lib.rs).
pub fn on_window_event(w: &Window, ev: &WindowEvent) {
    match (w.label(), ev) {
        // ✕ / Alt+F4 on the main window: hide next to the clock instead of quitting
        ("main", WindowEvent::CloseRequested { api, .. }) => {
            api.prevent_close();
            let _ = w.hide();
            first_hide_hint(w.app_handle());
        }
        (MENU, WindowEvent::CloseRequested { api, .. }) => {
            api.prevent_close();
            let _ = w.hide();
        }
        (MENU, WindowEvent::Focused(false)) => {
            LAST_BLUR.store(now_ms(), Ordering::Relaxed);
            let _ = w.hide();
        }
        _ => {}
    }
}

/// Tell the user once (ever) where the app went.
fn first_hide_hint(app: &AppHandle) {
    if HINT_SHOWN.swap(true, Ordering::Relaxed) {
        return;
    }
    let mark = app.path().app_data_dir().ok().map(|d| d.join("tray-hint.done"));
    if let Some(p) = &mark {
        if p.exists() {
            return;
        }
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(p));
        let _ = std::fs::write(p, b"1");
    }
    notify(app, "PacketYellow هنوز روشنه 🟡", "رفت کنار ساعت. برای باز کردن روش کلیک کن، برای خروج راست‌کلیک ← خروج");
}

/// Buttons of the tray popup.
#[tauri::command]
pub fn tray_action(app: AppHandle, action: String) {
    match action.as_str() {
        "show" => show_main(&app),
        "hide" => hide_menu(&app),
        "toggle" => {
            // the popup stays open so you can watch it connect
            let _ = app.emit_to("main", "tray://toggle", ());
        }
        "quit" => {
            hide_menu(&app);
            app.exit(0);
        }
        a if a.starts_with("goto:") => {
            show_main(&app);
            let _ = app.emit_to("main", "tray://goto", a.trim_start_matches("goto:").to_string());
        }
        _ => {}
    }
}

/// Change colour + tooltip (and tell the popup). Safe to call from any thread / task.
pub fn set(app: &AppHandle, s: Vpn, tip: &str) {
    if let Some(t) = app.tray_by_id(TRAY_ID) {
        let _ = t.set_icon(Some(icon(s)));
        // Windows cuts tray tooltips at 127 chars
        let tip: String = tip.chars().take(120).collect();
        let _ = t.set_tooltip(Some(tip));
    }
    let state = match s {
        Vpn::Off => "off",
        Vpn::Busy => "busy",
        Vpn::On => "on",
    };
    let _ = app.emit_to(MENU, "tray://vpn", TrayState { state, tip: tip.to_string() });
}

/// Windows toast notification (bottom-right).
pub fn notify(app: &AppHandle, title: &str, body: &str) {
    let _ = app.notification().builder().title(title).body(body).show();
}
