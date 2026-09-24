//! Live monitor: one sample per interval, emitted as `monitor://tick`.
//! mode "tcp"  = TCP handshake time (accurate on a normal connection)
//! mode "http" = HEAD on a kept-alive connection (accurate through VPN / proxy / TUN)
use crate::applog;
use crate::ping::{resolve, tcp_rtt_err};
use crate::probe;
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};
use tokio::time::{interval, MissedTickBehavior};

#[derive(Default)]
pub struct MonitorState {
    handle: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Tick {
    ts: u64,
    rtt_ms: Option<f64>,
    key: String,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Logs only when the state flips (ok -> fail / fail -> ok) so the log isn't flooded once per second.
struct Flip {
    failing: bool,
    fails: u32,
    what: String,
}
impl Flip {
    fn new(what: String) -> Self { Flip { failing: false, fails: 0, what } }
    fn ok(&mut self) {
        if self.failing {
            applog::info("monitor", format!("{} دوباره جواب داد (بعد از {} نمونه‌ی ناموفق)", self.what, self.fails));
        }
        self.failing = false;
        self.fails = 0;
    }
    fn fail(&mut self, reason: &str) {
        self.fails += 1;
        if !self.failing {
            self.failing = true;
            applog::warn("monitor", format!("{} جواب نداد: {reason}", self.what));
        }
    }
}

#[tauri::command]
pub async fn start_monitor(
    app: AppHandle,
    state: State<'_, MonitorState>,
    host: String,
    port: Option<u16>,
    interval_ms: Option<u64>,
    mode: Option<String>,
    url: Option<String>,
) -> Result<(), String> {
    let every = Duration::from_millis(interval_ms.unwrap_or(1000).max(250));
    // stop the previous monitor first, so two loops never run at the same time while we warm up
    if let Some(old) = state.handle.lock().map_err(|e| e.to_string())?.take() {
        old.abort();
    }

    let task = if mode.as_deref() == Some("http") {
        let url = url.filter(|u| !u.is_empty()).ok_or_else(|| "برای حالت HTTP آدرس لازمه".to_string())?;
        let client = probe::client(Duration::from_secs(3))?;
        applog::info("monitor", format!("شروع پایش HTTP: {url}"));
        tauri::async_runtime::spawn(async move {
            let mut flip = Flip::new(format!("HTTP {url}"));
            let _ = probe::http_once(&client, &url).await; // open the connection, not counted
            let mut tick = interval(every);
            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let rtt_ms = match probe::http_try(&client, &url).await {
                    Ok(v) => { flip.ok(); Some(v) }
                    Err(e) => { flip.fail(&e); None }
                };
                let _ = app.emit("monitor://tick", Tick { ts: now_ms(), rtt_ms, key: url.clone() });
            }
        })
    } else {
        let p = port.unwrap_or(443);
        let addr = resolve(&host, p).await.map_err(|e| {
            applog::error("monitor", format!("DNS برای {host} پیدا نشد: {e}"));
            e
        })?;
        applog::info("monitor", format!("شروع پایش TCP: {host}:{p} ({})", addr.ip()));
        tauri::async_runtime::spawn(async move {
            let mut flip = Flip::new(format!("TCP {host}:{p}"));
            let mut tick = interval(every);
            tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let rtt_ms = match tcp_rtt_err(addr, Duration::from_secs(2)).await {
                    Ok(v) => { flip.ok(); Some(v) }
                    Err(e) => { flip.fail(&e); None }
                };
                let _ = app.emit("monitor://tick", Tick { ts: now_ms(), rtt_ms, key: host.clone() });
            }
        })
    };

    let old = state.handle.lock().map_err(|e| e.to_string())?.replace(task);
    if let Some(old) = old {
        old.abort();
    }
    Ok(())
}

#[tauri::command]
pub fn stop_monitor(state: State<'_, MonitorState>) -> Result<(), String> {
    if let Some(h) = state.handle.lock().map_err(|e| e.to_string())?.take() {
        h.abort();
    }
    Ok(())
}
