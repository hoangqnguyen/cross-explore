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

/// A one-page PDF with a line of text, xref offsets computed.
fn tiny_pdf() -> Vec<u8> {
    let text = "BT /F1 24 Tf 72 700 Td (Cross Explore self test) Tj ET";
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{text}\nendstream", text.len()),
    ];
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offsets.push(out.len());
        out += &format!("{} 0 obj\n{o}\nendobj\n", i + 1);
    }
    let xref = out.len();
    out += &format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1);
    for o in offsets {
        out += &format!("{o:010} 00000 n \n");
    }
    out += &format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1);
    out.into_bytes()
}

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
        std::fs::write(base.join("doc.pdf"), tiny_pdf()).ok()?;
        // A few MiB of known bytes, for ranged reads of non-local files.
        std::fs::create_dir_all(base.join("media")).ok()?;
        std::fs::write(base.join("media").join("clip.bin"), (0..3_600_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect::<Vec<u8>>()).ok()?;
        std::fs::write(base.join("sheet.csv"), "Fruit,Qty\nApples,3\nPears,5\n").ok()?;
        std::fs::write(base.join("sub").join("nested.txt"), "deep inside\n").ok()?;
        // Many folders: a big listing arrives in large channel messages.
        for i in 0..400 {
            std::fs::create_dir_all(base.join("sub").join("wide").join(format!("folder number {i:03} with a fairly long name"))).ok()?;
        }
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
