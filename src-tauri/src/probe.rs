//! Shared HTTP helpers + IP classification used by every module.
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Browser-like UA: some CDNs block or throttle unknown user agents.
pub const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

/// HTTP client that honours the system proxy (so it goes through a VPN/proxy client like v2rayN).
pub fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(6)))
        .tcp_nodelay(true)
        .pool_idle_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())
}

/// Same as `client` but never follows redirects: used by the live monitor, which sends one request per
/// second for as long as the dashboard is open. Following a redirect chain every second (e.g. site.com ->
/// www.site.com -> /fa/) multiplied the traffic for no benefit; the first response is enough to time a round trip.
pub fn client_no_redirect(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(6)))
        .tcp_nodelay(true)
        .pool_idle_timeout(Duration::from_secs(90))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

/// One HEAD request on a kept-alive connection = one real round trip, even through a VPN/proxy.
pub async fn http_once(client: &reqwest::Client, url: &str) -> Option<f64> {
    http_try(client, url).await.ok()
}

/// Same as `http_once` but keeps the reason of the failure (for the log).
pub async fn http_try(client: &reqwest::Client, url: &str) -> Result<f64, String> {
    let start = Instant::now();
    match client.head(url).send().await {
        Ok(_) => Ok(start.elapsed().as_secs_f64() * 1000.0),
        Err(e) => Err(err_chain(&e)),
    }
}

/// Full error text including every `source()` (reqwest hides the useful part, e.g. "connection reset").
pub fn err_chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        s.push_str(": ");
        s.push_str(&c.to_string());
        cur = c.source();
    }
    s
}

/// 198.18.0.0/15: "fake-ip" range used by Clash / sing-box / v2rayN in TUN mode.
/// A TCP ping to these answers locally in ~1ms, so it is meaningless.
pub fn is_fake_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 198 && (o[1] == 18 || o[1] == 19)
        }
        _ => false,
    }
}

/// 10.10.34.0/24: Iranian filtering page (peyvandha) returned by poisoned DNS.
pub fn is_filter_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 10 && o[1] == 10 && o[2] == 34
        }
        _ => false,
    }
}

pub fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
