//! Reading bytes through the Vfs, so every provider (local, remote, inside
//! an archive) can be previewed the same way.

use cx_core::{CxError, Location, Result, Vfs};
use tokio::io::AsyncReadExt;

/// Read at most `max` bytes from the start of `loc`.
pub(crate) async fn read_prefix(vfs: &Vfs, loc: &Location, max: u64) -> Result<Vec<u8>> {
    let provider = vfs.provider(loc).await?;
    let stream = provider.open_read(loc, 0).await?;
    let mut buf = Vec::new();
    stream
        .take(max)
        .read_to_end(&mut buf)
        .await
        .map_err(|e| CxError::from_io(e, loc))?;
    Ok(buf)
}

/// Run blocking work (decoding, resizing, spawning tools) off the runtime.
/// If the caller's future is dropped the closure still finishes, but it is
/// bounded work on a single file, and its result is simply discarded.
pub(crate) async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| CxError::Io(format!("worker failed: {e}")))?
}
