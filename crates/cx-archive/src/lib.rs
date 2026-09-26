//! Archives as folders.
//!
//! [`ArchiveProvider`] serves `archive://<container-uri>!/inner/path`
//! locations: zip, tar (plain, gz, bz2, xz, zst) and 7z can be browsed and
//! read like any folder, and zip archives can also be edited (new folders,
//! new files, rename, delete). The container may be local or on any remote
//! provider; remote ones are downloaded to a cache first.
//!
//! [`compress`] and [`extract`] are the "Compress" / "Extract" commands and
//! work between any two providers.
//!
//! All codecs are pure Rust, so the crate cross-compiles without a C
//! toolchain. RAR is not supported (no maintained pure-Rust reader exists).

mod cache;
mod format;
mod index;
mod ops;
mod provider;
mod stream;
mod time;
mod walk;
mod zipedit;

pub use format::{is_archive, strip_archive_ext, ArchiveFormat};
pub use ops::{compress, extract, Progress};
pub use provider::ArchiveProvider;
pub use tokio_util::sync::CancellationToken;
