#![allow(dead_code)]

use cx_core::{Location, Provider};
use cx_local::LocalProvider;
use cx_testkit::{mem_endpoint, mem_vfs, MemProvider};
use cx_transfer::{JobId, JobRequest, JobSnapshot, JobState, TransferConfig, TransferEvent, TransferManager};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

pub struct Env {
    pub mgr: Arc<TransferManager>,
    pub remote: Arc<MemProvider>,
    pub events: Arc<Mutex<Vec<TransferEvent>>>,
    pub rx: mpsc::UnboundedReceiver<TransferEvent>,
    pub tmp: tempfile::TempDir,
    pub state: PathBuf,
}

pub fn config() -> TransferConfig {
    TransferConfig {
        retry_backoff: Duration::from_millis(5),
        progress_interval: Duration::from_millis(20),
        undo_uses_trash: false,
        ..TransferConfig::default()
    }
}

pub fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    env_with(tmp, state, MemProvider::with_scheme("sftp"))
}

pub fn env_with(tmp: tempfile::TempDir, state: PathBuf, remote: Arc<MemProvider>) -> Env {
    let vfs = mem_vfs(Arc::new(LocalProvider), remote.clone());
    let events = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = mpsc::unbounded_channel();
    let log = events.clone();
    let mgr = TransferManager::with_config(vfs, state.clone(), config(), move |e| {
        log.lock().unwrap().push(e.clone());
        let _ = tx.send(e);
    });
    Env { mgr, remote, events, rx, tmp, state }
}

impl Env {
    pub fn root(&self) -> &Path {
        self.tmp.path()
    }

    pub async fn run(&self, req: JobRequest) -> JobSnapshot {
        let id = self.mgr.submit(req);
        self.wait(id).await
    }

    pub async fn wait(&self, id: JobId) -> JobSnapshot {
        tokio::time::timeout(Duration::from_secs(20), self.mgr.wait(id)).await.expect("job timed out").unwrap()
    }

    pub fn finished_undo(&self, id: JobId) -> Option<cx_transfer::UndoOp> {
        self.events.lock().unwrap().iter().find_map(|e| match e {
            TransferEvent::Finished { id: i, undo, .. } if *i == id => undo.clone(),
            _ => None,
        })
    }

    /// Wait until the job has moved at least `bytes`.
    pub async fn wait_bytes(&self, id: JobId, bytes: u64) {
        for _ in 0..2000 {
            if self.mgr.job(id).unwrap().progress.bytes_done >= bytes {
                return;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        panic!("no progress");
    }
}

pub fn uri(p: &Path) -> String {
    Location::local(p).uri()
}

pub fn ruri(path: &str) -> String {
    Location::remote(mem_endpoint("nas"), path).uri()
}

pub fn write(p: &Path, data: &[u8]) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

pub fn set_mtime(p: &Path, ms: i64) {
    let f = std::fs::File::options().write(true).open(p).or_else(|_| std::fs::File::open(p)).unwrap();
    f.set_modified(UNIX_EPOCH + Duration::from_millis(ms as u64)).unwrap();
}

pub async fn mtime(p: &Path) -> Option<i64> {
    LocalProvider.stat(&Location::local(p)).await.unwrap().modified
}

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

/// Deterministic pseudo-random bytes.
pub fn bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

pub fn has_part_files(dir: &Path) -> bool {
    fn walk(d: &Path) -> bool {
        std::fs::read_dir(d).unwrap().flatten().any(|e| {
            let p = e.path();
            p.to_string_lossy().ends_with(".cxpart") || (p.is_dir() && walk(&p))
        })
    }
    walk(dir)
}

pub fn assert_done(s: &JobSnapshot) {
    assert_eq!(s.state, JobState::Done, "job failed: {:?} {:?}", s.error, s.errors);
    assert!(s.errors.is_empty(), "file errors: {:?}", s.errors);
}
