use super::AppState;
use cx_core::{Location, Result};
use serde::Serialize;
use std::collections::HashMap;

/// Off the main thread: one extended-attribute read per file.
#[tauri::command]
pub async fn tags_get(uris: Vec<String>, app: AppState<'_>) -> Result<HashMap<String, Vec<String>>> {
    let app = app.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        uris.into_iter()
            .map(|u| (app.tags.get(&u), u))
            .map(|(t, u)| (u, t))
            .collect()
    })
    .await
    .map_err(|e| cx_core::CxError::Io(e.to_string()))
}

#[tauri::command]
pub fn tags_set(uri: String, tags: Vec<String>, app: AppState<'_>) -> Result<()> {
    app.tags.set(&uri, tags)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaggedHit {
    uri: String,
    parent: String,
    rel_path: String,
    entry: cx_core::Entry,
}

#[tauri::command]
pub async fn tags_find(tag: String, app: AppState<'_>) -> Result<Vec<TaggedHit>> {
    let mut out = Vec::new();
    // Spotlight (`mdfind`) can take seconds: keep it off the async workers.
    let shared = app.inner().clone();
    let uris = tauri::async_runtime::spawn_blocking(move || shared.tags.find(&tag))
        .await
        .map_err(|e| cx_core::CxError::Io(e.to_string()))?;
    for uri in uris {
        let Ok(loc) = Location::parse(&uri) else {
            continue;
        };
        let Ok(provider) = app.vfs.provider(&loc).await else {
            continue;
        };
        let Ok(entry) = provider.stat(&loc).await else {
            continue;
        };
        let parent = loc.parent().map(|p| p.uri()).unwrap_or_default();
        out.push(TaggedHit {
            rel_path: entry.name.clone(),
            uri,
            parent,
            entry,
        });
    }
    Ok(out)
}
