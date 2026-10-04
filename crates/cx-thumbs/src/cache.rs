//! Disk LRU cache for generated thumbnails.
//!
//! Each thumbnail is one file named after a hash of its key, with its pixel
//! size and format in the name (`<hash>-<w>x<h>.png`), so nothing else has to
//! be stored and the in-memory index can be rebuilt from a directory listing
//! at startup. The file's mtime doubles as its "last used" time: hits touch
//! it (at most once per [`TOUCH_EVERY`]), which keeps the LRU order across
//! restarts.

use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A hit rewrites the file's mtime only if it is older than this: the
/// in-memory order is exact, the disk one only has to survive a restart, and
/// a write per hit made scrolling a cached folder write to disk constantly.
const TOUCH_EVERY: u64 = 60 * 60 * 1000;

/// A cached (or freshly made) thumbnail image.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cached {
    pub bytes: Vec<u8>,
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub entries: usize,
    pub bytes: u64,
}

struct Item {
    file: String,
    bytes: u64,
    /// Milliseconds since the epoch, strictly increasing per use.
    last_used: u64,
    /// The file's mtime, in the same unit.
    touched: u64,
}

#[derive(Default)]
struct State {
    items: HashMap<String, Item>,
    total: u64,
    clock: u64,
}

pub struct DiskCache {
    dir: PathBuf,
    max_bytes: u64,
    state: Mutex<State>,
    hits: AtomicU64,
    misses: AtomicU64,
}

impl DiskCache {
    /// Open (creating if needed) a cache in `dir` that keeps at most
    /// `max_bytes` of thumbnails.
    pub fn open(dir: impl Into<PathBuf>, max_bytes: u64) -> std::io::Result<DiskCache> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let mut state = State::default();
        for de in fs::read_dir(&dir)?.flatten() {
            let file = de.file_name().to_string_lossy().into_owned();
            let Some((hash, _)) = parse_name(&file) else {
                // Leftover temp files from a crash.
                if file.starts_with(".tmp") {
                    let _ = fs::remove_file(de.path());
                }
                continue;
            };
            let Ok(meta) = de.metadata() else { continue };
            let last_used = meta.modified().map(millis).unwrap_or(0);
            state.clock = state.clock.max(last_used);
            state.total += meta.len();
            state.items.insert(hash.to_string(), Item { file, bytes: meta.len(), last_used, touched: last_used });
        }
        let cache = DiskCache { dir, max_bytes, state: Mutex::new(state), hits: AtomicU64::new(0), misses: AtomicU64::new(0) };
        cache.evict();
        Ok(cache)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn stats(&self) -> CacheStats {
        let st = self.state.lock().unwrap();
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            entries: st.items.len(),
            bytes: st.total,
        }
    }

    /// Look up `key`; a hit refreshes its place in the LRU order.
    pub(crate) fn get(&self, key: &str) -> Option<Cached> {
        let hash = hash_key(key);
        let (file, touch) = {
            let mut st = self.state.lock().unwrap();
            let now = tick(&mut st);
            match st.items.get_mut(&hash) {
                Some(item) => {
                    item.last_used = now;
                    let touch = now.saturating_sub(item.touched) >= TOUCH_EVERY;
                    if touch {
                        item.touched = now;
                    }
                    (item.file.clone(), touch)
                }
                None => {
                    drop(st);
                    self.misses.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            }
        };
        let path = self.dir.join(&file);
        let (_, (width, height, mime)) = parse_name(&file)?;
        match fs::read(&path) {
            Ok(bytes) => {
                if touch {
                    if let Ok(f) = fs::File::options().write(true).open(&path) {
                        let _ = f.set_modified(SystemTime::now());
                    }
                }
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(Cached { bytes, mime, width, height })
            }
            Err(_) => {
                // Deleted behind our back: forget it.
                let mut st = self.state.lock().unwrap();
                if let Some(item) = st.items.remove(&hash) {
                    st.total -= item.bytes;
                }
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Store a thumbnail, evicting the least recently used ones if the cache
    /// grows past its budget. Failures are ignored: the cache is an
    /// optimisation, never a reason to fail a thumbnail.
    pub(crate) fn put(&self, key: &str, thumb: &Cached) {
        let hash = hash_key(key);
        let ext = if thumb.mime == "image/jpeg" { "jpg" } else { "png" };
        let file = format!("{hash}-{}x{}.{ext}", thumb.width, thumb.height);
        let tmp = self.dir.join(format!(".tmp-{hash}-{}", std::process::id()));
        let written = fs::File::create(&tmp).and_then(|mut f| f.write_all(&thumb.bytes)).and_then(|_| fs::rename(&tmp, self.dir.join(&file)));
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
            return;
        }
        {
            let mut st = self.state.lock().unwrap();
            let now = tick(&mut st);
            let bytes = thumb.bytes.len() as u64;
            if let Some(old) = st.items.insert(hash, Item { file: file.clone(), bytes, last_used: now, touched: now }) {
                st.total -= old.bytes;
                if old.file != file {
                    let _ = fs::remove_file(self.dir.join(&old.file));
                }
            }
            st.total += bytes;
        }
        self.evict();
    }

    /// Once over budget, trim to 90% of it, so a full cache sorts its index
    /// once per many new thumbnails rather than on every one.
    fn evict(&self) {
        let mut st = self.state.lock().unwrap();
        if st.total <= self.max_bytes {
            return;
        }
        let target = self.max_bytes / 10 * 9;
        let mut order: Vec<(u64, String)> = st.items.iter().map(|(h, i)| (i.last_used, h.clone())).collect();
        order.sort_unstable();
        for (_, hash) in order {
            if st.total <= target {
                break;
            }
            if let Some(item) = st.items.remove(&hash) {
                st.total -= item.bytes;
                let _ = fs::remove_file(self.dir.join(&item.file));
            }
        }
    }
}

fn tick(st: &mut State) -> u64 {
    st.clock = millis(SystemTime::now()).max(st.clock + 1);
    st.clock
}

fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_millis() as u64
}

/// 128-bit FNV-1a (two independent 64-bit lanes). Stable across Rust
/// versions and runs, unlike `DefaultHasher`, which matters for a disk cache.
fn hash_key(key: &str) -> String {
    let lane = |basis: u64| {
        key.bytes().fold(basis, |h, b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
    };
    format!("{:016x}{:016x}", lane(0xcbf2_9ce4_8422_2325), lane(0x6c62_272e_07bb_0142))
}

/// `<hash>-<w>x<h>.<ext>` → (hash, (w, h, mime)).
fn parse_name(file: &str) -> Option<(&str, (u32, u32, &'static str))> {
    let (stem, ext) = file.rsplit_once('.')?;
    let mime = match ext {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        _ => return None,
    };
    let (hash, dims) = stem.split_once('-')?;
    if hash.len() != 32 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let (w, h) = dims.split_once('x')?;
    Some((hash, (w.parse().ok()?, h.parse().ok()?, mime)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb(n: usize) -> Cached {
        Cached { bytes: vec![7; n], mime: "image/png", width: 4, height: 2 }
    }

    #[test]
    fn stores_reloads_and_evicts_least_recently_used() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = DiskCache::open(tmp.path(), 250).unwrap();
        cache.put("a", &thumb(100));
        cache.put("b", &thumb(100));
        assert_eq!(cache.get("a").unwrap(), thumb(100)); // a is now newer than b
        cache.put("c", &thumb(100)); // over budget: b goes
        assert!(cache.get("b").is_none());
        assert!(cache.get("a").is_some());
        assert!(cache.get("c").is_some());
        let st = cache.stats();
        assert_eq!((st.entries, st.bytes), (2, 200));

        // Survives a restart with the same contents.
        let again = DiskCache::open(tmp.path(), 250).unwrap();
        assert_eq!(again.stats().entries, 2);
        let got = again.get("c").unwrap();
        assert_eq!((got.width, got.height, got.mime), (4, 2, "image/png"));
    }

    #[test]
    fn keys_hash_stably() {
        assert_eq!(hash_key("file:///x.png\n256"), hash_key("file:///x.png\n256"));
        assert_ne!(hash_key("a"), hash_key("b"));
        assert_eq!(hash_key("").len(), 32);
    }
}
