//! Game-server probes + live multi-server monitor for the games tab.
//! Methods:
//!   tcp  - TCP handshake (no admin)
//!   icmp - real ICMP echo. Windows: IcmpSendEcho (no admin needed); others: system `ping`
//!   a2s  - Steam / Source / GoldSrc query (UDP A2S_INFO): real game-protocol RTT for CS 1.6, CS:S, CS:GO community servers...
//!   samp - SA-MP / open.mp query (UDP "p" ping packet)
//!   auto - ICMP if the host answers it, otherwise TCP (decided once per server)
use crate::applog;
use crate::ping::resolve;
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, State};
use tokio::net::UdpSocket;
use tokio::time::{interval, timeout, MissedTickBehavior};

pub const TIMEOUT: Duration = Duration::from_millis(2000);

pub fn norm_method(m: Option<&str>) -> &'static str {
    match m.unwrap_or("tcp").to_ascii_lowercase().as_str() {
        "icmp" => "icmp",
        "a2s" | "steam" | "source" => "a2s",
        "samp" | "sa-mp" | "openmp" => "samp",
        "auto" => "auto",
        _ => "tcp",
    }
}

/* ---------------- ICMP ---------------- */
#[cfg(windows)]
mod icmp_sys {
    use std::ffi::c_void;
    #[allow(dead_code)]
    #[repr(C)]
    struct IpOptionInformation {
        ttl: u8,
        tos: u8,
        flags: u8,
        options_size: u8,
        options_data: *mut u8,
    }
    #[allow(dead_code)]
    #[repr(C)]
    struct IcmpEchoReply {
        address: u32,
        status: u32,
        round_trip_time: u32,
        data_size: u16,
        reserved: u16,
        data: *mut c_void,
        options: IpOptionInformation,
    }
    #[link(name = "iphlpapi")]
    extern "system" {
        fn IcmpCreateFile() -> *mut c_void;
        fn IcmpCloseHandle(h: *mut c_void) -> i32;
        fn IcmpSendEcho(h: *mut c_void, dest: u32, req: *const c_void, req_size: u16, opts: *const c_void, reply: *mut c_void, reply_size: u32, timeout: u32) -> u32;
    }
    /// Blocking. Returns RTT in ms. IcmpSendEcho has 1 ms resolution; sub-ms replies are reported as 0.5.
    pub fn ping(ip: std::net::IpAddr, timeout_ms: u32) -> Result<f64, String> {
        let payload = [0x50u8; 32];
        let mut reply = vec![0u8; 256 + std::mem::size_of::<IcmpEchoReply>()];
        unsafe {
            match ip {
                std::net::IpAddr::V4(v4) => {
                    let h = IcmpCreateFile();
                    if h.is_null() || h as isize == -1 {
                        return Err("IcmpCreateFile failed".into());
                    }
                    let n = IcmpSendEcho(h, u32::from_ne_bytes(v4.octets()), payload.as_ptr() as *const c_void, payload.len() as u16, std::ptr::null(), reply.as_mut_ptr() as *mut c_void, reply.len() as u32, timeout_ms);
                    IcmpCloseHandle(h);
                    if n == 0 {
                        return Err("timeout".into());
                    }
                    let r: IcmpEchoReply = std::ptr::read_unaligned(reply.as_ptr() as *const IcmpEchoReply);
                    if r.status != 0 {
                        return Err(format!("ICMP status {}", r.status));
                    }
                    Ok(if r.round_trip_time == 0 { 0.5 } else { r.round_trip_time as f64 })
                }
                std::net::IpAddr::V6(_) => Err("ICMP روی IPv6 پشتیبانی نمیشه".into()),
            }
        }
    }
}

#[cfg(not(windows))]
mod icmp_sys {
    /// Uses the system `ping` binary (setuid / unprivileged ICMP socket), parses `time=12.3 ms`.
    pub fn ping(ip: std::net::IpAddr, timeout_ms: u32) -> Result<f64, String> {
        let secs = ((timeout_ms + 999) / 1000).max(1).to_string();
        let bin = if ip.is_ipv6() && cfg!(target_os = "macos") { "ping6" } else { "ping" };
        let wflag = if cfg!(target_os = "macos") { "-t" } else { "-W" };
        let out = std::process::Command::new(bin).args(["-c", "1", wflag, &secs, &ip.to_string()]).output().map_err(|e| e.to_string())?;
        let t = String::from_utf8_lossy(&out.stdout);
        t.split("time=").nth(1).and_then(|r| r.split_whitespace().next()).and_then(|v| v.trim_end_matches("ms").parse().ok()).ok_or_else(|| "timeout".into())
    }
}

pub async fn icmp(ip: IpAddr, to: Duration) -> Result<f64, String> {
    let ms = to.as_millis() as u32;
    tauri::async_runtime::spawn_blocking(move || icmp_sys::ping(ip, ms)).await.map_err(|e| e.to_string())?
}

/* ---------------- UDP game queries ---------------- */
/// Send one query, wait for a reply that `accept` recognises. RTT = send -> first valid reply.
async fn udp_exchange(addr: SocketAddr, to: Duration, pkt: &[u8], accept: impl Fn(&[u8]) -> bool) -> Result<f64, String> {
    let bind: SocketAddr = if addr.is_ipv4() { SocketAddr::from(([0, 0, 0, 0], 0)) } else { SocketAddr::from(([0u16; 8], 0)) };
    let sock = UdpSocket::bind(bind).await.map_err(|e| e.to_string())?;
    sock.connect(addr).await.map_err(|e| e.to_string())?;
    let mut buf = [0u8; 2048];
    let t0 = Instant::now();
    sock.send(pkt).await.map_err(|e| e.to_string())?;
    loop {
        let left = to.saturating_sub(t0.elapsed());
        if left.is_zero() {
            return Err("timeout".into());
        }
        match timeout(left, sock.recv(&mut buf)).await {
            Ok(Ok(n)) if accept(&buf[..n]) => return Ok(t0.elapsed().as_secs_f64() * 1000.0),
            Ok(Ok(_)) => continue, // unrelated packet
            Ok(Err(e)) => return Err(e.to_string()), // e.g. ICMP port unreachable
            Err(_) => return Err("timeout".into()),
        }
    }
}

const A2S_INFO: &[u8] = b"\xFF\xFF\xFF\xFFTSource Engine Query\0";

pub async fn a2s(addr: SocketAddr, to: Duration) -> Result<f64, String> {
    // any valid reply header (0x49 info, 0x6D GoldSrc info, 0x41 challenge) = one full round trip
    udp_exchange(addr, to, A2S_INFO, |b| b.len() >= 5 && b[..4] == [0xFF; 4] && matches!(b[4], 0x49 | 0x6D | 0x41)).await
}

pub async fn samp(addr: SocketAddr, to: Duration) -> Result<f64, String> {
    let IpAddr::V4(v4) = addr.ip() else { return Err("SA-MP فقط IPv4".into()) };
    let nonce: [u8; 4] = (SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(7)).to_le_bytes();
    let mut pkt = b"SAMP".to_vec();
    pkt.extend_from_slice(&v4.octets());
    pkt.extend_from_slice(&addr.port().to_le_bytes());
    pkt.push(b'p');
    pkt.extend_from_slice(&nonce);
    udp_exchange(addr, to, &pkt, move |b| b.len() >= 15 && &b[..4] == b"SAMP" && b[10] == b'p' && b[11..15] == nonce).await
}

/// One probe with an already resolved address.
pub async fn probe(method: &str, addr: SocketAddr, to: Duration) -> Result<f64, String> {
    match method {
        "icmp" => icmp(addr.ip(), to).await,
        "a2s" => a2s(addr, to).await,
        "samp" => samp(addr, to).await,
        _ => crate::ping::tcp_rtt_err(addr, to).await,
    }
}

/// "auto": ICMP if the host answers it (closest to UDP game traffic), else TCP.
pub async fn pick_method(addr: SocketAddr, port_given: bool) -> &'static str {
    for _ in 0..2 {
        if icmp(addr.ip(), Duration::from_millis(1200)).await.is_ok() {
            return "icmp";
        }
    }
    if port_given {
        return "tcp";
    }
    "icmp"
}

/* ---------------- live monitor ---------------- */
#[derive(Default)]
pub struct LiveState {
    handle: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveTarget {
    id: String,
    host: String,
    port: Option<u16>,
    method: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    id: String,
    ip: Option<String>,
    method: String,
    error: Option<String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LiveSample {
    id: String,
    rtt: Option<f64>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LiveTick {
    session: u64,
    ts: u64,
    samples: Vec<LiveSample>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Starts (or replaces) the live game monitor. Every tick probes all servers in parallel and emits
/// one `game://tick` event. Returns how each server will be measured (resolved IP + method).
#[tauri::command]
pub async fn game_live_start(app: AppHandle, state: State<'_, LiveState>, session: u64, targets: Vec<LiveTarget>, interval_ms: Option<u64>) -> Result<Vec<Resolved>, String> {
    if let Some(h) = state.handle.lock().map_err(|e| e.to_string())?.take() {
        h.abort();
    }
    if targets.is_empty() {
        return Err("سروری برای پایش نیست".into());
    }
    let every = Duration::from_millis(interval_ms.unwrap_or(1000).clamp(500, 10_000));
    let prepared: Vec<(Resolved, Option<(SocketAddr, &'static str)>)> = join_all(targets.into_iter().take(24).map(|t| async move {
        let wanted = norm_method(t.method.as_deref());
        let port = t.port.filter(|p| *p > 0);
        let default_port = match wanted { "a2s" => 27015, "samp" => 7777, _ => 443 };
        match resolve(&t.host, port.unwrap_or(default_port)).await {
            Ok(addr) => {
                let m = if wanted == "auto" { pick_method(addr, port.is_some()).await } else if wanted == "tcp" && port.is_none() { "icmp" } else { wanted };
                (Resolved { id: t.id, ip: Some(addr.ip().to_string()), method: m.into(), error: None }, Some((addr, m)))
            }
            Err(e) => (Resolved { id: t.id, ip: None, method: wanted.into(), error: Some(e) }, None),
        }
    }))
    .await;
    let resolved: Vec<Resolved> = prepared.iter().map(|(r, _)| r.clone()).collect();
    let jobs: Vec<(String, Option<(SocketAddr, &'static str)>)> = prepared.into_iter().map(|(r, a)| (r.id, a)).collect();
    applog::info("game", format!("پایش زنده‌ی بازی: {}", resolved.iter().map(|r| format!("{}={}", r.ip.as_deref().unwrap_or("?"), r.method)).collect::<Vec<_>>().join(" · ")));
    let task = tauri::async_runtime::spawn(async move {
        let mut tick = interval(every);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let to = every.min(TIMEOUT);
        loop {
            tick.tick().await;
            let samples = join_all(jobs.iter().map(|(id, a)| async move {
                let rtt = match a {
                    Some((addr, m)) => probe(m, *addr, to).await.ok(),
                    None => None,
                };
                LiveSample { id: id.clone(), rtt }
            }))
            .await;
            let _ = app.emit("game://tick", LiveTick { session, ts: now_ms(), samples });
        }
    });
    let old = state.handle.lock().map_err(|e| e.to_string())?.replace(task);
    if let Some(o) = old {
        o.abort();
    }
    Ok(resolved)
}

#[tauri::command]
pub fn game_live_stop(state: State<'_, LiveState>) -> Result<(), String> {
    if let Some(h) = state.handle.lock().map_err(|e| e.to_string())?.take() {
        h.abort();
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRes {
    ip: Option<String>,
    method: String,
    ms: Option<f64>,
    error: Option<String>,
}

/// One-shot test for the custom-game editor's «تست» button: best of 3 probes.
#[tauri::command]
pub async fn game_probe(host: String, port: Option<u16>, method: Option<String>) -> Result<ProbeRes, String> {
    let wanted = norm_method(method.as_deref());
    let port = port.filter(|p| *p > 0);
    let default_port = match wanted { "a2s" => 27015, "samp" => 7777, _ => 443 };
    let addr = resolve(host.trim(), port.unwrap_or(default_port)).await.map_err(|e| format!("آدرس پیدا نشد: {e}"))?;
    let m = if wanted == "auto" { pick_method(addr, port.is_some()).await } else if wanted == "tcp" && port.is_none() { "icmp" } else { wanted };
    let mut best: Option<f64> = None;
    let mut last_err = None;
    for _ in 0..3 {
        match probe(m, addr, TIMEOUT).await {
            Ok(v) => best = Some(best.map_or(v, |b| b.min(v))),
            Err(e) => last_err = Some(e),
        }
    }
    Ok(ProbeRes { ip: Some(addr.ip().to_string()), method: m.into(), ms: best, error: if best.is_none() { last_err.or(Some("بدون پاسخ".into())) } else { None } })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn methods() {
        assert_eq!(norm_method(Some("Steam")), "a2s");
        assert_eq!(norm_method(None), "tcp");
        assert_eq!(norm_method(Some("AUTO")), "auto");
    }
}
