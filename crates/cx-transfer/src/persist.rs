//! Job state on disk (`<state_dir>/job-<id>.json`), so unfinished jobs can
//! be listed and resumed after a crash or quit. Files are removed once a job
//! finishes.

use crate::undo::MovedItem;
use crate::{JobId, JobRequest};
use cx_core::TrashedItem;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// What a job has done so far. Doubles as the source of its undo operation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Record {
    /// Source URIs of files fully transferred.
    pub completed: BTreeSet<String>,
    /// Top-level source URI → destination URI chosen at scan time ("a - Copy"),
    /// so a resumed job writes to the same place.
    pub targets: BTreeMap<String, String>,
    /// Top-most items a copy created.
    pub created: Vec<String>,
    pub moved: Vec<MovedItem>,
    pub trashed: Vec<TrashedItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Saved {
    pub id: JobId,
    pub request: JobRequest,
    pub record: Record,
}

pub(crate) struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Store { dir }
    }

    fn path(&self, id: JobId) -> PathBuf {
        self.dir.join(format!("job-{id}.json"))
    }

    /// Write through a temp file so a crash never leaves half a JSON file.
    pub fn save(&self, saved: &Saved) {
        let Ok(json) = serde_json::to_vec(saved) else { return };
        let tmp = self.dir.join(format!(".job-{}.json.tmp", saved.id));
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, self.path(saved.id));
        }
    }

    pub fn remove(&self, id: JobId) {
        let _ = std::fs::remove_file(self.path(id));
    }

    pub fn load(&self) -> Vec<Saved> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        let mut out: Vec<Saved> = rd
            .flatten()
            .filter(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                n.starts_with("job-") && n.ends_with(".json")
            })
            .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
            .collect();
        out.sort_by_key(|s| s.id);
        out
    }
}
