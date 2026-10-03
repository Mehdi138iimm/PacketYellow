//! Locating, versioning and downloading the two cores: Xray (main) and sing-box (Hysteria2 / TUIC + TUN).
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Xray,
    SingBox,
}

impl Kind {
    pub fn exe(self) -> &'static str {
        match (self, cfg!(windows)) {
            (Kind::Xray, true) => "xray.exe",
            (Kind::Xray, false) => "xray",
            (Kind::SingBox, true) => "sing-box.exe",
            (Kind::SingBox, false) => "sing-box",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Xray => "Xray",
            Kind::SingBox => "sing-box",
        }
    }
    fn repo(self) -> &'static str {
        match self {
            Kind::Xray => "XTLS/Xray-core",
            Kind::SingBox => "SagerNet/sing-box",
        }
    }
    /// Release asset for this machine (Windows only).
    fn asset_matches(self, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        match (self, std::env::consts::ARCH) {
            (Kind::Xray, "x86_64") => n == "xray-windows-64.zip",
            (Kind::Xray, "aarch64") => n == "xray-windows-arm64-v8a.zip",
            (Kind::Xray, "x86") => n == "xray-windows-32.zip",
            (Kind::SingBox, "x86_64") => n.ends_with("-windows-amd64.zip"),
            (Kind::SingBox, "aarch64") => n.ends_with("-windows-arm64.zip"),
            (Kind::SingBox, "x86") => n.ends_with("-windows-386.zip"),
            _ => false,
        }
    }
}

pub fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let d = app.path().app_data_dir().map_err(|e| e.to_string())?.join("core");
    std::fs::create_dir_all(&d).map_err(|e| format!("ساخت پوشه‌ی هسته: {e}"))?;
    Ok(d)
}

pub fn find(app: &AppHandle, kind: Kind) -> Option<PathBuf> {
    let exe = kind.exe();
    let mut c = Vec::new();
    if let Ok(d) = data_dir(app) {
        c.push(d.join(exe));
    }
    if let Ok(me) = std::env::current_exe() {
        if let Some(dir) = me.parent() {
            c.push(dir.join(exe));
            c.push(dir.join("core").join(exe));
        }
    }
    if let Ok(r) = app.path().resource_dir() {
        c.push(r.join(exe));
    }
    c.into_iter().find(|p| p.is_file())
}

pub fn std_cmd(exe: &Path) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// "sing-box version 1.14.2" -> 1.14.2 · "Xray 26.7.28 (Xray, Penetrates Everything.) ..." -> 26.7.28
pub fn version(exe: &Path, kind: Kind) -> Option<String> {
    let out = std_cmd(exe).arg("version").output().ok()?;
    let t = String::from_utf8_lossy(&out.stdout);
    let first = t.lines().next()?;
    match kind {
        Kind::Xray => first.split_whitespace().nth(1).map(String::from),
        Kind::SingBox => first.split_whitespace().last().map(String::from),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreInfo {
    /// sing-box (kept as `path` / `version` for older UI code)
    path: Option<String>,
    version: Option<String>,
    xray_path: Option<String>,
    xray_version: Option<String>,
    dir: String,
    can_download: bool,
}

async fn ver_of(app: &AppHandle, kind: Kind) -> (Option<String>, Option<String>) {
    let path = find(app, kind);
    let version = match path.clone() {
        Some(p) => tauri::async_runtime::spawn_blocking(move || version(&p, kind)).await.ok().flatten(),
        None => None,
    };
    (path.map(|p| p.display().to_string()), version)
}

pub async fn info(app: &AppHandle) -> Result<CoreInfo, String> {
    let dir = data_dir(app)?;
    let (path, version) = ver_of(app, Kind::SingBox).await;
    let (xray_path, xray_version) = ver_of(app, Kind::Xray).await;
    Ok(CoreInfo { path, version, xray_path, xray_version, dir: dir.display().to_string(), can_download: cfg!(windows) })
}

#[derive(Serialize, Clone)]
struct Progress {
    core: &'static str,
    pct: f64,
    mb: f64,
}

/// Exact release file name for this machine (used when the GitHub API is not reachable).
fn asset_name(kind: Kind, tag: &str) -> Option<String> {
    let v = tag.trim_start_matches('v');
    Some(match (kind, std::env::consts::ARCH) {
        (Kind::Xray, "x86_64") => "Xray-windows-64.zip".to_string(),
        (Kind::Xray, "aarch64") => "Xray-windows-arm64-v8a.zip".to_string(),
        (Kind::Xray, "x86") => "Xray-windows-32.zip".to_string(),
        (Kind::SingBox, "x86_64") => format!("sing-box-{v}-windows-amd64.zip"),
        (Kind::SingBox, "aarch64") => format!("sing-box-{v}-windows-arm64.zip"),
        (Kind::SingBox, "x86") => format!("sing-box-{v}-windows-386.zip"),
        _ => return None,
    })
}

/// 1) GitHub API (gives size + skips pre-releases)
async fn api_asset(client: &reqwest::Client, kind: Kind) -> Result<(String, String, u64), String> {
    let rels: Vec<serde_json::Value> = client
        .get(format!("https://api.github.com/repos/{}/releases?per_page=20", kind.repo()))
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("اتصال به GitHub: {}", crate::probe::err_chain(&e)))?
        .error_for_status()
        .map_err(|e| format!("GitHub API: {e}"))?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    rels.iter()
        .filter(|r| !r["prerelease"].as_bool().unwrap_or(true) && !r["draft"].as_bool().unwrap_or(true))
        .find_map(|r| {
            r["assets"].as_array()?.iter().find_map(|a| {
                let n = a["name"].as_str()?;
                if kind.asset_matches(n) {
                    Some((r["tag_name"].as_str().unwrap_or("?").to_string(), a["browser_download_url"].as_str()?.to_string(), a["size"].as_u64().unwrap_or(0)))
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| format!("فایل {} برای این سیستم پیدا نشد", kind.name()))
}

/// 2) no API quota needed: github.com/<repo>/releases/latest redirects to .../releases/tag/<tag>.
/// The API answers 403 (60 requests/hour per IP) on Iranian CGNAT lines where hundreds of users share
/// one IP, which used to make "download cores" fail -> no core -> the whole VPN tab did nothing.
async fn latest_tag(kind: Kind, proxy: bool) -> Result<String, String> {
    let mut b = reqwest::Client::builder().user_agent(crate::probe::UA).connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none());
    if !proxy {
        b = b.no_proxy();
    }
    let c = b.build().map_err(|e| e.to_string())?;
    let r = c.get(format!("https://github.com/{}/releases/latest", kind.repo())).send().await.map_err(|e| format!("اتصال به github.com: {}", crate::probe::err_chain(&e)))?;
    let loc = r.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    loc.rsplit_once("/tag/").map(|(_, t)| t.trim().to_string()).filter(|t| !t.is_empty()).ok_or_else(|| format!("آخرین نسخه‌ی {} پیدا نشد (HTTP {})", kind.name(), r.status().as_u16()))
}

fn dl_client(proxy: bool) -> Result<reqwest::Client, String> {
    let mut b = reqwest::Client::builder().user_agent(crate::probe::UA).connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(900));
    if !proxy {
        b = b.no_proxy();
    }
    b.build().map_err(|e| e.to_string())
}

/// (tag, url, size): API first, then the redirect trick; each with the system proxy and without it
/// (a dead proxy left behind by another client must not block the download).
async fn find_asset(kind: Kind) -> Result<(String, String, u64, bool), String> {
    let mut errs: Vec<String> = Vec::new();
    for proxy in [true, false] {
        let client = dl_client(proxy)?;
        match api_asset(&client, kind).await {
            Ok((t, u, s)) => return Ok((t, u, s, proxy)),
            Err(e) => errs.push(e),
        }
        match latest_tag(kind, proxy).await {
            Ok(tag) => {
                if let Some(name) = asset_name(kind, &tag) {
                    let url = format!("https://github.com/{}/releases/download/{tag}/{name}", kind.repo());
                    return Ok((tag, url, 0, proxy));
                }
                errs.push(format!("فایل {} برای این سیستم پیدا نشد", kind.name()));
            }
            Err(e) => errs.push(e),
        }
    }
    errs.dedup();
    Err(errs.join(" · "))
}

/// Downloads the newest stable release of one core into the core folder.
pub async fn download(app: &AppHandle, kind: Kind) -> Result<String, String> {
    use futures_util::StreamExt;
    if !cfg!(windows) {
        return Err(format!("دانلود خودکار فقط برای ویندوز هست؛ {} رو دستی تو پوشه‌ی هسته بذار", kind.exe()));
    }
    let (tag, url, size, proxy) = find_asset(kind).await?;
    let client = dl_client(proxy)?;
    crate::applog::info("vpn", format!("دانلود {} {tag}: {url}", kind.name()));
    let resp = client.get(&url).send().await.map_err(|e| format!("دانلود: {}", crate::probe::err_chain(&e)))?.error_for_status().map_err(|e| e.to_string())?;
    let total = resp.content_length().unwrap_or(size).max(1);
    let mut buf: Vec<u8> = Vec::with_capacity(total.min(200_000_000) as usize);
    let mut stream = resp.bytes_stream();
    let mut last = 0.0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("دانلود قطع شد: {}", crate::probe::err_chain(&e)))?;
        buf.extend_from_slice(&chunk);
        let pct = (buf.len() as f64 / total as f64 * 100.0).min(100.0);
        if pct - last >= 1.0 {
            last = pct;
            let _ = app.emit("vpn://core", Progress { core: kind.name(), pct, mb: buf.len() as f64 / 1e6 });
        }
    }
    let dir = data_dir(app)?;
    tauri::async_runtime::spawn_blocking(move || extract(&buf, &dir, kind)).await.map_err(|e| e.to_string())??;
    crate::applog::info("vpn", format!("{} {tag} نصب شد", kind.name()));
    Ok(tag)
}

fn extract(zip_bytes: &[u8], dir: &Path, kind: Kind) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).map_err(|e| format!("فایل zip خرابه: {e}"))?;
    let mut got_exe = false;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        if !f.is_file() {
            continue;
        }
        let name = f.name().rsplit('/').next().unwrap_or("").to_string();
        let lower = name.to_ascii_lowercase();
        let wanted = lower.ends_with(".exe") || lower.ends_with(".dll") || lower.ends_with(".dat");
        if !wanted || name.contains("..") {
            continue;
        }
        let tmp = dir.join(format!("{name}.part"));
        {
            let mut out = std::fs::File::create(&tmp).map_err(|e| format!("نوشتن {name}: {e}"))?;
            std::io::copy(&mut f, &mut out).map_err(|e| format!("نوشتن {name}: {e}"))?;
        }
        std::fs::rename(&tmp, dir.join(&name)).map_err(|e| format!("جایگزینی {name} (برنامه‌ی دیگه‌ای ازش استفاده می‌کنه؟): {e}"))?;
        got_exe |= lower == kind.exe();
    }
    if !got_exe {
        return Err(format!("{} داخل فایل دانلودی نبود", kind.exe()));
    }
    Ok(())
}

pub fn open_dir(dir: &Path) {
    let prog = if cfg!(windows) { "explorer" } else if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(prog).arg(dir).spawn();
}
