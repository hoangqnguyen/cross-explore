//! The thumbnail service: cache lookup, then the cheapest generator that
//! can handle the file, with a cap on how many run at once.

use crate::cache::{CacheStats, Cached, DiskCache};
use crate::image_thumb;
use crate::read::{blocking, read_prefix};
use cx_core::{CxError, Location, Result, Vfs};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

/// Remote images larger than this are not downloaded just for a thumbnail;
/// the UI shows the type icon instead.
pub const MAX_REMOTE_IMAGE_BYTES: u64 = 30 * 1024 * 1024;

/// Decoding a large photo takes one core for tens of milliseconds and a lot
/// of memory, so a folder of 5,000 photos must not start 5,000 at once: one
/// at a time per core, within these bounds.
const CONCURRENT_RANGE: (usize, usize) = (2, 8);

fn max_concurrent() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    cores.clamp(CONCURRENT_RANGE.0, CONCURRENT_RANGE.1)
}

/// A semaphore that serves the newest waiter first. The views ask for the
/// thumbnails on screen; after a fast scroll the requests queued for rows
/// already scrolled away would otherwise all run before the visible ones.
/// (The web view can't tell us a request was abandoned.)
struct Gate {
    state: Mutex<GateState>,
}

struct GateState {
    free: usize,
    waiters: Vec<oneshot::Sender<Permit>>,
}

/// Held while generating; dropping it lets the next (newest) waiter in.
pub(crate) struct Permit(Option<Arc<Gate>>);

impl Gate {
    fn new(n: usize) -> Arc<Gate> {
        Arc::new(Gate { state: Mutex::new(GateState { free: n, waiters: Vec::new() }) })
    }

    async fn acquire(self: &Arc<Gate>) -> Result<Permit> {
        let rx = {
            let mut st = self.state.lock().unwrap();
            if st.free > 0 {
                st.free -= 1;
                return Ok(Permit(Some(self.clone())));
            }
            let (tx, rx) = oneshot::channel();
            st.waiters.push(tx);
            rx
        };
        // A permit sent to a waiter that gave up is dropped with the channel,
        // which passes it on.
        rx.await.map_err(|_| CxError::Cancelled)
    }

    fn release(self: Arc<Gate>) {
        loop {
            let next = {
                let mut st = self.state.lock().unwrap();
                match st.waiters.pop() {
                    Some(tx) => tx,
                    None => {
                        st.free += 1;
                        return;
                    }
                }
            };
            match next.send(Permit(Some(self.clone()))) {
                Ok(()) => return,
                // That waiter is gone: disarm the permit, try the next one.
                Err(mut p) => drop(p.0.take()),
            }
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(gate) = self.0.take() {
            gate.release();
        }
    }
}

/// Thumbnails are generated at most this large (and at least this small).
const SIZE_RANGE: (u32, u32) = (16, 2048);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Thumb {
    /// Encoded image (PNG or JPEG, see `mime`). Not serialized: the UI loads
    /// thumbnails as binary through the `cx-thumb://` URI scheme, where JSON
    /// would only bloat them.
    #[serde(skip)]
    pub bytes: Vec<u8>,
    /// "image/png" (images with transparency, OS thumbnails) or "image/jpeg".
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

impl From<Cached> for Thumb {
    fn from(c: Cached) -> Thumb {
        Thumb { bytes: c.bytes, mime: c.mime, width: c.width, height: c.height }
    }
}

pub struct Thumbnailer {
    cache: Arc<DiskCache>,
    permits: Arc<Gate>,
}

impl Thumbnailer {
    /// `cache_dir` is created if needed; at most `max_cache_bytes` of
    /// thumbnails are kept there, least recently used ones go first.
    pub fn new(cache_dir: impl Into<PathBuf>, max_cache_bytes: u64) -> Result<Thumbnailer> {
        let dir = cache_dir.into();
        let cache = DiskCache::open(&dir, max_cache_bytes).map_err(|e| CxError::from_io(e, dir.display()))?;
        Ok(Thumbnailer { cache: Arc::new(cache), permits: Gate::new(max_concurrent()) })
    }

    pub fn stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// A thumbnail of `loc` fitting in `size_px`×`size_px` (never larger than
    /// the original). `Unsupported` means "no preview for this file": the UI
    /// shows the type icon.
    pub async fn thumbnail(&self, vfs: &Vfs, loc: &Location, size_px: u32) -> Result<Thumb> {
        let size = size_px.clamp(SIZE_RANGE.0, SIZE_RANGE.1);
        let provider = vfs.provider(loc).await?;
        let entry = provider.stat(loc).await?;
        let ext = crate::extension(&entry.name);
        let want = if app_icon(&ext, entry.is_dir) { crate::os::Want::Icon } else { crate::os::Want::Thumbnail };
        if entry.is_dir && want != crate::os::Want::Icon {
            return Err(CxError::Unsupported("thumbnail of a folder".into()));
        }
        // Any change to the file changes its mtime or size, so stale
        // thumbnails are never served; they just age out of the cache.
        let key = format!("{}\n{size}\n{}\n{}", loc.uri(), entry.modified.unwrap_or(0), entry.size);
        let cache = self.cache.clone();
        let k = key.clone();
        if let Some(hit) = blocking(move || Ok(cache.get(&k))).await? {
            return Ok(hit.into());
        }

        let permit = self.permits.acquire().await?;
        let made = match loc.local_path() {
            Some(path) if image_thumb::is_decodable(&ext) => {
                let p = path.to_path_buf();
                // The permit moves into the worker: if this future is dropped
                // the decode still counts against the limit until it ends.
                match blocking(move || {
                    let _permit = permit;
                    image_thumb::from_path(&p, size)
                })
                .await
                {
                    Ok(t) => t,
                    // Unusual variants (CMYK JPEG, odd TIFFs): the OS may cope.
                    Err(e) => {
                        let _permit = self.permits.acquire().await?;
                        crate::os::thumbnail(path.to_path_buf(), size, want).await.map_err(|_| e)?
                    }
                }
            }
            Some(path) => {
                let _permit = permit;
                crate::os::thumbnail(path.to_path_buf(), size, want).await?
            }
            None => {
                if !image_thumb::is_decodable(&ext) {
                    return Err(CxError::Unsupported(format!("no thumbnail for remote {}", entry.name)));
                }
                if entry.size > MAX_REMOTE_IMAGE_BYTES {
                    return Err(CxError::Unsupported(format!("{} is too large to preview remotely", entry.name)));
                }
                let bytes = read_prefix(vfs, loc, MAX_REMOTE_IMAGE_BYTES + 1).await?;
                if bytes.len() as u64 > MAX_REMOTE_IMAGE_BYTES {
                    return Err(CxError::Unsupported(format!("{} is too large to preview remotely", entry.name)));
                }
                blocking(move || {
                    let _permit = permit;
                    image_thumb::from_bytes(&bytes, size)
                })
                .await?
            }
        };

        let cache = self.cache.clone();
        let stored = made.clone();
        let _ = blocking(move || {
            cache.put(&key, &stored);
            Ok(())
        })
        .await;
        Ok(made.into())
    }
}

/// Programs whose own icon is their picture: macOS app bundles (folders) and
/// Windows executables, installers and shortcuts.
fn app_icon(ext: &str, is_dir: bool) -> bool {
    if is_dir {
        cfg!(target_os = "macos") && ext == "app"
    } else {
        cfg!(windows) && matches!(ext, "exe" | "msi" | "lnk" | "com" | "scr" | "cpl" | "appx" | "msix" | "appref-ms")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn gate_serves_the_newest_waiter_first() {
        let gate = Gate::new(1);
        let held = gate.acquire().await.unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for i in 0..3 {
            let (g, order) = (gate.clone(), order.clone());
            tasks.push(tokio::spawn(async move {
                let _p = g.acquire().await.unwrap();
                order.lock().unwrap().push(i);
            }));
            tokio::task::yield_now().await;
            while gate.state.lock().unwrap().waiters.len() < i + 1 {
                tokio::task::yield_now().await;
            }
        }
        // A waiter that gives up doesn't swallow the permit.
        tasks.remove(2).abort();
        drop(held);
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(*order.lock().unwrap(), [1, 0]);
        assert_eq!(gate.state.lock().unwrap().free, 1);
    }
}
