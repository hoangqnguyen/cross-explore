mod commands;
mod places;

use tauri::Manager;

fn build_vfs() -> commands::VfsState {
    use std::sync::Arc;
    cx_core::Vfs::new(Arc::new(cx_local::LocalProvider), Arc::new(cx_core::MemoryCredentials::default()))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(commands::Watches::default())
        .manage(build_vfs())
        .invoke_handler(tauri::generate_handler![
            commands::list_dir,
            commands::watch_dir,
            commands::unwatch_dir,
            commands::create_folder,
            commands::rename_entry,
            commands::trash_entries,
            commands::open_entry,
            commands::ui_log,
            places::places,
            commands::free_space,
        ])
        .setup(|app| {
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
