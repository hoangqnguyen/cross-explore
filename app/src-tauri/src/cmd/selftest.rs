//! End-to-end self test: with CX_SELFTEST=1 the UI drives the real backend
//! through a scripted session and exits with the result (see ui/selftest.ts).

use super::AppState;
use cx_core::{Location, Result};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::AppHandle;

// A 2×2 red PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x02, 0x00, 0x00, 0x00, 0xFD, 0xD4, 0x9A, 0x73, 0x00,
    0x00, 0x00, 0x12, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x44, 0x0C, 0x70, 0x00, 0x00, 0x2A, 0x17, 0x05, 0xFB, 0x96, 0x11, 0x88, 0xB5, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44,
    0xAE, 0x42, 0x60, 0x82,
];

fn dir() -> &'static Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::var_os("CX_SELFTEST")?;
        let base = std::env::temp_dir().join(format!("cx-selftest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub")).ok()?;
        std::fs::write(base.join("alpha.txt"), "hello cross explore\nline two\n").ok()?;
        std::fs::write(base.join("beta.md"), "# Beta\n\nSome *markdown*.\n").ok()?;
        std::fs::write(base.join("photo.png"), PNG).ok()?;
        std::fs::write(base.join("sheet.csv"), "Fruit,Qty\nApples,3\nPears,5\n").ok()?;
        std::fs::write(base.join("sub").join("nested.txt"), "deep inside\n").ok()?;
        Some(base.canonicalize().unwrap_or(base))
    })
}

#[derive(Serialize)]
pub struct Config {
    uri: String,
    /// Test servers (see docker/) when CX_SELFTEST_REMOTE is set.
    remote: bool,
}

#[tauri::command]
pub fn selftest_config() -> Option<Config> {
    dir().as_ref().map(|d| Config { uri: Location::local(d).uri(), remote: std::env::var_os("CX_SELFTEST_REMOTE").is_some() })
}

/// Simulate another app changing the folder.
#[tauri::command]
pub fn selftest_touch(name: String) -> Result<()> {
    let d = dir().as_ref().ok_or_else(|| cx_core::CxError::Unsupported("self test".into()))?;
    std::fs::write(d.join(name), "made outside the app\n").map_err(|e| cx_core::CxError::from_io(e, "touch"))
}

#[tauri::command]
pub fn selftest_exit(code: i32, app: AppHandle, _state: AppState<'_>) {
    if let Some(d) = dir() {
        let _ = std::fs::remove_dir_all(d);
    }
    app.exit(code);
}
