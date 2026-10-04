//! The UI's view of long-running jobs. Transfer-engine events and archive
//! tasks (compress / extract) are folded into one snapshot per job, and each
//! change is pushed to the UI as a `job` event.

use crate::events::Events;
use cx_core::Entry;
use cx_transfer::{FileError, JobSnapshot, TransferEvent};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Errors kept on a running job: every update carries the whole list, so a
/// job failing on thousands of files would otherwise send O(n²) data. The
/// `finished` event still reports them all.
const LIVE_ERRORS: usize = 1000;
/// Per-file errors arrive in bursts; push at most this often, the rest ride
/// along with the next progress update.
const ERROR_PUSH_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiConflict {
    pub id: u64,
    pub source: Entry,
    pub source_uri: String,
    pub dest: Entry,
    pub dest_uri: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiJob {
    pub id: u64,
    pub kind: String,
    pub state: String,
    pub sources: Vec<String>,
    pub dest: Option<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: u64,
    pub files_total: u64,
    pub current: Option<String>,
    pub speed: f64,
    pub eta: Option<f64>,
    pub errors: Vec<FileError>,
    pub conflict: Option<UiConflict>,
    pub undo: Option<Value>,
    pub started_at: i64,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn name_of<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl UiJob {
    pub fn new(id: u64, kind: &str, sources: Vec<String>, dest: Option<String>) -> UiJob {
        UiJob {
            id,
            kind: kind.into(),
            state: "queued".into(),
            sources,
            dest,
            bytes_done: 0,
            bytes_total: 0,
            files_done: 0,
            files_total: 0,
            current: None,
            speed: 0.0,
            eta: None,
            errors: vec![],
            conflict: None,
            undo: None,
            started_at: now_ms(),
        }
    }

    fn from_snapshot(s: &JobSnapshot) -> UiJob {
        let mut j = UiJob::new(s.id.0, &name_of(&s.kind), s.sources.clone(), s.dest.clone());
        j.state = name_of(&s.state);
        j.apply_progress(&s.progress);
        j.errors = s.errors.clone();
        j.undo = s.undo.as_ref().and_then(|u| serde_json::to_value(u).ok());
        j
    }

    fn apply_progress(&mut self, p: &cx_transfer::Progress) {
        self.bytes_done = p.bytes_done;
        self.bytes_total = p.bytes_total;
        self.files_done = p.files_done;
        self.files_total = p.files_total;
        self.current = p.current.clone();
        self.speed = p.speed as f64;
        self.eta = p.eta_secs.map(|s| s as f64);
    }
}

pub struct Jobs {
    map: Mutex<HashMap<u64, UiJob>>,
    /// When each job last pushed an update caused by a file error.
    error_pushed: Mutex<HashMap<u64, Instant>>,
    events: Arc<Events>,
}

impl Jobs {
    pub fn new(events: Arc<Events>) -> Arc<Jobs> {
        Arc::new(Jobs {
            map: Mutex::new(HashMap::new()),
            error_pushed: Mutex::new(HashMap::new()),
            events,
        })
    }

    pub fn list(&self) -> Vec<UiJob> {
        let mut v: Vec<_> = self.map.lock().unwrap().values().cloned().collect();
        v.sort_by_key(|j| std::cmp::Reverse(j.started_at));
        v
    }

    /// Update a job and push it to the UI.
    pub fn update(&self, id: u64, f: impl FnOnce(&mut UiJob)) {
        self.update_then(id, true, f);
    }

    /// Update a job; push it to the UI when `push`. Serialized straight from
    /// the map, so the job (its source list can be 100k URIs) isn't cloned.
    fn update_then(&self, id: u64, push: bool, f: impl FnOnce(&mut UiJob)) {
        let payload = {
            let mut map = self.map.lock().unwrap();
            let Some(j) = map.get_mut(&id) else { return };
            f(j);
            if !push {
                return;
            }
            serde_json::json!({ "job": &*j })
        };
        self.events.emit_value("job", payload);
    }

    pub fn insert(&self, job: UiJob) {
        let id = job.id;
        self.map.lock().unwrap().insert(id, job);
        self.update(id, |_| {});
    }

    pub fn on_transfer(&self, e: TransferEvent) {
        match e {
            TransferEvent::JobAdded { job } => self.insert(UiJob::from_snapshot(&job)),
            TransferEvent::Progress { id, progress } => {
                self.update(id.0, |j| j.apply_progress(&progress))
            }
            TransferEvent::StateChanged { id, state } => self.update(id.0, |j| {
                j.state = name_of(&state);
                if j.state != "waitingForConflict" {
                    j.conflict = None;
                }
            }),
            TransferEvent::Conflict { id, conflict } => self.update(id.0, |j| {
                j.conflict = Some(UiConflict {
                    id: conflict.conflict_id,
                    source: conflict.source,
                    source_uri: conflict.source_uri,
                    dest: conflict.dest,
                    dest_uri: conflict.dest_uri,
                });
            }),
            TransferEvent::FileError { id, error } => {
                let push = {
                    let mut pushed = self.error_pushed.lock().unwrap();
                    let due = pushed
                        .get(&id.0)
                        .is_none_or(|t| t.elapsed() >= ERROR_PUSH_INTERVAL);
                    if due {
                        pushed.insert(id.0, Instant::now());
                    }
                    due
                };
                self.update_then(id.0, push, |j| {
                    if j.errors.len() < LIVE_ERRORS {
                        j.errors.push(error);
                    }
                })
            }
            TransferEvent::Finished {
                id,
                state,
                error,
                undo,
                errors,
            } => {
                self.error_pushed.lock().unwrap().remove(&id.0);
                self.update(id.0, |j| {
                    j.state = name_of(&state);
                    j.conflict = None;
                    j.speed = 0.0;
                    j.eta = None;
                    j.errors = errors;
                    if let Some(e) = error {
                        if j.errors.is_empty() {
                            j.errors.push(FileError {
                                uri: j.sources.first().cloned().unwrap_or_default(),
                                message: e,
                            });
                        }
                    }
                    j.undo = undo.and_then(|u| serde_json::to_value(u).ok());
                })
            }
        }
    }

    pub fn clear_finished(&self) {
        self.map
            .lock()
            .unwrap()
            .retain(|_, j| !matches!(j.state.as_str(), "done" | "failed" | "cancelled"));
    }
}
