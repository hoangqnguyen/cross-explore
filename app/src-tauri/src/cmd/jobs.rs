//! Transfers (copy / move / delete / trash through the transfer engine) and
//! archive jobs (compress / extract), all reported as UI jobs.

use super::AppState;
use crate::jobs::UiJob;
use crate::state::App;
use cx_core::{CxError, Location, Result};
use cx_transfer::{CompareBy, CompareOptions, ConflictPolicy, DiffItem, JobId, JobKind, JobRequest, Resolution, UndoOp};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiJobRequest {
    kind: String,
    sources: Vec<String>,
    dest: Option<String>,
    #[serde(default)]
    conflict: ConflictPolicy,
    #[serde(default)]
    verify: bool,
}

#[tauri::command]
pub async fn transfer_submit(req: UiJobRequest, app: AppState<'_>) -> Result<u64> {
    let kind = match req.kind.as_str() {
        "copy" => JobKind::Copy,
        "move" => JobKind::Move,
        "delete" => JobKind::Delete,
        "trash" => JobKind::Trash,
        "compress" => return Ok(compress(app.inner().clone(), req.sources, req.dest.ok_or_else(|| CxError::InvalidLocation("missing destination".into()))?)),
        "extract" => return Ok(extract(app.inner().clone(), req.sources, req.dest.ok_or_else(|| CxError::InvalidLocation("missing destination".into()))?)),
        other => return Err(CxError::Unsupported(format!("{other} jobs"))),
    };
    for s in &req.sources {
        Location::parse(s)?;
    }
    let id = app.transfers.submit(JobRequest { kind, sources: req.sources, dest: req.dest, conflict: req.conflict, verify: req.verify });
    Ok(id.0)
}

fn is_task(id: u64) -> bool {
    id >= 1 << 40
}

#[tauri::command]
pub fn transfer_pause(id: u64, app: AppState<'_>) {
    if !is_task(id) {
        app.transfers.pause(JobId(id));
    }
}

#[tauri::command]
pub fn transfer_resume(id: u64, app: AppState<'_>) {
    if !is_task(id) {
        app.transfers.resume(JobId(id));
    }
}

#[tauri::command]
pub fn transfer_cancel(id: u64, app: AppState<'_>) {
    if is_task(id) {
        app.cancel_task(id);
    } else {
        app.transfers.cancel(JobId(id));
    }
}

#[tauri::command]
pub fn transfer_resolve(id: u64, conflict_id: u64, resolution: Resolution, apply_to_all: bool, app: AppState<'_>) {
    app.transfers.resolve(JobId(id), conflict_id, resolution, apply_to_all);
}

#[tauri::command]
pub fn transfer_list(app: AppState<'_>) -> Vec<UiJob> {
    app.jobs.list()
}

#[tauri::command]
pub async fn undo(op: UndoOp, app: AppState<'_>) -> Result<()> {
    app.transfers.undo(op).await
}

#[tauri::command]
pub async fn compare_dirs(left: String, right: String, by_content: bool, app: AppState<'_>) -> Result<Vec<DiffItem>> {
    let opts = CompareOptions { recursive: true, by: if by_content { CompareBy::Content } else { CompareBy::SizeAndTime } };
    app.transfers.compare(&left, &right, opts).await
}

fn progress_to(j: &mut UiJob, p: &cx_archive::Progress, started: std::time::Instant) {
    j.state = "running".into();
    j.bytes_done = p.bytes_done;
    j.bytes_total = p.bytes_total;
    j.files_done = p.files_done;
    j.files_total = p.files_total;
    j.current = Some(p.current.clone()).filter(|c| !c.is_empty());
    let secs = started.elapsed().as_secs_f64().max(0.001);
    j.speed = p.bytes_done as f64 / secs;
    j.eta = (j.speed > 0.0 && p.bytes_total > p.bytes_done).then(|| (p.bytes_total - p.bytes_done) as f64 / j.speed);
}

fn finish(app: &App, id: u64, result: Result<Option<serde_json::Value>>) {
    app.finish_task(id);
    app.jobs.update(id, |j| {
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
                j.errors.push(cx_transfer::FileError { uri: j.sources.first().cloned().unwrap_or_default(), message: e.to_string() });
            }
        }
    });
}

fn compress(app: Arc<App>, sources: Vec<String>, dest: String) -> u64 {
    let (id, cancel) = app.task();
    app.jobs.insert(UiJob::new(id, "compress", sources.clone(), Location::parse(&dest).ok().and_then(|l| l.parent()).map(|p| p.uri())));
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        let result = async {
            let locs = sources.iter().map(|s| Location::parse(s)).collect::<Result<Vec<_>>>()?;
            let dest_loc = Location::parse(&dest)?;
            let jobs = app.jobs.clone();
            cx_archive::compress(&app.vfs, locs, dest_loc.clone(), move |p| jobs.update(id, |j| progress_to(j, p, started)), cancel).await?;
            Ok(serde_json::to_value(UndoOp::Copy { created: vec![dest_loc.uri()] }).ok())
        }
        .await;
        finish(&app, id, result);
    });
    id
}

fn extract(app: Arc<App>, sources: Vec<String>, dest: String) -> u64 {
    let (id, cancel) = app.task();
    app.jobs.insert(UiJob::new(id, "extract", sources.clone(), Some(dest.clone())));
    tauri::async_runtime::spawn(async move {
        let started = std::time::Instant::now();
        let result = async {
            let dest_loc = Location::parse(&dest)?;
            let mut created = Vec::new();
            for s in &sources {
                let jobs = app.jobs.clone();
                let out = cx_archive::extract(&app.vfs, Location::parse(s)?, dest_loc.clone(), move |p| jobs.update(id, |j| progress_to(j, p, started)), cancel.clone()).await?;
                created.push(out.uri());
            }
            Ok(serde_json::to_value(UndoOp::Copy { created }).ok())
        }
        .await;
        finish(&app, id, result);
    });
    id
}

#[tauri::command]
pub fn transfer_clear(app: AppState<'_>) {
    app.jobs.clear_finished();
}
