//! Reverting finished operations (Ctrl/Cmd+Z).

use crate::{JobRequest, JobState, TransferManager};
use cx_core::provider::list_all;
use cx_core::{CxError, Location, Provider, Result, TrashedItem};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovedItem {
    pub from: String,
    pub to: String,
}

/// How to revert one operation. Jobs report theirs in the `finished` event;
/// renames and new folders done directly by the UI build theirs with
/// [`UndoOp::rename`] and [`UndoOp::new_folder`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum UndoOp {
    /// Remove the top-most items a copy created (to the trash when the
    /// provider has one). Files that replaced existing ones are not listed:
    /// deleting them would not bring the originals back.
    Copy { created: Vec<String> },
    /// Move items back to where they came from.
    Move { items: Vec<MovedItem> },
    /// Rename `to` back to `from` inside `dir`.
    Rename { dir: String, from: String, to: String },
    /// Restore items from the system trash.
    Trash { items: Vec<TrashedItem> },
    /// Remove a folder created by "New folder", if it is still empty.
    NewFolder { uri: String },
}

impl UndoOp {
    pub fn rename(dir: &Location, from: &str, to: &str) -> UndoOp {
        UndoOp::Rename { dir: dir.uri(), from: from.into(), to: to.into() }
    }

    pub fn new_folder(loc: &Location) -> UndoOp {
        UndoOp::NewFolder { uri: loc.uri() }
    }
}

fn parent_of(loc: &Location) -> Result<Location> {
    loc.parent().ok_or_else(|| CxError::InvalidLocation(format!("{loc} has no parent")))
}

/// Create `dir` and missing parents.
async fn ensure_dir(p: &dyn Provider, dir: &Location) -> Result<()> {
    let mut missing = Vec::new();
    let mut cur = dir.clone();
    loop {
        match p.stat(&cur).await {
            Ok(e) if e.is_dir => break,
            Ok(_) => return Err(CxError::AlreadyExists(cur.uri())),
            Err(CxError::NotFound(_)) => {
                let parent = parent_of(&cur)?;
                missing.push(cur);
                cur = parent;
            }
            Err(e) => return Err(e),
        }
    }
    for d in missing.into_iter().rev() {
        match p.create_dir(&parent_of(&d)?, Some(&d.name())).await {
            Ok(_) | Err(CxError::AlreadyExists(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Try every item, report the first failure.
fn first_error(errors: Vec<CxError>) -> Result<()> {
    errors.into_iter().next().map_or(Ok(()), Err)
}

pub(crate) async fn execute(mgr: &TransferManager, op: UndoOp) -> Result<()> {
    let vfs = mgr.vfs().clone();
    match op {
        UndoOp::Copy { created } => {
            let mut errors = Vec::new();
            for uri in created.iter().rev() {
                let r = async {
                    let loc = Location::parse(uri)?;
                    let p = vfs.provider(&loc).await?;
                    if mgr.config().undo_uses_trash && p.capabilities().trash {
                        p.trash(&parent_of(&loc)?, &[loc.name()]).await.map(drop)
                    } else {
                        p.remove(&loc).await
                    }
                };
                match r.await {
                    Ok(()) | Err(CxError::NotFound(_)) => {}
                    Err(e) => errors.push(e),
                }
            }
            first_error(errors)
        }
        UndoOp::Move { items } => {
            let mut errors = Vec::new();
            for item in items.iter().rev() {
                if let Err(e) = move_back(mgr, item).await {
                    errors.push(e);
                }
            }
            first_error(errors)
        }
        UndoOp::Rename { dir, from, to } => {
            let dir = Location::parse(&dir)?;
            vfs.provider(&dir).await?.rename(&dir, &to, &from).await.map(drop)
        }
        UndoOp::Trash { items } => {
            let all_local = items.iter().all(|i| Location::parse(&i.original).map(|l| l.is_local()).unwrap_or(false));
            if !all_local {
                return Err(CxError::Unsupported("restoring remote items from the trash".into()));
            }
            tokio::task::spawn_blocking(move || cx_local::trash::restore(&items))
                .await
                .map_err(|e| CxError::Io(format!("restore failed: {e}")))?
        }
        UndoOp::NewFolder { uri } => {
            let loc = Location::parse(&uri)?;
            let p = vfs.provider(&loc).await?;
            if !list_all(p.as_ref(), &loc).await?.is_empty() {
                return Err(CxError::Unsupported(format!("\"{}\" is no longer empty", loc.name())));
            }
            p.remove(&loc).await
        }
    }
}

async fn move_back(mgr: &TransferManager, item: &MovedItem) -> Result<()> {
    let (from, to) = (Location::parse(&item.from)?, Location::parse(&item.to)?);
    let home = parent_of(&from)?;
    let p = mgr.vfs().provider(&from).await?;
    ensure_dir(p.as_ref(), &home).await?;
    if from.same_provider(&to) {
        return p.move_to(&to, &from).await;
    }
    // Across providers the bytes have to travel again: run it as a normal,
    // visible move job so the user sees progress.
    let id = mgr.submit(JobRequest::move_to(vec![item.to.clone()], home.uri()).with_conflict(crate::ConflictPolicy::Skip));
    let snap = mgr.wait(id).await.ok_or(CxError::Cancelled)?;
    if snap.state != JobState::Done || !snap.errors.is_empty() {
        let why = snap.error.or_else(|| snap.errors.first().map(|e| e.message.clone())).unwrap_or_else(|| "cancelled".into());
        return Err(CxError::Io(format!("moving {} back failed: {why}", to.name())));
    }
    if from.name() != to.name() {
        p.rename(&home, &to.name(), &from.name()).await?;
    }
    Ok(())
}
