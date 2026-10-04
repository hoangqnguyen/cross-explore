//! Job state on disk, so unfinished jobs can be listed and resumed after a
//! crash or quit: a snapshot (`<state_dir>/job-<id>.json`) written when the
//! job is submitted, plus a journal (`job-<id>.log`, one JSON line per
//! change) appended as it runs. Rewriting the whole record on every change
//! would cost O(files done) per save, quadratic over a big copy. Files are
//! removed once a job finishes.

use crate::undo::MovedItem;
use crate::{JobId, JobRequest};
use cx_core::TrashedItem;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::PathBuf;

/// What a job has done so far. Doubles as the source of its undo operation.
/// Change it only through the `add_*` methods, which also journal the change.
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
    /// Changes not yet appended to the journal.
    #[serde(skip)]
    unsaved: Vec<Op>,
}

/// One journal line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Op {
    Completed(String),
    Target(String, String),
    Created(String),
    Moved(MovedItem),
    Trashed(Vec<TrashedItem>),
}

impl Record {
    fn apply(&mut self, op: Op) {
        match op {
            Op::Completed(uri) => drop(self.completed.insert(uri)),
            Op::Target(src, dst) => drop(self.targets.insert(src, dst)),
            Op::Created(uri) => self.created.push(uri),
            Op::Moved(item) => self.moved.push(item),
            Op::Trashed(items) => self.trashed.extend(items),
        }
    }

    fn log(&mut self, op: Op) {
        self.unsaved.push(op.clone());
        self.apply(op);
    }

    pub fn add_completed(&mut self, uri: String) {
        self.log(Op::Completed(uri));
    }

    pub fn add_target(&mut self, src: String, dst: String) {
        self.log(Op::Target(src, dst));
    }

    pub fn add_created(&mut self, uri: String) {
        self.log(Op::Created(uri));
    }

    pub fn add_moved(&mut self, item: MovedItem) {
        self.log(Op::Moved(item));
    }

    pub fn add_trashed(&mut self, items: Vec<TrashedItem>) {
        self.log(Op::Trashed(items));
    }

    /// Changes made since the last call, for [`Store::append`].
    pub fn take_unsaved(&mut self) -> Vec<Op> {
        std::mem::take(&mut self.unsaved)
    }
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

    fn journal(&self, id: JobId) -> PathBuf {
        self.dir.join(format!("job-{id}.log"))
    }

    /// Write the snapshot through a temp file so a crash never leaves half a
    /// JSON file, and start an empty journal.
    pub fn save(&self, saved: &Saved) {
        let Ok(json) = serde_json::to_vec(saved) else { return };
        let tmp = self.dir.join(format!(".job-{}.json.tmp", saved.id));
        let _ = std::fs::remove_file(self.journal(saved.id));
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, self.path(saved.id));
        }
    }

    /// Append changes to the job's journal. Only for jobs with a snapshot.
    pub fn append(&self, id: JobId, ops: &[Op]) {
        if ops.is_empty() || !self.path(id).exists() {
            return;
        }
        let mut buf = Vec::new();
        for op in ops {
            if serde_json::to_writer(&mut buf, op).is_ok() {
                buf.push(b'\n');
            }
        }
        let file = std::fs::OpenOptions::new().create(true).append(true).open(self.journal(id));
        if let Ok(mut f) = file {
            let _ = f.write_all(&buf);
        }
    }

    pub fn remove(&self, id: JobId) {
        let _ = std::fs::remove_file(self.path(id));
        let _ = std::fs::remove_file(self.journal(id));
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
        for saved in &mut out {
            let Ok(log) = std::fs::read(self.journal(saved.id)) else { continue };
            // A crash mid-append leaves a torn last line: skip what won't parse.
            for line in log.split(|&b| b == b'\n') {
                if let Ok(op) = serde_json::from_slice::<Op>(line) {
                    saved.record.apply(op);
                }
            }
        }
        out.sort_by_key(|s| s.id);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_replays_onto_the_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        let id = JobId(7);
        let mut record = Record::default();
        record.add_target("a".into(), "b".into());
        record.take_unsaved();
        store.save(&Saved { id, request: JobRequest::copy(vec!["a".into()], "d"), record: record.clone() });

        record.add_completed("a/1".into());
        record.add_created("b".into());
        store.append(id, &record.take_unsaved());
        record.add_completed("a/2".into());
        store.append(id, &record.take_unsaved());
        // A crash mid-append leaves a torn last line.
        let mut f = std::fs::OpenOptions::new().append(true).open(store.journal(id)).unwrap();
        f.write_all(b"{\"completed\":\"a/").unwrap();

        let loaded = store.load();
        assert_eq!(loaded.len(), 1);
        let r = &loaded[0].record;
        assert_eq!(r.completed.iter().collect::<Vec<_>>(), ["a/1", "a/2"]);
        assert_eq!(r.created, ["b"]);
        assert_eq!(r.targets.get("a").map(String::as_str), Some("b"));

        // A fresh snapshot starts a fresh journal; removing drops both.
        store.save(&loaded[0]);
        assert!(!store.journal(id).exists());
        store.remove(id);
        assert!(std::fs::read_dir(tmp.path()).unwrap().next().is_none());
    }
}
