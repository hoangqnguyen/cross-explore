//! Embedded terminal sessions (the Commander One-style panel under the file
//! list).
//!
//! A terminal opens "in" the current [`Location`](cx_core::Location): the
//! user's login shell in a local folder, or the system `ssh` client landing
//! in the same folder for `sftp://` locations. SSH is always given an
//! explicit remote user — the local account is not assumed. A name is
//! remembered only after authentication succeeds, and a password login can
//! install a public key so the next session does not ask. Sessions run in a
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
mod ssh;
mod ssh_users;
mod terminals;

pub use command::{acceptable_ssh_token, posix_quote, user_at_host, ShellCommand};
pub use cx_core::{CxError, Result};
pub use event::TermEvent;
pub use ssh::{
    control_socket, install_public_key, next_ssh_id, parse_auth_method, prepare_ssh,
    watch_auth_log, AuthMethod, SshSession,
};
pub use ssh_users::SshUsers;
pub use terminals::{SshInfo, Terminals};
