//! Byte cache for playing and previewing files that live on servers.
//!
//! The web view reads media through `cxfile://` in many small HTTP ranges
//! (seek to the end for an MP4's index, back to the start, then onward as it
//! plays), and a preview pane and Quick Look may read the same file at once.
//! Opening the remote file afresh for each range is slow over SFTP/SMB/…, so
//! here the file is read in 1 MiB chunks that are shared between requests,
//! with a short read-ahead so playback doesn't stall, and `stat` results are
//! remembered briefly.

use cx_core::{CxError, Entry, Location, Provider, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

pub const CHUNK: u64 = 1 << 20;
/// Cached chunks across all files (64 MiB).
const CAPACITY: usize = 64;
/// Chunks read ahead of the last range served.
const READ_AHEAD: u64 = 4;
const STAT_TTL: Duration = Duration::from_secs(10);

type Key = (String, u64);

#[derive(Default)]
pub struct RemoteBytes {
    chunks: Mutex<HashMap<Key, (Arc<Vec<u8>>, u64)>>,
    stats: Mutex<HashMap<String, (Instant, Entry)>>,
    tick: std::sync::atomic::AtomicU64,
    /// Read-aheads in progress, so a burst of ranges starts one per spot.
    prefetching: Mutex<std::collections::HashSet<Key>>,
}

/// Identifies one version of a file: a changed file never serves old bytes.
fn version(loc: &Location, e: &Entry) -> String {
    format!("{}|{}|{:?}", loc.uri(), e.size, e.modified)
}

impl RemoteBytes {
    pub async fn stat(&self, provider: &dyn Provider, loc: &Location) -> Result<Entry> {
        let uri = loc.uri();
        if let Some((at, e)) = self.stats.lock().unwrap().get(&uri) {
            if at.elapsed() < STAT_TTL {
                return Ok(e.clone());
            }
        }
        let e = provider.stat(loc).await?;
        let mut stats = self.stats.lock().unwrap();
        if stats.len() > 256 {
            stats.retain(|_, (at, _)| at.elapsed() < STAT_TTL);
        }
        stats.insert(uri, (Instant::now(), e.clone()));
        Ok(e)
    }

    fn get(&self, key: &Key) -> Option<Arc<Vec<u8>>> {
        let t = self.tick.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut m = self.chunks.lock().unwrap();
        m.get_mut(key).map(|(data, used)| {
            *used = t;
            data.clone()
        })
    }

    fn put(&self, key: Key, data: Arc<Vec<u8>>) {
        let t = self.tick.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut m = self.chunks.lock().unwrap();
        m.insert(key, (data, t));
        while m.len() > CAPACITY {
            let oldest = m.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => m.remove(&k),
                None => break,
            };
        }
    }

    /// Read chunks `first..=last` of the file, fetching the missing ones with
    /// one sequential stream (starting at the first missing chunk).
    async fn chunks(&self, provider: &dyn Provider, loc: &Location, ver: &str, size: u64, first: u64, last: u64) -> Result<Vec<Arc<Vec<u8>>>> {
        let key = |i: u64| (ver.to_string(), i);
        let mut out: Vec<Option<Arc<Vec<u8>>>> = (first..=last).map(|i| self.get(&key(i))).collect();
        if let Some(missing) = out.iter().position(Option::is_none) {
            let from = first + missing as u64;
            let mut reader = provider.open_read(loc, from * CHUNK).await?;
            for i in from..=last {
                let want = CHUNK.min(size.saturating_sub(i * CHUNK)) as usize;
                let mut buf = vec![0u8; want];
                reader.read_exact(&mut buf).await.map_err(|e| CxError::io("read failed", e))?;
                let buf = Arc::new(buf);
                self.put(key(i), buf.clone());
                out[(i - first) as usize] = Some(buf);
            }
        }
        Ok(out.into_iter().map(|c| c.expect("filled above")).collect())
    }

    /// Bytes `start..start+len` of a remote file, from cached chunks.
    pub async fn read(&self, provider: &dyn Provider, loc: &Location, entry: &Entry, start: u64, len: u64) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let ver = version(loc, entry);
        let (first, last) = (start / CHUNK, (start + len - 1) / CHUNK);
        let parts = self.chunks(provider, loc, &ver, entry.size, first, last).await?;
        let mut out = Vec::with_capacity(len as usize);
        for (n, part) in parts.iter().enumerate() {
            let base = (first + n as u64) * CHUNK;
            let from = start.saturating_sub(base) as usize;
            let to = ((start + len).min(base + part.len() as u64) - base) as usize;
            if from < to {
                out.extend_from_slice(&part[from..to]);
            }
        }
        Ok(out)
    }

    /// Start reading the chunks after `end` in the background.
    pub fn read_ahead(self: &Arc<Self>, provider: Arc<dyn Provider>, loc: Location, entry: Entry, end: u64) {
        let ver = version(&loc, &entry);
        let first = end / CHUNK + 1;
        let last_chunk = entry.size.saturating_sub(1) / CHUNK;
        if first > last_chunk || self.get(&(ver.clone(), first)).is_some() {
            return;
        }
        if !self.prefetching.lock().unwrap().insert((ver.clone(), first)) {
            return;
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let last = (first + READ_AHEAD - 1).min(last_chunk);
            let _ = me.chunks(provider.as_ref(), &loc, &ver, entry.size, first, last).await;
            me.prefetching.lock().unwrap().remove(&(ver, first));
        });
    }

    #[cfg(test)]
    fn cached(&self) -> usize {
        self.chunks.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cx_testkit::MemProvider;

    #[tokio::test]
    async fn ranges_come_from_shared_chunks() {
        let data: Vec<u8> = (0..(5 * CHUNK + 12345)).map(|i| (i % 251) as u8).collect();
        let remote = MemProvider::with_scheme("sftp");
        remote.put("/v.mp4", data.clone());
        let loc = Location::parse("sftp://h/v.mp4").unwrap();
        let cache = RemoteBytes::default();
        let e = cache.stat(remote.as_ref(), &loc).await.unwrap();

        // A range across a chunk boundary, then ranges inside cached chunks.
        let a = cache.read(remote.as_ref(), &loc, &e, CHUNK - 10, 20).await.unwrap();
        assert_eq!(a, data[(CHUNK - 10) as usize..(CHUNK + 10) as usize]);
        assert_eq!(remote.reads_opened(), 1, "one stream for both chunks");
        let b = cache.read(remote.as_ref(), &loc, &e, 5, 100).await.unwrap();
        assert_eq!(b, data[5..105]);
        assert_eq!(remote.reads_opened(), 1, "served from cache");

        // The tail (short last chunk), like an MP4 index at the end.
        let size = data.len() as u64;
        let t = cache.read(remote.as_ref(), &loc, &e, size - 50, 50).await.unwrap();
        assert_eq!(t, data[(size - 50) as usize..]);
        assert_eq!(remote.reads_opened(), 2);
        assert!(cache.cached() <= CAPACITY);
    }

    #[tokio::test]
    async fn stat_is_remembered_briefly() {
        let remote = MemProvider::with_scheme("sftp");
        remote.put("/v.mp4", vec![1; 10]);
        let loc = Location::parse("sftp://h/v.mp4").unwrap();
        let cache = RemoteBytes::default();
        let a = cache.stat(remote.as_ref(), &loc).await.unwrap();
        remote.put("/v.mp4", vec![1; 20]);
        let b = cache.stat(remote.as_ref(), &loc).await.unwrap();
        assert_eq!(a.size, b.size, "second stat within the TTL comes from the cache");
    }
}
