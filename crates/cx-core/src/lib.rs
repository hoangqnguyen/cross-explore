//! Core model shared by every Cross Explore component.
//!
//! Everything the UI can browse is addressed by a [`Location`] (a URI such as
//! `file:///Users/me`, `sftp://pi@nas/home` or `archive://file:///x.zip!/docs`)
//! and served by a [`Provider`]. The [`Vfs`] maps locations to providers,
//! opening and caching connections to remote endpoints on demand.
//!
//! Providers stream directory listings in batches so the UI can paint the
//! first rows long before a large folder has been fully read, and expose
//! byte streams so the transfer engine can copy between any two of them.

pub mod change;
pub mod entry;
pub mod error;
pub mod location;
pub mod poll;
pub mod provider;
pub mod vfs;

pub use change::Change;
pub use entry::{Entry, EntryKind};
pub use error::{CxError, Result};
pub use location::{Crumb, Endpoint, Location, LocationInfo, Scheme};
pub use provider::{Capabilities, Provider, ReadStream, Space, TrashedItem, WatchGuard, WatchSink, WriteMode, WriteStream};
pub use vfs::{Connector, CredentialStore, Credentials, MemoryCredentials, Secret, Vfs};

/// Reject names that can't be a single path component.
pub fn validate_name(name: &str) -> Result<()> {
    let bad_char = |c: char| c == '/' || c == '\0' || (cfg!(windows) && "\\:*?\"<>|".contains(c));
    if name.is_empty() || name == "." || name == ".." || name.chars().any(bad_char) {
        return Err(CxError::InvalidName(name.to_string()));
    }
    Ok(())
}
