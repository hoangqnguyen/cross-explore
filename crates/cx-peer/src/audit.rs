//! Append-only log of what remote devices did here, one JSON object per
//! line (`audit.jsonl` in the state directory), so the owner can always see
//! who touched which shared file.

use crate::events::AuditRecord;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
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

/// The newest `limit` records in the log at `path`, newest first. Only the
/// end of the file is read, so a log that has grown large stays cheap.
pub fn recent(path: &Path, limit: usize) -> Vec<AuditRecord> {
    const TAIL: u64 = 512 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new() };
    let start = f.metadata().map(|m| m.len().saturating_sub(TAIL)).unwrap_or(0);
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_end(&mut buf).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&buf);
    // Reading from the middle: the first line is probably cut.
    let lines = text.lines().skip(usize::from(start > 0));
    let mut out: Vec<AuditRecord> = lines.filter_map(|l| serde_json::from_str(l).ok()).collect();
    out.reverse();
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(op: &str) -> AuditRecord {
        AuditRecord { time: 1, device_id: "d".into(), name: "n".into(), addr: "a".into(), op: op.into(), path: None, ok: true, error: None }
    }

    #[test]
    fn recent_is_newest_first_and_skips_junk() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path());
        assert!(recent(log.path(), 10).is_empty());
        for op in ["list", "read", "write"] {
            log.append(&rec(op));
        }
        std::fs::OpenOptions::new().append(true).open(log.path()).unwrap().write_all(b"not json\n").unwrap();
        log.append(&rec("remove"));
        let ops: Vec<String> = recent(log.path(), 3).into_iter().map(|r| r.op).collect();
        assert_eq!(ops, ["remove", "write", "read"]);
    }
}
