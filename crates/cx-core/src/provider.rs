use crate::{Entry, Location, Result};
use async_trait::async_trait;
use serde::Serialize;
use tokio::sync::mpsc;

/// What a provider can do. The UI adapts to these (for example it shows a
/// "live" dot only when changes are pushed rather than polled).
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub live_watch: bool,
    pub polling: bool,
    pub server_copy: bool,
    pub trash: bool,
    pub posix: bool,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn scheme(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;

    /// Stream the entries of `dir` into `sink` in batches and return how many
    /// were sent. The first batch is small so the UI can paint right away.
    /// Dropping the receiver cancels the listing.
    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize>;

    async fn stat(&self, loc: &Location) -> Result<Entry>;

    /// Create a folder called `name` inside `dir`. When `name` is `None` a
    /// free "New folder", "New folder (2)", … name is picked.
    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry>;

    /// Rename `from` to `to` inside `dir`. Never overwrites an existing entry.
    async fn rename(&self, dir: &Location, from: &str, to: &str) -> Result<Entry>;

    /// Move entries of `dir` to the system trash.
    async fn trash(&self, dir: &Location, names: &[String]) -> Result<()>;
}
