//! What background tasks send back to the UI loop. Streaming results
//! (listings, watch patches, search hits, engine events) are typed; one-off
//! results arrive as [`Msg::Apply`], a state update run on the UI thread,
//! so an async operation never touches app state while it runs.

use crate::app::App;
use cx_core::{Change, CxError};
use cx_engine::{EngineEvent, ListEvent, SearchEvent, WatchInfo};

pub type Update = Box<dyn FnOnce(&mut App) + Send>;

// Messages are moved straight into `handle_msg`; boxing the listing event
// would add an allocation per batch for nothing.
#[allow(clippy::large_enum_variant)]
pub enum Msg {
    List { token: u64, seq: u64, event: ListEvent },
    ListFailed { token: u64, seq: u64, error: CxError },
    Changes { token: u64, changes: Vec<Change> },
    Watching { token: u64, result: Result<WatchInfo, CxError> },
    Search { token: u64, event: SearchEvent },
    Engine(EngineEvent),
    Apply(Update),
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Msg::List { token, .. } => write!(f, "List({token})"),
            Msg::ListFailed { token, error, .. } => write!(f, "ListFailed({token}, {error})"),
            Msg::Changes { token, changes } => write!(f, "Changes({token}, {})", changes.len()),
            Msg::Watching { token, .. } => write!(f, "Watching({token})"),
            Msg::Search { token, .. } => write!(f, "Search({token})"),
            Msg::Engine(_) => write!(f, "Engine"),
            Msg::Apply(_) => write!(f, "Apply"),
        }
    }
}
