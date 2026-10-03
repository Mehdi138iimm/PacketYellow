//! App-level helpers for the «تنظیمات» (settings) tab: version, admin rights, relaunch as admin.
use serde::Serialize;
use tauri::AppHandle;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    version: String,
    os: &'static str,
    arch: &'static str,
    admin: bool,
    exe: String,
}

/// true when the process runs elevated ("Run as administrator"). TUN needs this on Windows.
#[cfg(windows)]
pub fn is_admin() -> bool {
    #[link(name = "shell32")]
    extern "system" {
        fn IsUserAnAdmin() -> i32;
    }
    unsafe { IsUserAnAdmin() != 0 }
}

#[cfg(not(windows))]
pub fn is_admin() -> bool {
    std::process::Command::new("id").arg("-u").output().map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0").unwrap_or(false)
}

#[tauri::command]
pub fn app_info(app: AppHandle) -> AppInfo {
    AppInfo {
        version: app.package_info().version.to_string(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        admin: is_admin(),
        exe: std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
    }
}

/// Starts a second, elevated copy of the app (UAC prompt) and closes this one.
/// The exit path (`RunEvent::Exit`) stops the VPN and restores the system proxy first.
#[tauri::command]
pub async fn app_relaunch_admin(app: AppHandle) -> Result<(), String> {
    if is_admin() {
        return Err("برنامه همین الان با دسترسی ادمین بازه".into());
    }
    tauri::async_runtime::spawn_blocking(start_elevated).await.map_err(|e| e.to_string())??;
    crate::applog::info("app", "برنامه با دسترسی ادمین دوباره باز شد");
    app.exit(0);
    Ok(())
}

#[cfg(windows)]
fn start_elevated() -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // single quotes inside a PowerShell string are escaped by doubling them
    let ps = format!("Start-Process -FilePath '{}' -Verb RunAs", exe.display().to_string().replace('\'', "''"));
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &ps])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| format!("اجرای PowerShell: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        // the user pressed "No" on the UAC prompt
        Err("دسترسی ادمین داده نشد (پنجره‌ی UAC رد شد)".into())
    }
}

#[cfg(not(windows))]
fn start_elevated() -> Result<(), String> {
    Err("این گزینه فقط روی ویندوز هست".into())
}

// ---------------------------------------------------------------------------------------------
// Update check (runs in Rust, not in the WebView)
//
// Why this moved out of main.js:
// * `/releases/latest` never returns pre-releases, and PacketYellow ships betas as pre-releases,
//   so an app on 0.5.0 never heard about 0.5.1.
// * The GitHub API allows 60 requests/hour per IP without a token. On Iranian CGNAT lines thousands
//   of users share one IP, so the API answers 403 most of the time and the check silently failed.
//   The public `releases.atom` feed is not rate limited the same way, so it is the fallback.
// ---------------------------------------------------------------------------------------------

const REPO: &str = "Mehdi138iimm/PacketYellow";

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    current: String,
    latest: Option<String>,
    url: String,
    /// the latest release is marked «Pre-release» on GitHub
    prerelease: bool,
    newer: bool,
    /// "stable" / "beta": how the INSTALLED version is published on GitHub (None = this version isn't on GitHub yet)
    current_channel: Option<&'static str>,
    /// "api" or "atom": which source answered
    source: &'static str,
}

/// "v0.5.1" / "0.5.1-beta" -> [0, 5, 1]
fn ver_num(v: &str) -> [u64; 3] {
    let mut out = [0u64; 3];
    let v = v.trim().trim_start_matches(['v', 'V']);
    for (i, part) in v.split(['.', '-', '+']).take(3).enumerate() {
        out[i] = part.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().unwrap_or(0);
    }
    out
}

fn is_newer(a: &str, b: &str) -> bool {
    ver_num(a) > ver_num(b)
}

fn chan(pre: bool) -> &'static str {
    if pre { "beta" } else { "stable" }
}

struct Rel {
    tag: String,
    url: String,
    /// None = unknown (the atom feed doesn't say)
    prerelease: Option<bool>,
}

async fn from_api(c: &reqwest::Client) -> Result<Vec<Rel>, String> {
    let r = c
        .get(format!("https://api.github.com/repos/{REPO}/releases?per_page=30"))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| crate::probe::err_chain(&e))?;
    if !r.status().is_success() {
        return Err(format!("GitHub API: HTTP {}", r.status().as_u16()));
    }
    let list: Vec<serde_json::Value> = r.json().await.map_err(|e| e.to_string())?;
    Ok(list
        .into_iter()
        .filter(|x| !x["draft"].as_bool().unwrap_or(false))
        .filter_map(|x| {
            Some(Rel {
                tag: x["tag_name"].as_str()?.to_string(),
                url: x["html_url"].as_str().unwrap_or_default().to_string(),
                prerelease: Some(x["prerelease"].as_bool().unwrap_or(false)),
            })
        })
        .collect())
}

async fn from_atom(c: &reqwest::Client) -> Result<Vec<Rel>, String> {
    let r = c
        .get(format!("https://github.com/{REPO}/releases.atom"))
        .send()
        .await
        .map_err(|e| crate::probe::err_chain(&e))?;
    if !r.status().is_success() {
        return Err(format!("releases.atom: HTTP {}", r.status().as_u16()));
    }
    let body = r.text().await.map_err(|e| e.to_string())?;
    let marker = "/releases/tag/";
    let mut out: Vec<Rel> = Vec::new();
    let mut rest = body.as_str();
    while let Some(i) = rest.find(marker) {
        let after = &rest[i + marker.len()..];
        let end = after.find(['"', '<', '\'', ' ']).unwrap_or(after.len());
        let tag = after[..end].to_string();
        if !tag.is_empty() && !out.iter().any(|x| x.tag == tag) {
            out.push(Rel { url: format!("https://github.com/{REPO}/releases/tag/{tag}"), tag, prerelease: None });
        }
        rest = &after[end..];
    }
    if out.is_empty() {
        return Err("releases.atom: هیچ نسخه‌ای پیدا نشد".into());
    }
    Ok(out)
}

/// The atom feed has no «Pre-release» flag, so read it from the release page itself.
async fn page_prerelease(c: &reqwest::Client, tag: &str) -> Option<bool> {
    let r = c.get(format!("https://github.com/{REPO}/releases/tag/{tag}")).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let body = r.text().await.ok()?;
    Some(body.contains(">Pre-release<") || (body.contains("Label--warning") && body.contains("Pre-release")))
}

/// Network errors in plain Persian (the raw text still goes to the log as a warning).
fn friendly(e: &str) -> String {
    let l = e.to_lowercase();
    if l.contains("timed out") || l.contains("timeout") {
        "گیت‌هاب جواب نداد (تایم‌اوت). احتمالاً گیت‌هاب روی اینترنتت کُنده یا فیلتره؛ با VPN دوباره امتحان کن".into()
    } else if l.contains("http 403") || l.contains("http 429") {
        "گیت‌هاب فعلاً اجازه‌ی درخواست نمی‌ده (محدودیت تعداد درخواست). چند دقیقه‌ی دیگه دوباره امتحان کن".into()
    } else if l.contains("http 5") {
        "سرور گیت‌هاب الان خطا می‌ده (HTTP 5xx). چند دقیقه‌ی دیگه دوباره امتحان کن".into()
    } else if l.contains("dns") || l.contains("resolve") || l.contains("connect") {
        "به گیت‌هاب وصل نشد. اینترنت یا فیلترینگ رو چک کن، یا با VPN امتحان کن".into()
    } else {
        "بررسی نسخه‌ی جدید نشد. بعداً دوباره امتحان کن".into()
    }
}

/// ONLY CHECKS: reads the release list from GitHub and compares it with the installed version.
/// Nothing is downloaded or installed; the UI just shows a button that opens the download page.
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    let c = crate::probe::client(std::time::Duration::from_secs(15))?;
    // both sources at once: on filtered lines one of them often answers while the other hangs
    let (api, atom) = tokio::join!(from_api(&c), from_atom(&c));
    let (list, source) = match (api, atom) {
        (Ok(l), _) if !l.is_empty() => (l, "api"),
        (_, Ok(l)) => (l, "atom"),
        (a, Err(e)) => {
            let why = a.err().unwrap_or_else(|| "لیست خالی".into());
            crate::applog::warn("update", format!("بررسی نسخه‌ی جدید نشد (فقط بررسی، آپدیت خودکاری در کار نیست): {why} · {e}"));
            return Err(friendly(&format!("{why} · {e}")));
        }
    };
    // highest version wins, not the newest date: a hotfix for an older line must not look like an update
    let mut best = list.iter().max_by(|a, b| ver_num(&a.tag).cmp(&ver_num(&b.tag))).map(|r| Rel { tag: r.tag.clone(), url: r.url.clone(), prerelease: r.prerelease });
    let mine = list.iter().find(|r| ver_num(&r.tag) == ver_num(&current)).map(|r| (r.tag.clone(), r.prerelease));
    let current_channel = match &mine {
        Some((_, Some(p))) => Some(chan(*p)),
        Some((tag, None)) => page_prerelease(&c, tag).await.map(chan),
        None => None,
    };
    if let Some(b) = best.as_mut() {
        if b.prerelease.is_none() {
            b.prerelease = if mine.as_ref().map(|m| ver_num(&m.0)) == Some(ver_num(&b.tag)) {
                current_channel.map(|c| c == "beta")
            } else {
                page_prerelease(&c, &b.tag).await
            };
        }
    }
    let newer = best.as_ref().map(|b| is_newer(&b.tag, &current)).unwrap_or(false);
    if newer {
        if let Some(b) = &best {
            crate::applog::info("update", format!("نسخه‌ی جدید منتشر شده: {} (الان {current}) · منبع: {source}", b.tag));
        }
    }
    Ok(UpdateInfo {
        current,
        url: best.as_ref().map(|b| b.url.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| format!("https://github.com/{REPO}/releases")),
        prerelease: best.as_ref().and_then(|b| b.prerelease).unwrap_or(false),
        latest: best.map(|b| b.tag),
        newer,
        current_channel,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn versions() {
        assert!(is_newer("v0.5.1", "0.5.0"));
        assert!(is_newer("v0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0-beta", "0.5.1"));
        assert!(!is_newer("v0.5.1", "0.5.1"));
        assert!(!is_newer("v0.4.3", "0.5.0"));
    }
}
