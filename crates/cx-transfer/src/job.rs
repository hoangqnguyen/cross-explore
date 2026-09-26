//! Public job model: what the UI submits and what it gets back.

use crate::UndoOp;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(pub u64);

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobKind {
    Copy,
    Move,
    /// Permanent delete.
    Delete,
    /// Move to the restorable trash.
    Trash,
}

/// What to do when a file already exists at the destination. Folders are
/// never replaced wholesale: two folders with the same name merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ConflictPolicy {
    /// Stop and emit a conflict event; [`crate::TransferManager::resolve`] continues.
    #[default]
    Ask,
    Replace,
    Skip,
    KeepBoth,
    ReplaceIfNewer,
}

/// The user's answer to one conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Resolution {
    Replace,
    Skip,
    KeepBoth,
    /// Replace only when the source is newer, otherwise skip.
    ReplaceIfNewer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRequest {
    pub kind: JobKind,
    /// URIs (or anything [`cx_core::Location::parse`] accepts) of the items.
    pub sources: Vec<String>,
    /// Destination folder for copy and move.
    #[serde(default)]
    pub dest: Option<String>,
    #[serde(default)]
    pub conflict: ConflictPolicy,
    /// After copying, re-read both sides and compare BLAKE3 hashes.
    #[serde(default)]
    pub verify: bool,
}

impl JobRequest {
    fn new(kind: JobKind, sources: Vec<String>, dest: Option<String>) -> Self {
        JobRequest { kind, sources, dest, conflict: ConflictPolicy::Ask, verify: false }
    }

    pub fn copy(sources: Vec<String>, dest: impl Into<String>) -> Self {
        Self::new(JobKind::Copy, sources, Some(dest.into()))
    }

    pub fn move_to(sources: Vec<String>, dest: impl Into<String>) -> Self {
        Self::new(JobKind::Move, sources, Some(dest.into()))
    }

    pub fn delete(sources: Vec<String>) -> Self {
        Self::new(JobKind::Delete, sources, None)
    }

    pub fn trash(sources: Vec<String>) -> Self {
        Self::new(JobKind::Trash, sources, None)
    }

    pub fn with_conflict(mut self, conflict: ConflictPolicy) -> Self {
        self.conflict = conflict;
        self
    }

    pub fn with_verify(mut self, verify: bool) -> Self {
        self.verify = verify;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum JobState {
    Queued,
    /// Walking source folders to compute totals.
    Scanning,
    Running,
    Paused,
    WaitingForConflict,
    Done,
    Failed,
    Cancelled,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        matches!(self, JobState::Done | JobState::Failed | JobState::Cancelled)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: u64,
    pub files_total: u64,
    /// Name of a file being transferred right now.
    pub current: Option<String>,
    /// Smoothed throughput in bytes per second.
    pub speed: u64,
    pub eta_secs: Option<u64>,
}

/// One item that could not be processed; the job carries on with the rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileError {
    pub uri: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub id: JobId,
    pub kind: JobKind,
    pub state: JobState,
    pub sources: Vec<String>,
    pub dest: Option<String>,
    pub progress: Progress,
    pub errors: Vec<FileError>,
    /// Why the whole job failed.
    pub error: Option<String>,
    pub undo: Option<UndoOp>,
}
