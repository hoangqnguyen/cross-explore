use crate::{FileError, JobId, JobSnapshot, JobState, Progress, UndoOp};
use cx_core::Entry;
use serde::{Deserialize, Serialize};

/// A file that already exists at the destination, shown side by side.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictInfo {
    pub conflict_id: u64,
    pub source: Entry,
    pub source_uri: String,
    pub dest: Entry,
    pub dest_uri: String,
}

/// Everything the UI hears about transfers. Serialized as
/// `{"type": "progress", "id": 3, ...}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum TransferEvent {
    JobAdded { job: JobSnapshot },
    /// At most ~10 per second per job.
    Progress { id: JobId, progress: Progress },
    StateChanged { id: JobId, state: JobState },
    /// The job waits until [`crate::TransferManager::resolve`] is called.
    Conflict { id: JobId, conflict: ConflictInfo },
    /// One item failed; the job continues.
    FileError { id: JobId, error: FileError },
    Finished {
        id: JobId,
        state: JobState,
        /// Why the whole job failed (`state == failed`).
        error: Option<String>,
        /// How to revert what the job did, when it can be reverted.
        undo: Option<UndoOp>,
        errors: Vec<FileError>,
    },
}
