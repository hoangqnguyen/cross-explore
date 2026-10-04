//! The macOS menu bar, Finder-style. Its Go menu gives Back / Forward /
//! Enclosing Folder real menu items with the standard shortcuts, so tools
//! that map mouse buttons to keystrokes (Logi Options+, BetterTouchTool…)
//! always reach them. Items run UI commands by id.

use crate::events::Events;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The items that run commands, by command id, so their shortcuts can follow
/// the user's own key bindings.
#[derive(Default)]
pub struct MenuKeys(Mutex<HashMap<String, tauri::menu::MenuItem<tauri::Wry>>>);

/// Set the shortcut of each named menu item (`None` = no shortcut). The UI
/// sends these when the user changes key bindings, so a key given to another
/// command stops being taken by the menu. Unparsable keys leave the item without one.
#[tauri::command]
pub async fn menu_set_keys(keys: HashMap<String, Option<String>>, state: tauri::State<'_, MenuKeys>) -> Result<(), String> {
    let items = state.0.lock().unwrap().clone();
    for (id, accel) in keys {
        let Some(item) = items.get(&id) else { continue };
        if item.set_accelerator(accel.as_deref()).is_err() {
            let _ = item.set_accelerator(None::<&str>);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn install(app: &tauri::App, events: Arc<Events>) -> tauri::Result<()> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
    use tauri::Manager;
    let h = app.handle();
    let keys = app.state::<MenuKeys>();
    let item = |id: &str, label: &str, accel: &str| {
        let item = MenuItemBuilder::with_id(id, label).accelerator(accel).build(h)?;
        keys.0.lock().unwrap().insert(id.to_string(), item.clone());
        Ok::<_, tauri::Error>(item)
    };

    let app_menu = SubmenuBuilder::new(h, "Cross Explore")
        .about(None)
        .separator()
        .item(&item("app.settings", "Settings…", "Cmd+,")?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(h, "File")
        .item(&item("tab.new", "New Tab", "Cmd+T")?)
        .item(&item("file.newFolder", "New Folder", "Cmd+Shift+N")?)
        .separator()
        .item(&item("file.copyTo", "Copy To…", "Shift+F5")?)
        .item(&item("file.moveTo", "Move To…", "Shift+F6")?)
        .separator()
        .item(&item("tab.close", "Close Tab", "Cmd+W")?)
        .build()?;
    // Standard edit actions keep working in text fields; the file list
    // handles the same shortcuts itself.
    let edit = SubmenuBuilder::new(h, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let go = SubmenuBuilder::new(h, "Go")
        .item(&item("nav.back", "Back", "Cmd+[")?)
        .item(&item("nav.forward", "Forward", "Cmd+]")?)
        .item(&item("nav.up", "Enclosing Folder", "Cmd+Up")?)
        .separator()
        .item(&item("nav.home", "Home", "Cmd+Shift+H")?)
        .item(&item("nav.editPath", "Go to Folder…", "Cmd+Shift+G")?)
        .item(&item("net.connect", "Connect to Server…", "Cmd+K")?)
        .build()?;
    let window = SubmenuBuilder::new(h, "Window")
        .minimize()
        .maximize()
        .separator()
        .fullscreen()
        .build()?;
    let menu = MenuBuilder::new(h)
        .items(&[&app_menu, &file, &edit, &go, &window])
        .build()?;
    app.set_menu(menu)?;
    app.on_menu_event(move |_, ev| {
        events.emit("command", serde_json::json!({ "id": ev.id().0 }));
    });
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &tauri::App, _events: Arc<Events>) -> tauri::Result<()> {
    Ok(())
}
