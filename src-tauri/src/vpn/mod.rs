//! VPN client (v2rayN / Hiddify style) with two cores:
//! - **Xray-core** runs VLESS / VMess / Trojan / Shadowsocks / SOCKS (REALITY, Vision, XHTTP, WS, gRPC ...)
//! - **sing-box** runs Hysteria2 / TUIC / Hysteria (and links Xray refuses), and is the TUN front-end:
//!   in TUN mode sing-box captures every app and hands the traffic to Xray's local port.
//!
//! v0.5.0 fixes "connected but nothing loads":
//! - the old code reported "connected" as soon as the core process started. Now every connection is
//!   verified with a real HTTP request through the tunnel before it is reported; if that fails the core is
//!   stopped and the real reason (from the core log) is shown instead of a fake "connected".
//! - TUN used UDP 1.1.1.1 for direct DNS (blocked / poisoned in Iran), so the server's own domain never
//!   resolved. It now uses the system's DNS server as it was before the tunnel came up, and Xray gets its
//!   server as a pre-resolved IP (no DNS loop through the tunnel).
//! - the system proxy / TUN / local port all share the same verified local SOCKS+HTTP port.
pub mod config;
pub mod cores;
pub mod link;
pub mod xray;
mod sysproxy;

use crate::applog;
use cores::Kind;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{oneshot, Mutex};
use tokio::time::sleep;

/// pids of the running connection's cores (killed synchronously on app exit)
static CORE_PIDS: StdMutex<Vec<u32>> = StdMutex::new(Vec::new());
static TEST_PID: AtomicU32 = AtomicU32::new(0);
static TEST_PID2: AtomicU32 = AtomicU32::new(0);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Plain HTTP on purpose: through the local HTTP proxy a response only comes back if the far end was
/// really reached (an https URL would get "200 Connection established" from the local core itself).
const TEST_URL: &str = "http://www.gstatic.com/generate_204";

fn set_pids(p: Vec<u32>) {
    if let Ok(mut g) = CORE_PIDS.lock() {
        *g = p;
    }
}
fn take_pids() -> Vec<u32> {
    CORE_PIDS.lock().map(|mut g| std::mem::take(&mut *g)).unwrap_or_default()
}

#[derive(Default)]
pub struct VpnState {
    run: Mutex<Option<Running>>,
    test: Mutex<()>,
}

struct Running {
    id: u64,
    stop: Option<oneshot::Sender<()>>,
    done: Option<oneshot::Receiver<()>>,
    aux: Vec<JoinHandle<()>>,
    status: Status,
}

#[derive(Clone)]
struct Clash {
    port: u16,
    secret: String,
}

/// Where live traffic numbers come from.
#[derive(Clone)]
enum Stats {
    Clash(Clash),
    /// Xray metrics endpoint (`/debug/vars`)
    Metrics(u16),
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    connected: bool,
    name: String,
    proto: String,
    host: String,
    port: u16,
    mode: String,
    local_port: u16,
    since: u64,
    system_proxy: bool,
    raw: String,
    /// "Xray" · "sing-box" · "Xray + sing-box (TUN)"
    engine: String,
    /// delay measured while connecting (ms)
    delay_ms: Option<u32>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StateEvent {
    status: Option<Status>,
    error: Option<String>,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn rand_hex() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut s = String::new();
    for i in 0..2u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(now_ms() ^ (i << 32) ^ u64::from(std::process::id()));
        s.push_str(&format!("{:016x}", h.finish()));
    }
    s
}

fn free_port() -> Result<u16, String> {
    let l = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())
}

/// n distinct free ports (all listeners are held until every port is picked).
fn free_ports(n: usize) -> Result<Vec<u16>, String> {
    let mut ls = Vec::with_capacity(n);
    for _ in 0..n {
        ls.push(std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?);
    }
    ls.iter().map(|l| l.local_addr().map(|a| a.port()).map_err(|e| e.to_string())).collect()
}

fn port_free(port: u16, lan: bool) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() && (!lan || std::net::TcpListener::bind(("0.0.0.0", port)).is_ok())
}

/// Client for 127.0.0.1 (clash API / metrics): never through the system proxy — that proxy may be us.
fn local_client(timeout: Option<Duration>) -> reqwest::Client {
    let mut b = reqwest::Client::builder().no_proxy().tcp_nodelay(true);
    if let Some(t) = timeout {
        b = b.timeout(t);
    }
    b.build().unwrap_or_default()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                for d in it.by_ref() {
                    if d.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

type Lines = Arc<StdMutex<VecDeque<String>>>;

struct Core {
    name: &'static str,
    child: tokio::process::Child,
    logs: Lines,
}

fn spawn_core(exe: &Path, cfg: &Path, dir: &Path, verbose: bool, name: &'static str) -> Result<Core, String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("run").arg("-c").arg(cfg).current_dir(dir).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(cores::CREATE_NO_WINDOW);
    let mut child = cmd.spawn().map_err(|e| format!("اجرای {name}: {e}"))?;
    let logs: Lines = Arc::new(StdMutex::new(VecDeque::new()));
    fn reader<R: tokio::io::AsyncRead + Unpin + Send + 'static>(r: R, logs: Lines, verbose: bool, name: &'static str) {
        tauri::async_runtime::spawn(async move {
            let mut lines = BufReader::new(r).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let l = strip_ansi(&l);
                let t = l.trim();
                if t.is_empty() {
                    continue;
                }
                if verbose {
                    let up = t.to_ascii_uppercase();
                    if up.contains("FATAL") || up.contains("ERROR") {
                        applog::warn(name, t);
                    } else if up.contains("WARN") {
                        applog::info(name, t);
                    }
                }
                if let Ok(mut g) = logs.lock() {
                    if g.len() >= 80 {
                        g.pop_front();
                    }
                    g.push_back(t.to_string());
                }
            }
        });
    }
    if let Some(o) = child.stdout.take() {
        reader(o, logs.clone(), verbose, name);
    }
    if let Some(e) = child.stderr.take() {
        reader(e, logs.clone(), verbose, name);
    }
    Ok(Core { name, child, logs })
}

fn last_lines(l: &Lines, n: usize) -> Vec<String> {
    l.lock().map(|g| g.iter().rev().take(n).rev().cloned().collect()).unwrap_or_default()
}

fn all_lines(cores: &[Core], n: usize) -> Vec<String> {
    cores.iter().flat_map(|c| last_lines(&c.logs, n)).collect()
}

async fn kill_all(cores: &mut [Core]) {
    for c in cores.iter_mut() {
        let _ = c.child.kill().await;
    }
    sleep(Duration::from_millis(150)).await; // let the readers flush the last lines
}

/// Wait until the clash API answers (= config loaded, inbounds listening) or the process dies.
async fn wait_ready(c: &Clash, child: &mut tokio::process::Child, max: Duration) -> Result<(), String> {
    let client = local_client(Some(Duration::from_millis(800)));
    let url = format!("http://127.0.0.1:{}/version", c.port);
    let t0 = Instant::now();
    loop {
        if let Ok(Some(st)) = child.try_wait() {
            return Err(format!("هسته بسته شد ({st})"));
        }
        if let Ok(r) = client.get(&url).bearer_auth(&c.secret).send().await {
            if r.status().is_success() {
                return Ok(());
            }
        }
        if t0.elapsed() > max {
            return Err("هسته آماده نشد (تایم‌اوت)".into());
        }
        sleep(Duration::from_millis(120)).await;
    }
}

/// Wait until a local port accepts connections (Xray has no clash API) or the process dies.
async fn wait_port(port: u16, child: &mut tokio::process::Child, max: Duration) -> Result<(), String> {
    let t0 = Instant::now();
    loop {
        if let Ok(Some(st)) = child.try_wait() {
            return Err(format!("هسته بسته شد ({st})"));
        }
        if tokio::time::timeout(Duration::from_millis(500), tokio::net::TcpStream::connect(("127.0.0.1", port))).await.map(|r| r.is_ok()).unwrap_or(false) {
            return Ok(());
        }
        if t0.elapsed() > max {
            return Err("هسته آماده نشد (تایم‌اوت)".into());
        }
        sleep(Duration::from_millis(100)).await;
    }
}

/// One HTTP request through the local proxy port; Ok(ms) only if the far end answered.
async fn http_via(port: u16, url: &str, timeout: Duration) -> Result<u32, String> {
    let px = reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).map_err(|e| e.to_string())?;
    let client = reqwest::Client::builder().proxy(px).user_agent(crate::probe::UA).timeout(timeout).tcp_nodelay(true).build().map_err(|e| e.to_string())?;
    let t = Instant::now();
    let r = client.get(url).send().await.map_err(|e| if e.is_timeout() { "timeout".to_string() } else { crate::probe::err_chain(&e) })?;
    let code = r.status().as_u16();
    let _ = r.bytes().await;
    if code >= 500 {
        return Err(format!("HTTP {code}"));
    }
    Ok(t.elapsed().as_millis().max(1) as u32)
}

/// Real check that traffic passes through the tunnel. Returns the delay of a warm request.
async fn verify(port: u16) -> Result<u32, String> {
    let mut last = String::new();
    for to in [6u64, 8, 10] {
        match http_via(port, TEST_URL, Duration::from_secs(to)).await {
            Ok(first) => {
                // second request reuses the tunnel: closer to what browsing feels like
                let warm = http_via(port, TEST_URL, Duration::from_secs(5)).await.unwrap_or(first);
                return Ok(first.min(warm));
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// TUN check: a direct request (no proxy at all) has to succeed through the TUN adapter.
async fn verify_system() -> Result<u32, String> {
    let client = reqwest::Client::builder().no_proxy().user_agent(crate::probe::UA).tcp_nodelay(true).build().map_err(|e| e.to_string())?;
    let mut last = String::new();
    for to in [6u64, 8, 10] {
        let t = Instant::now();
        match client.get(TEST_URL).timeout(Duration::from_secs(to)).send().await {
            Ok(r) if r.status().as_u16() < 500 => return Ok(t.elapsed().as_millis().max(1) as u32),
            Ok(r) => last = format!("HTTP {}", r.status().as_u16()),
            Err(e) => last = if e.is_timeout() { "timeout".into() } else { crate::probe::err_chain(&e) },
        }
        sleep(Duration::from_millis(500)).await;
    }
    Err(last)
}

/// First usable system DNS server (read before the TUN comes up).
fn system_dns() -> Option<String> {
    let (cfg, _) = hickory_resolver::system_conf::read_system_conf().ok()?;
    cfg.name_servers()
        .iter()
        .map(|n| n.socket_addr.ip())
        .find(|ip| !ip.is_loopback() && !ip.is_unspecified() && !crate::probe::is_fake_ip(ip) && ip.is_ipv4())
        .map(|ip| ip.to_string())
}

/// Turn raw core output into something a user can act on.
fn explain(base: &str, logs: &[String], mode: &str) -> String {
    let all = logs.join("\n");
    let low = all.to_ascii_lowercase();
    let b = base.to_ascii_lowercase();
    let tail = logs.iter().rev().find(|l| { let u = l.to_ascii_uppercase(); u.contains("FATAL") || u.contains("ERROR") || u.contains("FAILED") }).or(logs.last()).cloned().unwrap_or_default();
    if mode == "tun" && (low.contains("access is denied") || low.contains("elevat") || low.contains("operation not permitted") || low.contains("permission denied") || low.contains("administrator")) {
        return "حالت TUN دسترسی ادمین می‌خواد: برنامه رو با Run as administrator باز کن یا از حالت «پروکسی سیستم» استفاده کن".into();
    }
    if low.contains("address already in use") || low.contains("only one usage of each socket") {
        return "پورت محلی دست یه برنامه‌ی دیگه‌ست (مثلاً v2rayN یا Hiddify). پورت رو تو تنظیمات عوض کن یا اون برنامه رو ببند".into();
    }
    if low.contains("allowinsecure") {
        return "این کانفیگ گواهی ناامن (allowInsecure) داره که Xray جدید قبولش نمی‌کنه؛ از ساب‌دهنده بخواه pcs/vcn اضافه کنه".into();
    }
    if low.contains("decode config") || low.contains("unknown field") || low.contains("failed to build") || low.contains("infra/conf") || (low.contains("parse") && low.contains("outbound")) {
        return format!("هسته این کانفیگ رو قبول نکرد: {tail}");
    }
    if low.contains("reality") && (low.contains("verif") || low.contains("invalid")) {
        return "REALITY تایید نشد: public key (pbk)، shortId یا SNI کانفیگ با سرور نمی‌خونه".into();
    }
    if low.contains("x509") || low.contains("certificate") {
        return "گواهی TLS سرور معتبر نیست (SNI اشتباه یا گواهی منقضی/خودامضا)".into();
    }
    if low.contains("no such host") || low.contains("lookup ") && low.contains("server misbehaving") {
        return "آدرس سرور پیدا نشد (DNS). DNS مستقیم رو تو تنظیمات عوض کن یا کانفیگ دیگه‌ای امتحان کن".into();
    }
    if b.contains("timeout") || low.contains("i/o timeout") || low.contains("context deadline") {
        return "سرور جواب نداد (تایم‌اوت): احتمالاً فیلتر شده، خاموشه یا اینترنتت ضعیفه. «تست واقعی» بگیر و کانفیگ دیگه‌ای انتخاب کن".into();
    }
    if low.contains("connection refused") || low.contains("actively refused") || low.contains("connection reset") || b.contains("reset") {
        return "سرور اتصال رو رد کرد یا وسطش قطع شد (پورت بسته یا فیلتر). کانفیگ دیگه‌ای امتحان کن".into();
    }
    if tail.is_empty() { base.to_string() } else { format!("{base}: {tail}") }
}

fn emit_state(app: &AppHandle, status: Option<Status>, error: Option<String>) {
    let _ = app.emit("vpn://state", StateEvent { status, error });
}

async fn stop_running(mut r: Running) {
    if let Some(tx) = r.stop.take() {
        let _ = tx.send(());
    }
    if let Some(done) = r.done.take() {
        let _ = tokio::time::timeout(Duration::from_secs(4), done).await;
    }
    for h in r.aux.drain(..) {
        h.abort();
    }
    if r.status.system_proxy {
        let _ = tauri::async_runtime::spawn_blocking(sysproxy::restore).await;
    }
}

fn write_cfg(path: &Path, cfg: &Value) -> Result<(), String> {
    std::fs::write(path, serde_json::to_vec_pretty(cfg).unwrap_or_default()).map_err(|e| format!("نوشتن کانفیگ: {e}"))
}

/// Starts the core(s) for one profile. On success the local port is listening (not yet verified).
async fn start_cores(p: &link::Profile, opts: &config::Opts, dir: &Path, xr: Option<PathBuf>, sb: Option<PathBuf>) -> Result<(Vec<Core>, Stats, String), String> {
    let tun = opts.mode == "tun";
    let sys_dns = if tun { tauri::async_runtime::spawn_blocking(system_dns).await.ok().flatten() } else { None };
    let mut cores: Vec<Core> = Vec::new();
    match xr.filter(|_| !p.xray.is_null()) {
        Some(xr) => {
            let mut ob = p.xray.clone();
            let mut server_ip: Option<String> = None;
            if tun {
                match xray::server_of(&ob) {
                    Some(h) if !link::is_ip(&h) => {
                        let a = crate::ping::resolve(&h, p.port).await.map_err(|e| format!("آدرس سرور ({h}) پیدا نشد: {e}"))?;
                        let ip = a.ip().to_string();
                        xray::pin_server_ip(&mut ob, &ip);
                        server_ip = Some(ip);
                    }
                    Some(ip) => server_ip = Some(ip),
                    None => {}
                }
            }
            let mport = free_port()?;
            let path = dir.join("xray.json");
            // TUN: Xray asks the pre-tunnel DNS server itself (plain UDP from xray.exe = excluded from
            // the TUN). "localhost" would go through the Windows DNS service, which the TUN captures.
            let xdns = if tun { sys_dns.clone() } else { None };
            write_cfg(&path, &xray::build(&ob, opts, opts.port, mport, xdns.as_deref()))?;
            let mut c = spawn_core(&xr, &path, dir, true, "xray")?;
            if let Err(e) = wait_port(opts.port, &mut c.child, Duration::from_secs(10)).await {
                let mut v = vec![c];
                kill_all(&mut v).await;
                return Err(explain(&e, &all_lines(&v, 12), &opts.mode));
            }
            cores.push(c);
            let mut engine = "Xray".to_string();
            if tun {
                let sb = sb.ok_or("NO_CORE")?;
                let clash = Clash { port: free_port()?, secret: rand_hex() };
                let path = dir.join("tun.json");
                write_cfg(&path, &config::build_tun_front(opts, opts.port, clash.port, &clash.secret, sys_dns.as_deref(), server_ip.as_deref()))?;
                let mut f = spawn_core(&sb, &path, dir, true, "sing-box")?;
                let r = wait_ready(&clash, &mut f.child, Duration::from_secs(12)).await;
                cores.push(f);
                if let Err(e) = r {
                    kill_all(&mut cores).await;
                    return Err(explain(&e, &all_lines(&cores, 12), "tun"));
                }
                // the clash API answers before Windows has finished installing the TUN routes
                sleep(Duration::from_millis(1500)).await;
                engine = "Xray + sing-box (TUN)".into();
            }
            Ok((cores, Stats::Metrics(mport), engine))
        }
        None => {
            let sb = sb.filter(|_| !p.outbound.is_null()).ok_or("NO_CORE")?;
            let clash = Clash { port: free_port()?, secret: rand_hex() };
            let path = dir.join("config.json");
            write_cfg(&path, &config::build(&p.outbound, opts, clash.port, &clash.secret, "warn", sys_dns.as_deref()))?;
            let mut c = spawn_core(&sb, &path, dir, true, "sing-box")?;
            if let Err(e) = wait_ready(&clash, &mut c.child, Duration::from_secs(12)).await {
                let mut v = vec![c];
                kill_all(&mut v).await;
                return Err(explain(&e, &all_lines(&v, 12), &opts.mode));
            }
            cores.push(c);
            if tun {
                sleep(Duration::from_millis(1500)).await;
            }
            Ok((cores, Stats::Clash(clash), if tun { "sing-box (TUN)".into() } else { "sing-box".into() }))
        }
    }
}

#[tauri::command]
pub async fn vpn_connect(app: AppHandle, state: State<'_, VpnState>, raw: String, opts: Option<config::Opts>) -> Result<Status, String> {
    let mut opts = opts.unwrap_or_default();
    let auto_port = opts.port == 0;
    if auto_port {
        opts.port = config::DEFAULT_PORT;
    }
    let p = link::parse_link(&raw)?;
    let xr = cores::find(&app, Kind::Xray);
    let sb = cores::find(&app, Kind::SingBox);
    let can_xray = !p.xray.is_null() && xr.is_some();
    let can_sb = !p.outbound.is_null() && sb.is_some();
    if !can_xray && !can_sb {
        return Err("NO_CORE".into());
    }
    if opts.mode == "tun" && sb.is_none() {
        return Err("NO_CORE".into());
    }
    if !matches!(opts.mode.as_str(), "proxy" | "tun" | "local") {
        opts.mode = "proxy".into();
    }
    // without admin rights Windows refuses to create the TUN adapter; say so before starting anything
    if opts.mode == "tun" && cfg!(windows) && !crate::sys::is_admin() {
        return Err("NEED_ADMIN".into());
    }
    if opts.port < 1024 {
        return Err("پورت محلی باید بین 1024 و 65535 باشه".into());
    }
    let mut g = state.run.lock().await;
    if let Some(r) = g.take() {
        stop_running(r).await;
    }
    if let Some(d) = cores::data_dir(&app).ok().filter(|_| opts.mode == "tun" || !port_free(opts.port, opts.allow_lan)) {
        // cores left over from a crash / `npm run dev` restart hold the port and the "PacketYellow" TUN adapter
        let n = tauri::async_runtime::spawn_blocking(move || kill_leftovers(&d)).await.unwrap_or(0);
        if n > 0 {
            applog::info("vpn", format!("{n} هسته‌ی جامانده از اجرای قبلی بسته شد"));
            sleep(Duration::from_millis(400)).await;
        }
    }
    if !port_free(opts.port, opts.allow_lan) && auto_port {
        // port left empty = "anything that works": take a free one instead of failing
        let busy = opts.port;
        opts.port = free_port()?;
        applog::info("vpn", format!("پورت {busy} اشغال بود؛ خودکار پورت {} انتخاب شد", opts.port));
    }
    if !port_free(opts.port, opts.allow_lan) {
        return Err(format!("پورت {} آزاد نیست (شاید v2rayN / Hiddify بازه). پورت دیگه‌ای انتخاب کن", opts.port));
    }
    let dir = cores::data_dir(&app)?;
    applog::info("vpn", format!("اتصال به {} ({} · {}:{}) حالت {} · هسته {}", p.name, p.proto, p.host, p.port, opts.mode, if can_xray { "Xray" } else { "sing-box" }));

    let (mut cores, stats, engine) = match start_cores(&p, &opts, &dir, xr.filter(|_| can_xray), sb).await {
        Ok(v) => v,
        Err(e) => {
            applog::error("vpn", format!("اتصال ناموفق: {e}"));
            return Err(e);
        }
    };
    set_pids(cores.iter().filter_map(|c| c.child.id()).collect());

    // the part the old version skipped: does traffic actually pass?
    let delay = match verify(opts.port).await {
        Ok(d) => d,
        Err(e) => {
            sleep(Duration::from_millis(250)).await;
            let lines = all_lines(&cores, 15);
            kill_all(&mut cores).await;
            set_pids(vec![]);
            let msg = explain(&format!("هسته بالا اومد ولی از داخل تونل به اینترنت نرسید ({e})"), &lines, &opts.mode);
            applog::error("vpn", format!("اتصال ناموفق: {msg}"));
            return Err(msg);
        }
    };
    applog::info("vpn", format!("تونل تایید شد: تاخیر واقعی {delay} ms"));

    // TUN: the local port works, but does the *system's* traffic really enter the tunnel?
    // A plain request from this process (no proxy) must come back through the TUN adapter.
    if opts.mode == "tun" {
        if let Err(e) = verify_system().await {
            sleep(Duration::from_millis(250)).await;
            let lines = all_lines(&cores, 15);
            kill_all(&mut cores).await;
            set_pids(vec![]);
            let msg = explain(&format!("سرور کار می‌کنه ولی ترافیک ویندوز وارد TUN نشد ({e})"), &lines, "tun");
            applog::error("vpn", format!("اتصال TUN ناموفق: {msg}"));
            return Err(format!("{msg} · اگه آنتی‌ویروس / فایروال یا VPN دیگه‌ای روشنه ببندش، یا از حالت «پروکسی سیستم» استفاده کن"));
        }
        applog::info("vpn", "TUN تایید شد: ترافیک سیستم از تونل رد میشه");
    }

    let mut system_proxy = false;
    if opts.mode == "proxy" {
        match tauri::async_runtime::spawn_blocking({
            let port = opts.port;
            move || sysproxy::set(port)
        })
        .await
        .map_err(|e| e.to_string())?
        {
            Ok(()) => system_proxy = true,
            Err(e) => {
                applog::warn("vpn", format!("پروکسی سیستم تنظیم نشد: {e}"));
                let _ = app.emit("vpn://warn", format!("پروکسی سیستم تنظیم نشد: {e}. تونل روی 127.0.0.1:{} بازه و می‌تونی دستی ازش استفاده کنی", opts.port));
            }
        }
    }

    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let status = Status {
        connected: true,
        name: p.name.clone(),
        proto: p.proto.clone(),
        host: p.host.clone(),
        port: p.port,
        mode: opts.mode.clone(),
        local_port: opts.port,
        since: now_ms(),
        system_proxy,
        raw: p.raw.clone(),
        engine: engine.clone(),
        delay_ms: Some(delay),
    };
    let (stop_tx, stop_rx) = oneshot::channel::<()>();
    let (done_tx, done_rx) = oneshot::channel::<()>();
    let names: Vec<&'static str> = cores.iter().map(|c| c.name).collect();
    let logs: Vec<Lines> = cores.iter().map(|c| c.logs.clone()).collect();
    let mut children: Vec<tokio::process::Child> = cores.into_iter().map(|c| c.child).collect();
    let app2 = app.clone();
    let mode = opts.mode.clone();
    tauri::async_runtime::spawn(async move {
        let crashed: Option<(usize, String)> = {
            let waits: Vec<_> = children.iter_mut().map(|c| Box::pin(c.wait())).collect();
            tokio::select! {
                (r, i, _rest) = futures_util::future::select_all(waits) => Some((i, r.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string()))),
                _ = stop_rx => None,
            }
        };
        // one core down = the whole connection is down (Xray without its TUN front and vice versa)
        for c in children.iter_mut() {
            let _ = c.kill().await;
        }
        set_pids(vec![]);
        let _ = done_tx.send(());
        if let Some((i, code)) = crashed {
            sleep(Duration::from_millis(150)).await;
            let lines: Vec<String> = logs.iter().flat_map(|l| last_lines(l, 12)).collect();
            let msg = explain(&format!("هسته‌ی {} بسته شد ({code})", names.get(i).copied().unwrap_or("VPN")), &lines, &mode);
            applog::error("vpn", &msg);
            let st = app2.state::<VpnState>();
            let mut g = st.run.lock().await;
            if g.as_ref().map(|r| r.id) == Some(id) {
                if let Some(r) = g.take() {
                    stop_running(r).await;
                }
                drop(g);
                emit_state(&app2, None, Some(msg));
            }
        }
    });

    let aux = vec![
        tauri::async_runtime::spawn(traffic_loop(app.clone(), stats)),
        tauri::async_runtime::spawn({
            let (app, port) = (app.clone(), opts.port);
            async move {
                let r = check(port, Some(delay)).await;
                let _ = app.emit("vpn://check", r);
            }
        }),
    ];
    *g = Some(Running { id, stop: Some(stop_tx), done: Some(done_rx), aux, status: status.clone() });
    drop(g);
    applog::info("vpn", format!("وصل شد: {} · {} · پورت محلی {}", status.name, engine, status.local_port));
    emit_state(&app, Some(status.clone()), None);
    Ok(status)
}

#[tauri::command]
pub async fn vpn_disconnect(app: AppHandle, state: State<'_, VpnState>) -> Result<(), String> {
    let r = state.run.lock().await.take();
    if let Some(r) = r {
        stop_running(r).await;
        applog::info("vpn", "اتصال VPN قطع شد");
    }
    emit_state(&app, None, None);
    Ok(())
}

#[tauri::command]
pub async fn vpn_status(state: State<'_, VpnState>) -> Result<Option<Status>, String> {
    Ok(state.run.lock().await.as_ref().map(|r| r.status.clone()))
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Traffic {
    up: u64,
    down: u64,
    up_total: u64,
    down_total: u64,
}

async fn traffic_loop(app: AppHandle, stats: Stats) {
    match stats {
        Stats::Clash(c) => clash_traffic(app, c).await,
        Stats::Metrics(port) => xray_traffic(app, port).await,
    }
}

async fn clash_traffic(app: AppHandle, c: Clash) {
    let client = local_client(None);
    let url = format!("http://127.0.0.1:{}/traffic", c.port);
    let (mut up_total, mut down_total) = (0u64, 0u64);
    loop {
        if let Ok(resp) = client.get(&url).bearer_auth(&c.secret).send().await {
            let mut s = resp.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            while let Some(Ok(chunk)) = s.next().await {
                buf.extend_from_slice(&chunk);
                while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=pos).collect();
                    if let Ok(v) = serde_json::from_slice::<Value>(&line) {
                        let up = v["up"].as_u64().unwrap_or(0);
                        let down = v["down"].as_u64().unwrap_or(0);
                        up_total += up;
                        down_total += down;
                        let _ = app.emit("vpn://stats", Traffic { up, down, up_total, down_total });
                    }
                }
                if buf.len() > 64 * 1024 {
                    buf.clear();
                }
            }
        }
        sleep(Duration::from_secs(1)).await;
    }
}

/// Xray: cumulative counters of the "proxy" outbound from the metrics endpoint, once a second.
async fn xray_traffic(app: AppHandle, port: u16) {
    let client = local_client(Some(Duration::from_secs(2)));
    let url = format!("http://127.0.0.1:{port}/debug/vars");
    let mut prev: Option<(u64, u64)> = None;
    loop {
        if let Ok(r) = client.get(&url).send().await {
            if let Ok(v) = r.json::<Value>().await {
                let o = &v["stats"]["outbound"]["proxy"];
                let (up_total, down_total) = (o["uplink"].as_u64().unwrap_or(0), o["downlink"].as_u64().unwrap_or(0));
                let (up, down) = match prev {
                    Some((pu, pd)) => (up_total.saturating_sub(pu), down_total.saturating_sub(pd)),
                    None => (0, 0),
                };
                prev = Some((up_total, down_total));
                let _ = app.emit("vpn://stats", Traffic { up, down, up_total, down_total });
            }
        }
        sleep(Duration::from_secs(1)).await;
    }
}

#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    delay_ms: Option<u32>,
    ip: Option<String>,
    loc: Option<String>,
    colo: Option<String>,
    error: Option<String>,
}

async fn clash_delay(client: &reqwest::Client, c: &Clash, tag: &str, url: &str, timeout_ms: u32) -> Result<u32, String> {
    let u = format!("http://127.0.0.1:{}/proxies/{}/delay?timeout={}&url={}", c.port, tag, timeout_ms, link::pct_encode(url));
    let r = client.get(&u).bearer_auth(&c.secret).send().await.map_err(|e| crate::probe::err_chain(&e))?;
    let ok = r.status().is_success();
    let v: Value = r.json().await.unwrap_or(Value::Null);
    match v["delay"].as_u64() {
        Some(d) if ok && d > 0 => Ok(d as u32),
        _ => {
            let m = v["message"].as_str().unwrap_or("timeout").to_string();
            Err(if m.to_ascii_lowercase().contains("timeout") || m.contains("deadline") { "timeout".into() } else { m })
        }
    }
}

/// Delay through the tunnel + exit IP/country as the internet sees it (works the same for both cores).
async fn check(port: u16, known_delay: Option<u32>) -> Check {
    let mut out = Check::default();
    match known_delay {
        Some(d) => out.delay_ms = Some(d),
        None => match http_via(port, TEST_URL, Duration::from_secs(8)).await {
            Ok(d) => out.delay_ms = Some(d),
            Err(e) => out.error = Some(e),
        },
    }
    if let Ok(px) = reqwest::Proxy::all(format!("http://127.0.0.1:{port}")) {
        if let Ok(cl) = reqwest::Client::builder().proxy(px).user_agent(crate::probe::UA).timeout(Duration::from_secs(10)).build() {
            if let Ok(r) = cl.get("https://www.cloudflare.com/cdn-cgi/trace").send().await {
                if let Ok(t) = r.text().await {
                    for l in t.lines() {
                        match l.split_once('=') {
                            Some(("ip", v)) => out.ip = Some(v.to_string()),
                            Some(("loc", v)) => out.loc = Some(v.to_string()),
                            Some(("colo", v)) => out.colo = Some(v.to_string()),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    out
}

#[tauri::command]
pub async fn vpn_check(state: State<'_, VpnState>) -> Result<Check, String> {
    let port = {
        let g = state.run.lock().await;
        g.as_ref().ok_or("وصل نیستی")?.status.local_port
    };
    Ok(check(port, None).await)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DelayRes {
    index: usize,
    delay_ms: Option<u32>,
    error: Option<String>,
}

/// "outbounds[3]..." in a sing-box config error -> 3
fn bad_outbound(err: &str) -> Option<usize> {
    let i = err.find("outbounds[")? + "outbounds[".len();
    err[i..].split(']').next()?.parse().ok()
}

fn fail(app: &AppHandle, i: usize, e: String) -> DelayRes {
    let r = DelayRes { index: i, delay_ms: None, error: Some(e) };
    let _ = app.emit("vpn://delay", r.clone());
    r
}

/// Runs owned delay-test futures with at most `limit` in flight (replaces `buffer_unordered`).
/// Every future is `'static`, so nothing borrowed ends up inside the tauri command's future.
async fn run_limited<F>(futs: Vec<F>, limit: usize) -> Vec<DelayRes>
where
    F: std::future::Future<Output = DelayRes> + Send + 'static,
{
    let sem = Arc::new(tokio::sync::Semaphore::new(limit.max(1)));
    let mut set = tokio::task::JoinSet::new();
    for f in futs {
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            f.await
        });
    }
    let mut out = Vec::new();
    while let Some(r) = set.join_next().await {
        if let Ok(r) = r {
            out.push(r);
        }
    }
    out
}

/// sing-box group of the real-delay test: one throw-away core, delays through its clash API.
async fn test_singbox(app: &AppHandle, exe: &Path, dir: &Path, mut valid: Vec<(usize, Value)>, url: &str, timeout_ms: u32) -> Vec<DelayRes> {
    let mut out = Vec::new();
    let cfg_path = dir.join("test.json");
    let mut attempts = 0;
    while !valid.is_empty() {
        attempts += 1;
        let clash = match free_port() {
            Ok(p) => Clash { port: p, secret: rand_hex() },
            Err(e) => { out.extend(valid.drain(..).map(|(i, _)| fail(app, i, e.clone()))); break; }
        };
        let cfg = config::build_test(&valid, clash.port, &clash.secret);
        let _ = std::fs::write(&cfg_path, serde_json::to_vec(&cfg).unwrap_or_default());
        let mut c = match spawn_core(exe, &cfg_path, dir, false, "sing-box") {
            Ok(c) => c,
            Err(e) => { out.extend(valid.drain(..).map(|(i, _)| fail(app, i, e.clone()))); break; }
        };
        if let Err(e) = wait_ready(&clash, &mut c.child, Duration::from_secs(10)).await {
            let _ = c.child.kill().await;
            sleep(Duration::from_millis(150)).await;
            let lines = last_lines(&c.logs, 10);
            // one broken server must not kill the whole test: drop it and retry
            if let Some(k) = bad_outbound(&lines.join(" ")).filter(|k| *k < valid.len()) {
                if attempts < 30 {
                    let (i, _) = valid.remove(k);
                    out.push(fail(app, i, explain("کانفیگ نامعتبر", &lines, "")));
                    continue;
                }
            }
            let msg = explain(&e, &lines, "");
            out.extend(valid.drain(..).map(|(i, _)| fail(app, i, msg.clone())));
            break;
        }
        TEST_PID.store(c.child.id().unwrap_or(0), Ordering::SeqCst);
        let client = local_client(Some(Duration::from_millis(timeout_ms as u64 + 4000)));
        let ids: Vec<usize> = valid.iter().map(|(i, _)| *i).collect();
        // owned ('static) futures: borrowing inside a stream closure broke the Send check of
        // the tauri command ("implementation of FnOnce is not general enough")
        let clash_arc = Arc::new(clash.clone());
        let futs: Vec<_> = ids
            .into_iter()
            .map(|i| {
                let (client, clash, app, url) = (client.clone(), clash_arc.clone(), app.clone(), url.to_string());
                async move {
                    let r = match clash_delay(&client, &clash, &format!("p{i}"), &url, timeout_ms).await {
                        Ok(d) => DelayRes { index: i, delay_ms: Some(d), error: None },
                        Err(e) => DelayRes { index: i, delay_ms: None, error: Some(e) },
                    };
                    let _ = app.emit("vpn://delay", r.clone());
                    r
                }
            })
            .collect();
        let res: Vec<DelayRes> = run_limited(futs, 16).await;
        let _ = c.child.kill().await;
        TEST_PID.store(0, Ordering::SeqCst);
        out.extend(res);
        break;
    }
    let _ = std::fs::remove_file(&cfg_path);
    out
}

/// Xray group of the real-delay test: one throw-away core with a local port per server.
async fn test_xray(app: &AppHandle, exe: &Path, dir: &Path, mut valid: Vec<(usize, Value)>, url: &str, timeout_ms: u32) -> Vec<DelayRes> {
    let mut out = Vec::new();
    let cfg_path = dir.join("xtest.json");
    let mut attempts = 0;
    while !valid.is_empty() {
        attempts += 1;
        let ports = match free_ports(valid.len()) {
            Ok(p) => p,
            Err(e) => { out.extend(valid.drain(..).map(|(i, _)| fail(app, i, e.clone()))); break; }
        };
        let items: Vec<(usize, Value, u16)> = valid.iter().zip(ports.iter()).map(|((i, ob), p)| (*i, ob.clone(), *p)).collect();
        let _ = std::fs::write(&cfg_path, serde_json::to_vec(&xray::build_test(&items)).unwrap_or_default());
        let mut c = match spawn_core(exe, &cfg_path, dir, false, "xray") {
            Ok(c) => c,
            Err(e) => { out.extend(valid.drain(..).map(|(i, _)| fail(app, i, e.clone()))); break; }
        };
        if let Err(e) = wait_port(ports[0], &mut c.child, Duration::from_secs(10)).await {
            let _ = c.child.kill().await;
            sleep(Duration::from_millis(150)).await;
            let lines = last_lines(&c.logs, 10);
            if let Some(pos) = xray::bad_tag(&lines.join(" ")).and_then(|k| valid.iter().position(|(i, _)| *i == k)) {
                if attempts < 30 {
                    let (i, _) = valid.remove(pos);
                    out.push(fail(app, i, explain("کانفیگ نامعتبر", &lines, "")));
                    continue;
                }
            }
            let msg = explain(&e, &lines, "");
            out.extend(valid.drain(..).map(|(i, _)| fail(app, i, msg.clone())));
            break;
        }
        TEST_PID2.store(c.child.id().unwrap_or(0), Ordering::SeqCst);
        let to = Duration::from_millis(timeout_ms as u64);
        // owned (index, port) pairs + owned futures: the old `items.iter().map(|(i, _, port)| ..)`
        // closure took a borrowed tuple and caused "implementation of FnOnce is not general enough"
        let targets: Vec<(usize, u16)> = items.iter().map(|(i, _, p)| (*i, *p)).collect();
        let futs: Vec<_> = targets
            .into_iter()
            .map(|(i, port)| {
                let (app, url) = (app.clone(), url.to_string());
                async move {
                    let r = match http_via(port, &url, to).await {
                        Ok(d) => DelayRes { index: i, delay_ms: Some(d), error: None },
                        Err(e) => DelayRes { index: i, delay_ms: None, error: Some(e) },
                    };
                    let _ = app.emit("vpn://delay", r.clone());
                    r
                }
            })
            .collect();
        let res: Vec<DelayRes> = run_limited(futs, 16).await;
        let _ = c.child.kill().await;
        TEST_PID2.store(0, Ordering::SeqCst);
        out.extend(res);
        break;
    }
    let _ = std::fs::remove_file(&cfg_path);
    out
}

/// Real delay (like v2rayN / Hiddify): every server gets a real HTTP request through its own core.
/// Xray servers and sing-box servers are tested in parallel. Results stream as `vpn://delay` events.
#[tauri::command]
pub async fn vpn_delay_test(app: AppHandle, state: State<'_, VpnState>, links: Vec<String>, url: Option<String>, timeout_ms: Option<u32>) -> Result<Vec<DelayRes>, String> {
    let _lock = state.test.try_lock().map_err(|_| "یه تست دیگه در حال اجراست".to_string())?;
    let xr = cores::find(&app, Kind::Xray);
    let sb = cores::find(&app, Kind::SingBox);
    if xr.is_none() && sb.is_none() {
        return Err("NO_CORE".into());
    }
    let url = url.filter(|u| u.starts_with("http")).unwrap_or_else(|| TEST_URL.into());
    let timeout_ms = timeout_ms.unwrap_or(5000).clamp(1000, 15000);
    let mut out: Vec<Option<DelayRes>> = vec![None; links.len()];
    let (mut xg, mut sg) = (Vec::new(), Vec::new());
    for (i, l) in links.iter().enumerate() {
        match link::parse_link(l) {
            Ok(p) if !p.xray.is_null() && xr.is_some() => xg.push((i, p.xray)),
            Ok(p) if !p.outbound.is_null() && sb.is_some() => sg.push((i, p.outbound)),
            Ok(p) => out[i] = Some(fail(&app, i, format!("هسته‌ی {} نصب نیست", if p.xray.is_null() { "sing-box" } else { "Xray" }))),
            Err(e) => out[i] = Some(fail(&app, i, e)),
        }
    }
    let dir = cores::data_dir(&app)?;
    let fx = async {
        match (&xr, xg.is_empty()) {
            (Some(x), false) => test_xray(&app, x, &dir, xg, &url, timeout_ms).await,
            _ => Vec::new(),
        }
    };
    let fs = async {
        match (&sb, sg.is_empty()) {
            (Some(s), false) => test_singbox(&app, s, &dir, sg, &url, timeout_ms).await,
            _ => Vec::new(),
        }
    };
    let (rx, rs) = tokio::join!(fx, fs);
    for r in rx.into_iter().chain(rs) {
        let i = r.index;
        if i < out.len() {
            out[i] = Some(r);
        }
    }
    let ok = out.iter().flatten().filter(|r| r.delay_ms.is_some()).count();
    applog::info("vpn", format!("تست تاخیر واقعی: {ok} از {} سرور جواب داد", links.len()));
    Ok(out.into_iter().enumerate().map(|(i, r)| r.unwrap_or(DelayRes { index: i, delay_ms: None, error: Some("تست نشد".into()) })).collect())
}

#[tauri::command]
pub fn vpn_inspect(raw: String) -> Result<Value, String> {
    Ok(link::parse_link(&raw)?.outbound)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParseItem {
    ok: bool,
    profile: Option<link::Profile>,
    error: Option<String>,
    raw: String,
}

#[tauri::command]
pub fn vpn_parse(text: String) -> Vec<ParseItem> {
    link::split_text(&text)
        .into_iter()
        .map(|raw| match link::parse_link(&raw) {
            Ok(p) => ParseItem { ok: true, profile: Some(p), error: None, raw },
            Err(e) => ParseItem { ok: false, profile: None, error: Some(e), raw },
        })
        .collect()
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SubInfo {
    upload: Option<u64>,
    download: Option<u64>,
    total: Option<u64>,
    expire: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubResult {
    items: Vec<ParseItem>,
    info: Option<SubInfo>,
    title: Option<String>,
}

/// Download a subscription (goes through the system proxy, so it also works while connected).
#[tauri::command]
pub async fn vpn_fetch_sub(url: String) -> Result<SubResult, String> {
    let url = url.trim().to_string();
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("لینک ساب باید با http یا https شروع بشه".into());
    }
    // panels (Marzban, 3x-ui, ...) pick the output format from the User-Agent; v2rayN = plain links
    let client = reqwest::Client::builder().user_agent("v2rayN/7.10").timeout(Duration::from_secs(20)).connect_timeout(Duration::from_secs(8)).build().map_err(|e| e.to_string())?;
    let r = client.get(&url).send().await.map_err(|e| format!("دریافت ساب: {}", crate::probe::err_chain(&e)))?;
    if !r.status().is_success() {
        return Err(format!("سرور ساب جواب داد: {}", r.status()));
    }
    let h = |k: &str| r.headers().get(k).and_then(|v| v.to_str().ok()).map(String::from);
    let info = h("subscription-userinfo").map(|s| {
        let mut i = SubInfo::default();
        for kv in s.split(';') {
            if let Some((k, v)) = kv.trim().split_once('=') {
                let v = v.trim().parse::<f64>().ok().map(|x| x as u64);
                match k.trim() {
                    "upload" => i.upload = v,
                    "download" => i.download = v,
                    "total" => i.total = v,
                    "expire" => i.expire = v,
                    _ => {}
                }
            }
        }
        i
    });
    let title = h("profile-title").map(|t| {
        let dec = t.strip_prefix("base64:").map(|b| link::b64(b).unwrap_or_default());
        dec.filter(|d| !d.is_empty()).unwrap_or(t)
    });
    let body = r.text().await.map_err(|e| e.to_string())?;
    let items = vpn_parse(body);
    if items.is_empty() {
        return Err("ساب خالیه یا فرمتش شناخته نشد".into());
    }
    applog::info("vpn", format!("ساب دریافت شد: {} کانفیگ", items.len()));
    Ok(SubResult { items, info, title })
}

#[tauri::command]
pub async fn vpn_core_info(app: AppHandle) -> Result<cores::CoreInfo, String> {
    cores::info(&app).await
}

/// Downloads / updates both cores (Xray first: it runs almost every config).
#[tauri::command]
pub async fn vpn_core_download(app: AppHandle, state: State<'_, VpnState>) -> Result<cores::CoreInfo, String> {
    if state.run.lock().await.is_some() {
        return Err("اول اتصال VPN رو قطع کن".into());
    }
    let _t = state.test.try_lock().map_err(|_| "صبر کن تست تاخیر تموم بشه".to_string())?;
    let mut errs = Vec::new();
    for kind in [Kind::Xray, Kind::SingBox] {
        if let Err(e) = cores::download(&app, kind).await {
            applog::error("vpn", format!("دانلود {}: {e}", kind.name()));
            errs.push(format!("{}: {e}", kind.name()));
        }
    }
    let info = cores::info(&app).await?;
    if errs.len() == 2 {
        return Err(errs.join(" · "));
    }
    if !errs.is_empty() {
        applog::warn("vpn", format!("یکی از هسته‌ها دانلود نشد: {}", errs.join(" · ")));
    }
    Ok(info)
}

#[tauri::command]
pub fn vpn_open_core_dir(app: AppHandle) -> Result<(), String> {
    cores::open_dir(&cores::data_dir(&app)?);
    Ok(())
}

fn kill_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill").args(["/F", "/T", "/PID", &pid.to_string()]).creation_flags(cores::CREATE_NO_WINDOW).output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).output();
    }
}

/// Called on startup: restore a system proxy left behind by a crash.
pub fn init(app: &AppHandle) {
    if let Ok(d) = cores::data_dir(app) {
        sysproxy::init(d.clone());
        std::thread::spawn(move || {
            let n = kill_leftovers(&d);
            if n > 0 {
                applog::warn("vpn", format!("{n} هسته‌ی جامانده از اجرای قبلی بسته شد"));
            }
        });
    }
    sysproxy::recover();
}

/// Kills xray / sing-box processes that run from *our* core folder but aren't ours anymore
/// (crash, `npm run dev` restart). Other apps' cores (v2rayN, Hiddify) live elsewhere and are left alone.
/// Returns how many were killed.
fn kill_leftovers(dir: &Path) -> usize {
    let mine: Vec<u32> = CORE_PIDS.lock().map(|g| g.clone()).unwrap_or_default();
    let skip: Vec<u32> = mine.into_iter().chain([TEST_PID.load(Ordering::SeqCst), TEST_PID2.load(Ordering::SeqCst)]).filter(|p| *p != 0).collect();
    let pids: Vec<u32> = cores_in(dir).into_iter().filter(|p| !skip.contains(p)).collect();
    for p in &pids {
        kill_pid(*p);
    }
    pids.len()
}

/// pids of xray / sing-box processes whose exe lives in `dir`.
#[cfg(windows)]
fn cores_in(dir: &Path) -> Vec<u32> {
    use std::os::windows::process::CommandExt;
    let d = dir.display().to_string().replace('\'', "''");
    let ps = format!(
        "Get-Process xray,sing-box -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -and $_.Path.StartsWith('{d}', [StringComparison]::OrdinalIgnoreCase) }} | ForEach-Object {{ $_.Id }}"
    );
    match std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &ps]).creation_flags(cores::CREATE_NO_WINDOW).output() {
        Ok(o) => String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(not(windows))]
fn cores_in(_dir: &Path) -> Vec<u32> {
    Vec::new()
}

/// Settings tab: "my internet is dead" button. Turns off a system proxy we (or any local client) left behind.
#[tauri::command]
pub async fn vpn_restore_proxy(state: State<'_, VpnState>) -> Result<String, String> {
    if state.run.lock().await.as_ref().map(|r| r.status.system_proxy).unwrap_or(false) {
        return Err("الان با پروکسی سیستم وصلی؛ اول اتصال رو قطع کن".into());
    }
    tauri::async_runtime::spawn_blocking(sysproxy::force_off).await.map_err(|e| e.to_string())
}

/// Settings tab: what the Windows proxy is set to right now.
#[tauri::command]
pub async fn vpn_proxy_info() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(sysproxy::describe).await.map_err(|e| e.to_string())
}

/// Settings tab: close cores left over from an earlier run.
#[tauri::command]
pub async fn vpn_kill_leftovers(app: AppHandle, state: State<'_, VpnState>) -> Result<usize, String> {
    if state.run.lock().await.is_some() {
        return Err("اول اتصال VPN رو قطع کن".into());
    }
    let d = cores::data_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || kill_leftovers(&d)).await.map_err(|e| e.to_string())
}

/// Called synchronously on app exit: the async runtime may already be gone, so no awaits here.
pub fn shutdown() {
    for pid in take_pids() {
        kill_pid(pid);
    }
    kill_pid(TEST_PID.swap(0, Ordering::SeqCst));
    kill_pid(TEST_PID2.swap(0, Ordering::SeqCst));
    if sysproxy::is_on() {
        sysproxy::restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helpers() {
        assert_eq!(bad_outbound("FATAL decode config at test.json: outbounds[12].tls: unknown"), Some(12));
        assert_eq!(bad_outbound("nothing"), None);
        assert_eq!(strip_ansi("\u{1b}[31mERROR\u{1b}[0m x"), "ERROR x");
        assert_eq!(rand_hex().len(), 32);
    }
}
