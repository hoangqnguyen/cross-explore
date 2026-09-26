//! The local file system as a Cross Explore [`Provider`](cx_core::Provider):
//! streamed listings, live watching through the OS (FSEvents,
//! ReadDirectoryChangesW, inotify), copy-on-write clones and a restorable trash.

mod provider;
pub mod trash;
pub mod watch;

pub use provider::LocalProvider;
pub use watch::{watch_dir, DirWatch};
