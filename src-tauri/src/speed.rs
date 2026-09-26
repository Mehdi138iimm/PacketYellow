//! Speed test (Cloudflare): multi-connection, **data-budgeted** (v0.4.3).
//!
//! v0.4.3: the old test was time-based (10 s download + 8 s upload at full speed), so on a 90 Mbps line it
//! burned ~100 MB of quota per run. Now every test has a hard byte budget (default 10 MB total, 75% download /
//! 25% upload) shared by all connections; a worker can only request bytes it has reserved from the budget,
//! so the test can never download more than the budget. Time caps still apply for slow lines.
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
/// Hard time caps. On a slow line the budget is not reached, so the test just ends earlier with less data.
const DL_SECS: f64 = 8.0;
const UL_SECS: f64 = 6.0;
/// First part of each budget is TCP slow-start and is not used for the speed number.
const WARM_FRAC: f64 = 0.2;
const DL_STREAMS: usize = 4;
const UL_STREAMS: usize = 3;
/// Largest single download request; the server refuses huge sizes. Steps down on any non-2xx.
const DL_CAPS: [u64; 4] = [4_000_000, 2_000_000, 1_000_000, 250_000];
const UL_MIN: usize = 32 * 1024;
const UL_MAX: usize = 2 * 1024 * 1024;
/// Total data budget per test (download + upload), in MB. The UI picks one of the presets.
const BUDGET_DEFAULT_MB: u32 = 10;
const BUDGET_MIN_MB: u32 = 3;
const BUDGET_MAX_MB: u32 = 50;
/// Share of the budget used for download (rest = upload).
const DL_SHARE: f64 = 0.75;
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

/// Watches a transfer phase and returns Mbps.
/// Stops when the byte budget is used up, every worker is done, the time cap is hit, or the user cancels.
/// Speed = bytes after the "warm point" (first ~20% of the budget = TCP slow-start) divided by the time
/// between the warm point and the last byte that actually arrived (idle tail after the budget is not counted).
async fn run_phase(app: &AppHandle, phase: &'static str, secs: f64, budget: u64, counter: &AtomicU64, active: &AtomicUsize, loaded: Option<&Mutex<Vec<f64>>>) -> f64 {
    let start = Instant::now();
    let warm_bytes = (budget as f64 * WARM_FRAC) as u64;
    let mut warm: Option<(f64, u64)> = None;
    let mut last: (f64, u64) = (0.0, 0);
    let mut last_emit = 0.0;
    let mut result;
    loop {
        // fine sampling: with a small budget a fast line finishes in well under a second
        sleep(Duration::from_millis(25)).await;
        let t = start.elapsed().as_secs_f64();
        let b = counter.load(Relaxed);
        if b > last.1 {
            last = (t, b);
        }
        if warm.is_none() && b >= warm_bytes && t >= 0.05 {
            warm = Some((t, b));
        }
        result = match warm {
            Some((wt, wb)) if last.0 - wt > 0.05 && last.1 > wb => mbps((last.1 - wb) as f64, last.0 - wt),
            _ => mbps(last.1 as f64, last.0),
        };
        let finished_soon = b >= budget || (t > 0.3 && active.load(Relaxed) == 0);
        if t - last_emit >= 0.2 || finished_soon {
            last_emit = t;
            let lat = loaded.and_then(|m| m.lock().ok().and_then(|v| v.last().copied()));
            let pct = (b as f64 / budget.max(1) as f64).max(t / secs) * 100.0;
            emit(app, phase, result, pct, lat);
        }
        let finished = finished_soon;
        if finished || t >= secs || CANCEL.load(Relaxed) {
            break;
        }
    }
    result
}

/// Takes up to `want` bytes from the shared budget; 0 = budget exhausted.
fn reserve(left: &AtomicU64, want: u64) -> u64 {
    match left.fetch_update(Relaxed, Relaxed, |l| if l == 0 { None } else { Some(l - l.min(want)) }) {
        Ok(prev) => prev.min(want),
        Err(_) => 0,
    }
}

/// Decrements the "active workers" counter when a worker task ends (also on abort).
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Relaxed);
    }
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
pub async fn speed_test(app: AppHandle, budget_mb: Option<u32>) -> Result<SpeedResult, String> {
    if RUNNING.swap(true, Relaxed) {
        return Err("یه تست سرعت دیگه هنوز در حال اجراست".into());
    }
    let _guard = RunGuard;
    CANCEL.store(false, Relaxed);
    let budget_mb = budget_mb.unwrap_or(BUDGET_DEFAULT_MB).clamp(BUDGET_MIN_MB, BUDGET_MAX_MB);
    applog::info("speed", format!("شروع تست سرعت (سقف مصرف {budget_mb} MB)"));
    let r = speed_inner(&app, budget_mb).await;
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

async fn speed_inner(app: &AppHandle, budget_mb: u32) -> Result<SpeedResult, String> {
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

    let total = budget_mb as u64 * 1_000_000;
    let dl_budget = (total as f64 * DL_SHARE) as u64;
    let ul_budget = total - dl_budget;

    // ---- download: 4 parallel connections sharing one byte budget ----
    let dl = Arc::new(AtomicU64::new(0));
    let loaded = Arc::new(Mutex::new(Vec::<f64>::new()));
    let mut handles: Vec<JoinHandle<()>> = Vec::new();
    if !CANCEL.load(Relaxed) {
        let left = Arc::new(AtomicU64::new(dl_budget));
        let cap_idx = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(DL_STREAMS));
        // ~2 requests per stream: enough to get past slow-start without wasting bytes on overhead
        let per_req = (dl_budget / (DL_STREAMS as u64 * 2)).max(100_000);
        for _ in 0..DL_STREAMS {
            let (c, n, s, idx, w, left, act) = (client.clone(), dl.clone(), stop.clone(), cap_idx.clone(), warns.clone(), left.clone(), active.clone());
            handles.push(spawn(async move {
                let _a = Active(act);
                while !s.load(Relaxed) {
                    let i = idx.load(Relaxed).min(DL_CAPS.len() - 1);
                    let size = reserve(&left, per_req.min(DL_CAPS[i]));
                    if size == 0 {
                        break; // budget used up
                    }
                    let resp = match c.get(format!("{BASE}/__down?bytes={size}")).timeout(Duration::from_secs(12)).send().await {
                        Ok(r) if r.status().is_success() => r,
                        Ok(r) => {
                            w.warn(format!("دانلود {} KB رد شد: HTTP {}", size / 1000, r.status()));
                            left.fetch_add(size, Relaxed); // nothing downloaded: give the bytes back
                            let _ = idx.compare_exchange(i, (i + 1).min(DL_CAPS.len() - 1), Relaxed, Relaxed);
                            sleep(Duration::from_millis(200)).await;
                            continue;
                        }
                        Err(e) => {
                            w.warn(format!("خطای اتصال دانلود: {}", err_chain(&e)));
                            left.fetch_add(size, Relaxed);
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
        // loaded-latency probe on its own connection (0-byte requests)
        {
            let (c, s, l) = (client.clone(), stop.clone(), loaded.clone());
            handles.push(spawn(async move {
                let url = format!("{BASE}/__down?bytes=0");
                sleep(Duration::from_millis(250)).await;
                while !s.load(Relaxed) {
                    if let Ok((ms, _)) = tiny_get(&c, &url).await {
                        if let Ok(mut v) = l.lock() {
                            v.push(ms);
                        }
                    }
                    sleep(Duration::from_millis(250)).await;
                }
            }));
        }
        out.download_mbps = run_phase(app, "download", DL_SECS, dl_budget, &dl, &active, Some(&loaded)).await;
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

    // ---- upload: 3 connections sharing one byte budget, only completed bytes count ----
    let ul = Arc::new(AtomicU64::new(0));
    if !CANCEL.load(Relaxed) {
        emit(app, "upload", 0.0, 0.0, None);
        let left = Arc::new(AtomicU64::new(ul_budget));
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(UL_STREAMS));
        let chunk = ((ul_budget / (UL_STREAMS as u64 * 2)) as usize).clamp(UL_MIN, UL_MAX);
        let payload = random_bytes(chunk);
        for _ in 0..UL_STREAMS {
            let (c, n, s, p, w, left, act) = (client.clone(), ul.clone(), stop.clone(), payload.clone(), warns.clone(), left.clone(), active.clone());
            handles.push(spawn(async move {
                let _a = Active(act);
                let mut fails = 0;
                while !s.load(Relaxed) {
                    let size = reserve(&left, chunk as u64) as usize;
                    if size == 0 {
                        break;
                    }
                    let ok = match c.post(format!("{BASE}/__up")).timeout(Duration::from_secs(12)).body(p.slice(0..size)).send().await {
                        Ok(r) => {
                            let status = r.status();
                            let _ = r.bytes().await;
                            if status.is_success() {
                                n.fetch_add(size as u64, Relaxed);
                                true
                            } else {
                                w.warn(format!("آپلود رد شد: HTTP {status}"));
                                false
                            }
                        }
                        Err(e) => {
                            w.warn(format!("خطای آپلود: {}", err_chain(&e)));
                            false
                        }
                    };
                    if !ok {
                        // a refused upload may already have used some quota: don't hand the bytes back
                        // endlessly, give up on this stream after a few failures
                        fails += 1;
                        if fails >= 3 {
                            break;
                        }
                        sleep(Duration::from_millis(300)).await;
                    }
                }
            }));
        }
        out.upload_mbps = run_phase(app, "upload", UL_SECS, ul_budget, &ul, &active, None).await;
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
