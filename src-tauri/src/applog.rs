//! In-app log: every module writes here; the UI "لاگ" tab shows it live (event `log://entry`).
//! Ring buffer of the last 1000 entries, kept in memory only.
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

const CAP: usize = 1000;
static APP: OnceLock<AppHandle> = OnceLock::new();
static BUF: Mutex<VecDeque<Entry>> = Mutex::new(VecDeque::new());

#[derive(Serialize, Clone)]
pub struct Entry {
    pub ts: u64,
    pub level: &'static str,
    pub src: String,
    pub msg: String,
}

pub fn init(app: AppHandle) {
    let _ = APP.set(app);
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn push(level: &'static str, src: &str, msg: impl Into<String>) {
    let e = Entry { ts: now_ms(), level, src: src.to_string(), msg: msg.into() };
    #[cfg(debug_assertions)]
    eprintln!("[{}] {}: {}", e.level, e.src, e.msg);
    if let Ok(mut b) = BUF.lock() {
        if b.len() >= CAP {
            b.pop_front();
        }
        b.push_back(e.clone());
    }
    if let Some(app) = APP.get() {
        let _ = app.emit("log://entry", e);
    }
}

pub fn info(src: &str, msg: impl Into<String>) { push("info", src, msg) }
pub fn warn(src: &str, msg: impl Into<String>) { push("warn", src, msg) }
pub fn error(src: &str, msg: impl Into<String>) { push("error", src, msg) }

#[tauri::command]
pub fn get_logs() -> Vec<Entry> {
    BUF.lock().map(|b| b.iter().cloned().collect()).unwrap_or_default()
}

#[tauri::command]
pub fn clear_logs() {
    if let Ok(mut b) = BUF.lock() {
        b.clear();
    }
}

/// Lets the frontend write JS errors into the same log.
#[tauri::command]
pub fn log_from_ui(level: String, src: String, msg: String) {
    let lvl: &'static str = match level.as_str() { "error" => "error", "warn" => "warn", _ => "info" };
    push(lvl, &src, msg);
}
