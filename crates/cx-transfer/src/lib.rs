//! The Cross Explore transfer engine.
//!
//! A [`TransferManager`] runs copy, move, delete and trash jobs between any
//! locations the [`cx_core::Vfs`] can reach. Each job is scanned for totals,
//! then walked: folders are created in order while files stream concurrently
//! (limited per endpoint across all jobs). Progress, conflicts, per-file
//! errors and completion arrive as [`TransferEvent`]s; finished jobs carry an
//! [`UndoOp`] that [`TransferManager::undo`] can execute.
//!
//! Fast paths come first: a move inside one provider is a rename, a copy
//! inside one provider tries [`Provider::copy_within`](cx_core::Provider::copy_within)
//! (clonefile/reflink, server-side copy). Everything else streams through
//! `<name>.cxpart` files that are renamed into place when complete, so a
//! destination never holds half a file under its real name, and a dropped
//! connection resumes where it stopped.
//!
//! [`compare()`] and [`sync_plan`] implement folder compare/sync.

mod compare;
mod control;
mod copy;
mod engine;
mod event;
mod job;
mod limits;
mod manager;
pub mod naming;
mod persist;
mod progress;
mod undo;

pub use compare::{compare, sync_plan, CompareBy, CompareOptions, DiffItem, DiffKind, SyncDirection};
pub use event::{ConflictInfo, TransferEvent};
pub use job::{ConflictPolicy, FileError, JobId, JobKind, JobRequest, JobSnapshot, JobState, Progress, Resolution};
pub use manager::{TransferConfig, TransferManager};
pub use undo::{MovedItem, UndoOp};
