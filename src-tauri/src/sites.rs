//! Site check in 4 stages so the app can say *why* a site doesn't open:
//! DNS -> TCP ping -> HTTP(S) request -> real RTT over the kept-alive connection.
use crate::ping::{resolve_all, tcp_rtt};
use crate::probe::{self, err_chain, is_fake_ip, is_filter_ip, ms_since};
use crate::stats::median;
use futures_util::stream::{self, StreamExt};
use serde::Serialize;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SiteResult {
    input: String,
    url: String,
    host: String,
    ip: Option<String>,
    ok: bool,
    /// ok | slow | restricted | server_error | filtered | tls_fail | tcp_fail | dns_fail | timeout | invalid
    state: String,
    status: Option<u16>,
    dns_ms: Option<f64>,
    ping_ms: Option<f64>,
    http_ms: Option<f64>,
    rtt_ms: Option<f64>,
    final_url: Option<String>,
    note: Option<String>,
    error: Option<String>,
}

async fn check_one(client: reqwest::Client, raw: String) -> SiteResult {
    let input = raw.trim().to_string();
    let url = if input.starts_with("http://") || input.starts_with("https://") { input.clone() } else { format!("https://{input}") };
    let mut r = SiteResult { input: input.clone(), url: url.clone(), state: "invalid".into(), ..Default::default() };
    let parsed = match reqwest::Url::parse(&url) {
        Ok(u) => u,
        Err(e) => {
            r.error = Some(e.to_string());
            return r;
        }
    };
    let host = match parsed.host_str() {
        Some(h) => h.trim_matches(|c| c == '[' || c == ']').to_string(),
        None => return r,
    };
    let port = parsed.port_or_known_default().unwrap_or(443);
    r.host = host.clone();

    // 1) DNS (system resolver)
    let t = Instant::now();
    let addr: Option<SocketAddr> = match resolve_all(&host, port).await {
        Ok(addrs) => {
            r.dns_ms = Some(ms_since(t));
            let a = addrs.iter().find(|a| a.is_ipv4()).copied().unwrap_or(addrs[0]);
            r.ip = Some(a.ip().to_string());
            Some(a)
        }
        Err(e) => {
            r.error = Some(e);
            None
        }
    };

    // 2) TCP ping (skipped for fake-ip: it would answer locally in ~1ms)
    if let Some(a) = addr {
        let ip = a.ip();
        if is_filter_ip(&ip) {
            r.state = "filtered".into();
            r.error = Some("DNS آدرس صفحه‌ی فیلتر رو برگردوند (10.10.34.x)".into());
            return r;
        }
        if is_fake_ip(&ip) {
            r.note = Some("fake-ip".into());
        } else {
            let to = Duration::from_millis(2500);
            let _ = tcp_rtt(a, to).await;
            let mut v = Vec::new();
            for _ in 0..3 {
                if let Some(x) = tcp_rtt(a, to).await {
                    v.push(x);
                }
            }
            r.ping_ms = median(&mut v);
        }
    }

    // 3) HTTP: always tried, a system proxy / VPN may reach it even if direct DNS/TCP failed.
    // HEAD instead of GET: we only need the status code + final URL, not the page. A GET pulled the start
    // of every page (often hundreds of KB) before the connection was dropped. HEAD also keeps the connection
    // alive, so stage 4 reuses it instead of doing a new TLS handshake.
    let t = Instant::now();
    match client.head(parsed.clone()).send().await {
        Ok(resp) => {
            r.http_ms = Some(ms_since(t));
            let code = resp.status().as_u16();
            r.status = Some(code);
            let fin = resp.url().clone();
            drop(resp);
            let fin_host = fin.host_str().unwrap_or("").to_string();
            if fin_host.contains("peyvandha") || fin_host.starts_with("10.10.34.") {
                r.state = "filtered".into();
                r.error = Some("به صفحه‌ی فیلتر ریدایرکت شد".into());
                return r;
            }
            if fin.as_str() != parsed.as_str() {
                r.final_url = Some(fin.to_string());
            }
            // 4) real round trip on the kept-alive connection (works through VPN)
            let _ = probe::http_once(&client, fin.as_str()).await;
            let mut v = Vec::new();
            for _ in 0..3 {
                if let Some(x) = probe::http_once(&client, fin.as_str()).await {
                    v.push(x);
                }
            }
            r.rtt_ms = median(&mut v);
            if r.ping_ms.is_none() && r.rtt_ms.is_some() {
                r.ping_ms = r.rtt_ms;
                if r.note.is_none() {
                    r.note = Some("http".into());
                }
            }
            let slow = r.http_ms.unwrap_or(0.0) > 2500.0;
            let state: &str = match code {
                403 | 451 => "restricted",
                500..=599 => "server_error",
                _ if slow => "slow",
                _ => "ok",
            };
            r.ok = state != "server_error";
            r.state = state.to_string();
        }
        Err(e) => {
            let msg = err_chain(&e);
            let state: &str = if e.is_timeout() {
                "timeout"
            } else if r.ip.is_none() {
                "dns_fail"
            } else if r.ping_ms.is_some() {
                // TCP works but HTTPS dies: classic SNI / TLS filtering
                "tls_fail"
            } else {
                "tcp_fail"
            };
            r.state = state.to_string();
            r.error = Some(msg);
        }
    }
    r
}

#[tauri::command]
pub async fn check_sites(urls: Vec<String>) -> Result<Vec<SiteResult>, String> {
    let client = probe::client(Duration::from_secs(10))?;
    let out = stream::iter(urls.into_iter().map(|u| check_one(client.clone(), u)))
        .buffered(6)
        .collect::<Vec<SiteResult>>()
        .await;
    let ok = out.iter().filter(|r| r.ok).count();
    crate::applog::info("sites", format!("تست {} سایت: {ok} باز", out.len()));
    for r in out.iter().filter(|r| !r.ok || r.state != "ok") {
        crate::applog::warn("sites", format!("{} → {}{}", r.input, r.state, r.error.as_deref().map(|e| format!(" · {e}")).unwrap_or_default()));
    }
    Ok(out)
}
