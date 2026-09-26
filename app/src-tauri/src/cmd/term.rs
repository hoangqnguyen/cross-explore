//! Embedded terminal sessions (a PTY per panel), streamed to xterm.js.

use cx_core::{Location, Result};
use cx_term::Terminals;
use tauri::ipc::Channel;
use tauri::State;

#[tauri::command]
pub fn term_open(uri: String, cols: u16, rows: u16, on_event: Channel<serde_json::Value>, terms: State<'_, Terminals>) -> Result<u64> {
    let loc = Location::parse(&uri)?;
    terms.open(&loc, cols, rows, move |e| {
        if let Ok(v) = serde_json::to_value(&e) {
            let _ = on_event.send(v);
        }
    })
}

#[tauri::command]
pub fn term_write(id: u64, data: String, terms: State<'_, Terminals>) -> Result<()> {
    terms.write(id, data.as_bytes())
}

#[tauri::command]
pub fn term_resize(id: u64, cols: u16, rows: u16, terms: State<'_, Terminals>) -> Result<()> {
    terms.resize(id, cols, rows)
}

#[tauri::command]
pub fn term_close(id: u64, terms: State<'_, Terminals>) {
    terms.close(id);
}

/// The shell's current folder, so the file view can follow `cd`.
#[tauri::command]
pub fn term_cwd(id: u64, terms: State<'_, Terminals>) -> Option<String> {
    terms.cwd(id).map(|p| Location::local(p).uri())
}
