//! TCP "ping": time a TCP handshake. Works without admin rights on every OS.
use crate::probe::is_fake_ip;
use crate::stats::{summarize, PingStats};
use futures_util::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::net::{lookup_host, TcpStream};
use tokio::time::{sleep, timeout};

/// System resolver (respects the user's DNS / VPN), with a hard timeout.
pub async fn resolve_all(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    let addrs: Vec<SocketAddr> = timeout(Duration::from_secs(5), lookup_host((host, port)))
        .await
        .map_err(|_| "DNS timeout".to_string())?
        .map_err(|e| format!("DNS error: {e}"))?
        .collect();
    if addrs.is_empty() {
        return Err("no address found".into());
    }
    Ok(addrs)
}

/// Prefer IPv4: many Iranian ISPs hand out broken or no IPv6.
pub async fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    let addrs = resolve_all(host, port).await?;
    Ok(addrs.iter().find(|a| a.is_ipv4()).copied().unwrap_or(addrs[0]))
}

pub async fn tcp_rtt(addr: SocketAddr, to: Duration) -> Option<f64> {
    tcp_rtt_err(addr, to).await.ok()
}

/// TCP handshake time, or why it failed ("timeout", "connection refused", ...).
pub async fn tcp_rtt_err(addr: SocketAddr, to: Duration) -> Result<f64, String> {
    let start = Instant::now();
    match timeout(to, TcpStream::connect(addr)).await {
        Ok(Ok(_stream)) => Ok(start.elapsed().as_secs_f64() * 1000.0),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("timeout ({} ms)", to.as_millis())),
    }
}

pub async fn run_ping(host: &str, port: u16, count: u32, interval_ms: u64) -> Result<PingStats, String> {
    run_ping_with(host, port, count, interval_ms, "tcp").await
}

/// Same as `run_ping` with a probe method (tcp / icmp / a2s / samp / auto), see `gameping`.
pub async fn run_ping_with(host: &str, port: u16, count: u32, interval_ms: u64, method: &str) -> Result<PingStats, String> {
    let addr = resolve(host, port).await?;
    let to = Duration::from_millis(2500);
    let method = match method {
        "auto" => crate::gameping::pick_method(addr, port > 0).await,
        "tcp" if port == 0 => "icmp", // no port given: only ICMP makes sense
        m => m,
    };
    // Warm-up probe, not counted: primes ARP / NAT / route caches so the first sample isn't inflated.
    let _ = crate::gameping::probe(method, addr, to).await;
    let mut samples = Vec::with_capacity(count as usize);
    for i in 0..count {
        samples.push(crate::gameping::probe(method, addr, to).await.ok());
        if i + 1 < count {
            sleep(Duration::from_millis(interval_ms)).await;
        }
    }
    let mut s = summarize(host, port, addr.ip().to_string(), samples);
    if is_fake_ip(&addr.ip()) {
        s.note = Some("fake-ip".into());
    } else if method != "tcp" {
        s.note = Some(method.into());
    }
    Ok(s)
}

#[tauri::command]
pub async fn ping_host(host: String, port: Option<u16>, count: Option<u32>) -> Result<PingStats, String> {
    run_ping(&host, port.unwrap_or(443), count.unwrap_or(10).clamp(1, 100), 250).await
}

#[derive(Deserialize)]
pub struct Target {
    pub name: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub method: Option<String>,
}

/// Ping many targets (max 8 at once so they don't skew each other), fastest first.
#[tauri::command]
pub async fn ping_many(targets: Vec<Target>, count: Option<u32>) -> Vec<PingStats> {
    let count = count.unwrap_or(5).clamp(1, 50);
    let mut out: Vec<PingStats> = stream::iter(targets.into_iter().map(|t| async move {
        let method = crate::gameping::norm_method(t.method.as_deref());
        match run_ping_with(&t.host, t.port, count, 200, method).await {
            Ok(mut s) => {
                s.name = t.name;
                s
            }
            Err(e) => PingStats::failed(t.name, t.host, t.port, e),
        }
    }))
    .buffer_unordered(8)
    .collect()
    .await;
    let failed: Vec<String> = out.iter().filter(|s| s.avg_ms.is_none()).map(|s| format!("{}:{} ({})", s.host, s.port, s.error.as_deref().unwrap_or("همه‌ی پینگ‌ها تایم‌اوت"))).collect();
    let n = out.len();
    crate::applog::info("ping", format!("پینگ {n} هدف: {} موفق، {} ناموفق", n - failed.len(), failed.len()));
    if !failed.is_empty() {
        crate::applog::warn("ping", format!("بدون پاسخ: {}", failed.iter().take(15).cloned().collect::<Vec<_>>().join(" · ")));
    }
    out.sort_by(|a, b| {
        a.avg_ms
            .unwrap_or(f64::MAX)
            .partial_cmp(&b.avg_ms.unwrap_or(f64::MAX))
            .unwrap_or(Ordering::Equal)
    });
    out
}

#[derive(Serialize)]
pub struct SteamServer {
    dc: String,
    host: String,
    port: u16,
}

/// Live list of Steam connection servers (one per datacenter) for CS2 / Dota 2.
#[tauri::command]
pub async fn steam_servers() -> Result<Vec<SteamServer>, String> {
    let r = steam_servers_inner().await;
    match &r {
        Ok(v) => crate::applog::info("steam", format!("{} دیتاسنتر Steam دریافت شد", v.len())),
        Err(e) => crate::applog::error("steam", format!("لیست سرورهای Steam: {e}")),
    }
    r
}

async fn steam_servers_inner() -> Result<Vec<SteamServer>, String> {
    let client = crate::probe::client(Duration::from_secs(8))?;
    let v: serde_json::Value = client
        .get("https://api.steampowered.com/ISteamDirectory/GetCMListForConnect/v1/?cellid=0&maxcount=300")
        .send()
        .await
        .map_err(|e| crate::probe::err_chain(&e))?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    let list = v["response"]["serverlist"].as_array().cloned().unwrap_or_default();
    let mut out: Vec<SteamServer> = Vec::new();
    for kind in ["websockets", "netfilter"] {
        for s in &list {
            if s["type"].as_str() != Some(kind) {
                continue;
            }
            let Some(ep) = s["endpoint"].as_str() else { continue };
            let dc = s["dc"].as_str().unwrap_or("?").to_string();
            if out.iter().any(|o| o.dc == dc) {
                continue;
            }
            if let Some((h, p)) = ep.rsplit_once(':') {
                if let Ok(port) = p.parse::<u16>() {
                    out.push(SteamServer { dc, host: h.to_string(), port });
                }
            }
        }
    }
    if out.is_empty() {
        return Err("Steam server list unavailable".into());
    }
    Ok(out)
}
