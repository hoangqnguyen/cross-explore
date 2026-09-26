//! Per-endpoint stream limits shared by all jobs, so ten jobs against one
//! NAS don't open forty parallel streams.

use cx_core::{CxError, Location, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) struct Limits {
    local: usize,
    remote: usize,
    map: Mutex<HashMap<String, Arc<Semaphore>>>,
}

fn key(loc: &Location) -> String {
    match loc {
        Location::Local(_) => "local".into(),
        Location::Remote { endpoint, .. } => endpoint.uri(),
        Location::Archive { container, .. } => format!("archive:{}", container.uri()),
    }
}

impl Limits {
    pub fn new(local: usize, remote: usize) -> Self {
        Limits { local: local.max(1), remote: remote.max(1), map: Mutex::new(HashMap::new()) }
    }

    fn semaphore(&self, loc: &Location) -> (String, Arc<Semaphore>) {
        let k = key(loc);
        let n = if loc.is_local() { self.local } else { self.remote };
        let s = self.map.lock().unwrap().entry(k.clone()).or_insert_with(|| Arc::new(Semaphore::new(n))).clone();
        (k, s)
    }

    /// One stream slot on each side. Slots are always taken in key order so
    /// two jobs copying in opposite directions can't deadlock.
    pub async fn acquire(&self, src: &Location, dst: &Location) -> Result<Vec<OwnedSemaphorePermit>> {
        let mut sems = vec![self.semaphore(src), self.semaphore(dst)];
        sems.sort_by(|a, b| a.0.cmp(&b.0));
        sems.dedup_by(|a, b| a.0 == b.0);
        let mut permits = Vec::with_capacity(2);
        for (_, s) in sems {
            permits.push(s.acquire_owned().await.map_err(|_| CxError::Cancelled)?);
        }
        Ok(permits)
    }
}
