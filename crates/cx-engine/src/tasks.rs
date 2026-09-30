//! Cancellable background tasks (searches, archive jobs, folder sizes).
//!
//! Task ids start at 2^40 so they never collide with transfer-engine job
//! ids: the UI shows both in one list and routes "cancel" by the id alone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub const FIRST_TASK_ID: u64 = 1 << 40;

/// Whether `id` belongs to a task (rather than a transfer-engine job).
pub fn is_task(id: u64) -> bool {
    id >= FIRST_TASK_ID
}

pub struct Tasks {
    map: Mutex<HashMap<u64, CancellationToken>>,
    next: AtomicU64,
}

impl Default for Tasks {
    fn default() -> Self {
        Tasks {
            map: Mutex::new(HashMap::new()),
            next: AtomicU64::new(FIRST_TASK_ID),
        }
    }
}

impl Tasks {
    /// A fresh id with no token (peer offers use these for their job rows).
    pub fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    /// Register a cancellable task; returns its id and token.
    pub fn start(&self) -> (u64, CancellationToken) {
        let id = self.next_id();
        let token = CancellationToken::new();
        self.map.lock().unwrap().insert(id, token.clone());
        (id, token)
    }

    pub fn finish(&self, id: u64) {
        self.map.lock().unwrap().remove(&id);
    }

    /// Cancel a running task; false when it already finished.
    pub fn cancel(&self, id: u64) -> bool {
        match self.map.lock().unwrap().remove(&id) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_cancel() {
        let t = Tasks::default();
        let (id, token) = t.start();
        assert!(is_task(id));
        assert!(!is_task(5));
        assert!(t.cancel(id));
        assert!(token.is_cancelled());
        assert!(!t.cancel(id));
    }
}
