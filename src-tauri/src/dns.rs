//! DNS benchmark: speed, min/median, success rate and whether the server returns filtered answers.
use crate::probe::is_filter_ip;
use crate::stats::median;
use futures_util::future::join_all;
use hickory_resolver::config::{LookupIpStrategy, NameServerConfigGroup, ResolverConfig, ResolverOpts};
use hickory_resolver::TokioAsyncResolver;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

#[derive(Deserialize)]
pub struct Resolver {
    name: String,
    /// "1.1.1.1", "1.1.1.1:5353", "2606:4700::1111" or "[2606:4700::1111]:53"
    ip: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsResult {
    name: String,
    ip: String,
    ok: bool,
    avg_ms: Option<f64>,
    min_ms: Option<f64>,
    median_ms: Option<f64>,
    success_pct: f64,
    answers: Vec<String>,
    /// Some(true) = returns the 10.10.34.x filter page for youtube.com
    filtered: Option<bool>,
    error: Option<String>,
}

fn parse_server(s: &str) -> Option<SocketAddr> {
    let s = s.trim();
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return Some(sa);
    }
    s.trim_matches(|c| c == '[' || c == ']').parse::<IpAddr>().ok().map(|ip| SocketAddr::new(ip, 53))
}

fn fqdn(d: &str) -> String {
    if d.ends_with('.') { d.to_string() } else { format!("{d}.") }
}

async fn bench_one(r: Resolver, domains: Vec<String>, rounds: u32) -> DnsResult {
    let mut out = DnsResult {
        name: r.name,
        ip: r.ip.clone(),
        ok: false,
        avg_ms: None,
        min_ms: None,
        median_ms: None,
        success_pct: 0.0,
        answers: vec![],
        filtered: None,
        error: None,
    };
    let Some(sa) = parse_server(&r.ip) else {
        out.error = Some("آدرس نامعتبر".into());
        return out;
    };

    let group = NameServerConfigGroup::from_ips_clear(&[sa.ip()], sa.port(), true);
    let config = ResolverConfig::from_parts(None, vec![], group);
    let mut opts = ResolverOpts::default();
    opts.cache_size = 0; // measure the server, not our cache
    opts.attempts = 1;
    opts.timeout = Duration::from_secs(2);
    opts.use_hosts_file = false;
    opts.ip_strategy = LookupIpStrategy::Ipv4Only;
    let resolver = TokioAsyncResolver::tokio(config, opts);

    let mut times = Vec::new();
    let mut total = 0u32;
    'outer: for _ in 0..rounds {
        for d in &domains {
            total += 1;
            let start = Instant::now();
            match resolver.lookup_ip(fqdn(d).as_str()).await {
                Ok(lookup) => {
                    times.push(start.elapsed().as_secs_f64() * 1000.0);
                    if out.answers.is_empty() {
                        out.answers = lookup.iter().take(3).map(|a| a.to_string()).collect();
                    }
                }
                Err(e) => out.error = Some(e.to_string()),
            }
            // dead server: don't wait 2s x every query
            if times.is_empty() && total >= 2 {
                break 'outer;
            }
        }
    }

    if !times.is_empty() {
        out.filtered = match resolver.lookup_ip("youtube.com.").await {
            Ok(l) => Some(l.iter().any(|ip| is_filter_ip(&ip))),
            Err(_) => None,
        };
    }

    out.ok = !times.is_empty();
    out.success_pct = if total == 0 { 0.0 } else { times.len() as f64 / total as f64 * 100.0 };
    if out.ok {
        out.avg_ms = Some(times.iter().sum::<f64>() / times.len() as f64);
        out.min_ms = times.iter().copied().reduce(f64::min);
        out.median_ms = median(&mut times);
        out.error = None;
    }
    out
}

#[tauri::command]
pub async fn dns_benchmark(resolvers: Vec<Resolver>, domains: Option<Vec<String>>, rounds: Option<u32>) -> Vec<DnsResult> {
    let domains = domains.unwrap_or_else(|| {
        ["google.com", "github.com", "wikipedia.org", "digikala.com", "cloudflare.com"].iter().map(|s| s.to_string()).collect()
    });
    let rounds = rounds.unwrap_or(2).clamp(1, 5);
    let mut out = join_all(resolvers.into_iter().map(|r| bench_one(r, domains.clone(), rounds))).await;
    out.sort_by(|a, b| {
        let ka = a.median_ms.unwrap_or(f64::MAX) + (100.0 - a.success_pct) * 10.0;
        let kb = b.median_ms.unwrap_or(f64::MAX) + (100.0 - b.success_pct) * 10.0;
        ka.partial_cmp(&kb).unwrap_or(Ordering::Equal)
    });
    let ok = out.iter().filter(|r| r.ok).count();
    crate::applog::info("dns", format!("بنچمارک {} DNS: {ok} جواب دادن{}", out.len(), out.first().filter(|r| r.ok).map(|r| format!("، سریع‌ترین {} ({:.0} ms)", r.name, r.median_ms.unwrap_or(0.0))).unwrap_or_default()));
    let dead: Vec<String> = out.iter().filter(|r| !r.ok).map(|r| format!("{} ({})", r.ip, r.error.as_deref().unwrap_or("بدون پاسخ"))).collect();
    if !dead.is_empty() {
        crate::applog::warn("dns", format!("بدون پاسخ: {}", dead.join(" · ")));
    }
    out
}
