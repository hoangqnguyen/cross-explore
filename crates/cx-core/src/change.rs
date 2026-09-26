use crate::Entry;
use serde::Serialize;

/// One patch to a watched folder's rows.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Change {
    /// The entry was added or its metadata changed.
    Upsert { entry: Entry },
    Remove { name: String },
    /// Too much changed (or events were lost): re-list the folder.
    Reset,
}
