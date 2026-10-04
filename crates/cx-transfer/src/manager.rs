use crate::control::{Control, Run};
use crate::event::ConflictInfo;
use crate::limits::Limits;
use crate::persist::{Record, Saved, Store};
use crate::progress::{Counters, Meter};
use crate::{compare, engine, undo};
use crate::{CompareOptions, DiffItem, FileError, JobId, JobKind, JobRequest, JobSnapshot, JobState, Resolution, TransferEvent, UndoOp};
use cx_core::{CxError, Entry, Result, Vfs};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::{oneshot, watch};

/// Tuning knobs. The defaults suit real use; tests shrink the delays.
#[derive(Debug, Clone)]
pub struct TransferConfig {
    /// Parallel file streams per local disk.
    pub local_streams: usize,
    /// Parallel file streams per remote endpoint.
    pub remote_streams: usize,
    /// Copy buffer size.
    pub chunk_size: usize,
    /// Retries after a connection error mid-file (each resumes where it stopped).
    pub retries: u32,
    /// First retry delay; doubles each time.
    pub retry_backoff: Duration,
    /// How often progress events are emitted per job.
    pub progress_interval: Duration,
    /// Undoing a copy sends the copies to the trash (when the provider has
    /// one) instead of deleting them.
    pub undo_uses_trash: bool,
}

impl Default for TransferConfig {
    fn default() -> Self {
        TransferConfig {
            local_streams: 4,
            remote_streams: 4,
            chunk_size: 1024 * 1024,
            retries: 3,
            retry_backoff: Duration::from_millis(500),
            progress_interval: Duration::from_millis(100),
            undo_uses_trash: true,
        }
    }
}

struct Status {
    phase: JobState,
    error: Option<String>,
    errors: Vec<FileError>,
    undo: Option<UndoOp>,
    started: bool,
}

#[derive(Default)]
struct ConflictSlot {
    pending: Option<(u64, oneshot::Sender<(Resolution, bool)>)>,
    /// Answer given with "apply to all".
    sticky: Option<Resolution>,
}

/// One job's live state, shared by its runner, file tasks and the manager.
pub(crate) struct Job {
    pub id: JobId,
    pub req: JobRequest,
    pub ctrl: Control,
    pub counters: Counters,
    pub record: Mutex<Record>,
    /// Restored from disk: partial `.cxpart` files may be resumed.
    pub restored: bool,
    status: Mutex<Status>,
    conflict: Mutex<ConflictSlot>,
    dirty: AtomicBool,
    done: watch::Sender<bool>,
}

impl Job {
    fn state(&self) -> JobState {
        let s = self.status.lock().unwrap();
        if !s.phase.is_finished() && self.ctrl.get() == Run::Paused {
            JobState::Paused
        } else {
            s.phase
        }
    }

    fn snapshot(&self) -> JobSnapshot {
        let state = self.state();
        let s = self.status.lock().unwrap();
        JobSnapshot {
            id: self.id,
            kind: self.req.kind,
            state,
            sources: self.req.sources.clone(),
            dest: self.req.dest.clone(),
            progress: self.counters.snapshot(),
            errors: s.errors.clone(),
            error: s.error.clone(),
            undo: s.undo.clone(),
        }
    }

    /// Update the record and mark it for the next save.
    pub fn record<T>(&self, f: impl FnOnce(&mut Record) -> T) -> T {
        let out = f(&mut self.record.lock().unwrap());
        self.dirty.store(true, Ordering::Relaxed);
        out
    }

    fn saved(&self) -> Saved {
        let mut record = self.record.lock().unwrap();
        // The snapshot holds these; the journal starts after it.
        record.take_unsaved();
        Saved { id: self.id, request: self.req.clone(), record: record.clone() }
    }
}

/// Owns all transfer jobs. Create one per app with [`TransferManager::new`]
/// and keep the `Arc`.
pub struct TransferManager {
    me: Weak<TransferManager>,
    /// Runtime jobs are spawned on, so `submit` also works from threads
    /// outside it (e.g. synchronous Tauri commands).
    rt: Option<tokio::runtime::Handle>,
    vfs: Arc<Vfs>,
    config: TransferConfig,
    events: Box<dyn Fn(TransferEvent) + Send + Sync>,
    store: Store,
    pub(crate) limits: Limits,
    jobs: Mutex<BTreeMap<JobId, Arc<Job>>>,
    next_id: AtomicU64,
    next_conflict: AtomicU64,
}

impl TransferManager {
    /// `state_dir` keeps unfinished jobs across restarts; `events` receives
    /// every [`TransferEvent`] (forward them to the UI). Create it inside a
    /// tokio runtime: jobs run on that runtime.
    pub fn new(vfs: Arc<Vfs>, state_dir: PathBuf, events: impl Fn(TransferEvent) + Send + Sync + 'static) -> Arc<Self> {
        Self::with_config(vfs, state_dir, TransferConfig::default(), events)
    }

    pub fn with_config(vfs: Arc<Vfs>, state_dir: PathBuf, config: TransferConfig, events: impl Fn(TransferEvent) + Send + Sync + 'static) -> Arc<Self> {
        let store = Store::new(state_dir);
        // New ids must not collide with jobs waiting on disk.
        let first = store.load().iter().map(|s| s.id.0).max().unwrap_or(0) + 1;
        Arc::new_cyclic(|me| TransferManager {
            me: me.clone(),
            rt: tokio::runtime::Handle::try_current().ok(),
            vfs,
            limits: Limits::new(config.local_streams, config.remote_streams),
            config,
            events: Box::new(events),
            store,
            jobs: Mutex::new(BTreeMap::new()),
            next_id: AtomicU64::new(first),
            next_conflict: AtomicU64::new(1),
        })
    }

    pub fn vfs(&self) -> &Arc<Vfs> {
        &self.vfs
    }

    pub fn config(&self) -> &TransferConfig {
        &self.config
    }

    pub(crate) fn emit(&self, e: TransferEvent) {
        (self.events)(e);
    }

    fn arc(&self) -> Arc<TransferManager> {
        self.me.upgrade().expect("TransferManager is alive while in use")
    }

    fn get(&self, id: JobId) -> Option<Arc<Job>> {
        self.jobs.lock().unwrap().get(&id).cloned()
    }

    fn insert(&self, id: JobId, req: JobRequest, record: Record, restored: bool, run: Run) -> Arc<Job> {
        let job = Arc::new(Job {
            id,
            req,
            ctrl: Control::new(run),
            counters: Counters::default(),
            record: Mutex::new(record),
            restored,
            status: Mutex::new(Status { phase: JobState::Queued, error: None, errors: Vec::new(), undo: None, started: false }),
            conflict: Mutex::new(ConflictSlot::default()),
            dirty: AtomicBool::new(false),
            done: watch::Sender::new(false),
        });
        self.jobs.lock().unwrap().insert(id, job.clone());
        self.emit(TransferEvent::JobAdded { job: job.snapshot() });
        job
    }

    /// Queue a job; it starts right away (file streams are still limited per
    /// endpoint across all jobs).
    pub fn submit(&self, req: JobRequest) -> JobId {
        let id = JobId(self.next_id.fetch_add(1, Ordering::Relaxed));
        let job = self.insert(id, req, Record::default(), false, Run::Running);
        self.store.save(&job.saved());
        self.start(job);
        id
    }

    fn start(&self, job: Arc<Job>) {
        {
            let mut s = job.status.lock().unwrap();
            if s.started || s.phase.is_finished() {
                return;
            }
            s.started = true;
        }
        let mgr = self.arc();
        let run = async move {
            let ticker = tokio::spawn(tick(mgr.clone(), job.clone()));
            let outcome = engine::run(&mgr, &job).await;
            ticker.abort();
            mgr.finish(&job, outcome);
        };
        match &self.rt {
            Some(rt) => drop(rt.spawn(run)),
            None => drop(tokio::spawn(run)),
        }
    }

    fn finish(&self, job: &Job, outcome: Result<()>) {
        let (state, error) = match outcome {
            Ok(()) => (JobState::Done, None),
            Err(CxError::Cancelled) => (JobState::Cancelled, None),
            Err(e) => (JobState::Failed, Some(e.to_string())),
        };
        let undo = build_undo(job.req.kind, &job.record.lock().unwrap());
        let errors = {
            let mut s = job.status.lock().unwrap();
            s.phase = state;
            s.error = error.clone();
            s.undo = undo.clone();
            s.errors.clone()
        };
        job.counters.set_current(None);
        self.store.remove(job.id);
        self.emit(TransferEvent::Progress { id: job.id, progress: job.counters.snapshot() });
        self.emit(TransferEvent::StateChanged { id: job.id, state });
        self.emit(TransferEvent::Finished { id: job.id, state, error, undo, errors });
        job.done.send_replace(true);
    }

    pub(crate) fn set_phase(&self, job: &Job, phase: JobState) {
        job.status.lock().unwrap().phase = phase;
        self.emit(TransferEvent::StateChanged { id: job.id, state: job.state() });
    }

    pub(crate) fn file_error(&self, job: &Job, uri: String, err: &CxError) {
        let error = FileError { uri, message: err.to_string() };
        job.status.lock().unwrap().errors.push(error.clone());
        self.emit(TransferEvent::FileError { id: job.id, error });
    }

    /// Settle a conflict by policy, "apply to all" answer, or by asking.
    pub(crate) async fn ask(&self, job: &Job, source: (String, Entry), dest: (String, Entry)) -> Result<Resolution> {
        if let Some(r) = job.conflict.lock().unwrap().sticky {
            return Ok(r);
        }
        let conflict_id = self.next_conflict.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        job.conflict.lock().unwrap().pending = Some((conflict_id, tx));
        let prev = job.status.lock().unwrap().phase;
        self.set_phase(job, JobState::WaitingForConflict);
        let conflict = ConflictInfo { conflict_id, source_uri: source.0, source: source.1, dest_uri: dest.0, dest: dest.1 };
        self.emit(TransferEvent::Conflict { id: job.id, conflict });
        let answer = tokio::select! {
            a = rx => a.map_err(|_| CxError::Cancelled),
            _ = job.ctrl.cancelled() => Err(CxError::Cancelled),
        };
        job.conflict.lock().unwrap().pending = None;
        let (res, all) = answer?;
        if all {
            job.conflict.lock().unwrap().sticky = Some(res);
        }
        self.set_phase(job, prev);
        Ok(res)
    }

    /// Pause at the next chunk boundary.
    pub fn pause(&self, id: JobId) {
        let Some(job) = self.get(id) else { return };
        if !job.state().is_finished() && job.ctrl.set(Run::Paused) {
            self.emit(TransferEvent::StateChanged { id, state: JobState::Paused });
        }
    }

    /// Continue a paused job, or start one restored from disk.
    pub fn resume(&self, id: JobId) {
        let Some(job) = self.get(id) else { return };
        if job.state().is_finished() {
            return;
        }
        if job.ctrl.set(Run::Running) {
            self.emit(TransferEvent::StateChanged { id, state: job.state() });
        }
        self.start(job);
    }

    /// Stop the job; partial files are removed.
    pub fn cancel(&self, id: JobId) {
        let Some(job) = self.get(id) else { return };
        if job.state().is_finished() {
            return;
        }
        job.ctrl.set(Run::Cancelled);
        let never_started = !job.status.lock().unwrap().started;
        if never_started {
            job.status.lock().unwrap().started = true;
            self.finish(&job, Err(CxError::Cancelled));
        }
    }

    /// Answer a conflict. With `apply_to_all` the answer is reused for every
    /// later conflict of this job.
    pub fn resolve(&self, id: JobId, conflict_id: u64, resolution: Resolution, apply_to_all: bool) {
        let Some(job) = self.get(id) else { return };
        let mut slot = job.conflict.lock().unwrap();
        if slot.pending.as_ref().map(|p| p.0) == Some(conflict_id) {
            let (_, tx) = slot.pending.take().unwrap();
            let _ = tx.send((resolution, apply_to_all));
        }
    }

    pub fn jobs(&self) -> Vec<JobSnapshot> {
        self.jobs.lock().unwrap().values().map(|j| j.snapshot()).collect()
    }

    pub fn job(&self, id: JobId) -> Option<JobSnapshot> {
        self.get(id).map(|j| j.snapshot())
    }

    /// Wait until the job is done, failed or cancelled.
    pub async fn wait(&self, id: JobId) -> Option<JobSnapshot> {
        let job = self.get(id)?;
        let mut rx = job.done.subscribe();
        let _ = rx.wait_for(|d| *d).await;
        Some(job.snapshot())
    }

    /// Forget finished jobs (the "Clear" button in the transfers flyout).
    pub fn clear_finished(&self) {
        self.jobs.lock().unwrap().retain(|_, j| !j.state().is_finished());
    }

    /// Load jobs that were unfinished when the app last quit. They come back
    /// paused; call [`resume`](Self::resume) to continue (already copied
    /// files are skipped and partial files continue where they stopped) or
    /// [`cancel`](Self::cancel) to drop them.
    pub fn restore_pending(&self) -> Vec<JobSnapshot> {
        let mut out = Vec::new();
        for saved in self.store.load() {
            if self.get(saved.id).is_some() {
                continue;
            }
            self.next_id.fetch_max(saved.id.0 + 1, Ordering::Relaxed);
            let job = self.insert(saved.id, saved.request, saved.record, true, Run::Paused);
            out.push(job.snapshot());
        }
        out
    }

    /// Revert an operation reported in a `finished` event (or built by the
    /// UI for renames and new folders).
    pub async fn undo(&self, op: UndoOp) -> Result<()> {
        undo::execute(self, op).await
    }

    /// Compare two folders (see [`crate::compare()`]).
    pub async fn compare(&self, left: &str, right: &str, opts: CompareOptions) -> Result<Vec<DiffItem>> {
        compare::compare(&self.vfs, left, right, opts).await
    }

    /// Append the record's changes since the last save to the job's journal.
    pub(crate) async fn save_if_dirty(&self, job: &Arc<Job>) {
        if !job.dirty.swap(false, Ordering::Relaxed) || job.state().is_finished() {
            return;
        }
        let ops = job.record.lock().unwrap().take_unsaved();
        if ops.is_empty() {
            return;
        }
        let (mgr, id) = (self.arc(), job.id);
        let _ = tokio::task::spawn_blocking(move || mgr.store.append(id, &ops)).await;
    }
}

fn build_undo(kind: JobKind, r: &Record) -> Option<UndoOp> {
    match kind {
        JobKind::Copy if !r.created.is_empty() => Some(UndoOp::Copy { created: r.created.clone() }),
        JobKind::Move if !r.moved.is_empty() => Some(UndoOp::Move { items: r.moved.clone() }),
        JobKind::Trash if !r.trashed.is_empty() => Some(UndoOp::Trash { items: r.trashed.clone() }),
        _ => None,
    }
}

/// Emits throttled progress and saves the job record while the job runs.
async fn tick(mgr: Arc<TransferManager>, job: Arc<Job>) {
    let mut meter = Meter::new(job.counters.bytes_done());
    let mut last = None;
    let mut interval = tokio::time::interval(mgr.config.progress_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let mut p = job.counters.snapshot();
        let running = job.state() == JobState::Running;
        meter.sample(&mut p, running);
        if last.as_ref() != Some(&p) {
            mgr.emit(TransferEvent::Progress { id: job.id, progress: p.clone() });
            last = Some(p);
        }
        mgr.save_if_dirty(&job).await;
    }
}
