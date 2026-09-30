//! The front end's view of long-running jobs. Transfer-engine events,
//! archive tasks (compress / extract) and peer offers are folded into one
//! snapshot per job, and each change is emitted as [`EngineEvent::Job`].

use crate::events::{Emitter, EngineEvent};
use cx_core::Entry;
use cx_transfer::{FileError, JobSnapshot, TransferEvent, UndoOp};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobConflict {
    pub id: u64,
    pub source: Entry,
    pub source_uri: String,
    pub dest: Entry,
    pub dest_uri: String,
}

/// One job as the UI shows it. `kind` is "copy", "move", "delete", "trash",
/// "compress", "extract", "send" or "receive"; `state` is "queued",
/// "scanning", "running", "paused", "waitingForConflict", "done", "failed"
/// or "cancelled" (the transfer engine's names, camelCased).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JobView {
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
    pub conflict: Option<JobConflict>,
    pub undo: Option<UndoOp>,
    /// Milliseconds since the Unix epoch.
    pub started_at: i64,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The serde name of a unit enum value ("waitingForConflict").
pub(crate) fn name_of<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl JobView {
    pub fn new(id: u64, kind: &str, sources: Vec<String>, dest: Option<String>) -> JobView {
        JobView {
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

    pub fn is_finished(&self) -> bool {
        matches!(self.state.as_str(), "done" | "failed" | "cancelled")
    }

    /// 0.0–1.0 by bytes, falling back to files.
    pub fn fraction(&self) -> f64 {
        if self.bytes_total > 0 {
            (self.bytes_done as f64 / self.bytes_total as f64).min(1.0)
        } else if self.files_total > 0 {
            (self.files_done as f64 / self.files_total as f64).min(1.0)
        } else if self.state == "done" {
            1.0
        } else {
            0.0
        }
    }

    fn from_snapshot(s: &JobSnapshot) -> JobView {
        let mut j = JobView::new(s.id.0, &name_of(&s.kind), s.sources.clone(), s.dest.clone());
        j.state = name_of(&s.state);
        j.apply_progress(&s.progress);
        j.errors = s.errors.clone();
        j.undo = s.undo.clone();
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
    map: Mutex<HashMap<u64, JobView>>,
    events: Arc<Emitter>,
}

impl Jobs {
    pub fn new(events: Arc<Emitter>) -> Arc<Jobs> {
        Arc::new(Jobs {
            map: Mutex::new(HashMap::new()),
            events,
        })
    }

    /// Newest first.
    pub fn list(&self) -> Vec<JobView> {
        let mut v: Vec<_> = self.map.lock().unwrap().values().cloned().collect();
        v.sort_by_key(|j| (std::cmp::Reverse(j.started_at), std::cmp::Reverse(j.id)));
        v
    }

    pub fn get(&self, id: u64) -> Option<JobView> {
        self.map.lock().unwrap().get(&id).cloned()
    }

    /// Update a job and emit it.
    pub fn update(&self, id: u64, f: impl FnOnce(&mut JobView)) {
        let snapshot = {
            let mut map = self.map.lock().unwrap();
            let Some(j) = map.get_mut(&id) else { return };
            f(j);
            j.clone()
        };
        self.events.emit(EngineEvent::Job { job: snapshot });
    }

    pub fn insert(&self, job: JobView) {
        let id = job.id;
        self.map.lock().unwrap().insert(id, job);
        self.update(id, |_| {});
    }

    pub fn on_transfer(&self, e: TransferEvent) {
        match e {
            TransferEvent::JobAdded { job } => self.insert(JobView::from_snapshot(&job)),
            TransferEvent::Progress { id, progress } => {
                self.update(id.0, |j| j.apply_progress(&progress))
            }
            // A finished state is reported once, by `Finished` (which the
            // manager sends right after), complete with errors and undo.
            TransferEvent::StateChanged { state, .. } if state.is_finished() => {}
            TransferEvent::StateChanged { id, state } => self.update(id.0, |j| {
                j.state = name_of(&state);
                if j.state != "waitingForConflict" {
                    j.conflict = None;
                }
            }),
            TransferEvent::Conflict { id, conflict } => self.update(id.0, |j| {
                j.conflict = Some(JobConflict {
                    id: conflict.conflict_id,
                    source: conflict.source,
                    source_uri: conflict.source_uri,
                    dest: conflict.dest,
                    dest_uri: conflict.dest_uri,
                });
            }),
            TransferEvent::FileError { id, error } => self.update(id.0, |j| j.errors.push(error)),
            TransferEvent::Finished {
                id,
                state,
                error,
                undo,
                errors,
            } => self.update(id.0, |j| {
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
                j.undo = undo;
            }),
        }
    }

    pub fn clear_finished(&self) {
        self.map.lock().unwrap().retain(|_, j| !j.is_finished());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cx_transfer::{JobId, JobState};

    #[test]
    fn transfer_events_fold_into_one_view() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Emitter::default());
        let s = seen.clone();
        events.set(Arc::new(move |e| {
            if let EngineEvent::Job { job } = e {
                s.lock().unwrap().push(job.state);
            }
        }));
        let jobs = Jobs::new(events);
        jobs.insert(JobView::new(
            7,
            "copy",
            vec!["file:///a".into()],
            Some("file:///b".into()),
        ));
        jobs.on_transfer(TransferEvent::StateChanged {
            id: JobId(7),
            state: JobState::Running,
        });
        jobs.on_transfer(TransferEvent::Finished {
            id: JobId(7),
            state: JobState::Failed,
            error: Some("boom".into()),
            undo: None,
            errors: vec![],
        });
        let j = jobs.get(7).unwrap();
        assert_eq!(j.state, "failed");
        assert_eq!(j.errors[0].message, "boom");
        assert!(j.is_finished());
        assert_eq!(*seen.lock().unwrap(), vec!["queued", "running", "failed"]);
        jobs.clear_finished();
        assert!(jobs.list().is_empty());
    }
}
