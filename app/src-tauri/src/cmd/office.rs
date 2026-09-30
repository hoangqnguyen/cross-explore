//! Previews of Office / OpenDocument / RTF / CSV files (cx-office).

use super::AppState;
use cx_core::{Location, Result};
use cx_office::{OfficePreview, OfficeRenderer};
use serde::Serialize;
use std::path::Path;
use std::sync::OnceLock;

static RENDERER: OnceLock<OfficeRenderer> = OnceLock::new();

pub fn init(cache_dir: &Path) {
    let _ = RENDERER.set(OfficeRenderer::new(cache_dir.join("office")));
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum OfficeView {
    /// Self-contained, script-free HTML (show in a sandboxed frame).
    Html {
        html: String,
        title: Option<String>,
        pages: Option<u32>,
    },
    /// A LibreOffice-made PDF, as a local file URI.
    Pdf { uri: String },
}

/// Render `uri` for Quick Look. `prefer_pdf` uses LibreOffice (when
/// installed) for a page-exact view; the built-in renderer is the fallback.
#[tauri::command]
pub async fn preview_office(
    uri: String,
    prefer_pdf: Option<bool>,
    app: AppState<'_>,
) -> Result<OfficeView> {
    let loc = Location::parse(&uri)?;
    let renderer = RENDERER.get_or_init(|| OfficeRenderer::new(app.cache_dir.join("office")));
    Ok(
        match renderer
            .preview(&app.vfs, &loc, prefer_pdf.unwrap_or(false))
            .await?
        {
            OfficePreview::Html { html, title, pages } => OfficeView::Html { html, title, pages },
            OfficePreview::Pdf { path } => OfficeView::Pdf {
                uri: Location::local(&path).uri(),
            },
        },
    )
}

/// Whether LibreOffice was found (for the "exact layout" toggle).
#[tauri::command]
pub async fn office_has_libreoffice() -> bool {
    tokio::task::spawn_blocking(|| {
        RENDERER
            .get()
            .and_then(|r| r.libreoffice().map(|_| ()))
            .is_some()
    })
    .await
    .unwrap_or(false)
}
