//! Recursive name and content search, streamed to the UI in batches.

use super::AppState;
use cx_core::{Entry, Location, Result};
use cx_search::{Cancel, ContentQuery, KindFilter, MatchMode, SearchQuery};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tauri::ipc::Channel;
use tokio::sync::mpsc;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiQuery {
    text: String,
    mode: Option<String>,
    content: Option<String>,
    kind: Option<String>,
    #[serde(default)]
    include_hidden: bool,
    max_results: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiHit {
    uri: String,
    parent: String,
    rel_path: String,
    entry: Entry,
    line: Option<u64>,
    snippet: Option<String>,
}

#[derive(Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SearchEvent {
    Hits {
        hits: Vec<UiHit>,
    },
    Done {
        scanned: u64,
        elapsed_ms: f64,
        truncated: bool,
    },
}

fn parent_of(uri: &str) -> String {
    Location::parse(uri)
        .ok()
        .and_then(|l| l.parent())
        .map(|p| p.uri())
        .unwrap_or_default()
}

#[tauri::command]
pub async fn search_start(
    root: String,
    query: UiQuery,
    on_event: Channel<SearchEvent>,
    app: AppState<'_>,
) -> Result<u64> {
    let root = Location::parse(&root)?;
    let (id, token) = app.task();
    let cancel = Cancel::new();
    let files = SearchQuery {
        text: query.text.clone(),
        mode: match query.mode.as_deref() {
            Some("glob") => MatchMode::Glob,
            Some("regex") => MatchMode::Regex,
            Some("substring") => MatchMode::Substring,
            _ => MatchMode::Auto,
        },
        kind: match query.kind.as_deref() {
            Some("file") => KindFilter::File,
            Some("dir") => KindFilter::Dir,
            _ => KindFilter::Any,
        },
        include_hidden: query.include_hidden,
        max_results: query.max_results.or(Some(20_000)),
        ..Default::default()
    };
    let app = app.inner().clone();
    {
        let cancel = cancel.clone();
        tauri::async_runtime::spawn(async move {
            token.cancelled().await;
            cancel.cancel();
        });
    }
    tauri::async_runtime::spawn(async move {
        let started = Instant::now();
        let stats = match query.content.filter(|c| !c.is_empty()) {
            Some(pattern) => {
                let (tx, mut rx) = mpsc::channel(8);
                let q = ContentQuery {
                    pattern,
                    regex: false,
                    case_sensitive: None,
                    whole_word: false,
                    files: SearchQuery {
                        text: String::new(),
                        ..files
                    },
                    ..Default::default()
                };
                let run = cx_search::search_content(app.vfs.clone(), root, q, tx, cancel);
                let forward = async {
                    while let Some(batch) = rx.recv().await {
                        let hits = batch
                            .into_iter()
                            .map(|h| {
                                let first = h.matches.first();
                                UiHit {
                                    parent: parent_of(&h.uri),
                                    uri: h.uri,
                                    rel_path: h.rel_path,
                                    entry: h.entry,
                                    line: first.map(|m| m.line_number),
                                    snippet: first.map(|m| m.line.clone()),
                                }
                            })
                            .collect();
                        let _ = on_event.send(SearchEvent::Hits { hits });
                    }
                };
                tokio::join!(run, forward).0
            }
            None => {
                let (tx, mut rx) = mpsc::channel(8);
                let run = cx_search::search(app.vfs.clone(), root, files, tx, cancel);
                let forward = async {
                    while let Some(batch) = rx.recv().await {
                        let hits = batch
                            .into_iter()
                            .map(|h| UiHit {
                                uri: h.uri,
                                parent: h.parent_uri,
                                rel_path: h.rel_path,
                                entry: h.entry,
                                line: None,
                                snippet: None,
                            })
                            .collect();
                        let _ = on_event.send(SearchEvent::Hits { hits });
                    }
                };
                tokio::join!(run, forward).0
            }
        };
        let (scanned, truncated) = stats
            .as_ref()
            .map(|s| (s.entries_scanned, s.truncated))
            .unwrap_or((0, false));
        let _ = on_event.send(SearchEvent::Done {
            scanned,
            elapsed_ms: started.elapsed().as_secs_f64() * 1e3,
            truncated,
        });
        app.finish_task(id);
    });
    Ok(id)
}

#[tauri::command]
pub fn cancel_task(id: u64, app: AppState<'_>) {
    app.cancel_task(id);
}
