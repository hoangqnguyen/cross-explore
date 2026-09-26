mod cmd;
mod credentials;
mod events;
mod jobs;
mod places;
mod protocols;
mod sftp;
mod state;
mod tags;

use cx_core::Vfs;
use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_drag::init());
    builder
        .manage(cmd::files::Watches::default())
        .manage(cmd::files::RecentListings::default())
        .manage(cx_term::Terminals::new())
        .register_asynchronous_uri_scheme_protocol("cxfile", protocols::file_protocol)
        .register_asynchronous_uri_scheme_protocol("cxthumb", protocols::thumb_protocol)
        .invoke_handler(tauri::generate_handler![
            cmd::files::list_dir,
            cmd::files::watch_dir,
            cmd::files::unwatch_dir,
            cmd::files::stat_entry,
            cmd::files::create_folder,
            cmd::files::rename_entry,
            cmd::files::trash_entries,
            cmd::files::free_space,
            cmd::files::dir_size,
            cmd::files::preview_text,
            cmd::files::ui_log,
            cmd::files::subscribe,
            cmd::system::open_entry,
            cmd::system::reveal_entry,
            cmd::system::open_terminal,
            cmd::system::os_clipboard_set,
            cmd::system::os_clipboard_get,
            cmd::system::drag_icon,
            cmd::jobs::transfer_submit,
            cmd::jobs::transfer_pause,
            cmd::jobs::transfer_resume,
            cmd::jobs::transfer_cancel,
            cmd::jobs::transfer_resolve,
            cmd::jobs::transfer_list,
            cmd::jobs::transfer_clear,
            cmd::jobs::undo,
            cmd::jobs::compare_dirs,
            cmd::search::search_start,
            cmd::search::cancel_task,
            cmd::net::connect_server,
            cmd::net::disconnect_server,
            cmd::net::connections,
            cmd::net::trust_host_key,
            cmd::peer::peer_status,
            cmd::peer::peer_set_enabled,
            cmd::peer::peer_set_shares,
            cmd::peer::peer_set_auto_trust,
            cmd::peer::peer_pair_code,
            cmd::peer::peer_pair,
            cmd::peer::peer_forget,
            cmd::peer::peer_send,
            cmd::peer::peer_respond,
            cmd::peer::discovery_devices,
            cmd::peer::discovery_refresh,
            cmd::tags::tags_get,
            cmd::tags::tags_set,
            cmd::tags::tags_find,
            cmd::term::term_open,
            cmd::term::term_write,
            cmd::term::term_resize,
            cmd::term::term_close,
            cmd::term::term_cwd,
            places::places,
            cmd::selftest::selftest_config,
            cmd::selftest::selftest_touch,
            cmd::selftest::selftest_exit,
        ])
        .setup(|app| {
            let state = build_state(app)?;
            app.manage(state.clone());
            cmd::peer::start_discovery(state.clone());
            tauri::async_runtime::spawn(async move {
                if let Err(e) = cmd::peer::start_peer(state).await {
                    eprintln!("peer mode unavailable: {e}");
                }
            });

            let window = app.get_webview_window("main").expect("main window");
            style_window(&window);
            // The UI shows the window after its first paint. If it never gets
            // there (a script error), show it anyway rather than stay invisible.
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                let _ = window.show();
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Cross Explore");
}

fn build_state(app: &tauri::App) -> Result<Arc<state::App>, Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    let cache_dir = app.path().app_cache_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    std::fs::create_dir_all(&cache_dir)?;
    #[cfg(target_os = "android")]
    {
        // No Unix home on Android: the app's own storage is "home", with a
        // Documents folder people can fill from the network.
        let home = data_dir.join("files");
        std::fs::create_dir_all(home.join("Documents"))?;
        std::fs::create_dir_all(home.join("Downloads"))?;
        cx_core::location::set_home(home);
    }

    let vfs = Vfs::new(Arc::new(cx_local::LocalProvider), state::App::credentials());
    vfs.register(Arc::new(cx_smb::SmbConnector::new()));
    vfs.register(Arc::new(cx_webdav::DavConnector::http()));
    vfs.register(Arc::new(cx_webdav::DavConnector::https()));
    sftp::register(&vfs, &data_dir);
    cx_archive::ArchiveProvider::install(&vfs, cache_dir.join("archives"));

    let events = Arc::new(events::Events::default());
    let jobs = jobs::Jobs::new(events.clone());
    let transfers = {
        let jobs = jobs.clone();
        let vfs = vfs.clone();
        let dir = data_dir.join("transfers");
        // The manager spawns onto the current runtime, so build it inside one.
        tauri::async_runtime::block_on(async move { cx_transfer::TransferManager::new(vfs, dir, move |e| jobs.on_transfer(e)) })
    };
    // Jobs interrupted by a quit come back paused, ready to resume.
    for job in transfers.restore_pending() {
        jobs.on_transfer(cx_transfer::TransferEvent::JobAdded { job });
    }
    let thumbs = cx_thumbs::Thumbnailer::new(cache_dir.join("thumbs"), 512 << 20)?;
    Ok(Arc::new(state::App::new(vfs, events, jobs, transfers, thumbs, data_dir, cache_dir)))
}

/// Translucent materials where the OS has them: vibrancy on macOS (the
/// window keeps native traffic lights over our tab strip), Mica on Windows 11
/// (we draw our own caption buttons). Linux keeps an opaque window.
fn style_window(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        places::set_translucent(true);
        use tauri::window::{Effect, EffectState, EffectsBuilder};
        let _ = window.set_effects(EffectsBuilder::new().effect(Effect::Sidebar).state(EffectState::FollowsWindowActiveState).build());
    }
    #[cfg(windows)]
    {
        use tauri::window::{Effect, EffectsBuilder};
        let _ = window.set_decorations(false);
        let _ = window.set_shadow(true);
        // Mica needs Windows 11 (build 22000+); older systems get a solid window.
        let build: u32 = sysinfo::System::kernel_version().and_then(|v| v.parse().ok()).unwrap_or(0);
        if build >= 22000 && window.set_effects(EffectsBuilder::new().effect(Effect::Mica).build()).is_ok() {
            places::set_translucent(true);
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    let _ = window;
}
