//! Core model shared by every Cross Explore component.
//!
//! Everything the UI can browse is addressed by a [`Location`] (a URI such as
//! `file:///Users/me` or, later, `sftp://nas/home`) and served by a
//! [`Provider`]. Providers stream directory listings in batches so the UI can
//! paint the first rows long before a large folder has been fully read.

pub mod entry;
pub mod error;
pub mod local;
pub mod location;
pub mod provider;

pub use entry::{Entry, EntryKind};
pub use error::{CxError, Result};
pub use local::LocalProvider;
pub use location::{Crumb, Location, LocationInfo};
pub use provider::{Capabilities, Provider};
