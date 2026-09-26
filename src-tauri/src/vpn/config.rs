//! sing-box (>= 1.12) configuration builder.
//! Every user option is optional: empty / 0 falls back to a sane default (see `Opts::default`).
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::net::IpAddr;

pub const DEFAULT_PORT: u16 = 2080;
pub const DEFAULT_REMOTE_DNS: &str = "1.1.1.1";

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Opts {
    /// "proxy" = system proxy, "tun" = whole system (admin), "local" = only a local SOCKS/HTTP port
    pub mode: String,
    /// 0 = automatic (2080, or any free port if 2080 is busy)
    pub port: u16,
    /// .ir domains + private IPs go direct
    pub bypass_iran: bool,
    /// DNS used through the tunnel. Empty = 1.1.1.1 (DoH).
    /// Accepts: 1.1.1.1 · 8.8.8.8:53 · https://dns.google/dns-query · tls://1.1.1.1 · udp:// tcp:// quic:// h3://
    pub remote_dns: String,
    /// DNS used for direct traffic (.ir, the server's own domain). Empty = system DNS.
    pub direct_dns: String,
    /// reject UDP/443 (QUIC) so browsers fall back to TCP: far more stable over ws / grpc / tcp configs
    pub block_quic: bool,
    pub allow_lan: bool,
    /// Xray only: split the TLS ClientHello into small pieces (helps against SNI filtering / DPI)
    pub fragment: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            mode: "proxy".into(),
            port: 0,
            bypass_iran: true,
            remote_dns: String::new(),
            direct_dns: String::new(),
            block_quic: true,
            allow_lan: false,
            fragment: false,
        }
    }
}

fn is_ip(h: &str) -> bool {
    h.parse::<IpAddr>().is_ok()
}

/// Tag an outbound and make its server lookup IPv4-first (broken IPv6 is the #1 cause of
/// "connects but nothing loads" on Iranian ISPs).
fn prep(ob: &Value, tag: &str) -> Value {
    let mut o = ob.clone();
    if let Value::Object(m) = &mut o {
        m.insert("tag".into(), json!(tag));
        let domain = m.get("server").and_then(|s| s.as_str()).map(|s| !is_ip(s)).unwrap_or(false);
        if domain && !m.contains_key("domain_resolver") {
            m.insert("domain_resolver".into(), json!({ "server": "dns-local", "strategy": "prefer_ipv4" }));
        }
    }
    o
}

/// "host", "host:port", "[v6]:port", "v6" -> (host, port)
fn split_host_port(s: &str) -> (String, Option<u16>) {
    if let Some(r) = s.strip_prefix('[') {
        if let Some(end) = r.find(']') {
            return (r[..end].to_string(), r[end + 1..].trim_start_matches(':').parse().ok());
        }
    }
    if s.matches(':').count() == 1 {
        let (h, p) = s.split_once(':').unwrap_or((s, ""));
        return (h.to_string(), p.parse().ok());
    }
    (s.to_string(), None)
}

/// User text -> sing-box 1.12 DNS server object.
/// `via_proxy`: goes through the tunnel (plain IP = DoH, the most censorship-proof choice).
pub fn dns_server(input: &str, tag: &str, via_proxy: bool) -> Value {
    let s = input.trim().trim_end_matches('/');
    const SCHEMES: [(&str, &str); 6] = [("https://", "https"), ("h3://", "h3"), ("tls://", "tls"), ("quic://", "quic"), ("tcp://", "tcp"), ("udp://", "udp")];
    let mut kind = "";
    let mut rest = s;
    for (p, k) in SCHEMES {
        if let Some(r) = s.strip_prefix(p) {
            kind = k;
            rest = r;
            break;
        }
    }
    let (hp, path) = match rest.split_once('/') {
        Some((h, p)) => (h, Some(format!("/{p}"))),
        None => (rest, None),
    };
    let (host, port) = split_host_port(hp);
    if kind.is_empty() {
        kind = match (via_proxy, port) {
            (true, Some(53)) => "tcp", // classic DNS through a proxy: TCP is reliable, UDP often isn't
            (true, _) => "https",
            (false, _) => "udp",
        };
    }
    let mut m = Map::new();
    m.insert("type".into(), json!(kind));
    m.insert("tag".into(), json!(tag));
    let host_is_ip = is_ip(&host);
    m.insert("server".into(), json!(host));
    if let Some(p) = port {
        m.insert("server_port".into(), json!(p));
    }
    if kind == "https" || kind == "h3" {
        m.insert("path".into(), json!(path.unwrap_or_else(|| "/dns-query".into())));
    }
    if via_proxy {
        m.insert("detour".into(), json!("proxy"));
    }
    if !host_is_ip {
        m.insert("domain_resolver".into(), json!("dns-boot"));
    }
    Value::Object(m)
}

/// Direct DNS used inside TUN. The system resolver itself is captured by the tunnel there (DNS loop),
/// so we talk plain UDP to the DNS server the system had *before* the tunnel came up (usually the
/// router). The old code used 1.1.1.1 here, which Iranian ISPs block/poison on UDP 53, so the server's
/// own domain never resolved and TUN "connected" without any traffic.
fn tun_direct_dns(o: &Opts, sys_dns: Option<&str>) -> Value {
    if !o.direct_dns.trim().is_empty() {
        dns_server(&o.direct_dns, "dns-local", false)
    } else {
        dns_server(sys_dns.unwrap_or("8.8.8.8"), "dns-local", false)
    }
}

/// `exclude`: addresses that must never enter the TUN (the VPN server itself). Belt and braces next to
/// the process_name rule: if Windows can't tell which process owns a socket, Xray's connection to its
/// server would loop back into the TUN and nothing would load.
fn tun_inbound(exclude: Option<&str>) -> Value {
    let mut t = json!({
        "type": "tun", "tag": "tun-in", "interface_name": "PacketYellow",
        "address": ["172.19.0.1/30", "fdfe:dcba:9876::1/126"],
        "mtu": 9000, "auto_route": true, "strict_route": true, "stack": "mixed",
        "udp_timeout": "5m"
    });
    if let Some(ip) = exclude.and_then(|h| h.parse::<IpAddr>().ok()) {
        let cidr = if ip.is_ipv4() { format!("{ip}/32") } else { format!("{ip}/128") };
        t["route_exclude_address"] = json!([cidr]);
    }
    t
}

/// sing-box as a TUN front-end for Xray (what v2rayN does): every app's traffic enters the TUN and is
/// handed to Xray's local SOCKS port. Xray's own sockets are excluded by process name, and Xray gets
/// its server as an IP (see `xray::pin_server_ip`), so there is no routing / DNS loop.
pub fn build_tun_front(o: &Opts, socks_port: u16, clash_port: u16, secret: &str, sys_dns: Option<&str>, server_ip: Option<&str>) -> Value {
    let remote = if o.remote_dns.trim().is_empty() { DEFAULT_REMOTE_DNS } else { o.remote_dns.trim() };
    let mut dns_rules = vec![];
    let mut rules = vec![
        json!({ "process_name": ["xray.exe", "xray", "sing-box.exe", "sing-box"], "outbound": "direct" }),
        json!({ "action": "sniff" }),
        json!({ "protocol": "dns", "action": "hijack-dns" }),
        json!({ "ip_is_private": true, "outbound": "direct" }),
    ];
    if o.block_quic {
        rules.push(json!({ "network": "udp", "port": 443, "action": "reject" }));
    }
    if o.bypass_iran {
        dns_rules.push(json!({ "domain_suffix": [".ir"], "server": "dns-local" }));
        rules.push(json!({ "domain_suffix": [".ir"], "outbound": "direct" }));
    }
    json!({
        "log": { "level": "warn", "timestamp": true },
        "dns": {
            "servers": [ dns_server(remote, "dns-remote", true), tun_direct_dns(o, sys_dns), json!({ "type": "udp", "tag": "dns-boot", "server": sys_dns.unwrap_or("8.8.8.8") }) ],
            "rules": dns_rules,
            "final": "dns-remote",
            "strategy": "prefer_ipv4"
        },
        "inbounds": [ tun_inbound(server_ip) ],
        "outbounds": [
            { "type": "socks", "tag": "proxy", "server": "127.0.0.1", "server_port": socks_port, "version": "5" },
            { "type": "direct", "tag": "direct" }
        ],
        "route": {
            "rules": rules,
            "final": "proxy",
            "auto_detect_interface": true,
            "default_domain_resolver": { "server": "dns-local", "strategy": "prefer_ipv4" }
        },
        "experimental": { "clash_api": { "external_controller": format!("127.0.0.1:{clash_port}"), "secret": secret } }
    })
}

/// Config for a real connection. `o.port` must already be resolved (non-zero).
pub fn build(outbound: &Value, o: &Opts, clash_port: u16, secret: &str, log_level: &str, sys_dns: Option<&str>) -> Value {
    let listen = if o.allow_lan { "0.0.0.0" } else { "127.0.0.1" };
    let port = if o.port == 0 { DEFAULT_PORT } else { o.port };
    let mut inbounds = vec![json!({ "type": "mixed", "tag": "mixed-in", "listen": listen, "listen_port": port })];
    if o.mode == "tun" {
        inbounds.push(tun_inbound(None));
    }
    let remote = if o.remote_dns.trim().is_empty() { DEFAULT_REMOTE_DNS } else { o.remote_dns.trim() };
    let tun = o.mode == "tun";
    // in TUN mode the system resolver is itself captured by the tunnel (DNS loop / timeouts),
    // so "system DNS" becomes plain UDP there
    let local = if tun {
        tun_direct_dns(o, sys_dns)
    } else if !o.direct_dns.trim().is_empty() {
        dns_server(&o.direct_dns, "dns-local", false)
    } else {
        json!({ "type": "local", "tag": "dns-local" })
    };
    let boot = if tun { json!({ "type": "udp", "tag": "dns-boot", "server": sys_dns.unwrap_or("8.8.8.8") }) } else { json!({ "type": "local", "tag": "dns-boot" }) };

    let mut dns_rules = vec![];
    let mut rules = vec![
        json!({ "action": "sniff" }),
        json!({ "protocol": "dns", "action": "hijack-dns" }),
        json!({ "ip_is_private": true, "outbound": "direct" }),
    ];
    if o.block_quic {
        rules.push(json!({ "network": "udp", "port": 443, "action": "reject" }));
    }
    if o.bypass_iran {
        dns_rules.push(json!({ "domain_suffix": [".ir"], "server": "dns-local" }));
        rules.push(json!({ "domain_suffix": [".ir"], "outbound": "direct" }));
    }
    json!({
        "log": { "level": log_level, "timestamp": true },
        "dns": {
            "servers": [ dns_server(remote, "dns-remote", true), local, boot ],
            "rules": dns_rules,
            "final": "dns-remote",
            "strategy": "prefer_ipv4",
            "cache_capacity": 4096
        },
        "inbounds": inbounds,
        "outbounds": [ prep(outbound, "proxy"), { "type": "direct", "tag": "direct" } ],
        "route": {
            "rules": rules,
            "final": "proxy",
            "auto_detect_interface": true,
            "default_domain_resolver": { "server": "dns-local", "strategy": "prefer_ipv4" }
        },
        "experimental": { "clash_api": { "external_controller": format!("127.0.0.1:{clash_port}"), "secret": secret } }
    })
}

/// Config for the "real delay" test: every server is an outbound (p0, p1, ...), no inbounds.
/// Delays are measured through the clash API (`/proxies/<tag>/delay`).
pub fn build_test(outbounds: &[(usize, Value)], clash_port: u16, secret: &str) -> Value {
    let mut obs: Vec<Value> = outbounds.iter().map(|(i, ob)| prep(ob, &format!("p{i}"))).collect();
    obs.push(json!({ "type": "direct", "tag": "direct" }));
    json!({
        "log": { "level": "error", "timestamp": false },
        "dns": { "servers": [ { "type": "local", "tag": "dns-local" } ] },
        "inbounds": [],
        "outbounds": obs,
        "route": { "final": "direct", "auto_detect_interface": true, "default_domain_resolver": "dns-local" },
        "experimental": { "clash_api": { "external_controller": format!("127.0.0.1:{clash_port}"), "secret": secret } }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shape() {
        let ob = json!({ "type": "vless", "server": "1.2.3.4", "server_port": 443, "uuid": "x" });
        let mut o = Opts::default();
        o.mode = "tun".into();
        let c = build(&ob, &o, 9090, "s", "warn", Some("192.168.1.1"));
        assert_eq!(c["outbounds"][0]["tag"], "proxy");
        assert_eq!(c["inbounds"][0]["listen_port"], DEFAULT_PORT);
        assert_eq!(c["inbounds"][1]["type"], "tun");
        assert_eq!(c["dns"]["servers"][0]["detour"], "proxy");
        assert_eq!(c["dns"]["servers"][0]["type"], "https");
        assert_eq!(c["dns"]["servers"][1]["type"], "udp"); // no system resolver inside TUN
        assert_eq!(c["dns"]["servers"][1]["server"], "192.168.1.1");
        let f = build_tun_front(&o, 2080, 9090, "s", None, Some("5.6.7.8"));
        assert_eq!(f["outbounds"][0]["server_port"], 2080);
        assert_eq!(f["inbounds"][0]["route_exclude_address"][0], "5.6.7.8/32");
        let t = build_test(&[(3, ob)], 1, "s");
        assert_eq!(t["outbounds"][0]["tag"], "p3");
    }
    #[test]
    fn dns_forms() {
        assert_eq!(dns_server("8.8.8.8:53", "d", true)["type"], "tcp");
        assert_eq!(dns_server("https://dns.google/dns-query", "d", true)["domain_resolver"], "dns-boot");
        assert_eq!(dns_server("tls://1.1.1.1", "d", true)["type"], "tls");
        assert_eq!(dns_server("78.157.42.100", "d", false)["type"], "udp");
        assert_eq!(dns_server("[2606:4700::1111]:53", "d", false)["server_port"], 53);
    }
    #[test]
    fn domain_server_prefers_v4() {
        let ob = json!({ "type": "vless", "server": "cdn.example.com", "server_port": 80 });
        let c = build(&ob, &Opts::default(), 1, "s", "warn", None);
        assert_eq!(c["outbounds"][0]["domain_resolver"]["strategy"], "prefer_ipv4");
    }
}
