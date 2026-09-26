use crate::{Change, CxError, Entry, Location, Result};
use async_trait::async_trait;
use serde::Serialize;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;

/// What a provider can do. The UI adapts to these (for example it shows a
/// "live" dot only when changes are pushed rather than polled).
#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// Changes are pushed by the server/OS as they happen.
    pub live_watch: bool,
    /// Changes are found by re-listing periodically.
    pub polling: bool,
    /// Copies within the provider happen server-side (no download/upload).
    pub server_copy: bool,
    /// Deleting can go to a restorable trash.
    pub trash: bool,
    /// POSIX permissions are meaningful.
    pub posix: bool,
    /// Writes are possible at all.
    pub writable: bool,
}

pub type ReadStream = Pin<Box<dyn AsyncRead + Send>>;
pub type WriteStream = Pin<Box<dyn AsyncWrite + Send>>;

/// Called with batches of changes for a watched folder.
pub type WatchSink = Arc<dyn Fn(Vec<Change>) + Send + Sync>;

/// Keeps a watch alive; dropping it stops the watch.
pub struct WatchGuard(#[allow(dead_code)] Box<dyn Send + Sync>);

impl WatchGuard {
    pub fn new(inner: impl Send + Sync + 'static) -> Self {
        WatchGuard(Box::new(inner))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Fail with `AlreadyExists` if the file exists.
    CreateNew,
    /// Create or replace.
    Truncate,
    /// Continue a partial file (resumed transfer).
    Append,
}

/// Where a trashed item went, so the move can be undone.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrashedItem {
    pub original: String,
    /// URI of the item inside the trash, when the platform reports it.
    pub trashed: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Space {
    pub free: u64,
    pub total: u64,
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn scheme(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;

    /// Stream the entries of `dir` into `sink` in batches and return how many
    /// were sent. The first batch should be small so the UI can paint right
    /// away. Dropping the receiver cancels the listing.
    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize>;

    async fn stat(&self, loc: &Location) -> Result<Entry>;

    /// Create a folder called `name` inside `dir`. When `name` is `None` a
    /// free "New folder", "New folder (2)", … name is picked.
    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry>;

    /// Move or rename within this provider. Never overwrites: fails with
    /// `AlreadyExists` when `dst` exists (except a case-only rename).
    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()>;

    /// Rename `from` to `to` inside `dir`.
    async fn rename(&self, dir: &Location, from: &str, to: &str) -> Result<Entry> {
        crate::validate_name(to)?;
        let dst = dir.join(to);
        self.move_to(&dir.join(from), &dst).await?;
        self.stat(&dst).await
    }

    /// Permanently delete a file or a folder with everything in it.
    async fn remove(&self, loc: &Location) -> Result<()>;

    /// Move entries of `dir` to a restorable trash.
    async fn trash(&self, dir: &Location, names: &[String]) -> Result<Vec<TrashedItem>> {
        let _ = (dir, names);
        Err(CxError::Unsupported(format!("{} has no trash", self.scheme())))
    }

    /// Read a file from `offset` to the end.
    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream>;

    /// Write a file. Callers must `shutdown()` the stream to finish the write.
    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream>;

    /// Best effort: keep modification times when copying.
    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        let _ = (loc, ms);
        Ok(())
    }

    /// Copy without moving bytes through this machine, when possible.
    /// Returns `Ok(false)` if the provider can't, so the caller streams instead.
    async fn copy_within(&self, src: &Location, dst: &Location) -> Result<bool> {
        let _ = (src, dst);
        Ok(false)
    }

    /// Push changes of `dir` into `sink`. `Ok(None)` means "not supported";
    /// callers then fall back to polling (see [`crate::poll`]).
    async fn watch(&self, dir: &Location, sink: WatchSink) -> Result<Option<WatchGuard>> {
        let _ = (dir, sink);
        Ok(None)
    }

    async fn free_space(&self, loc: &Location) -> Result<Option<Space>> {
        let _ = loc;
        Ok(None)
    }
}

/// Collect a whole listing (convenience for tests and small folders).
pub async fn list_all(provider: &dyn Provider, dir: &Location) -> Result<Vec<Entry>> {
    let (tx, mut rx) = mpsc::channel(16);
    let list = provider.list(dir, tx);
    let collect = async {
        let mut out = Vec::new();
        while let Some(b) = rx.recv().await {
            out.extend(b);
        }
        out
    };
    let (res, entries) = tokio::join!(list, collect);
    res?;
    Ok(entries)
}
