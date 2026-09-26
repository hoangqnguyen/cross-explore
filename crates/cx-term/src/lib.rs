//! Embedded terminal sessions (the Commander One-style panel under the file
//! list).
//!
//! A terminal opens "in" the current [`Location`](cx_core::Location): the
//! user's login shell in a local folder, or the system `ssh` client landing
//! in the same folder for `sftp://` locations. Sessions run in a
//! pseudo-terminal from `portable-pty` (openpty on Unix, ConPTY on Windows);
//! output is coalesced and delivered as [`TermEvent`]s ready to be pushed
//! through a Tauri channel to xterm.js.
//!
//! Mobile builds compile the same API without the PTY stack; opening a
//! session there fails with [`CxError::Unsupported`].

mod command;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod cwd;
mod event;
#[cfg(not(any(target_os = "ios", target_os = "android")))]
mod session;
mod terminals;

pub use command::{posix_quote, ShellCommand};
pub use cx_core::{CxError, Result};
pub use event::TermEvent;
pub use terminals::Terminals;
