//! Transfers (copy / move / delete / trash through the transfer engine) and
//! archive jobs (compress / extract), all reported as [`JobView`]s.

use crate::jobs::JobView;
use crate::tasks::is_task;
use crate::Engine;
use cx_core::{CxError, Location, Result};
use cx_transfer::{
    CompareBy, CompareOptions, ConflictPolicy, DiffItem, FileError, JobId, JobKind, JobRequest,
    Resolution, SyncDirection, UndoOp,
};
use serde::Deserialize;
use std::sync::Arc;

/// A job as a front end submits it. `kind` is "copy", "move", "delete",
/// "trash", "compress" (dest = the new .zip) or "extract" (dest = folder).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitRequest {
    pub kind: String,
    pub sources: Vec<String>,
    pub dest: Option<String>,
    #[serde(default)]
    pub conflict: ConflictPolicy,
    #[serde(default)]
    pub verify: bool,
}

impl SubmitRequest {
    pub fn new(kind: &str, sources: Vec<String>, dest: Option<String>) -> Self {
        SubmitRequest {
            kind: kind.into(),
            sources,
            dest,
            conflict: ConflictPolicy::Ask,
            verify: false,
        }
    }

    pub fn with_conflict(mut self, conflict: ConflictPolicy) -> Self {
        self.conflict = conflict;
        self
    }
}

fn progress_to(j: &mut JobView, p: &cx_archive::Progress, started: std::time::Instant) {
    j.state = "running".into();
    j.bytes_done = p.bytes_done;
    j.bytes_total = p.bytes_total;
    j.files_done = p.files_done;
    j.files_total = p.files_total;
    j.current = Some(p.current.clone()).filter(|c| !c.is_empty());
    let secs = started.elapsed().as_secs_f64().max(0.001);
    j.speed = p.bytes_done as f64 / secs;
    j.eta = (j.speed > 0.0 && p.bytes_total > p.bytes_done)
        .then(|| (p.bytes_total - p.bytes_done) as f64 / j.speed);
}

impl Engine {
    /// Start a job; returns its id (a transfer id or a task id).
    pub fn submit(self: &Arc<Self>, req: SubmitRequest) -> Result<u64> {
        let missing = || CxError::InvalidLocation("missing destination".into());
        let kind = match req.kind.as_str() {
            "copy" => JobKind::Copy,
            "move" => JobKind::Move,
            "delete" => JobKind::Delete,
            "trash" => JobKind::Trash,
            "compress" => return Ok(self.compress(req.sources, req.dest.ok_or_else(missing)?)),
            "extract" => return Ok(self.extract(req.sources, req.dest.ok_or_else(missing)?)),
            other => return Err(CxError::Unsupported(format!("{other} jobs"))),
        };
        for s in &req.sources {
            Location::parse(s)?;
        }
        let id = self.transfers.submit(JobRequest {
            kind,
            sources: req.sources,
            dest: req.dest,
            conflict: req.conflict,
            verify: req.verify,
        });
        Ok(id.0)
    }

    pub fn pause(&self, id: u64) {
        if !is_task(id) {
            self.transfers.pause(JobId(id));
        }
    }

    pub fn resume(&self, id: u64) {
        if !is_task(id) {
            self.transfers.resume(JobId(id));
        }
    }

    pub fn cancel(&self, id: u64) {
        if is_task(id) {
            self.tasks.cancel(id);
        } else {
            self.transfers.cancel(JobId(id));
        }
    }

    /// Answer a conflict; `apply_to_all` answers the job's later ones too.
    pub fn resolve(&self, id: u64, conflict_id: u64, resolution: Resolution, apply_to_all: bool) {
        self.transfers
            .resolve(JobId(id), conflict_id, resolution, apply_to_all);
    }

    pub fn job_list(&self) -> Vec<JobView> {
        self.jobs.list()
    }

    pub fn clear_finished(&self) {
        self.jobs.clear_finished();
        self.transfers.clear_finished();
    }

    pub async fn undo(&self, op: UndoOp) -> Result<()> {
        self.transfers.undo(op).await
    }

    /// Recursive folder comparison, by size and time or by content.
    pub async fn compare_dirs(
        &self,
        left: &str,
        right: &str,
        by_content: bool,
    ) -> Result<Vec<DiffItem>> {
        let opts = CompareOptions {
            recursive: true,
            by: if by_content {
                CompareBy::Content
            } else {
                CompareBy::SizeAndTime
            },
        };
        self.transfers.compare(left, right, opts).await
    }

    /// Submit the copy jobs that make the two folders agree; returns their ids.
    pub fn sync_dirs(
        &self,
        left: &str,
        right: &str,
        diff: &[DiffItem],
        direction: SyncDirection,
    ) -> Result<Vec<u64>> {
        let plan = cx_transfer::sync_plan(left, right, diff, direction)?;
        Ok(plan
            .into_iter()
            .map(|r| self.transfers.submit(r).0)
            .collect())
    }

    fn finish_task(&self, id: u64, result: Result<Option<UndoOp>>) {
        self.tasks.finish(id);
        self.jobs.update(id, |j| {
            j.speed = 0.0;
            j.eta = None;
            j.current = None;
            match result {
                Ok(undo) => {
                    j.state = "done".into();
                    j.undo = undo;
                }
                Err(CxError::Cancelled) => j.state = "cancelled".into(),
                Err(e) => {
                    j.state = "failed".into();
                    j.errors.push(FileError {
                        uri: j.sources.first().cloned().unwrap_or_default(),
                        message: e.to_string(),
                    });
                }
            }
        });
    }

    fn compress(self: &Arc<Self>, sources: Vec<String>, dest: String) -> u64 {
        let (id, cancel) = self.tasks.start();
        self.jobs.insert(JobView::new(
            id,
            "compress",
            sources.clone(),
            Location::parse(&dest)
                .ok()
                .and_then(|l| l.parent())
                .map(|p| p.uri()),
        ));
        let engine = self.clone();
        self.rt.spawn(async move {
            let started = std::time::Instant::now();
            let result = async {
                let locs = sources
                    .iter()
                    .map(|s| Location::parse(s))
                    .collect::<Result<Vec<_>>>()?;
                let dest_loc = Location::parse(&dest)?;
                let jobs = engine.jobs.clone();
                cx_archive::compress(
                    &engine.vfs,
                    locs,
                    dest_loc.clone(),
                    move |p| jobs.update(id, |j| progress_to(j, p, started)),
                    cancel,
                )
                .await?;
                Ok(Some(UndoOp::Copy {
                    created: vec![dest_loc.uri()],
                }))
            }
            .await;
            engine.finish_task(id, result);
        });
        id
    }

    fn extract(self: &Arc<Self>, sources: Vec<String>, dest: String) -> u64 {
        let (id, cancel) = self.tasks.start();
        self.jobs.insert(JobView::new(
            id,
            "extract",
            sources.clone(),
            Some(dest.clone()),
        ));
        let engine = self.clone();
        self.rt.spawn(async move {
            let started = std::time::Instant::now();
            let result = async {
                let dest_loc = Location::parse(&dest)?;
                let mut created = Vec::new();
                for s in &sources {
                    let jobs = engine.jobs.clone();
                    let out = cx_archive::extract(
                        &engine.vfs,
                        Location::parse(s)?,
                        dest_loc.clone(),
                        move |p| jobs.update(id, |j| progress_to(j, p, started)),
                        cancel.clone(),
                    )
                    .await?;
                    created.push(out.uri());
                }
                Ok(Some(UndoOp::Copy { created }))
            }
            .await;
            engine.finish_task(id, result);
        });
        id
    }

    /// Wait until a job (transfer or task) finishes; for tests and scripts.
    pub async fn wait_job(&self, id: u64) -> Option<JobView> {
        loop {
            match self.jobs.get(id) {
                Some(j) if j.is_finished() => return Some(j),
                None if !is_task(id) && self.transfers.job(JobId(id)).is_none() => return None,
                _ => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        }
    }
}
