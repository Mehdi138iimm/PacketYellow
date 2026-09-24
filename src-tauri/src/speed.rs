//! Speed test (Cloudflare): multi-connection, time-based, first 1.5s (TCP slow-start) discarded.
//! Also measures idle latency/jitter over HTTP and "loaded latency" (bufferbloat) during download.
//!
//! v0.3.1 fixes:
//! - download asked for 100 MB per request; the server rejects that size (4xx) and the old code retried
//!   silently forever -> "0.0 Mbps" with a full progress bar. Now it starts at 25 MB and steps down
//!   (25 -> 10 -> 5 -> 1 MB) on any non-2xx, and every failure is written to the log.
//! - upload retried in a tight loop (no sleep) when the server returned an error status.
//! - upload chunks could grow to 8 MB; on a slow line one chunk never finished inside the 8 s window,
//!   so upload showed 0. Chunk size now adapts both ways (target ~0.25-1 s per request, max 4 MB).
//! - latency requests had no timeout; one stuck request froze the whole test.
//! - a second test could start while the previous one was still stopping (shared CANCEL flag).
use crate::applog;
use crate::probe::err_chain;
use crate::stats::median;
use futures_util::StreamExt;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::async_runtime::{spawn, JoinHandle};
use tauri::{AppHandle, Emitter};
use tokio::time::sleep;

static CANCEL: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);
const BASE: &str = "https://speed.cloudflare.com";
const DL_SECS: f64 = 10.0;
const UL_SECS: f64 = 8.0;
const WARMUP: f64 = 1.5;
const DL_STREAMS: usize = 4;
const UL_STREAMS: usize = 3;
/// Download sizes to try, biggest first. The index is shared by all streams: once a size is refused,
/// every stream moves to the next smaller one.
const DL_SIZES: [u64; 4] = [25_000_000, 10_000_000, 5_000_000, 1_000_000];
const UL_MIN: usize = 64 * 1024;
const UL_MAX: usize = 4 * 1024 * 1024;
const REQ_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    phase: &'static str,
    mbps: f64,
    pct: f64,
    latency_ms: Option<f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedResult {
    download_mbps: f64,
    upload_mbps: f64,
    latency_ms: Option<f64>,
    jitter_ms: Option<f64>,
    loaded_latency_ms: Option<f64>,
    colo: Option<String>,
    data_mb: f64,
    cancelled: bool,
    /// non-fatal problems (e.g. upload blocked) shown under the result
    warnings: Vec<String>,
}

fn mbps(bytes: f64, secs: f64) -> f64 {
    if secs <= 0.0 { 0.0 } else { bytes * 8.0 / secs / 1_000_000.0 }
}

fn emit(app: &AppHandle, phase: &'static str, mbps: f64, pct: f64, latency_ms: Option<f64>) {
    let _ = app.emit("speed://progress", Progress { phase, mbps, pct: pct.clamp(0.0, 100.0), latency_ms });
}

/// Incompressible payload so no middlebox can shrink the upload.
fn random_bytes(n: usize) -> bytes::Bytes {
    let mut x: u64 = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7) | 1;
    let mut v = vec![0u8; n];
    for chunk in v.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let b = x.to_le_bytes();
        chunk.copy_from_slice(&b[..chunk.len()]);
    }
    bytes::Bytes::from(v)
}

/// Rate-limited logger shared by the worker streams (at most one line per distinct message).
#[derive(Default)]
struct Once(Mutex<Vec<String>>);
impl Once {
    fn warn(&self, msg: String) {
        if let Ok(mut v) = self.0.lock() {
            if !v.contains(&msg) && v.len() < 20 {
                applog::warn("speed", msg.clone());
                v.push(msg);
            }
        }
    }
}

async fn tiny_get(client: &reqwest::Client, url: &str) -> Result<(f64, reqwest::header::HeaderMap), String> {
    let t = Instant::now();
    let r = client.get(url).timeout(REQ_TIMEOUT).send().await.map_err(|e| err_chain(&e))?;
    let status = r.status();
    let headers = r.headers().clone();
    r.bytes().await.map_err(|e| err_chain(&e))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    Ok((t.elapsed().as_secs_f64() * 1000.0, headers))
}

async fn idle_latency(client: &reqwest::Client, app: &AppHandle) -> (Option<f64>, Option<f64>, Option<String>) {
    let url = format!("{BASE}/__down?bytes=0");
    let mut colo = None;
    // first request opens the connection (DNS + TCP + TLS), not counted
    match tiny_get(client, &url).await {
        Ok((ms, h)) => {
            colo = h.get("cf-ray").and_then(|v| v.to_str().ok()).and_then(|s| s.rsplit('-').next()).map(|s| s.to_string());
            applog::info("speed", format!("اتصال به {BASE} برقرار شد ({ms:.0} ms، دیتاسنتر {})", colo.as_deref().unwrap_or("?")));
        }
        Err(e) => applog::error("speed", format!("اتصال اول به سرور تست ناموفق: {e}")),
    }
    let n = 16;
    let mut v = Vec::new();
    let mut last_err = None;
    for i in 0..n {
        if CANCEL.load(Relaxed) {
            break;
        }
        match tiny_get(client, &url).await {
            Ok((ms, _)) => v.push(ms),
            Err(e) => last_err = Some(e),
        }
        emit(app, "latency", 0.0, (i + 1) as f64 / n as f64 * 100.0, v.last().copied());
    }
    if let Some(e) = last_err {
        applog::warn("speed", format!("سنجش تاخیر: {} از {n} درخواست ناموفق (آخرین خطا: {e})", n - v.len()));
    }
    // jitter from the samples in the order they were taken (median() sorts in place, so compute it first)
    let jitter = if v.len() > 1 { Some(v.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (v.len() - 1) as f64) } else { None };
    (median(&mut v), jitter, colo)
}

async fn run_phase(app: &AppHandle, phase: &'static str, secs: f64, counter: &AtomicU64, loaded: Option<&Mutex<Vec<f64>>>) -> f64 {
    let start = Instant::now();
    let mut warm: Option<(f64, u64)> = None;
    let mut result;
    loop {
        sleep(Duration::from_millis(200)).await;
        let t = start.elapsed().as_secs_f64();
        let b = counter.load(Relaxed);
        if warm.is_none() && t >= WARMUP {
            warm = Some((t, b));
        }
        result = match warm {
            Some((wt, wb)) if t - wt > 0.3 => mbps(b.saturating_sub(wb) as f64, t - wt),
            _ => mbps(b as f64, t),
        };
        let lat = loaded.and_then(|m| m.lock().ok().and_then(|v| v.last().copied()));
        emit(app, phase, result, t / secs * 100.0, lat);
        if t >= secs || CANCEL.load(Relaxed) {
            break;
        }
    }
    result
}

#[tauri::command]
pub fn speed_cancel() {
    if RUNNING.load(Relaxed) {
        applog::info("speed", "توقف تست توسط کاربر");
    }
    CANCEL.store(true, Relaxed);
}

struct RunGuard;
impl Drop for RunGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Relaxed);
    }
}

#[tauri::command]
pub async fn speed_test(app: AppHandle) -> Result<SpeedResult, String> {
    if RUNNING.swap(true, Relaxed) {
        return Err("یه تست سرعت دیگه هنوز در حال اجراست".into());
    }
    let _guard = RunGuard;
    CANCEL.store(false, Relaxed);
    applog::info("speed", "شروع تست سرعت");
    let r = speed_inner(&app).await;
    match &r {
        Ok(o) => applog::info(
            "speed",
            format!(
                "نتیجه: دانلود {:.1} · آپلود {:.1} Mbps · تاخیر {} / زیر بار {} ms · {:.0} MB{}",
                o.download_mbps,
                o.upload_mbps,
                o.latency_ms.map(|v| format!("{v:.0}")).unwrap_or("--".into()),
                o.loaded_latency_ms.map(|v| format!("{v:.0}")).unwrap_or("--".into()),
                o.data_mb,
                if o.cancelled { " (متوقف شد)" } else { "" }
            ),
        ),
        Err(e) => applog::error("speed", e.clone()),
    }
    r
}

async fn speed_inner(app: &AppHandle) -> Result<SpeedResult, String> {
    let client = reqwest::Client::builder()
        .user_agent(crate::probe::UA)
        .connect_timeout(Duration::from_secs(8))
        .tcp_nodelay(true)
        .build()
        .map_err(|e| e.to_string())?;
    let warns = Arc::new(Once::default());
    let mut warnings = Vec::new();

    // ---- idle latency ----
    let (latency_ms, jitter_ms, colo) = idle_latency(&client, app).await;
    if latency_ms.is_none() && !CANCEL.load(Relaxed) {
        return Err("سرور تست سرعت (speed.cloudflare.com) جواب نداد. جزئیات تو بخش «لاگ».".into());
    }
    let mut out = SpeedResult { download_mbps: 0.0, upload_mbps: 0.0, latency_ms, jitter_ms, loaded_latency_ms: None, colo, data_mb: 0.0, cancelled: false, warnings: vec![] };

    // ---- download: 4 parallel connections ----
    let dl = Arc::new(AtomicU64::new(0));
    let size_idx = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let loaded = Arc::new(Mutex::new(Vec::<f64>::new()));
    let mut handles: Vec<JoinHandle<()>> = Vec::new();
    if !CANCEL.load(Relaxed) {
        for _ in 0..DL_STREAMS {
            let (c, n, s, idx, w) = (client.clone(), dl.clone(), stop.clone(), size_idx.clone(), warns.clone());
            handles.push(spawn(async move {
                while !s.load(Relaxed) {
                    let i = idx.load(Relaxed).min(DL_SIZES.len() - 1);
                    let size = DL_SIZES[i];
                    let resp = match c.get(format!("{BASE}/__down?bytes={size}")).send().await {
                        Ok(r) if r.status().is_success() => r,
                        Ok(r) => {
                            w.warn(format!("دانلود {} MB رد شد: HTTP {}", size / 1_000_000, r.status()));
                            // step down to a smaller request size (only once per failed size)
                            let _ = idx.compare_exchange(i, (i + 1).min(DL_SIZES.len() - 1), Relaxed, Relaxed);
                            sleep(Duration::from_millis(200)).await;
                            continue;
                        }
                        Err(e) => {
                            w.warn(format!("خطای اتصال دانلود: {}", err_chain(&e)));
                            sleep(Duration::from_millis(300)).await;
                            continue;
                        }
                    };
                    let mut st = resp.bytes_stream();
                    while let Some(chunk) = st.next().await {
                        match chunk {
                            Ok(b) => {
                                n.fetch_add(b.len() as u64, Relaxed);
                            }
                            Err(e) => {
                                w.warn(format!("دانلود وسط کار قطع شد: {}", err_chain(&e)));
                                break;
                            }
                        }
                        if s.load(Relaxed) {
                            break;
                        }
                    }
                }
            }));
        }
        // loaded-latency probe on its own connection
        {
            let (c, s, l) = (client.clone(), stop.clone(), loaded.clone());
            handles.push(spawn(async move {
                let url = format!("{BASE}/__down?bytes=0");
                sleep(Duration::from_millis(1000)).await;
                while !s.load(Relaxed) {
                    if let Ok((ms, _)) = tiny_get(&c, &url).await {
                        if let Ok(mut v) = l.lock() {
                            v.push(ms);
                        }
                    }
                    sleep(Duration::from_millis(400)).await;
                }
            }));
        }
        out.download_mbps = run_phase(app, "download", DL_SECS, &dl, Some(&loaded)).await;
        stop.store(true, Relaxed);
        for h in handles.drain(..) {
            h.abort();
        }
        let mut lv = loaded.lock().map(|g| g.clone()).unwrap_or_default();
        out.loaded_latency_ms = median(&mut lv);
        if dl.load(Relaxed) == 0 && !CANCEL.load(Relaxed) {
            return Err("دانلود از سرور تست انجام نشد (احتمالاً مسدوده یا اتصال قطعه). جزئیات تو بخش «لاگ».".into());
        }
        applog::info("speed", format!("دانلود تمام شد: {:.1} Mbps ({:.1} MB)", out.download_mbps, dl.load(Relaxed) as f64 / 1e6));
    }

    // ---- upload: 3 connections, adaptive chunk size, only completed bytes count ----
    let ul = Arc::new(AtomicU64::new(0));
    if !CANCEL.load(Relaxed) {
        emit(app, "upload", 0.0, 0.0, None);
        let stop = Arc::new(AtomicBool::new(false));
        let payload = random_bytes(UL_MAX);
        for _ in 0..UL_STREAMS {
            let (c, n, s, p, w) = (client.clone(), ul.clone(), stop.clone(), payload.clone(), warns.clone());
            handles.push(spawn(async move {
                let mut size: usize = 128 * 1024;
                while !s.load(Relaxed) {
                    let t = Instant::now();
                    match c.post(format!("{BASE}/__up")).timeout(Duration::from_secs(15)).body(p.slice(0..size)).send().await {
                        Ok(r) => {
                            let status = r.status();
                            let _ = r.bytes().await;
                            if status.is_success() {
                                n.fetch_add(size as u64, Relaxed);
                                let el = t.elapsed();
                                if el < Duration::from_millis(250) {
                                    size = (size * 2).min(UL_MAX);
                                } else if el > Duration::from_millis(1000) {
                                    size = (size / 2).max(UL_MIN);
                                }
                            } else {
                                w.warn(format!("آپلود رد شد: HTTP {status}"));
                                size = (size / 2).max(UL_MIN);
                                sleep(Duration::from_millis(300)).await;
                            }
                        }
                        Err(e) => {
                            w.warn(format!("خطای آپلود: {}", err_chain(&e)));
                            size = (size / 2).max(UL_MIN);
                            sleep(Duration::from_millis(300)).await;
                        }
                    }
                }
            }));
        }
        out.upload_mbps = run_phase(app, "upload", UL_SECS, &ul, None).await;
        stop.store(true, Relaxed);
        for h in handles.drain(..) {
            h.abort();
        }
        if ul.load(Relaxed) == 0 && !CANCEL.load(Relaxed) {
            warnings.push("آپلود انجام نشد (سرور آپلود مسدوده یا خط خیلی کنده)".to_string());
        }
    }

    out.data_mb = (dl.load(Relaxed) + ul.load(Relaxed)) as f64 / 1_000_000.0;
    out.cancelled = CANCEL.load(Relaxed);
    out.warnings = warnings;
    emit(app, "done", out.upload_mbps, 100.0, None);
    Ok(out)
}
