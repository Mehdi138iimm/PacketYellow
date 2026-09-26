//! Windows system proxy (WinINet registry keys) with backup/restore.
//! The previous settings are also written to disk, so a crash never leaves the user's
//! internet pointing at a dead local port: the next start restores them.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

static ON: AtomicBool = AtomicBool::new(false);
static BACKUP_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
struct Prev {
    enable: bool,
    server: Option<String>,
    overrides: Option<String>,
    ours: String,
    /// PAC script (AutoConfigURL). Windows prefers it over the manual proxy, so a PAC left behind by
    /// another VPN app made our system proxy silently do nothing. Removed while connected, put back after.
    #[serde(default)]
    pac: Option<String>,
}

pub fn init(dir: PathBuf) {
    if let Ok(mut g) = BACKUP_PATH.lock() {
        *g = Some(dir.join("sysproxy-backup.json"));
    }
}

fn backup_file() -> Option<PathBuf> {
    BACKUP_PATH.lock().ok().and_then(|g| g.clone())
}

pub fn is_on() -> bool {
    ON.load(Ordering::SeqCst)
}

#[cfg(windows)]
mod imp {
    use std::os::windows::process::CommandExt;
    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    #[link(name = "wininet")]
    extern "system" {
        fn InternetSetOptionW(h: *mut core::ffi::c_void, opt: u32, buf: *mut core::ffi::c_void, len: u32) -> i32;
    }

    fn reg(args: &[&str]) -> Result<String, String> {
        let out = std::process::Command::new("reg").args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }

    pub fn read() -> (bool, Option<String>, Option<String>) {
        let (en, srv, ovr, _) = read_all();
        (en, srv, ovr)
    }

    /// (ProxyEnable, ProxyServer, ProxyOverride, AutoConfigURL)
    pub fn read_all() -> (bool, Option<String>, Option<String>, Option<String>) {
        let t = reg(&["query", KEY]).unwrap_or_default();
        let (mut en, mut srv, mut ovr, mut pac) = (false, None, None, None);
        for line in t.lines() {
            let p: Vec<&str> = line.split_whitespace().collect();
            if p.len() < 3 {
                continue;
            }
            match p[0] {
                "ProxyEnable" => en = p[2] == "0x1",
                "ProxyServer" => srv = Some(p[2..].join(" ")),
                "ProxyOverride" => ovr = Some(p[2..].join(" ")),
                "AutoConfigURL" => pac = Some(p[2..].join(" ")),
                _ => {}
            }
        }
        (en, srv, ovr, pac)
    }

    /// Some(url) = set the PAC script, None = remove it.
    pub fn set_pac(pac: Option<&str>) -> Result<(), String> {
        match pac {
            Some(u) => reg(&["add", KEY, "/v", "AutoConfigURL", "/t", "REG_SZ", "/d", u, "/f"]).map(|_| ()),
            None => {
                let _ = reg(&["delete", KEY, "/v", "AutoConfigURL", "/f"]); // fails when it doesn't exist: fine
                Ok(())
            }
        }
    }

    pub fn write(enable: bool, server: Option<&str>, overrides: Option<&str>) -> Result<(), String> {
        if let Some(s) = server {
            reg(&["add", KEY, "/v", "ProxyServer", "/t", "REG_SZ", "/d", s, "/f"])?;
        }
        if let Some(o) = overrides {
            reg(&["add", KEY, "/v", "ProxyOverride", "/t", "REG_SZ", "/d", o, "/f"])?;
        }
        reg(&["add", KEY, "/v", "ProxyEnable", "/t", "REG_DWORD", "/d", if enable { "1" } else { "0" }, "/f"])?;
        // tell running apps (browsers, WinINet) to re-read the settings right now
        unsafe {
            InternetSetOptionW(std::ptr::null_mut(), 39, std::ptr::null_mut(), 0); // SETTINGS_CHANGED
            InternetSetOptionW(std::ptr::null_mut(), 37, std::ptr::null_mut(), 0); // REFRESH
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn read() -> (bool, Option<String>, Option<String>) {
        (false, None, None)
    }
    pub fn read_all() -> (bool, Option<String>, Option<String>, Option<String>) {
        (false, None, None, None)
    }
    pub fn set_pac(_: Option<&str>) -> Result<(), String> {
        Ok(())
    }
    pub fn write(_: bool, _: Option<&str>, _: Option<&str>) -> Result<(), String> {
        Err("تنظیم خودکار پروکسی سیستم فقط روی ویندوز هست؛ پروکسی رو دستی روی 127.0.0.1 بذار".into())
    }
}

const BYPASS: &str = "localhost;127.*;10.*;172.16.*;172.17.*;172.18.*;172.19.*;172.20.*;172.21.*;172.22.*;172.23.*;172.24.*;172.25.*;172.26.*;172.27.*;172.28.*;172.29.*;172.30.*;172.31.*;192.168.*;<local>";

pub fn set(port: u16) -> Result<(), String> {
    let ours = format!("127.0.0.1:{port}");
    let (enable, server, overrides, pac) = imp::read_all();
    // don't back up our own settings (e.g. reconnect after a crash)
    let already_ours = enable && server.as_deref() == Some(ours.as_str());
    if !already_ours {
        let prev = Prev { enable, server, overrides, ours: ours.clone(), pac: pac.clone() };
        if let Some(f) = backup_file() {
            let _ = std::fs::write(f, serde_json::to_vec(&prev).unwrap_or_default());
        }
    }
    if pac.is_some() {
        imp::set_pac(None)?;
        crate::applog::info("vpn", "اسکریپت PAC قبلی ویندوز موقتاً برداشته شد (جلوی پروکسی رو می‌گرفت)");
    }
    imp::write(true, Some(&ours), Some(BYPASS))?;
    ON.store(true, Ordering::SeqCst);
    // read it back: group policy / another VPN app can overwrite the keys right away
    let (en, cur, _, pac_now) = imp::read_all();
    if cfg!(windows) && (!en || cur.as_deref() != Some(ours.as_str()) || pac_now.is_some()) {
        return Err(format!("ویندوز پروکسی رو قبول نکرد (الان: {}). یه برنامه‌ی دیگه (v2rayN، Hiddify، ...) یا Group Policy داره عوضش می‌کنه", cur.unwrap_or_else(|| "خالی".into())));
    }
    crate::applog::info("vpn", format!("پروکسی سیستم روی {ours} تنظیم شد"));
    Ok(())
}

/// What Windows is using right now (for the settings tab / diagnostics).
pub fn describe() -> String {
    let (en, srv, _, pac) = imp::read_all();
    match (en, srv, pac) {
        (_, _, Some(p)) => format!("PAC: {p}"),
        (true, Some(s), _) => format!("روشن · {s}"),
        _ => "خاموش".into(),
    }
}

/// Emergency button: "my internet is dead after closing the app". Turns the proxy off if it points
/// at 127.0.0.1 (ours or a dead leftover of any local client), otherwise restores the backup.
pub fn force_off() -> String {
    if backup_file().map(|f| f.exists()).unwrap_or(false) {
        ON.store(true, Ordering::SeqCst);
        restore();
        return describe();
    }
    let (en, srv, _) = imp::read();
    if en && srv.as_deref().map(|s| s.contains("127.0.0.1") || s.contains("localhost")).unwrap_or(false) {
        let _ = imp::write(false, None, None);
        crate::applog::info("vpn", "پروکسی محلی جامانده خاموش شد");
    }
    describe()
}

/// Restore what the user had before we touched it. Safe to call many times.
pub fn restore() {
    let file = backup_file();
    let prev: Option<Prev> = file.as_ref().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok());
    let was_on = ON.swap(false, Ordering::SeqCst);
    let (en, cur, _) = imp::read();
    match prev {
        Some(p) => {
            // only touch the registry if it still points at us (the user may have changed it meanwhile)
            if !en || cur.as_deref() == Some(p.ours.as_str()) {
                let _ = imp::write(p.enable, p.server.as_deref(), p.overrides.as_deref());
                if p.pac.is_some() {
                    let _ = imp::set_pac(p.pac.as_deref());
                }
                crate::applog::info("vpn", "پروکسی سیستم به حالت قبل برگشت");
            }
        }
        None if was_on => {
            let _ = imp::write(false, None, None);
        }
        None => {}
    }
    if let Some(f) = file {
        let _ = std::fs::remove_file(f);
    }
}

/// On startup: a backup file means the app died while connected -> put things back.
pub fn recover() {
    if backup_file().map(|f| f.exists()).unwrap_or(false) {
        crate::applog::warn("vpn", "برنامه دفعه‌ی قبل درست بسته نشد؛ پروکسی سیستم بازگردانی شد");
        ON.store(true, Ordering::SeqCst);
        restore();
    }
}
