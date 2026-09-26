//! Lock-free counters updated by file tasks, and the smoothed speed meter the
//! ticker derives from them.

use crate::Progress;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct Counters {
    bytes_done: AtomicU64,
    bytes_total: AtomicU64,
    files_done: AtomicU64,
    files_total: AtomicU64,
    current: Mutex<Option<String>>,
}

impl Counters {
    pub fn add_total(&self, files: u64, bytes: u64) {
        self.files_total.fetch_add(files, Relaxed);
        self.bytes_total.fetch_add(bytes, Relaxed);
    }

    pub fn sub_total(&self, files: u64, bytes: u64) {
        let _ = self.files_total.fetch_update(Relaxed, Relaxed, |v| Some(v.saturating_sub(files)));
        let _ = self.bytes_total.fetch_update(Relaxed, Relaxed, |v| Some(v.saturating_sub(bytes)));
    }

    pub fn add_bytes(&self, n: u64) {
        self.bytes_done.fetch_add(n, Relaxed);
    }

    pub fn sub_bytes(&self, n: u64) {
        let _ = self.bytes_done.fetch_update(Relaxed, Relaxed, |v| Some(v.saturating_sub(n)));
    }

    pub fn file_done(&self) {
        self.files_done.fetch_add(1, Relaxed);
    }

    /// Count items that won't be transferred (skipped, failed early) as done
    /// so the bar still reaches the end.
    pub fn skip(&self, files: u64, bytes: u64) {
        self.files_done.fetch_add(files, Relaxed);
        self.bytes_done.fetch_add(bytes, Relaxed);
    }

    pub fn set_current(&self, name: Option<String>) {
        *self.current.lock().unwrap() = name;
    }

    pub fn bytes_done(&self) -> u64 {
        self.bytes_done.load(Relaxed)
    }

    pub fn snapshot(&self) -> Progress {
        Progress {
            bytes_done: self.bytes_done.load(Relaxed),
            bytes_total: self.bytes_total.load(Relaxed),
            files_done: self.files_done.load(Relaxed),
            files_total: self.files_total.load(Relaxed),
            current: self.current.lock().unwrap().clone(),
            speed: 0,
            eta_secs: None,
        }
    }
}

/// Exponentially smoothed throughput, so the number the user reads doesn't
/// jump around with every chunk.
pub(crate) struct Meter {
    last_bytes: u64,
    last_at: Instant,
    speed: f64,
}

/// Weight of the newest sample; with 10 samples a second this settles in
/// about a second.
const ALPHA: f64 = 0.2;

impl Meter {
    pub fn new(bytes: u64) -> Self {
        Meter { last_bytes: bytes, last_at: Instant::now(), speed: 0.0 }
    }

    /// Fill in speed and ETA from the change since the last sample.
    pub fn sample(&mut self, p: &mut Progress, running: bool) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_at).max(Duration::from_millis(1)).as_secs_f64();
        let inst = p.bytes_done.saturating_sub(self.last_bytes) as f64 / dt;
        self.speed = if !running { 0.0 } else if self.speed == 0.0 { inst } else { ALPHA * inst + (1.0 - ALPHA) * self.speed };
        self.last_bytes = p.bytes_done;
        self.last_at = now;
        p.speed = self.speed as u64;
        p.eta_secs = (self.speed >= 1.0).then(|| (p.bytes_total.saturating_sub(p.bytes_done) as f64 / self.speed).ceil() as u64);
    }
}
