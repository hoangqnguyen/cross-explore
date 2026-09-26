//! Append-only log of what remote devices did here, one JSON object per
//! line (`audit.jsonl` in the state directory), so the owner can always see
//! who touched which shared file.

use crate::events::AuditRecord;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct AuditLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl AuditLog {
    pub fn new(state_dir: &std::path::Path) -> AuditLog {
        AuditLog { path: state_dir.join("audit.jsonl"), lock: Mutex::new(()) }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Best effort: a full disk must not break file access for peers.
    pub fn append(&self, rec: &AuditRecord) {
        let Ok(mut line) = serde_json::to_vec(rec) else { return };
        line.push(b'\n');
        let _g = self.lock.lock().unwrap();
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = f.write_all(&line);
        }
    }
}
