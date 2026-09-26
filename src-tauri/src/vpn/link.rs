//! Share-link parser: vless / vmess / trojan / ss / hysteria2 / hysteria / tuic / socks
//! -> sing-box outbound JSON (+ Xray outbound, see `xray.rs`). Pure functions, no I/O (unit-tested at the bottom).
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::net::IpAddr;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub name: String,
    pub proto: String,
    pub host: String,
    pub port: u16,
    /// ws / grpc / tcp / h2 / httpupgrade / quic ...
    pub net: String,
    /// tls / reality / none
    pub sec: String,
    pub raw: String,
    /// sni / path / host header / fingerprint / alpn / flow ... (shown like v2rayN's columns)
    pub detail: Value,
    /// sing-box outbound (Null when sing-box can't speak this link, e.g. XHTTP)
    #[serde(skip)]
    pub outbound: Value,
    /// Xray outbound (Null when Xray can't speak this link, e.g. Hysteria2 / TUIC)
    #[serde(skip)]
    pub xray: Value,
    /// which core will be used when connecting: "xray" or "sing-box"
    pub engine: String,
}

fn detail_of(ob: &Value) -> Value {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let t = &ob["tls"];
    let tr = &ob["transport"];
    let kind = tr["type"].as_str().unwrap_or("");
    let host_hdr = match kind {
        "ws" => s(&tr["headers"]["Host"]),
        "httpupgrade" => s(&tr["host"]),
        "http" => tr["host"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default(),
        _ => String::new(),
    };
    let path = if kind == "grpc" { s(&tr["service_name"]) } else { s(&tr["path"]) };
    let alpn = t["alpn"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(",")).unwrap_or_default();
    json!({
        "sni": s(&t["server_name"]),
        "fp": s(&t["utls"]["fingerprint"]),
        "alpn": alpn,
        "insecure": t["insecure"].as_bool().unwrap_or(false),
        "flow": s(&ob["flow"]),
        "hostHeader": host_hdr,
        "path": path,
        "earlyData": tr["max_early_data"].as_u64().unwrap_or(0),
        "method": s(&ob["method"]),
    })
}

pub(super) type Q = HashMap<String, String>;

/// Lenient base64 (standard / url-safe, with or without padding, with whitespace).
pub fn b64(s: &str) -> Option<String> {
    let mut t: String = s.chars().filter(|c| !c.is_whitespace()).map(|c| match c { '-' => '+', '_' => '/', c => c }).collect();
    while t.ends_with('=') {
        t.pop();
    }
    while t.len() % 4 != 0 {
        t.push('=');
    }
    let bytes = STANDARD.decode(t.as_bytes()).ok()?;
    String::from_utf8(bytes).ok()
}

pub fn pct_decode(s: &str) -> String {
    fn hex(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn pct_encode(s: &str) -> String {
    let mut o = String::with_capacity(s.len() * 3);
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || b"-_.~".contains(&c) {
            o.push(c as char);
        } else {
            o.push_str(&format!("%{c:02X}"));
        }
    }
    o
}

/// Subscription body / pasted text -> list of link lines (decodes a base64 subscription).
pub fn split_text(text: &str) -> Vec<String> {
    let t = text.trim().trim_start_matches('\u{feff}');
    let decoded;
    let body = if !t.contains("://") {
        match b64(t) {
            Some(d) if d.contains("://") => {
                decoded = d;
                decoded.as_str()
            }
            _ => t,
        }
    } else {
        t
    };
    body.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))
        .map(String::from)
        .collect()
}

pub(super) struct Uri {
    pub user: String,
    pub host: String,
    pub port_spec: String,
    #[allow(dead_code)]
    pub path: String,
    pub q: Q,
    pub name: String,
}

pub(super) fn parse_query(s: &str) -> Q {
    let mut q = Q::new();
    for kv in s.split('&').filter(|x| !x.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        q.insert(pct_decode(k), pct_decode(v));
    }
    q
}

/// `scheme://[user@]host[:port][/path][?query][#name]` without the scheme. Hand-written because
/// real-world links break strict URL parsers (port ranges, raw `/` in passwords, IPv6, ...).
pub(super) fn parse_uri(body: &str) -> Result<Uri, String> {
    let (body, frag) = body.split_once('#').unwrap_or((body, ""));
    let (body, query) = body.split_once('?').unwrap_or((body, ""));
    // userinfo ends at the LAST '@' (passwords may contain '@' or '/')
    let (user, rest) = match body.rfind('@') {
        Some(i) => (&body[..i], &body[i + 1..]),
        None => ("", body),
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (host, port_spec) = if let Some(r) = hostport.strip_prefix('[') {
        let end = r.find(']').ok_or("IPv6 ناقص")?;
        (&r[..end], r[end + 1..].trim_start_matches(':'))
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) => (h, p),
            None => (hostport, ""),
        }
    };
    if host.is_empty() {
        return Err("آدرس سرور خالیه".into());
    }
    Ok(Uri {
        user: pct_decode(user),
        host: host.to_string(),
        port_spec: port_spec.to_string(),
        path: path.to_string(),
        q: parse_query(query),
        name: pct_decode(frag).trim().to_string(),
    })
}

pub(super) fn port_of(spec: &str) -> Result<u16, String> {
    let first = spec.split(|c| c == ',' || c == '-').next().unwrap_or("").trim();
    match first.parse::<u16>() {
        Ok(p) if p > 0 => Ok(p),
        _ => Err(format!("پورت نامعتبر: {spec}")),
    }
}

pub(super) fn get<'a>(q: &'a Q, keys: &[&str]) -> &'a str {
    keys.iter().find_map(|k| q.get(*k).map(|s| s.trim()).filter(|s| !s.is_empty())).unwrap_or("")
}

pub(super) fn truthy(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes")
}

pub(super) fn is_ip(h: &str) -> bool {
    h.parse::<IpAddr>().is_ok()
}

pub(super) fn csv(v: &str) -> Vec<String> {
    v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// TLS / REALITY block. `default_on`: trojan / hysteria / tuic always use TLS.
fn tls(q: &Q, server: &str, default_on: bool) -> Result<Option<Value>, String> {
    let sec = get(q, &["security"]).to_ascii_lowercase();
    let on = match sec.as_str() {
        "tls" | "xtls" | "reality" => true,
        "none" => false,
        "" => default_on,
        other => return Err(format!("security ناشناخته: {other}")),
    };
    if !on {
        return Ok(None);
    }
    let mut t = Map::new();
    t.insert("enabled".into(), json!(true));
    let host_hdr = get(q, &["host"]).split(',').next().unwrap_or("").trim().to_string();
    let sni = [get(q, &["sni", "peer", "serverName"]).to_string(), host_hdr, if is_ip(server) { String::new() } else { server.to_string() }]
        .into_iter()
        .find(|s| !s.is_empty());
    if let Some(s) = sni {
        t.insert("server_name".into(), json!(s));
    }
    if truthy(get(q, &["allowInsecure", "insecure", "allow_insecure", "skip-cert-verify"])) {
        t.insert("insecure".into(), json!(true));
    }
    let alpn = csv(get(q, &["alpn"]));
    if !alpn.is_empty() {
        t.insert("alpn".into(), json!(alpn));
    }
    let mut fp = get(q, &["fp", "fingerprint"]).to_ascii_lowercase();
    if sec == "reality" {
        let pbk = get(q, &["pbk", "publicKey"]);
        if pbk.is_empty() {
            return Err("REALITY بدون public key (pbk)".into());
        }
        if !t.contains_key("server_name") {
            return Err("REALITY بدون sni".into());
        }
        t.insert("reality".into(), json!({ "enabled": true, "public_key": pbk, "short_id": get(q, &["sid", "shortId"]) }));
        if fp.is_empty() {
            fp = "chrome".into(); // REALITY needs uTLS
        }
    }
    if !fp.is_empty() && fp != "none" {
        const OK: [&str; 10] = ["chrome", "firefox", "edge", "safari", "360", "qq", "ios", "android", "random", "randomized"];
        let fp = if OK.contains(&fp.as_str()) { fp } else { "chrome".to_string() };
        t.insert("utls".into(), json!({ "enabled": true, "fingerprint": fp }));
    }
    Ok(Some(Value::Object(t)))
}

/// V2Ray transport block (None = plain TCP).
fn transport(q: &Q) -> Result<(String, Option<Value>), String> {
    let net = get(q, &["type", "net"]).to_ascii_lowercase();
    let host = get(q, &["host"]);
    let path = get(q, &["path"]);
    let v = match net.as_str() {
        "" | "tcp" | "raw" | "none" => {
            if get(q, &["headerType"]).eq_ignore_ascii_case("http") {
                return Err("TCP با هدر HTTP تو sing-box پشتیبانی نمیشه".into());
            }
            return Ok(("tcp".into(), None));
        }
        "ws" | "websocket" => {
            let mut path = if path.is_empty() { "/".to_string() } else { path.to_string() };
            let mut m = Map::new();
            m.insert("type".into(), json!("ws"));
            // early data: "/path?ed=2048" (xray style) -> sing-box max_early_data
            let mut ed: Option<u32> = get(q, &["ed"]).parse().ok();
            if let Some((p, qs)) = path.clone().split_once('?') {
                let pq = parse_query(qs);
                if let Some(e) = pq.get("ed").and_then(|e| e.parse::<u32>().ok()) {
                    ed = Some(e);
                    let rest: Vec<String> = qs.split('&').filter(|kv| !kv.starts_with("ed=")).map(String::from).collect();
                    path = if rest.is_empty() { p.to_string() } else { format!("{p}?{}", rest.join("&")) };
                }
            }
            m.insert("path".into(), json!(path));
            if !host.is_empty() {
                m.insert("headers".into(), json!({ "Host": host }));
            }
            if let Some(e) = ed.filter(|e| *e > 0) {
                m.insert("max_early_data".into(), json!(e));
                m.insert("early_data_header_name".into(), json!("Sec-WebSocket-Protocol"));
            }
            Value::Object(m)
        }
        "grpc" | "gun" => json!({ "type": "grpc", "service_name": get(q, &["serviceName", "service_name", "path"]).trim_start_matches('/') }),
        "http" | "h2" => {
            let mut m = Map::new();
            m.insert("type".into(), json!("http"));
            let hosts = csv(host);
            if !hosts.is_empty() {
                m.insert("host".into(), json!(hosts));
            }
            if !path.is_empty() {
                m.insert("path".into(), json!(path));
            }
            Value::Object(m)
        }
        "httpupgrade" => {
            let mut m = Map::new();
            m.insert("type".into(), json!("httpupgrade"));
            if !host.is_empty() {
                m.insert("host".into(), json!(host));
            }
            m.insert("path".into(), json!(if path.is_empty() { "/" } else { path }));
            Value::Object(m)
        }
        "quic" => json!({ "type": "quic" }),
        "xhttp" | "splithttp" => return Err("XHTTP فقط با هسته‌ی Xray کار می‌کنه (sing-box پشتیبانی نمی‌کنه)".into()),
        "kcp" | "mkcp" => return Err("mKCP تو sing-box پشتیبانی نمیشه".into()),
        other => return Err(format!("ترنسپورت ناشناخته: {other}")),
    };
    Ok((net, Some(v)))
}

fn finish(mut ob: Map<String, Value>, mut t: Option<Value>, tr: Option<Value>) -> Value {
    // share links often carry alpn=h2,http/1.1 for every transport. WebSocket / HTTPUpgrade break when
    // TLS negotiates h2, gRPC breaks without it: keep only what the transport can speak.
    let kind = tr.as_ref().and_then(|v| v["type"].as_str()).unwrap_or("").to_string();
    if let Some(Value::Object(tm)) = t.as_mut() {
        if let Some(list) = tm.get("alpn").and_then(|a| a.as_array()).cloned() {
            let names: Vec<String> = list.iter().filter_map(|x| x.as_str().map(String::from)).collect();
            let keep: Vec<String> = match kind.as_str() {
                "ws" | "httpupgrade" => names.into_iter().filter(|n| n != "h2" && n != "h3").collect(),
                "grpc" if !names.iter().any(|n| n == "h2") => vec![],
                _ => names,
            };
            if keep.is_empty() {
                tm.remove("alpn");
            } else {
                tm.insert("alpn".into(), json!(keep));
            }
        }
    }
    if let Some(t) = t {
        ob.insert("tls".into(), t);
    }
    if let Some(tr) = tr {
        ob.insert("transport".into(), tr);
    }
    Value::Object(ob)
}

fn base(kind: &str, host: &str, port: u16) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("type".into(), json!(kind));
    m.insert("server".into(), json!(host));
    m.insert("server_port".into(), json!(port));
    m
}

fn sec_of(t: &Option<Value>) -> String {
    match t {
        Some(v) if v.get("reality").is_some() => "reality".into(),
        Some(_) => "tls".into(),
        None => "none".into(),
    }
}

/// Parse one share link for both cores. Xray is preferred (it is the reference implementation that
/// Iranian panels like 3x-ui / Marzban are built and tested against, and the only one with XHTTP);
/// sing-box is used for what Xray can't do (Hysteria2 / TUIC / Hysteria, h2, "insecure" TLS).
pub fn parse_link(line: &str) -> Result<Profile, String> {
    let sb = parse_singbox(line);
    let xr = super::xray::parse(line);
    match (sb, xr) {
        (Ok(mut p), Ok(x)) => {
            p.xray = x.xray;
            p.engine = "xray".into();
            // Xray knows more transports; its view of net/detail is the one used for the connection
            p.detail = x.detail;
            Ok(p)
        }
        (Ok(p), Err(_)) => Ok(p),
        (Err(_), Ok(x)) => Ok(x),
        (Err(e), Err(xe)) => Err(if xe.starts_with("NOT_XRAY") { e } else { xe }),
    }
}

/// sing-box view of a share link.
pub fn parse_singbox(line: &str) -> Result<Profile, String> {
    let raw = line.trim().to_string();
    let (scheme, body) = raw.split_once("://").ok_or("لینک معتبر نیست")?;
    let scheme = scheme.to_ascii_lowercase();
    let (proto, host, port, net, t, ob, name): (String, String, u16, String, Option<Value>, Value, String) = match scheme.as_str() {
        "vless" => {
            let u = parse_uri(body)?;
            if u.user.is_empty() {
                return Err("UUID خالیه".into());
            }
            let port = port_of(&u.port_spec)?;
            let t = tls(&u.q, &u.host, false)?;
            let (net, tr) = transport(&u.q)?;
            let mut ob = base("vless", &u.host, port);
            ob.insert("uuid".into(), json!(u.user));
            let flow = get(&u.q, &["flow"]);
            if !flow.is_empty() {
                ob.insert("flow".into(), json!(flow.trim_end_matches("-udp443")));
            }
            ob.insert("packet_encoding".into(), json!("xudp"));
            let sec = t.clone();
            ("vless".into(), u.host, port, net, sec, finish(ob, t, tr), u.name)
        }
        "vmess" => {
            let (b, frag) = body.split_once('#').unwrap_or((body, ""));
            match b64(b).and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
                Some(j) => {
                    let s = |k: &str| match &j[k] {
                        Value::String(v) => v.trim().to_string(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => if *b { "1".into() } else { String::new() },
                        _ => String::new(),
                    };
                    let host = s("add");
                    let port = port_of(&s("port"))?;
                    let id = s("id");
                    if host.is_empty() || id.is_empty() {
                        return Err("لینک vmess ناقصه".into());
                    }
                    let net = s("net");
                    let mut q = Q::new();
                    q.insert("type".into(), net.clone());
                    q.insert("headerType".into(), s("type"));
                    q.insert("host".into(), s("host"));
                    q.insert("path".into(), s("path"));
                    if net == "grpc" {
                        q.insert("serviceName".into(), s("path"));
                    }
                    q.insert("security".into(), if s("tls").is_empty() { "none".into() } else { s("tls") });
                    q.insert("sni".into(), s("sni"));
                    q.insert("alpn".into(), s("alpn"));
                    q.insert("fp".into(), s("fp"));
                    q.insert("allowInsecure".into(), s("allowInsecure"));
                    q.insert("pbk".into(), s("pbk"));
                    q.insert("sid".into(), s("sid"));
                    let t = tls(&q, &host, false)?;
                    let (net, tr) = transport(&q)?;
                    let mut ob = base("vmess", &host, port);
                    ob.insert("uuid".into(), json!(id));
                    let scy = s("scy");
                    ob.insert("security".into(), json!(if scy.is_empty() { "auto".to_string() } else { scy }));
                    ob.insert("alter_id".into(), json!(s("aid").parse::<u32>().unwrap_or(0)));
                    let name = if !s("ps").is_empty() { s("ps") } else { pct_decode(frag) };
                    let sec = t.clone();
                    ("vmess".into(), host, port, net, sec, finish(ob, t, tr), name)
                }
                None => {
                    // vmess://uuid@host:port?type=ws&security=tls  (URI form)
                    let u = parse_uri(body)?;
                    let port = port_of(&u.port_spec)?;
                    let t = tls(&u.q, &u.host, false)?;
                    let (net, tr) = transport(&u.q)?;
                    let mut ob = base("vmess", &u.host, port);
                    ob.insert("uuid".into(), json!(u.user));
                    let enc = get(&u.q, &["encryption", "scy"]);
                    ob.insert("security".into(), json!(if enc.is_empty() { "auto" } else { enc }));
                    let sec = t.clone();
                    ("vmess".into(), u.host, port, net, sec, finish(ob, t, tr), u.name)
                }
            }
        }
        "trojan" => {
            let u = parse_uri(body)?;
            if u.user.is_empty() {
                return Err("پسورد trojan خالیه".into());
            }
            let port = port_of(&u.port_spec)?;
            let t = tls(&u.q, &u.host, true)?;
            let (net, tr) = transport(&u.q)?;
            let mut ob = base("trojan", &u.host, port);
            ob.insert("password".into(), json!(u.user));
            let sec = t.clone();
            ("trojan".into(), u.host, port, net, sec, finish(ob, t, tr), u.name)
        }
        "ss" => {
            let (b, frag) = body.split_once('#').unwrap_or((body, ""));
            let (b, query) = b.split_once('?').unwrap_or((b, ""));
            let b = b.trim_end_matches('/');
            let full = if b.contains('@') { b.to_string() } else { b64(b).ok_or("لینک ss نامعتبره")? };
            let at = full.rfind('@').ok_or("لینک ss نامعتبره")?;
            let (ui, hp) = (&full[..at], full[at + 1..].trim_end_matches('/'));
            let ui = pct_decode(ui);
            let cred = if ui.contains(':') { ui } else { b64(&ui).ok_or("اطلاعات ss نامعتبره")? };
            let (method, pass) = cred.split_once(':').ok_or("method:password پیدا نشد")?;
            let u = parse_uri(hp)?;
            let port = port_of(&u.port_spec)?;
            let mut ob = base("shadowsocks", &u.host, port);
            ob.insert("method".into(), json!(method.to_ascii_lowercase()));
            ob.insert("password".into(), json!(pass));
            let q = parse_query(query);
            let plugin = get(&q, &["plugin"]);
            if !plugin.is_empty() {
                let (pn, opts) = plugin.split_once(';').unwrap_or((plugin, ""));
                let pn = match pn {
                    "obfs-local" | "simple-obfs" => "obfs-local",
                    "v2ray-plugin" => "v2ray-plugin",
                    other => return Err(format!("پلاگین ss پشتیبانی نمیشه: {other}")),
                };
                ob.insert("plugin".into(), json!(pn));
                ob.insert("plugin_opts".into(), json!(opts));
            }
            ("ss".into(), u.host, port, "tcp".into(), None, Value::Object(ob), pct_decode(frag).trim().to_string())
        }
        "hysteria2" | "hy2" => {
            let u = parse_uri(body)?;
            // "443", "443,20000-30000", "20000-30000" and/or ?mport=... (port hopping)
            let mut ports: Vec<String> = Vec::new();
            let mut single: Option<u16> = None;
            let mport = get(&u.q, &["mport"]).to_string();
            for part in u.port_spec.split(',').chain(mport.split(',')).map(str::trim).filter(|s| !s.is_empty()) {
                if let Some((a, b)) = part.split_once('-') {
                    let a = a.trim().parse::<u16>().map_err(|_| format!("رنج پورت نامعتبر: {part}"))?;
                    let b = b.trim().parse::<u16>().map_err(|_| format!("رنج پورت نامعتبر: {part}"))?;
                    ports.push(format!("{}:{}", a.min(b), a.max(b)));
                } else {
                    let p = part.parse::<u16>().map_err(|_| format!("پورت نامعتبر: {part}"))?;
                    match single {
                        None => single = Some(p),
                        Some(s0) if s0 != p => ports.push(format!("{p}:{p}")),
                        _ => {}
                    }
                }
            }
            if single.is_none() {
                single = ports.first().and_then(|r| r.split(':').next()).and_then(|x| x.parse().ok());
            }
            let port = single.filter(|p| *p > 0).ok_or("پورت hysteria2 پیدا نشد")?;
            let mut q = u.q.clone();
            q.entry("security".into()).or_insert_with(|| "tls".into());
            if get(&q, &["alpn"]).is_empty() {
                q.insert("alpn".into(), "h3".into());
            }
            q.remove("fp"); // QUIC: no uTLS
            let t = tls(&q, &u.host, true)?;
            let mut ob = base("hysteria2", &u.host, port);
            if !ports.is_empty() {
                ob.insert("server_ports".into(), json!(ports));
            }
            let pass = if u.user.is_empty() { get(&u.q, &["auth", "password"]).to_string() } else { u.user.clone() };
            ob.insert("password".into(), json!(pass));
            let obfs = get(&u.q, &["obfs"]);
            if !obfs.is_empty() && obfs != "none" {
                ob.insert("obfs".into(), json!({ "type": obfs, "password": get(&u.q, &["obfs-password", "obfs_password"]) }));
            }
            let up: u32 = get(&u.q, &["upmbps", "up"]).trim_end_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(0);
            let down: u32 = get(&u.q, &["downmbps", "down"]).trim_end_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(0);
            if up > 0 { ob.insert("up_mbps".into(), json!(up)); }
            if down > 0 { ob.insert("down_mbps".into(), json!(down)); }
            let sec = t.clone();
            ("hysteria2".into(), u.host, port, "quic".into(), sec, finish(ob, t, None), u.name)
        }
        "hysteria" => {
            let u = parse_uri(body)?;
            let port = port_of(&u.port_spec)?;
            let mut q = u.q.clone();
            q.insert("security".into(), "tls".into());
            q.remove("fp");
            let t = tls(&q, &u.host, true)?;
            let mut ob = base("hysteria", &u.host, port);
            ob.insert("up_mbps".into(), json!(get(&u.q, &["upmbps", "up"]).parse::<u32>().unwrap_or(10)));
            ob.insert("down_mbps".into(), json!(get(&u.q, &["downmbps", "down"]).parse::<u32>().unwrap_or(50)));
            let auth = get(&u.q, &["auth", "auth_str"]);
            if !auth.is_empty() {
                ob.insert("auth_str".into(), json!(auth));
            }
            let obfs = get(&u.q, &["obfsParam", "obfs-password"]);
            if !obfs.is_empty() {
                ob.insert("obfs".into(), json!(obfs));
            }
            let sec = t.clone();
            ("hysteria".into(), u.host, port, "quic".into(), sec, finish(ob, t, None), u.name)
        }
        "tuic" => {
            let u = parse_uri(body)?;
            let port = port_of(&u.port_spec)?;
            let (uuid, pass) = u.user.split_once(':').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((u.user.clone(), get(&u.q, &["password"]).to_string()));
            if uuid.is_empty() {
                return Err("UUID خالیه".into());
            }
            let mut q = u.q.clone();
            q.insert("security".into(), "tls".into());
            q.remove("fp");
            if get(&q, &["alpn"]).is_empty() {
                q.insert("alpn".into(), "h3".into());
            }
            let mut t = tls(&q, &u.host, true)?;
            if truthy(get(&u.q, &["disable_sni"])) {
                if let Some(Value::Object(m)) = t.as_mut() {
                    m.insert("disable_sni".into(), json!(true));
                }
            }
            let mut ob = base("tuic", &u.host, port);
            ob.insert("uuid".into(), json!(uuid));
            ob.insert("password".into(), json!(pass));
            let cc = get(&u.q, &["congestion_control", "congestion"]);
            ob.insert("congestion_control".into(), json!(if cc.is_empty() { "bbr" } else { cc }));
            let relay = get(&u.q, &["udp_relay_mode"]);
            if !relay.is_empty() {
                ob.insert("udp_relay_mode".into(), json!(relay));
            }
            let sec = t.clone();
            ("tuic".into(), u.host, port, "quic".into(), sec, finish(ob, t, None), u.name)
        }
        "socks" | "socks5" => {
            let u = parse_uri(body)?;
            let port = port_of(&u.port_spec)?;
            let mut ob = base("socks", &u.host, port);
            ob.insert("version".into(), json!("5"));
            if !u.user.is_empty() {
                let cred = if u.user.contains(':') { u.user.clone() } else { b64(&u.user).unwrap_or_else(|| u.user.clone()) };
                let (a, b) = cred.split_once(':').unwrap_or((cred.as_str(), ""));
                ob.insert("username".into(), json!(a));
                ob.insert("password".into(), json!(b));
            }
            ("socks".into(), u.host, port, "tcp".into(), None, Value::Object(ob), u.name)
        }
        "wireguard" | "wg" => return Err("WireGuard فعلاً پشتیبانی نمیشه".into()),
        "ssr" => return Err("ShadowsocksR تو sing-box پشتیبانی نمیشه".into()),
        other => return Err(format!("پروتکل ناشناخته: {other}")),
    };
    let sec = sec_of(&t);
    let name = if name.is_empty() { format!("{proto} · {host}:{port}") } else { name };
    let detail = detail_of(&ob);
    Ok(Profile { name, proto, host, port, net, sec, raw, detail, outbound: ob, xray: Value::Null, engine: "sing-box".into() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vless_reality() {
        let p = parse_link("vless://11111111-2222-3333-4444-555555555555@1.2.3.4:443?encryption=none&flow=xtls-rprx-vision&security=reality&sni=www.speedtest.net&fp=chrome&pbk=abcDEF123&sid=6ba8&type=tcp#My%20Server").unwrap();
        assert_eq!(p.name, "My Server");
        assert_eq!(p.sec, "reality");
        assert_eq!(p.outbound["tls"]["reality"]["public_key"], "abcDEF123");
        assert_eq!(p.outbound["tls"]["server_name"], "www.speedtest.net");
        assert_eq!(p.outbound["flow"], "xtls-rprx-vision");
        assert!(p.outbound.get("transport").is_none());
    }

    #[test]
    fn vless_ws_early_data() {
        let p = parse_link("vless://id@example.com:8443?type=ws&security=tls&path=%2Fws%3Fed%3D2048&host=cdn.example.com").unwrap();
        let tr = &p.outbound["transport"];
        assert_eq!(tr["type"], "ws");
        assert_eq!(tr["path"], "/ws");
        assert_eq!(tr["max_early_data"], 2048);
        assert_eq!(tr["headers"]["Host"], "cdn.example.com");
        assert_eq!(p.outbound["tls"]["server_name"], "cdn.example.com");
    }

    #[test]
    fn vmess_json() {
        let j = r#"{"v":"2","ps":"وی‌مس","add":"a.example.com","port":"2053","id":"uuid-x","aid":"0","scy":"auto","net":"grpc","type":"none","host":"","path":"svc","tls":"tls","sni":"","fp":"firefox"}"#;
        let link = format!("vmess://{}", STANDARD.encode(j));
        let p = parse_link(&link).unwrap();
        assert_eq!(p.name, "وی‌مس");
        assert_eq!(p.port, 2053);
        assert_eq!(p.outbound["transport"]["service_name"], "svc");
        assert_eq!(p.outbound["tls"]["server_name"], "a.example.com");
        assert_eq!(p.outbound["tls"]["utls"]["fingerprint"], "firefox");
    }

    #[test]
    fn ss_formats() {
        let a = parse_link(&format!("ss://{}@1.1.1.1:8388#a", STANDARD.encode("aes-256-gcm:pa@ss"))).unwrap();
        assert_eq!(a.outbound["password"], "pa@ss");
        let b = parse_link(&format!("ss://{}#b", STANDARD.encode("chacha20-ietf-poly1305:pw@2.2.2.2:443"))).unwrap();
        assert_eq!(b.host, "2.2.2.2");
        let c = parse_link("ss://2022-blake3-aes-128-gcm:a%2Bb%3D@[2001:db8::1]:443").unwrap();
        assert_eq!(c.outbound["password"], "a+b=");
        assert_eq!(c.host, "2001:db8::1");
    }

    #[test]
    fn hy2_ports() {
        let p = parse_link("hysteria2://pass@h.example.com:443,20000-30000/?obfs=salamander&obfs-password=x&insecure=1#hy").unwrap();
        assert_eq!(p.port, 443);
        assert_eq!(p.outbound["server_ports"][0], "20000:30000");
        assert_eq!(p.outbound["obfs"]["type"], "salamander");
        assert_eq!(p.outbound["tls"]["insecure"], true);
        assert_eq!(p.outbound["tls"]["alpn"][0], "h3");
    }

    #[test]
    fn trojan_tuic_errors() {
        let t = parse_link("trojan://p%40ss@t.example.com:443?type=grpc&serviceName=g#t").unwrap();
        assert_eq!(t.outbound["password"], "p@ss");
        assert_eq!(t.sec, "tls");
        let u = parse_link("tuic://uuid:pw@1.2.3.4:443?congestion_control=cubic&sni=x.com").unwrap();
        assert_eq!(u.outbound["congestion_control"], "cubic");
        assert!(parse_singbox("vless://id@h:443?type=xhttp").is_err());
        assert_eq!(parse_link("vless://id@h:443?type=xhttp&security=tls&sni=h").unwrap().engine, "xray");
        assert!(parse_link("vless://id@h:0").is_err());
        assert!(parse_link("hello").is_err());
    }

    #[test]
    fn alpn_fixups() {
        let p = parse_link("vless://id@a.com:443?type=ws&security=tls&alpn=h2%2Chttp%2F1.1&host=a.com").unwrap();
        assert_eq!(p.outbound["tls"]["alpn"], json!(["http/1.1"]));
        let p = parse_link("vless://id@a.com:443?type=ws&security=tls&alpn=h2").unwrap();
        assert!(p.outbound["tls"].get("alpn").is_none());
        let j = r#"{"v":"2","add":"a.com","port":443,"id":"u","net":"ws","tls":"tls","allowInsecure":true}"#;
        let p = parse_link(&format!("vmess://{}", STANDARD.encode(j))).unwrap();
        assert_eq!(p.outbound["tls"]["insecure"], true);
    }

    #[test]
    fn subscription_text() {
        let body = STANDARD.encode("vless://a@h:1#x\n\ntrojan://b@h:2#y\n");
        assert_eq!(split_text(&body).len(), 2);
        assert_eq!(pct_encode("https://a.b/c"), "https%3A%2F%2Fa.b%2Fc");
    }
}
