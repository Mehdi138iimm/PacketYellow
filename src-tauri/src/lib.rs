mod applog;
mod dns;
mod gameping;
mod monitor;
mod netinfo;
mod ping;
mod probe;
mod sites;
mod speed;
mod stats;
mod sys;
mod vpn;



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
        .manage(gameping::LiveState::default())
        .manage(vpn::VpnState::default())
        .setup(|app| {
            applog::init(app.handle().clone());
            vpn::init(app.handle());
            let v = app.package_info().version.to_string();
            applog::info("app", format!("PacketYellow v{v} (بتا) شروع شد · {} {} · ادمین: {}", std::env::consts::OS, std::env::consts::ARCH, if sys::is_admin() { "بله" } else { "نه" }));
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
            gameping::game_live_start,
            gameping::game_live_stop,
            gameping::game_probe,
            vpn::vpn_connect,
            vpn::vpn_disconnect,
            vpn::vpn_status,
            vpn::vpn_check,
            vpn::vpn_delay_test,
            vpn::vpn_parse,
            vpn::vpn_inspect,
            vpn::vpn_fetch_sub,
            vpn::vpn_core_info,
            vpn::vpn_core_download,
            vpn::vpn_open_core_dir,
            vpn::vpn_restore_proxy,
            vpn::vpn_proxy_info,
            vpn::vpn_kill_leftovers,
            sys::app_info,
            sys::app_relaunch_admin,
            sys::check_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building PacketYellow")
        .run(|_app, event| {
            // never leave sing-box running or the system proxy pointing at a dead port
            if let tauri::RunEvent::Exit = event {
                vpn::shutdown();
            }
        });
}
