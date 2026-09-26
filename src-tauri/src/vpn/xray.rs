//! Xray-core: share link -> outbound, and the configs this app runs.
//!
//! Why Xray: it is the reference implementation of VLESS / REALITY / XTLS-Vision / XHTTP. The panels
//! most Iranian servers run (3x-ui, Marzban, Hiddify, ...) are written and tested against it, so a link
//! that works in v2rayN works here too. sing-box is still used for Hysteria2 / TUIC / Hysteria and as
//! the TUN front-end (see `config::build_tun_front`), exactly like v2rayN does.
use super::config::Opts;
use super::link::{self, csv, get, is_ip, parse_query, parse_uri, port_of, truthy, Profile, Q};
use serde_json::{json, Map, Value};

/// Private / local ranges that always go direct (no geoip.dat needed).
const PRIVATE: [&str; 9] = ["127.0.0.0/8", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "100.64.0.0/10", "::1/128", "fc00::/7", "fe80::/10"];

fn s_of(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

/// streamSettings for a link. Returns (net shown in the UI, security, streamSettings).
fn stream(q: &Q, server: &str, default_tls: bool) -> Result<(String, String, Value), String> {
    let sec_in = get(q, &["security"]).to_ascii_lowercase();
    let sec = match sec_in.as_str() {
        "tls" | "xtls" => "tls",
        "reality" => "reality",
        "none" => "none",
        "" => if default_tls { "tls" } else { "none" },
        other => return Err(format!("security ناشناخته: {other}")),
    };
    let net_in = get(q, &["type", "net"]).to_ascii_lowercase();
    let host = get(q, &["host"]);
    let host1 = host.split(',').next().unwrap_or("").trim().to_string();
    let path = get(q, &["path"]);
    let mut st = Map::new();
    let net: &str = match net_in.as_str() {
        "" | "tcp" | "raw" | "none" => {
            if get(q, &["headerType"]).eq_ignore_ascii_case("http") {
                let hosts = csv(host);
                let paths: Vec<String> = if path.is_empty() { vec!["/".into()] } else { csv(path) };
                st.insert("tcpSettings".into(), json!({ "header": { "type": "http", "request": { "path": paths, "headers": { "Host": hosts } } } }));
            }
            "tcp"
        }
        "ws" | "websocket" => {
            // Xray reads "?ed=2048" from the path itself
            let mut ws = Map::new();
            ws.insert("path".into(), json!(if path.is_empty() { "/" } else { path }));
            if !host1.is_empty() {
                ws.insert("host".into(), json!(host1));
            }
            st.insert("wsSettings".into(), Value::Object(ws));
            "ws"
        }
        "grpc" | "gun" => {
            let mut g = Map::new();
            g.insert("serviceName".into(), json!(get(q, &["serviceName", "service_name", "path"]).trim_start_matches('/')));
            if get(q, &["mode"]).eq_ignore_ascii_case("multi") {
                g.insert("multiMode".into(), json!(true));
            }
            let auth = get(q, &["authority"]);
            if !auth.is_empty() {
                g.insert("authority".into(), json!(auth));
            }
            st.insert("grpcSettings".into(), Value::Object(g));
            "grpc"
        }
        "httpupgrade" => {
            let mut h = Map::new();
            h.insert("path".into(), json!(if path.is_empty() { "/" } else { path }));
            if !host1.is_empty() {
                h.insert("host".into(), json!(host1));
            }
            st.insert("httpupgradeSettings".into(), Value::Object(h));
            "httpupgrade"
        }
        "xhttp" | "splithttp" => {
            let mut x = Map::new();
            x.insert("path".into(), json!(if path.is_empty() { "/" } else { path }));
            if !host1.is_empty() {
                x.insert("host".into(), json!(host1));
            }
            let mode = get(q, &["mode"]);
            x.insert("mode".into(), json!(if mode.is_empty() { "auto" } else { mode }));
            if let Ok(extra @ Value::Object(_)) = serde_json::from_str::<Value>(get(q, &["extra"])) {
                x.insert("extra".into(), extra);
            }
            st.insert("xhttpSettings".into(), Value::Object(x));
            "xhttp"
        }
        // removed from Xray: sing-box handles these links
        "http" | "h2" | "quic" | "kcp" | "mkcp" => return Err(format!("NOT_XRAY: {net_in}")),
        other => return Err(format!("ترنسپورت ناشناخته: {other}")),
    };
    st.insert("network".into(), json!(net));
    st.insert("security".into(), json!(sec));

    if sec != "none" {
        let sni = [get(q, &["sni", "peer", "serverName"]).to_string(), host1.clone(), if is_ip(server) { String::new() } else { server.to_string() }]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or_default();
        let mut fp = get(q, &["fp", "fingerprint"]).to_ascii_lowercase();
        if fp.is_empty() || fp == "none" {
            fp = "chrome".into(); // uTLS chrome: what every Iranian client sends by default
        }
        let mut t = Map::new();
        if !sni.is_empty() {
            t.insert("serverName".into(), json!(sni));
        }
        t.insert("fingerprint".into(), json!(fp));
        if sec == "reality" {
            let pbk = get(q, &["pbk", "publicKey", "password"]);
            if pbk.is_empty() {
                return Err("REALITY بدون public key (pbk)".into());
            }
            if sni.is_empty() {
                return Err("REALITY بدون sni".into());
            }
            t.insert("publicKey".into(), json!(pbk));
            t.insert("shortId".into(), json!(get(q, &["sid", "shortId"])));
            let spx = get(q, &["spx", "spiderX"]);
            if !spx.is_empty() {
                t.insert("spiderX".into(), json!(spx));
            }
            let pqv = get(q, &["pqv", "mldsa65Verify"]);
            if !pqv.is_empty() {
                t.insert("mldsa65Verify".into(), json!(pqv));
            }
            st.insert("realitySettings".into(), Value::Object(t));
        } else {
            let mut alpn = csv(get(q, &["alpn"]));
            if net == "ws" || net == "httpupgrade" {
                alpn.retain(|a| a != "h2" && a != "h3");
            }
            if !alpn.is_empty() {
                t.insert("alpn".into(), json!(alpn));
            }
            // Xray removed "allowInsecure" (fatal since 2026-06-01). Links can carry the replacement
            // (pcs = pinned cert sha256, vcn = verify by name); otherwise sing-box takes the link.
            let pcs = get(q, &["pcs", "pinnedPeerCertSha256"]);
            let vcn = get(q, &["vcn", "verifyPeerCertByName"]);
            if !pcs.is_empty() {
                t.insert("pinnedPeerCertSha256".into(), json!(pcs));
            }
            if !vcn.is_empty() {
                t.insert("verifyPeerCertByName".into(), json!(vcn));
            }
            if truthy(get(q, &["allowInsecure", "insecure", "allow_insecure", "skip-cert-verify"])) && pcs.is_empty() && vcn.is_empty() {
                return Err("NOT_XRAY: insecure".into());
            }
            let ech = get(q, &["ech"]);
            if !ech.is_empty() {
                t.insert("echConfigList".into(), json!(ech));
            }
            st.insert("tlsSettings".into(), Value::Object(t));
        }
    }
    Ok((net.to_string(), sec.to_string(), Value::Object(st)))
}

fn detail(st: &Value, flow: &str, method: &str) -> Value {
    let t = if st["realitySettings"].is_object() { &st["realitySettings"] } else { &st["tlsSettings"] };
    let net = st["network"].as_str().unwrap_or("tcp");
    let (host, path) = match net {
        "ws" => (s_of(&st["wsSettings"]["host"]), s_of(&st["wsSettings"]["path"])),
        "httpupgrade" => (s_of(&st["httpupgradeSettings"]["host"]), s_of(&st["httpupgradeSettings"]["path"])),
        "xhttp" => (s_of(&st["xhttpSettings"]["host"]), s_of(&st["xhttpSettings"]["path"])),
        "grpc" => (s_of(&st["grpcSettings"]["authority"]), s_of(&st["grpcSettings"]["serviceName"])),
        _ => (
            st["tcpSettings"]["header"]["request"]["headers"]["Host"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default(),
            String::new(),
        ),
    };
    let alpn = t["alpn"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default();
    json!({
        "sni": s_of(&t["serverName"]),
        "fp": s_of(&t["fingerprint"]),
        "alpn": alpn,
        "insecure": false,
        "flow": flow,
        "hostHeader": host,
        "path": path,
        "earlyData": 0,
        "method": method,
        "mode": s_of(&st["xhttpSettings"]["mode"]),
    })
}

fn profile(proto: &str, host: String, port: u16, net: String, sec: String, name: String, raw: &str, ob: Value, det: Value) -> Profile {
    let name = if name.is_empty() { format!("{proto} · {host}:{port}") } else { name };
    Profile { name, proto: proto.into(), host, port, net, sec, raw: raw.trim().to_string(), detail: det, outbound: Value::Null, xray: ob, engine: "xray".into() }
}

/// Xray view of a share link. Errors starting with "NOT_XRAY" mean "fine, but sing-box's job".
pub fn parse(line: &str) -> Result<Profile, String> {
    let raw = line.trim();
    let (scheme, body) = raw.split_once("://").ok_or("لینک معتبر نیست")?;
    match scheme.to_ascii_lowercase().as_str() {
        "vless" => {
            let u = parse_uri(body)?;
            if u.user.is_empty() {
                return Err("UUID خالیه".into());
            }
            let port = port_of(&u.port_spec)?;
            let (net, sec, st) = stream(&u.q, &u.host, false)?;
            let flow = get(&u.q, &["flow"]).to_string();
            let enc = get(&u.q, &["encryption"]);
            let mut user = Map::new();
            user.insert("id".into(), json!(u.user));
            user.insert("encryption".into(), json!(if enc.is_empty() { "none" } else { enc }));
            if !flow.is_empty() {
                user.insert("flow".into(), json!(flow));
            }
            let ob = json!({ "protocol": "vless", "settings": { "vnext": [ { "address": u.host, "port": port, "users": [ Value::Object(user) ] } ] }, "streamSettings": st });
            let det = detail(&ob["streamSettings"], &flow, "");
            Ok(profile("vless", u.host, port, net, sec, u.name, raw, ob, det))
        }
        "vmess" => {
            let (b, frag) = body.split_once('#').unwrap_or((body, ""));
            let j = link::b64(b).and_then(|s| serde_json::from_str::<Value>(&s).ok());
            let (host, port, id, aid, scy, q, name) = match j {
                Some(j) => {
                    let s = |k: &str| match &j[k] {
                        Value::String(v) => v.trim().to_string(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => if *b { "1".into() } else { String::new() },
                        _ => String::new(),
                    };
                    let net = s("net");
                    let mut q = Q::new();
                    q.insert("type".into(), net.clone());
                    q.insert("headerType".into(), s("type"));
                    q.insert("host".into(), s("host"));
                    q.insert("path".into(), s("path"));
                    if net == "grpc" {
                        q.insert("serviceName".into(), s("path"));
                        q.insert("mode".into(), s("type"));
                    }
                    q.insert("security".into(), if s("tls").is_empty() { "none".into() } else { s("tls") });
                    for (k, jk) in [("sni", "sni"), ("alpn", "alpn"), ("fp", "fp"), ("allowInsecure", "allowInsecure"), ("pbk", "pbk"), ("sid", "sid"), ("spx", "spx"), ("pcs", "pcs"), ("vcn", "vcn")] {
                        q.insert(k.into(), s(jk));
                    }
                    let name = if !s("ps").is_empty() { s("ps") } else { link::pct_decode(frag) };
                    (s("add"), port_of(&s("port"))?, s("id"), s("aid").parse::<u32>().unwrap_or(0), s("scy"), q, name)
                }
                None => {
                    let u = parse_uri(body)?;
                    let port = port_of(&u.port_spec)?;
                    let scy = get(&u.q, &["encryption", "scy"]).to_string();
                    (u.host, port, u.user, 0, scy, u.q, u.name)
                }
            };
            if host.is_empty() || id.is_empty() {
                return Err("لینک vmess ناقصه".into());
            }
            let (net, sec, st) = stream(&q, &host, false)?;
            let ob = json!({ "protocol": "vmess", "settings": { "vnext": [ { "address": host, "port": port, "users": [ { "id": id, "alterId": aid, "security": if scy.is_empty() { "auto".to_string() } else { scy } } ] } ] }, "streamSettings": st });
            let det = detail(&ob["streamSettings"], "", "");
            Ok(profile("vmess", host, port, net, sec, name, raw, ob, det))
        }
        "trojan" => {
            let u = parse_uri(body)?;
            if u.user.is_empty() {
                return Err("پسورد trojan خالیه".into());
            }
            let port = port_of(&u.port_spec)?;
            let (net, sec, st) = stream(&u.q, &u.host, true)?;
            let flow = get(&u.q, &["flow"]).to_string();
            let mut srv = Map::new();
            srv.insert("address".into(), json!(u.host));
            srv.insert("port".into(), json!(port));
            srv.insert("password".into(), json!(u.user));
            let ob = json!({ "protocol": "trojan", "settings": { "servers": [ Value::Object(srv) ] }, "streamSettings": st });
            let det = detail(&ob["streamSettings"], &flow, "");
            Ok(profile("trojan", u.host, port, net, sec, u.name, raw, ob, det))
        }
        "ss" => {
            let (b, frag) = body.split_once('#').unwrap_or((body, ""));
            let (b, query) = b.split_once('?').unwrap_or((b, ""));
            let b = b.trim_end_matches('/');
            let full = if b.contains('@') { b.to_string() } else { link::b64(b).ok_or("لینک ss نامعتبره")? };
            let at = full.rfind('@').ok_or("لینک ss نامعتبره")?;
            let (ui, hp) = (&full[..at], full[at + 1..].trim_end_matches('/'));
            let ui = link::pct_decode(ui);
            let cred = if ui.contains(':') { ui } else { link::b64(&ui).ok_or("اطلاعات ss نامعتبره")? };
            let (method, pass) = cred.split_once(':').ok_or("method:password پیدا نشد")?;
            if !get(&parse_query(query), &["plugin"]).is_empty() {
                return Err("NOT_XRAY: ss plugin".into());
            }
            let u = parse_uri(hp)?;
            let port = port_of(&u.port_spec)?;
            let method = method.to_ascii_lowercase();
            let ob = json!({ "protocol": "shadowsocks", "settings": { "servers": [ { "address": u.host, "port": port, "method": method, "password": pass } ] }, "streamSettings": { "network": "tcp", "security": "none" } });
            let det = detail(&ob["streamSettings"], "", &method);
            Ok(profile("ss", u.host, port, "tcp".into(), "none".into(), link::pct_decode(frag).trim().to_string(), raw, ob, det))
        }
        "socks" | "socks5" => {
            let u = parse_uri(body)?;
            let port = port_of(&u.port_spec)?;
            let mut srv = Map::new();
            srv.insert("address".into(), json!(u.host));
            srv.insert("port".into(), json!(port));
            if !u.user.is_empty() {
                let cred = if u.user.contains(':') { u.user.clone() } else { link::b64(&u.user).unwrap_or_else(|| u.user.clone()) };
                let (a, b) = cred.split_once(':').unwrap_or((cred.as_str(), ""));
                srv.insert("users".into(), json!([{ "user": a, "pass": b }]));
            }
            let ob = json!({ "protocol": "socks", "settings": { "servers": [ Value::Object(srv) ] } });
            Ok(profile("socks", u.host, port, "tcp".into(), "none".into(), u.name, raw, ob, json!({})))
        }
        other => Err(format!("NOT_XRAY: {other}")),
    }
}

/// Server address of an Xray outbound.
pub fn server_of(ob: &Value) -> Option<String> {
    let s = &ob["settings"];
    s["vnext"][0]["address"].as_str().or(s["servers"][0]["address"].as_str()).map(String::from)
}

/// Replace the server domain by an already resolved IP, keeping SNI / Host headers on the domain.
/// Used in TUN mode: otherwise Xray would ask Windows DNS for its own server, Windows DNS is captured
/// by the tunnel, the tunnel needs Xray... and nothing ever connects.
pub fn pin_server_ip(ob: &mut Value, ip: &str) {
    let Some(domain) = server_of(ob).filter(|d| !is_ip(d)) else { return };
    // pointer_mut: IndexMut on a missing key would insert null / panic
    if let Some(a) = ob.pointer_mut("/settings/vnext/0/address") {
        *a = json!(ip);
    } else if let Some(a) = ob.pointer_mut("/settings/servers/0/address") {
        *a = json!(ip);
    }
    let Some(st) = ob.get_mut("streamSettings").and_then(|v| v.as_object_mut()) else { return };
    if let Some(Value::Object(t)) = st.get_mut("tlsSettings") {
        t.entry("serverName").or_insert(json!(domain));
    }
    for k in ["wsSettings", "httpupgradeSettings", "xhttpSettings"] {
        if let Some(Value::Object(m)) = st.get_mut(k) {
            m.entry("host").or_insert(json!(domain));
        }
    }
    if let Some(Value::Object(g)) = st.get_mut("grpcSettings") {
        g.entry("authority").or_insert(json!(domain));
    }
}

fn prep(ob: &Value, tag: &str, fragment: bool) -> Value {
    let mut o = ob.clone();
    if let Value::Object(m) = &mut o {
        m.insert("tag".into(), json!(tag));
        let proto = m.get("protocol").and_then(|p| p.as_str()).unwrap_or("").to_string();
        if proto != "socks" {
            let st = m.entry("streamSettings").or_insert(json!({}));
            if let Value::Object(st) = st {
                let so = st.entry("sockopt").or_insert(json!({}));
                if let Value::Object(so) = so {
                    // IPv4 first: broken IPv6 is the #1 cause of "connected but nothing loads" on Iranian ISPs
                    so.insert("domainStrategy".into(), json!("UseIPv4v6"));
                    if fragment {
                        so.insert("dialerProxy".into(), json!("frag"));
                    }
                }
            }
        }
    }
    o
}

fn freedom(tag: &str) -> Value {
    json!({ "protocol": "freedom", "tag": tag, "settings": { "domainStrategy": "UseIPv4v6" } })
}

/// Config for a real connection: one SOCKS inbound (it also speaks HTTP on the same port), the
/// server as the first (= default) outbound, stats over the metrics HTTP endpoint.
/// `dns`: plain DNS server IP for Xray's own lookups (TUN mode), None = system resolver.
pub fn build(outbound: &Value, o: &Opts, port: u16, metrics_port: u16, dns: Option<&str>) -> Value {
    let listen = if o.allow_lan { "0.0.0.0" } else { "127.0.0.1" };
    let mut rules = vec![json!({ "type": "field", "ip": PRIVATE, "outboundTag": "direct" })];
    if o.block_quic {
        rules.push(json!({ "type": "field", "network": "udp", "port": "443", "outboundTag": "block" }));
    }
    if o.bypass_iran {
        rules.push(json!({ "type": "field", "domain": ["domain:ir"], "outboundTag": "direct" }));
    }
    let mut outbounds = vec![prep(outbound, "proxy", o.fragment), freedom("direct"), json!({ "protocol": "blackhole", "tag": "block" })];
    if o.fragment {
        outbounds.push(json!({
            "protocol": "freedom", "tag": "frag",
            "settings": { "domainStrategy": "UseIPv4v6", "fragment": { "packets": "tlshello", "length": "100-200", "interval": "10-20" } }
        }));
    }
    let dns_servers: Vec<Value> = match dns {
        Some(ip) => vec![json!(ip), json!("localhost")],
        None => vec![json!("localhost")],
    };
    json!({
        "log": { "loglevel": "warning" },
        "dns": { "servers": dns_servers, "queryStrategy": "UseIPv4" },
        "stats": {},
        "policy": { "system": { "statsOutboundUplink": true, "statsOutboundDownlink": true } },
        "metrics": { "tag": "metrics", "listen": format!("127.0.0.1:{metrics_port}") },
        "inbounds": [{
            "tag": "in", "listen": listen, "port": port, "protocol": "socks",
            "settings": { "auth": "noauth", "udp": true },
            "sniffing": { "enabled": true, "destOverride": ["http", "tls", "quic"], "routeOnly": true }
        }],
        "outbounds": outbounds,
        "routing": { "domainStrategy": "AsIs", "rules": rules }
    })
}

/// Config for the "real delay" test: server i gets its own SOCKS/HTTP inbound on `port` -> outbound p{i}.
pub fn build_test(items: &[(usize, Value, u16)]) -> Value {
    let inbounds: Vec<Value> = items.iter().map(|(i, _, port)| json!({ "tag": format!("in{i}"), "listen": "127.0.0.1", "port": port, "protocol": "socks", "settings": { "auth": "noauth", "udp": false } })).collect();
    let mut outbounds: Vec<Value> = items.iter().map(|(i, ob, _)| prep(ob, &format!("p{i}"), false)).collect();
    outbounds.push(json!({ "protocol": "blackhole", "tag": "block" }));
    let rules: Vec<Value> = items.iter().map(|(i, _, _)| json!({ "type": "field", "inboundTag": [format!("in{i}")], "outboundTag": format!("p{i}") })).collect();
    json!({
        "log": { "loglevel": "error" },
        "dns": { "servers": ["localhost"], "queryStrategy": "UseIPv4" },
        "inbounds": inbounds,
        "outbounds": outbounds,
        "routing": { "domainStrategy": "AsIs", "rules": rules }
    })
}

/// "... failed to build outbound config with tag p12 ..." -> 12
pub fn bad_tag(err: &str) -> Option<usize> {
    let i = err.find("with tag p")? + "with tag p".len();
    let digits: String = err[i..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vless_reality_vision() {
        let p = parse("vless://11111111-2222-3333-4444-555555555555@1.2.3.4:443?encryption=none&flow=xtls-rprx-vision&security=reality&sni=www.speedtest.net&fp=chrome&pbk=abc&sid=6ba8&type=tcp#R").unwrap();
        assert_eq!(p.xray["settings"]["vnext"][0]["users"][0]["flow"], "xtls-rprx-vision");
        assert_eq!(p.xray["streamSettings"]["realitySettings"]["publicKey"], "abc");
        assert_eq!(p.xray["streamSettings"]["security"], "reality");
    }
    #[test]
    fn xhttp_and_ws() {
        let p = parse("vless://id@cdn.example.com:443?type=xhttp&security=tls&path=%2Fx&host=a.example.com&mode=packet-up#x").unwrap();
        assert_eq!(p.xray["streamSettings"]["network"], "xhttp");
        assert_eq!(p.xray["streamSettings"]["xhttpSettings"]["mode"], "packet-up");
        assert_eq!(p.xray["streamSettings"]["tlsSettings"]["serverName"], "a.example.com");
        let w = parse("vless://id@1.2.3.4:80?type=ws&path=%2Fws%3Fed%3D2048&host=h.com").unwrap();
        assert_eq!(w.xray["streamSettings"]["wsSettings"]["path"], "/ws?ed=2048");
        assert_eq!(w.xray["streamSettings"]["security"], "none");
    }
    #[test]
    fn insecure_goes_to_singbox() {
        assert!(parse("trojan://p@h.com:443?allowInsecure=1").unwrap_err().starts_with("NOT_XRAY"));
        assert!(parse("trojan://p@h.com:443?allowInsecure=1&pcs=AA").is_ok());
        assert!(parse("hysteria2://p@h.com:443").unwrap_err().starts_with("NOT_XRAY"));
    }
    #[test]
    fn pin_ip() {
        let mut p = parse("vless://id@cdn.example.com:443?type=ws&security=tls&path=%2F").unwrap();
        pin_server_ip(&mut p.xray, "5.6.7.8");
        assert_eq!(server_of(&p.xray).unwrap(), "5.6.7.8");
        assert_eq!(p.xray["streamSettings"]["tlsSettings"]["serverName"], "cdn.example.com");
        assert_eq!(p.xray["streamSettings"]["wsSettings"]["host"], "cdn.example.com");
        assert_eq!(bad_tag("infra/conf: failed to build outbound config with tag p12 > x"), Some(12));
    }
}
