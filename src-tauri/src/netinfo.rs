//! Network info v0.3.1: local adapters (with Windows adapter description, MAC, speed, MTU, gateway),
//! Wi-Fi details, system proxy, public IP *through* the proxy/VPN and *direct*, ISP/ASN from several
//! geo services in parallel (merged), Iranian operator recognition by ASN, and VPN detection with reasons.
use crate::applog;
use crate::probe::{self, is_fake_ip};
use futures_util::future::join_all;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::net::{IpAddr, UdpSocket};
use std::time::Duration;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Iface {
    name: String,
    desc: Option<String>,
    ip: String,
    v6: bool,
    loopback: bool,
    netmask: Option<String>,
    primary: bool,
    vpn: bool,
    kind: String,
    mac: Option<String>,
    speed: Option<String>,
    mtu: Option<u32>,
    gateway: Option<String>,
}

#[derive(Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Wifi {
    ssid: Option<String>,
    signal: Option<String>,
    radio: Option<String>,
    channel: Option<String>,
    rate: Option<String>,
    auth: Option<String>,
}

#[derive(Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Geo {
    ip: Option<String>,
    isp: Option<String>,
    org: Option<String>,
    asn: Option<String>,
    country: Option<String>,
    country_code: Option<String>,
    region: Option<String>,
    city: Option<String>,
    timezone: Option<String>,
    proxy: Option<bool>,
    hosting: Option<bool>,
    mobile: Option<bool>,
    /// Persian name of a known Iranian operator (by ASN)
    operator_fa: Option<String>,
    /// iran-isp | iran-dc | datacenter | cdn | vpn | isp
    asn_kind: Option<String>,
    sources: Vec<String>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NetInfo {
    interfaces: Vec<Iface>,
    primary_ip: Option<String>,
    primary_iface: Option<String>,
    primary_desc: Option<String>,
    gateway: Option<String>,
    mac: Option<String>,
    link_speed: Option<String>,
    mtu: Option<u32>,
    wifi: Option<Wifi>,
    hotspot: Option<String>,
    // public, as seen by apps (through system proxy / VPN)
    public_ip: Option<String>,
    public_ip6: Option<String>,
    location: Option<String>,
    colo: Option<String>,
    warp: Option<String>,
    http: Option<String>,
    tls: Option<String>,
    isp: Option<String>,
    org: Option<String>,
    asn: Option<String>,
    operator_fa: Option<String>,
    asn_kind: Option<String>,
    country: Option<String>,
    country_code: Option<String>,
    region: Option<String>,
    city: Option<String>,
    timezone: Option<String>,
    proxy: Option<bool>,
    hosting: Option<bool>,
    mobile: Option<bool>,
    geo_sources: Vec<String>,
    // direct (bypassing the system proxy); differs from public_ip when a proxy client is on
    direct: Option<Geo>,
    system_proxy: Option<String>,
    pac_url: Option<String>,
    dns_servers: Vec<String>,
    vpn: bool,
    vpn_type: Option<String>,
    vpn_reasons: Vec<String>,
    cgnat: bool,
    notes: Vec<String>,
}

/* ------------------------------------------------------------------ ASN knowledge */

/// Iranian operators by ASN -> Persian name + whether it's a mobile network.
fn iran_asn(asn: u32) -> Option<(&'static str, bool)> {
    Some(match asn {
        44244 => ("ایرانسل", true),
        197207 => ("همراه اول", true),
        57218 => ("رایتل", true),
        58224 => ("مخابرات ایران (TCI)", false),
        12880 => ("زیرساخت (DCI/ITC)", false),
        48159 => ("زیرساخت (TIC)", false),
        49666 => ("زیرساخت (TIC)", false),
        31549 => ("شاتل", false),
        16322 => ("پارس آنلاین", false),
        43754 => ("آسیاتک", false),
        49100 => ("پیشگامان", false),
        25184 => ("افرانت", false),
        42337 => ("رسپینا", false),
        24631 => ("فناوا", false),
        39501 => ("صبانت", false),
        50810 => ("مبین‌نت", false),
        56402 => ("داده گستر عصر نوین (های‌وب)", false),
        205647 => ("پیشگامان", false),
        44889 => ("فرابوم", false),
        41881 => ("فن‌آوا", false),
        51074 => ("مبین‌نت", false),
        58085 => ("ابرآروان", false),
        202468 => ("ابرآروان", false),
        205585 => ("ابرآروان", false),
        43211 => ("ابرآروان", false),
        62442 => ("سامانه‌گستر سحابپرداز", false),
        34918 => ("پیشگامان", false),
        60976 => ("پارس‌آنلاین", false),
        31410 => ("فناپ", false),
        47262 => ("هایوب", false),
        57563 => ("ایرانسل (TD-LTE)", false),
        64422 => ("سیمای رایانه", false),
        39308 => ("ادسل", false),
        _ => return None,
    })
}

/// Datacenters / hosting commonly used for VPN servers.
fn dc_asn(asn: u32) -> Option<&'static str> {
    Some(match asn {
        13335 | 209242 => "Cloudflare",
        24940 | 213230 => "Hetzner",
        16276 => "OVH",
        14061 => "DigitalOcean",
        51167 => "Contabo",
        20473 => "Vultr (Choopa)",
        63949 => "Linode / Akamai",
        16509 | 14618 => "Amazon AWS",
        15169 | 396982 => "Google Cloud",
        8075 => "Microsoft Azure",
        31898 => "Oracle Cloud",
        9009 => "M247",
        60781 | 28753 => "Leaseweb",
        210644 => "Aeza",
        60068 => "Datacamp (CDN77)",
        47583 => "Hostinger",
        199524 => "G-Core",
        53667 => "FranTech (BuyVM)",
        62240 => "Clouvider",
        212238 => "Datacamp",
        202422 => "G-Core",
        44477 => "Stark Industries",
        200651 => "Flokinet",
        215311 => "Regxa",
        8100 => "QuadraNet",
        36352 => "ColoCrossing",
        35916 => "Multacom",
        396356 => "Latitude.sh",
        207713 => "Global Internet Solutions",
        49981 => "WorldStream",
        51396 => "Pfcloud",
        136787 => "TEFINCOM (NordVPN)",
        212513 => "Proton",
        209854 => "Surfshark",
        _ => return None,
    })
}

fn asn_num(s: &str) -> Option<u32> {
    let t = s.trim().trim_start_matches("AS").trim_start_matches("as");
    t.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

/* ------------------------------------------------------------------ interface heuristics */

const VPN_HINTS: &[&str] = &[
    "tun", "tap-", "tap0", "tap-windows", "wg", "wireguard", "wintun", "vpn", "v2ray", "xray", "clash", "sing-box", "singbox",
    "sing-tun", "nekoray", "nekobox", "hiddify", "outline", "openvpn", "ppp", "utun", "warp", "cloudflare", "tailscale", "zerotier",
    "mihomo", "psiphon", "proton", "nord", "expressvpn", "amnezia", "happ", "streisand", "fortinet", "forticlient", "anyconnect",
    "cisco", "pangp", "globalprotect", "juniper", "pulse", "check point", "sonicwall", "softether", "kerio", "windscribe",
    "surfshark", "hamachi", "radmin", "l2tp", "sstp", "ikev2", "wan miniport", "hotspot shield", "cyberghost", "mullvad",
];

fn looks_vpn(text: &str) -> bool {
    let n = text.to_lowercase();
    if ["teredo", "isatap", "6to4", "loopback", "ip-https", "kernel debug"].iter().any(|x| n.contains(x)) {
        return false;
    }
    VPN_HINTS.iter().any(|h| n.contains(h))
}

fn kind_of(name: &str, desc: &str, loopback: bool, vpn: bool) -> String {
    let n = format!("{} {}", name, desc).to_lowercase();
    let k = if loopback {
        "لوپ‌بک"
    } else if vpn {
        "VPN / تونل"
    } else if n.contains("wi-fi") || n.contains("wifi") || n.contains("wlan") || n.contains("wireless") || n.contains("802.11") || name.to_lowercase().starts_with("wl") {
        "Wi‑Fi"
    } else if n.contains("vethernet") || n.contains("docker") || n.contains("virtual") || n.contains("vmware") || n.contains("vbox") || n.contains("hyper-v") || n.starts_with("br-") || n.starts_with("veth") {
        "مجازی"
    } else if n.contains("rndis") || n.contains("remote ndis") || n.contains("usb") && n.contains("ether") {
        "USB تترینگ گوشی"
    } else if n.contains("bluetooth") {
        "بلوتوث"
    } else if n.contains("cellular") || n.contains("mobile") || n.contains("wwan") || n.contains("rmnet") || n.contains("mbim") {
        "مودم موبایل"
    } else if n.contains("ethernet") || n.starts_with("eth") || n.starts_with("en") || n.contains("lan") || n.contains("gbe") || n.contains("realtek") || n.contains("intel") {
        "اترنت"
    } else {
        "دیگر"
    };
    k.to_string()
}

/// Local IP of the default route (no packet is sent).
fn primary_local_ip() -> Option<IpAddr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    s.local_addr().ok().map(|a| a.ip())
}

/// Windows placeholder DNS (fec0:0:0:ffff::1..3) shows up on every adapter without IPv6 DNS: noise.
fn junk_dns(s: &str) -> bool {
    s.starts_with("fec0:0:0:ffff::") || s == "0.0.0.0" || s.is_empty()
}

/* ------------------------------------------------------------------ Windows details (PowerShell) */

#[derive(Default)]
struct WinInfo {
    adapters: HashMap<String, (Option<String>, Option<String>, Option<String>, bool)>, // alias -> (desc, mac, speed, hardware)
    mtu: HashMap<String, u32>,
    gateways: HashMap<String, String>,
    default_alias: Option<String>,
    split_default_alias: Option<String>, // alias holding 0.0.0.0/1 + 128.0.0.0/1 (full-tunnel VPN)
    dns_by_alias: Vec<(String, Vec<String>)>,
    proxy: Option<String>,
    pac: Option<String>,
    wifi: Option<Wifi>,
}

#[cfg(windows)]
fn run_hidden(cmd: &str, args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new(cmd).args(args).creation_flags(CREATE_NO_WINDOW).output().ok()?;
    if !out.status.success() && out.stdout.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(windows)]
const PS_SCRIPT: &str = r#"
$ErrorActionPreference='SilentlyContinue'
[Console]::OutputEncoding=[Text.Encoding]::UTF8
$ad=@(Get-NetAdapter | Where-Object {$_.Status -eq 'Up'} | Select-Object Name,InterfaceDescription,ifIndex,MacAddress,LinkSpeed,HardwareInterface)
$mtu=@(Get-NetIPInterface -AddressFamily IPv4 | Select-Object InterfaceAlias,NlMtu,InterfaceMetric)
$rt=@(Get-NetRoute -AddressFamily IPv4 | Where-Object {$_.DestinationPrefix -in '0.0.0.0/0','0.0.0.0/1','128.0.0.0/1'} | Select-Object InterfaceAlias,DestinationPrefix,NextHop,RouteMetric,@{n='IfMetric';e={(Get-NetIPInterface -InterfaceIndex $_.ifIndex -AddressFamily IPv4).InterfaceMetric}})
$dns=@(Get-DnsClientServerAddress | Where-Object {$_.ServerAddresses.Count -gt 0} | Select-Object InterfaceAlias,ServerAddresses)
$px=Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings' | Select-Object ProxyEnable,ProxyServer,AutoConfigURL
@{adapters=$ad;mtu=$mtu;routes=$rt;dns=$dns;proxy=$px} | ConvertTo-Json -Depth 4 -Compress
"#;

#[cfg(windows)]
fn win_info() -> WinInfo {
    let mut w = WinInfo::default();
    let Some(txt) = run_hidden("powershell.exe", &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", PS_SCRIPT]) else {
        applog::warn("netinfo", "PowerShell اجرا نشد؛ جزئیات آداپتورها در دسترس نیست");
        return w;
    };
    let v: Value = match serde_json::from_str(txt.trim().trim_start_matches('\u{feff}')) {
        Ok(v) => v,
        Err(e) => {
            applog::warn("netinfo", format!("خروجی PowerShell خونده نشد: {e}"));
            return w;
        }
    };
    let arr = |x: &Value| -> Vec<Value> { match x { Value::Array(a) => a.clone(), Value::Null => vec![], o => vec![o.clone()] } };
    let s = |x: &Value| x.as_str().filter(|s| !s.is_empty()).map(String::from);
    for a in arr(&v["adapters"]) {
        if let Some(name) = s(&a["Name"]) {
            w.adapters.insert(name, (s(&a["InterfaceDescription"]), s(&a["MacAddress"]).map(|m| m.replace('-', ":")), s(&a["LinkSpeed"]), a["HardwareInterface"].as_bool().unwrap_or(false)));
        }
    }
    for m in arr(&v["mtu"]) {
        if let (Some(n), Some(mtu)) = (s(&m["InterfaceAlias"]), m["NlMtu"].as_u64()) {
            w.mtu.insert(n, mtu as u32);
        }
    }
    let mut best: Option<(u64, String)> = None;
    let mut half: HashMap<String, u8> = HashMap::new();
    for r in arr(&v["routes"]) {
        let (Some(alias), Some(prefix)) = (s(&r["InterfaceAlias"]), s(&r["DestinationPrefix"])) else { continue };
        let hop = s(&r["NextHop"]).unwrap_or_default();
        if prefix == "0.0.0.0/0" {
            let metric = r["RouteMetric"].as_u64().unwrap_or(0) + r["IfMetric"].as_u64().unwrap_or(0);
            if hop != "0.0.0.0" {
                w.gateways.entry(alias.clone()).or_insert(hop);
            }
            if best.as_ref().map(|(m, _)| metric < *m).unwrap_or(true) {
                best = Some((metric, alias));
            }
        } else {
            *half.entry(alias).or_default() += 1;
        }
    }
    w.default_alias = best.map(|b| b.1);
    w.split_default_alias = half.into_iter().find(|(_, n)| *n >= 2).map(|(a, _)| a);
    for d in arr(&v["dns"]) {
        if let Some(alias) = s(&d["InterfaceAlias"]) {
            let list: Vec<String> = arr(&d["ServerAddresses"]).iter().filter_map(|x| x.as_str().map(String::from)).filter(|x| !junk_dns(x)).collect();
            if !list.is_empty() {
                w.dns_by_alias.push((alias, list));
            }
        }
    }
    let px = &v["proxy"];
    if px["ProxyEnable"].as_u64() == Some(1) {
        w.proxy = s(&px["ProxyServer"]);
    }
    w.pac = s(&px["AutoConfigURL"]);

    // Wi-Fi (English field names; localized Windows just won't show this block)
    if let Some(t) = run_hidden("netsh", &["wlan", "show", "interfaces"]) {
        let mut wf = Wifi::default();
        for line in t.lines() {
            let Some((k, val)) = line.split_once(':') else { continue };
            let (k, val) = (k.trim(), val.trim().to_string());
            match k {
                "SSID" => wf.ssid = Some(val),
                "Signal" => wf.signal = Some(val),
                "Radio type" => wf.radio = Some(val),
                "Channel" => wf.channel = Some(val),
                "Receive rate (Mbps)" => wf.rate = Some(format!("{val} Mbps")),
                "Authentication" => wf.auth = Some(val),
                _ => {}
            }
        }
        if wf.ssid.is_some() {
            w.wifi = Some(wf);
        }
    }
    w
}

#[cfg(not(windows))]
fn win_info() -> WinInfo {
    let mut w = WinInfo::default();
    for k in ["https_proxy", "HTTPS_PROXY", "http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"] {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                w.proxy = Some(v);
                break;
            }
        }
    }
    w
}

/* ------------------------------------------------------------------ public IP / geo */

async fn trace(client: &reqwest::Client, url: &str) -> Option<Vec<(String, String)>> {
    let text = match client.get(url).send().await.and_then(|r| r.error_for_status()) {
        Ok(r) => r.text().await.ok()?,
        Err(e) => {
            applog::warn("netinfo", format!("{url}: {}", probe::err_chain(&e)));
            return None;
        }
    };
    let kv: Vec<(String, String)> = text.lines().filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))).collect();
    if kv.iter().any(|(k, _)| k == "ip") { Some(kv) } else { None }
}

async fn get_json(client: &reqwest::Client, url: &str, tag: &str) -> Option<Value> {
    match client.get(url).send().await.and_then(|r| r.error_for_status()) {
        Ok(r) => match r.json::<Value>().await {
            Ok(v) => Some(v),
            Err(e) => {
                applog::warn("netinfo", format!("{tag}: پاسخ نامعتبر ({e})"));
                None
            }
        },
        Err(e) => {
            applog::warn("netinfo", format!("{tag}: {}", probe::err_chain(&e)));
            None
        }
    }
}

fn js(v: &Value) -> Option<String> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(String::from)
}

/// One geo provider, normalised.
async fn provider(client: reqwest::Client, which: &'static str, ip: Option<String>) -> Option<Geo> {
    let ipq = ip.clone().unwrap_or_default();
    let mut g = Geo { sources: vec![which.to_string()], ..Default::default() };
    match which {
        "ip-api" => {
            let v = get_json(&client, &format!("http://ip-api.com/json/{ipq}?fields=status,message,query,country,countryCode,regionName,city,isp,org,as,timezone,proxy,hosting,mobile"), which).await?;
            if v["status"].as_str() != Some("success") {
                applog::warn("netinfo", format!("ip-api: {}", v["message"].as_str().unwrap_or("fail")));
                return None;
            }
            g.ip = js(&v["query"]);
            g.country = js(&v["country"]);
            g.country_code = js(&v["countryCode"]);
            g.region = js(&v["regionName"]);
            g.city = js(&v["city"]);
            g.isp = js(&v["isp"]);
            g.org = js(&v["org"]);
            g.asn = js(&v["as"]).and_then(|a| a.split_whitespace().next().map(String::from));
            g.timezone = js(&v["timezone"]);
            g.proxy = v["proxy"].as_bool();
            g.hosting = v["hosting"].as_bool();
            g.mobile = v["mobile"].as_bool();
        }
        "ipwho.is" => {
            let v = get_json(&client, &format!("https://ipwho.is/{ipq}"), which).await?;
            if v["success"].as_bool() != Some(true) {
                return None;
            }
            g.ip = js(&v["ip"]);
            g.country = js(&v["country"]);
            g.country_code = js(&v["country_code"]);
            g.region = js(&v["region"]);
            g.city = js(&v["city"]);
            g.isp = js(&v["connection"]["isp"]);
            g.org = js(&v["connection"]["org"]);
            g.asn = v["connection"]["asn"].as_u64().map(|a| format!("AS{a}"));
            g.timezone = js(&v["timezone"]["id"]);
        }
        "ipinfo" => {
            let url = if ipq.is_empty() { "https://ipinfo.io/json".to_string() } else { format!("https://ipinfo.io/{ipq}/json") };
            let v = get_json(&client, &url, which).await?;
            g.ip = js(&v["ip"]);
            g.ip.as_ref()?;
            g.city = js(&v["city"]);
            g.region = js(&v["region"]);
            g.country_code = js(&v["country"]);
            g.timezone = js(&v["timezone"]);
            if let Some(org) = js(&v["org"]) {
                match org.split_once(' ') {
                    Some((a, name)) if a.starts_with("AS") => {
                        g.asn = Some(a.to_string());
                        g.isp = Some(name.to_string());
                    }
                    _ => g.isp = Some(org),
                }
            }
        }
        "ipapi.co" => {
            let url = if ipq.is_empty() { "https://ipapi.co/json/".to_string() } else { format!("https://ipapi.co/{ipq}/json/") };
            let v = get_json(&client, &url, which).await?;
            if v["error"].as_bool() == Some(true) {
                return None;
            }
            g.ip = js(&v["ip"]);
            g.city = js(&v["city"]);
            g.region = js(&v["region"]);
            g.country = js(&v["country_name"]);
            g.country_code = js(&v["country_code"]);
            g.timezone = js(&v["timezone"]);
            g.isp = js(&v["org"]);
            g.asn = js(&v["asn"]);
        }
        "ifconfig.co" => {
            if !ipq.is_empty() {
                return None;
            }
            let v = get_json(&client, "https://ifconfig.co/json", which).await?;
            g.ip = js(&v["ip"]);
            g.country = js(&v["country"]);
            g.country_code = js(&v["country_iso"]);
            g.city = js(&v["city"]);
            g.region = js(&v["region_name"]);
            g.timezone = js(&v["time_zone"]);
            g.isp = js(&v["asn_org"]);
            g.asn = js(&v["asn"]);
        }
        "ipify" => {
            if !ipq.is_empty() {
                return None;
            }
            let v = get_json(&client, "https://api.ipify.org?format=json", which).await?;
            g.ip = js(&v["ip"]);
            g.ip.as_ref()?;
        }
        _ => return None,
    }
    Some(g)
}

fn merge(into: &mut Geo, g: Geo) {
    macro_rules! take { ($($f:ident),*) => { $( if into.$f.is_none() { into.$f = g.$f.clone(); } )* } }
    take!(ip, isp, org, asn, country, country_code, region, city, timezone, proxy, hosting, mobile);
    into.sources.extend(g.sources);
}

/// Query every provider in parallel, keep answers that agree with the majority IP, merge by priority.
async fn geo_all(client: &reqwest::Client, ip: Option<String>) -> Geo {
    const ORDER: [&str; 6] = ["ip-api", "ipwho.is", "ipinfo", "ipapi.co", "ifconfig.co", "ipify"];
    let res: Vec<Option<Geo>> = join_all(ORDER.iter().map(|w| provider(client.clone(), *w, ip.clone()))).await;
    let got: Vec<Geo> = res.into_iter().flatten().collect();
    // majority IP (providers behind different routes can disagree when split-tunnelling)
    let mut count: HashMap<String, usize> = HashMap::new();
    for g in &got {
        if let Some(i) = &g.ip {
            *count.entry(i.clone()).or_default() += 1;
        }
    }
    let major = ip.or_else(|| count.into_iter().max_by_key(|(_, n)| *n).map(|(i, _)| i));
    let mut out = Geo::default();
    for g in got {
        if major.is_none() || g.ip.is_none() || g.ip == major {
            merge(&mut out, g);
        }
    }
    if out.ip.is_none() {
        out.ip = major;
    }
    classify(&mut out);
    out
}

fn classify(g: &mut Geo) {
    let Some(n) = g.asn.as_deref().and_then(asn_num) else { return };
    g.asn = Some(format!("AS{n}"));
    if let Some((fa, mobile)) = iran_asn(n) {
        g.operator_fa = Some(fa.to_string());
        g.asn_kind = Some(if fa.contains("ابرآروان") || fa.contains("های‌وب") || fa.contains("هایوب") { "iran-dc" } else { "iran-isp" }.to_string());
        if mobile {
            g.mobile = Some(true);
        } else if g.mobile.is_none() {
            g.mobile = Some(false);
        }
    } else if let Some(dc) = dc_asn(n) {
        g.asn_kind = Some(if n == 13335 || n == 209242 { "cdn" } else { "datacenter" }.to_string());
        if g.org.is_none() {
            g.org = Some(dc.to_string());
        }
        if g.hosting.is_none() {
            g.hosting = Some(true);
        }
    } else if g.hosting == Some(true) {
        g.asn_kind = Some("datacenter".into());
    } else {
        g.asn_kind = Some("isp".into());
    }
}

/* ------------------------------------------------------------------ command */

#[tauri::command]
pub async fn network_info() -> Result<NetInfo, String> {
    let mut info = NetInfo::default();
    let win = tauri::async_runtime::spawn_blocking(win_info).await.unwrap_or_default();

    // ---- local ----
    let primary = primary_local_ip();
    info.primary_ip = primary.map(|i| i.to_string());
    for i in if_addrs::get_if_addrs().map_err(|e| e.to_string())? {
        let ip = i.ip();
        let loopback = i.is_loopback();
        let netmask = match &i.addr {
            if_addrs::IfAddr::V4(a) => Some(a.netmask.to_string()),
            if_addrs::IfAddr::V6(a) => Some(a.netmask.to_string()),
        };
        let (desc, mac, speed, hardware) = win.adapters.get(&i.name).cloned().unwrap_or((None, None, None, false));
        let vpn = !loopback && (looks_vpn(&i.name) || desc.as_deref().map(looks_vpn).unwrap_or(false)) && !hardware;
        let is_primary = Some(ip) == primary;
        let kind = kind_of(&i.name, desc.as_deref().unwrap_or(""), loopback, vpn);
        if is_primary {
            info.primary_iface = Some(i.name.clone());
            info.primary_desc = desc.clone();
            info.mac = mac.clone();
            info.link_speed = speed.clone();
            info.mtu = win.mtu.get(&i.name).copied();
            info.gateway = win.gateways.get(&i.name).cloned();
        }
        if let IpAddr::V4(v4) = ip {
            let o = v4.octets();
            if is_primary && o[0] == 100 && (64..=127).contains(&o[1]) {
                info.cgnat = true;
            }
        }
        let mtu = win.mtu.get(&i.name).copied();
        let gateway = win.gateways.get(&i.name).cloned();
        info.interfaces.push(Iface { name: i.name, desc, ip: ip.to_string(), v6: ip.is_ipv6(), loopback, netmask, primary: is_primary, vpn, kind, mac, speed, mtu, gateway });
    }
    info.interfaces.sort_by_key(|i| (!i.primary, i.loopback, i.v6));
    info.wifi = win.wifi.clone();
    info.system_proxy = win.proxy.clone();
    info.pac_url = win.pac.clone();

    // hotspot / tethering by gateway
    let gw = info.gateway.clone().or_else(|| win.default_alias.as_ref().and_then(|a| win.gateways.get(a).cloned()));
    info.hotspot = match gw.as_deref() {
        Some("192.168.43.1") | Some("192.168.42.129") => Some("هات‌اسپات / تترینگ گوشی اندروید".into()),
        Some("172.20.10.1") => Some("هات‌اسپات آیفون".into()),
        _ if info.primary_desc.as_deref().map(|d| d.to_lowercase().contains("rndis")).unwrap_or(false) => Some("تترینگ USB گوشی".into()),
        _ => None,
    };
    if info.gateway.is_none() {
        info.gateway = gw;
    }

    // DNS: prefer the primary adapter's servers first, drop Windows placeholders
    if let Some(pa) = &info.primary_iface {
        if let Some((_, l)) = win.dns_by_alias.iter().find(|(a, _)| a == pa) {
            info.dns_servers.extend(l.iter().cloned());
        }
    }
    for (_, l) in &win.dns_by_alias {
        for d in l {
            if !info.dns_servers.contains(d) {
                info.dns_servers.push(d.clone());
            }
        }
    }
    if info.dns_servers.is_empty() {
        match hickory_resolver::system_conf::read_system_conf() {
            Ok((cfg, _)) => {
                for ns in cfg.name_servers() {
                    let s = ns.socket_addr.ip().to_string();
                    if !junk_dns(&s) && !info.dns_servers.contains(&s) {
                        info.dns_servers.push(s);
                    }
                }
            }
            Err(e) => applog::warn("netinfo", format!("DNS سیستم خونده نشد: {e}")),
        }
    }

    // ---- public: through system proxy (what apps see) + direct (no proxy) ----
    let client = probe::client(Duration::from_secs(6))?;
    let direct_client = reqwest::Client::builder()
        .user_agent(probe::UA)
        .timeout(Duration::from_secs(6))
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;

    let t4_fut = async {
        for url in ["https://1.1.1.1/cdn-cgi/trace", "https://speed.cloudflare.com/cdn-cgi/trace", "https://www.cloudflare.com/cdn-cgi/trace", "https://1.0.0.1/cdn-cgi/trace"] {
            if let Some(t) = trace(&client, url).await {
                return Some(t);
            }
        }
        None
    };
    let (t4, t6, geo) = tokio::join!(t4_fut, trace(&client, "https://[2606:4700:4700::1111]/cdn-cgi/trace"), geo_all(&client, None));
    for (k, v) in t4.clone().unwrap_or_default() {
        match k.as_str() {
            "ip" if v.contains(':') => info.public_ip6 = Some(v),
            "ip" => info.public_ip = Some(v),
            "loc" => info.location = Some(v),
            "colo" => info.colo = Some(v),
            "warp" => info.warp = Some(v),
            "http" => info.http = Some(v),
            "tls" => info.tls = Some(v),
            _ => {}
        }
    }
    for (k, v) in t6.unwrap_or_default() {
        if k == "ip" && v.contains(':') {
            info.public_ip6 = Some(v);
        }
    }
    if info.public_ip.is_none() {
        info.public_ip = geo.ip.clone().filter(|i| !i.contains(':'));
    }
    // if trace and geo saw different exit IPs (split routing), re-query geo for the trace IP
    let alt = match (&info.public_ip, &geo.ip) {
        (Some(a), Some(b)) if a != b && !b.contains(':') => Some((a.clone(), b.clone())),
        _ => None,
    };
    let geo = if let Some((a, b)) = alt {
        applog::warn("netinfo", format!("IP خروجی متفاوت دیده شد: Cloudflare {a} / سرویس موقعیت {b} (Split tunnel؟)"));
        info.notes.push(format!("سرویس‌های مختلف IPهای متفاوتی دیدن ({a} و {b})؛ احتمالاً Split tunneling یا چند مسیر خروجی داری"));
        geo_all(&client, Some(a)).await
    } else {
        geo
    };
    if geo.sources.is_empty() && t4.is_none() {
        applog::error("netinfo", "هیچ سرویس IP/موقعیتی جواب نداد (اینترنت قطعه یا پروکسی سیستم خرابه)");
    }
    info.isp = geo.isp.clone();
    info.org = geo.org.clone().filter(|o| Some(o) != geo.isp.as_ref());
    info.asn = geo.asn.clone();
    info.operator_fa = geo.operator_fa.clone();
    info.asn_kind = geo.asn_kind.clone();
    info.country = geo.country.clone();
    info.country_code = geo.country_code.clone().or_else(|| info.location.clone());
    info.region = geo.region.clone();
    info.city = geo.city.clone();
    info.timezone = geo.timezone.clone();
    info.proxy = geo.proxy;
    info.hosting = geo.hosting;
    info.mobile = geo.mobile;
    info.geo_sources = geo.sources.clone();

    // direct IP only matters when a proxy is set (with TUN both are the same anyway)
    if info.system_proxy.is_some() || info.pac_url.is_some() {
        let d = geo_all(&direct_client, None).await;
        if d.ip.is_some() {
            info.direct = Some(d);
        }
    }

    // ---- VPN detection (independent signals) ----
    let mut reasons = Vec::new();
    let mut vpn_type: Option<&str> = None;
    for i in info.interfaces.iter().filter(|i| i.vpn) {
        let r = format!("آداپتور تونل فعال: {}{}", i.name, i.desc.as_deref().map(|d| format!(" ({d})")).unwrap_or_default());
        if !reasons.contains(&r) {
            reasons.push(r);
        }
    }
    let primary_is_vpn = info.interfaces.iter().any(|i| i.primary && i.vpn);
    if primary.map(|p| is_fake_ip(&p)).unwrap_or(false) || primary_is_vpn {
        reasons.push("مسیر پیش‌فرض اینترنت از آداپتور تونل رد میشه (حالت TUN / VPN کامل)".into());
        vpn_type = Some("TUN / VPN کامل");
    }
    if let Some(a) = &win.split_default_alias {
        reasons.push(format!("روت‌های 0.0.0.0/1 و 128.0.0.0/1 روی «{a}»: کل ترافیک از VPN رد میشه (OpenVPN/WireGuard)"));
        vpn_type.get_or_insert("VPN کامل (Full tunnel)");
    }
    if info.dns_servers.iter().any(|d| d.parse::<IpAddr>().map(|ip| is_fake_ip(&ip)).unwrap_or(false)) {
        reasons.push("DNS سیستم در حالت Fake-IP هست (Clash / sing-box / v2rayN TUN)".into());
        vpn_type.get_or_insert("TUN (Fake-IP)");
    }
    if let Some(p) = &info.system_proxy {
        let local = p.contains("127.0.0.1") || p.contains("localhost") || p.contains("[::1]");
        reasons.push(format!("پروکسی سیستم روشنه: {p}{}", if local { " (کلاینت محلی مثل v2rayN / Nekoray / Hiddify)" } else { "" }));
        vpn_type.get_or_insert("پروکسی سیستم");
    } else if let Some(p) = &info.pac_url {
        reasons.push(format!("اسکریپت PAC پروکسی تنظیم شده: {p}"));
        vpn_type.get_or_insert("پروکسی (PAC)");
    }
    if let (Some(d), Some(p)) = (&info.direct, &info.public_ip) {
        if d.ip.as_ref() != Some(p) {
            reasons.push(format!("IP خروجی برنامه‌ها ({p}) با IP مستقیم خطت ({}) فرق داره", d.ip.as_deref().unwrap_or("?")));
        }
    }
    if matches!(info.warp.as_deref(), Some("on") | Some("plus")) {
        reasons.push("Cloudflare WARP روشنه".into());
        vpn_type.get_or_insert("Cloudflare WARP");
    }
    if info.proxy == Some(true) {
        reasons.push("IP عمومی تو دیتابیس‌ها به‌عنوان پروکسی / VPN ثبت شده".into());
    }
    match info.asn_kind.as_deref() {
        Some("datacenter") => reasons.push(format!("IP عمومی مال دیتاسنتره ({}{}), نه اپراتور خانگی/موبایل", info.asn.as_deref().unwrap_or(""), info.isp.as_deref().map(|i| format!(" · {i}")).unwrap_or_default())),
        Some("cdn") if info.warp.as_deref() != Some("off") => reasons.push("IP عمومی مال شبکه‌ی Cloudflare هست".into()),
        _ if info.hosting == Some(true) => reasons.push("IP عمومی مال یه دیتاسنتره، نه اپراتور".into()),
        _ => {}
    }
    let iran_line = matches!(info.asn_kind.as_deref(), Some("iran-isp") | Some("iran-dc"));
    if let Some(cc) = info.country_code.as_deref() {
        if cc != "IR" && !iran_line {
            // exit outside Iran: on its own not proof (travelling), but together with anything else it is
            if !reasons.is_empty() || info.asn_kind.as_deref() != Some("isp") {
                reasons.push(format!("IP خروجی از کشور {cc}"));
            }
        }
    }
    if iran_line && reasons.iter().all(|r| r.starts_with("آداپتور")) {
        // tunnel adapter exists but traffic still exits through an Iranian ISP: VPN installed but not routing
        if !reasons.is_empty() {
            info.notes.push("آداپتور VPN هست ولی ترافیک از اپراتور ایرانی خارج میشه؛ VPN یا خاموشه یا فقط برای بعضی برنامه‌هاست".into());
            reasons.clear();
        }
    }
    if info.cgnat {
        info.notes.push("IP محلیت تو رنج CGNAT (100.64.0.0/10) هست؛ IP عمومی بین چند کاربر مشترکه و پورت‌فورواردینگ کار نمی‌کنه".into());
    }
    if info.public_ip.is_none() && info.public_ip6.is_none() {
        info.notes.push("IP عمومی پیدا نشد؛ یا اینترنت قطعه یا همه‌ی سرویس‌های IP فیلتر/مسدودن. جزئیات تو بخش «لاگ»".into());
    }
    info.vpn = !reasons.is_empty();
    info.vpn_type = if info.vpn { Some(vpn_type.unwrap_or("VPN / پروکسی").to_string()) } else { None };
    info.vpn_reasons = reasons;

    applog::info(
        "netinfo",
        format!(
            "IP {} · {} {} · {} · منابع: {} · DNS: {} · VPN: {}",
            info.public_ip.as_deref().unwrap_or("?"),
            info.operator_fa.as_deref().or(info.isp.as_deref()).unwrap_or("ISP ?"),
            info.asn.as_deref().unwrap_or(""),
            info.country_code.as_deref().unwrap_or("?"),
            if info.geo_sources.is_empty() { "هیچ".to_string() } else { info.geo_sources.join(", ") },
            if info.dns_servers.is_empty() { "?".to_string() } else { info.dns_servers.join(", ") },
            info.vpn_type.as_deref().unwrap_or("خیر")
        ),
    );
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asn_parse() {
        assert_eq!(asn_num("AS44244 Irancell"), Some(44244));
        assert_eq!(asn_num("44244"), Some(44244));
        assert!(iran_asn(197207).is_some());
        assert!(dc_asn(24940).is_some());
        assert!(looks_vpn("Wintun Userspace Tunnel"));
        assert!(looks_vpn("TAP-Windows Adapter V9"));
        assert!(!looks_vpn("Teredo Tunneling Pseudo-Interface"));
        assert!(!looks_vpn("Realtek PCIe GbE Family Controller"));
        assert!(junk_dns("fec0:0:0:ffff::1"));
    }
}
