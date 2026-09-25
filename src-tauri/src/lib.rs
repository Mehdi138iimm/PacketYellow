mod applog;
mod dns;
mod monitor;
mod netinfo;
mod ping;
mod probe;
mod sites;
mod speed;
mod stats;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Rust panics end up in the in-app log instead of silently killing a task
    std::panic::set_hook(Box::new(|p| {
        let loc = p.location().map(|l| format!(" @ {}:{}", l.file(), l.line())).unwrap_or_default();
        applog::error("panic", format!("{p}{loc}"));
    }));
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(monitor::MonitorState::default())
        .setup(|app| {
            applog::init(app.handle().clone());
            let v = app.package_info().version.to_string();
            applog::info("app", format!("PacketYellow v{v} شروع شد · {} {}", std::env::consts::OS, std::env::consts::ARCH));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ping::ping_host,
            ping::ping_many,
            ping::steam_servers,
            sites::check_sites,
            dns::dns_benchmark,
            speed::speed_test,
            speed::speed_cancel,
            netinfo::network_info,
            netinfo::net_signature,
            monitor::start_monitor,
            monitor::stop_monitor,
            applog::get_logs,
            applog::clear_logs,
            applog::log_from_ui,
        ])
        .run(tauri::generate_context!())
        .expect("error while running PacketYellow");
}
