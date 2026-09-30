//! Recursive name and content search, streamed in batches.

use crate::Engine;
use cx_core::{Entry, Location, Result};
use cx_search::{Cancel, ContentQuery, KindFilter, MatchMode, SearchQuery};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;

/// What the search box sends. `mode` is "auto", "glob", "regex" or
/// "substring"; `kind` is "any", "file" or "dir". A non-empty `content`
/// makes it a content search (names are then not filtered by `text`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub text: String,
    pub mode: Option<String>,
    pub content: Option<String>,
    pub kind: Option<String>,
    #[serde(default)]
    pub include_hidden: bool,
    pub max_results: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHitView {
    pub uri: String,
    pub parent: String,
    pub rel_path: String,
    pub entry: Entry,
    /// Content search: first matching line (1-based) and its text.
    pub line: Option<u64>,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SearchEvent {
    Hits {
        hits: Vec<SearchHitView>,
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

impl Engine {
    /// Start a search below `root`; results stream to `on_event` from a
    /// background task. Returns a task id for [`Engine::cancel_task`].
    pub fn search_start(
        self: &Arc<Self>,
        root: &str,
        query: SearchRequest,
        on_event: impl Fn(SearchEvent) + Send + Sync + 'static,
    ) -> Result<u64> {
        let root = Location::parse(root)?;
        let (id, token) = self.tasks.start();
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
        {
            let cancel = cancel.clone();
            self.rt.spawn(async move {
                token.cancelled().await;
                cancel.cancel();
            });
        }
        let engine = self.clone();
        self.rt.spawn(async move {
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
                    let run = cx_search::search_content(engine.vfs.clone(), root, q, tx, cancel);
                    let forward = async {
                        while let Some(batch) = rx.recv().await {
                            let hits = batch
                                .into_iter()
                                .map(|h| {
                                    let first = h.matches.first();
                                    SearchHitView {
                                        parent: parent_of(&h.uri),
                                        uri: h.uri,
                                        rel_path: h.rel_path,
                                        entry: h.entry,
                                        line: first.map(|m| m.line_number),
                                        snippet: first.map(|m| m.line.clone()),
                                    }
                                })
                                .collect();
                            on_event(SearchEvent::Hits { hits });
                        }
                    };
                    tokio::join!(run, forward).0
                }
                None => {
                    let (tx, mut rx) = mpsc::channel(8);
                    let run = cx_search::search(engine.vfs.clone(), root, files, tx, cancel);
                    let forward = async {
                        while let Some(batch) = rx.recv().await {
                            let hits = batch
                                .into_iter()
                                .map(|h| SearchHitView {
                                    uri: h.uri,
                                    parent: h.parent_uri,
                                    rel_path: h.rel_path,
                                    entry: h.entry,
                                    line: None,
                                    snippet: None,
                                })
                                .collect();
                            on_event(SearchEvent::Hits { hits });
                        }
                    };
                    tokio::join!(run, forward).0
                }
            };
            let (scanned, truncated) = stats
                .as_ref()
                .map(|s| (s.entries_scanned, s.truncated))
                .unwrap_or((0, false));
            on_event(SearchEvent::Done {
                scanned,
                elapsed_ms: started.elapsed().as_secs_f64() * 1e3,
                truncated,
            });
            engine.tasks.finish(id);
        });
        Ok(id)
    }

    pub fn cancel_task(&self, id: u64) -> bool {
        self.tasks.cancel(id)
    }
}
