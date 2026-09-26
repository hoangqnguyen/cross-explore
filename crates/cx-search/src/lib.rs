//! Search for Cross Explore.
//!
//! * [`search`] walks any location (local, remote or inside an archive)
//!   breadth-first and streams name matches in batches, so hits close to the
//!   starting folder show up first and the UI paints before the walk ends.
//! * [`search_content`] greps file contents. Local trees use ripgrep's
//!   internals (parallel walker, SIMD searcher, binary detection); other
//!   providers fall back to streaming reads of reasonably small files.
//! * [`fuzzy_rank`] ranks short candidate lists (actions, favorites, recent
//!   folders) for the command palette.
//!
//! Both searches take a [`Cancel`] token and stop quickly when it fires or
//! when the receiving end of the sink is dropped.

mod batch;
mod cancel;
mod content;
mod filter;
mod fold;
mod fuzzy;
mod name;

pub use cancel::Cancel;
pub use content::{search_content, ContentHit, ContentQuery, LineMatch};
pub use filter::{KindFilter, MatchMode, SearchQuery};
pub use fold::fold;
pub use fuzzy::{fuzzy_rank, FuzzyMatch};
pub use name::{search, SearchHit};

use serde::Serialize;

/// Totals reported when a search ends (normally, truncated or cancelled).
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SearchStats {
    pub dirs_scanned: u64,
    pub entries_scanned: u64,
    /// Content search only: files whose contents were read.
    pub files_searched: u64,
    pub hits: u64,
    /// Folders or files that could not be read (permission denied, vanished…).
    pub errors: u64,
    pub elapsed_ms: u64,
    /// Stopped early because `maxResults` / `maxFiles` was reached.
    pub truncated: bool,
    /// Stopped early because the search was cancelled or the sink was dropped.
    pub cancelled: bool,
}

fn bad_pattern(e: impl std::fmt::Display) -> cx_core::CxError {
    cx_core::CxError::InvalidName(format!("invalid search pattern: {e}"))
}
