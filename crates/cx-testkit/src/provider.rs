use crate::stream::{Fault, MemRead, MemWrite, Pace};
use async_trait::async_trait;
use cx_core::{validate_name, Capabilities, CxError, Entry, EntryKind, Location, Provider, ReadStream, Result, WriteMode, WriteStream};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Dir { modified: i64 },
    File { data: Vec<u8>, modified: i64 },
}

/// Paths are POSIX-style keys ("/" is the root, which always exists).
pub(crate) type Tree = BTreeMap<String, Node>;

#[derive(Debug, Clone)]
struct Knobs {
    pace: Pace,
    list_batch: usize,
    read_fault: Option<Fault>,
    write_fault: Option<Fault>,
}

/// An in-memory file system.
///
/// It serves `Remote` locations (by their POSIX path), `Archive` locations
/// (by their inner path) and `Local` locations (by their path with `/`
/// separators), so it can stand in for any provider in a [`cx_core::Vfs`].
pub struct MemProvider {
    scheme: &'static str,
    tree: Arc<Mutex<Tree>>,
    knobs: Mutex<Knobs>,
    reads_opened: AtomicUsize,
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn key(loc: &Location) -> String {
    let raw = match loc {
        Location::Remote { path, .. } => path.clone(),
        Location::Archive { inner, .. } => inner.clone(),
        Location::Local(p) => p.to_string_lossy().replace('\\', "/"),
    };
    normalize(&raw)
}

fn normalize(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    format!("/{}", parts.join("/"))
}

fn parent_key(k: &str) -> Option<String> {
    if k == "/" {
        return None;
    }
    let i = k.rfind('/')?;
    Some(if i == 0 { "/".into() } else { k[..i].to_string() })
}

fn child_key(dir: &str, name: &str) -> String {
    if dir == "/" { format!("/{name}") } else { format!("{dir}/{name}") }
}

fn is_under(k: &str, dir: &str) -> bool {
    k == dir || dir == "/" || (k.starts_with(dir) && k.as_bytes().get(dir.len()) == Some(&b'/'))
}

fn entry_for(name: &str, node: &Node) -> Entry {
    let (kind, size, modified) = match node {
        Node::Dir { modified } => (EntryKind::Dir, 0, *modified),
        Node::File { data, modified } => (EntryKind::File, data.len() as u64, *modified),
    };
    Entry {
        name: name.to_string(),
        kind,
        is_dir: kind == EntryKind::Dir,
        size,
        modified: Some(modified),
        created: None,
        hidden: name.starts_with('.'),
        readonly: false,
    }
}

fn require_dir(tree: &Tree, k: &str) -> Result<()> {
    match tree.get(k) {
        Some(Node::Dir { .. }) => Ok(()),
        Some(Node::File { .. }) => Err(CxError::InvalidLocation(format!("{k} is not a folder"))),
        None => Err(CxError::NotFound(k.to_string())),
    }
}

impl MemProvider {
    pub fn new() -> Arc<MemProvider> {
        Self::with_scheme("mem")
    }

    /// `scheme` is what [`Provider::scheme`] reports (e.g. "sftp").
    pub fn with_scheme(scheme: &'static str) -> Arc<MemProvider> {
        let mut tree = Tree::new();
        tree.insert("/".into(), Node::Dir { modified: now_ms() });
        Arc::new(MemProvider {
            scheme,
            tree: Arc::new(Mutex::new(tree)),
            knobs: Mutex::new(Knobs { pace: Pace::default(), list_batch: 64, read_fault: None, write_fault: None }),
            reads_opened: AtomicUsize::new(0),
        })
    }

    /// Sleep this long before every chunk read or written.
    pub fn set_chunk_delay(&self, delay: Duration) {
        self.knobs.lock().unwrap().pace.delay = delay;
    }

    /// Cap each stream at roughly this many bytes per second (`None` = unlimited).
    pub fn set_throughput(&self, bytes_per_sec: Option<u64>) {
        self.knobs.lock().unwrap().pace.bytes_per_sec = bytes_per_sec;
    }

    /// Largest chunk a single read or write moves (default 64 KiB).
    pub fn set_chunk_size(&self, bytes: usize) {
        self.knobs.lock().unwrap().pace.chunk = bytes.max(1);
    }

    /// Number of entries per listing batch (default 64).
    pub fn set_list_batch(&self, n: usize) {
        self.knobs.lock().unwrap().list_batch = n.max(1);
    }

    /// The next `times` read streams fail with a connection reset once they
    /// have delivered `after` bytes, like a dropped link mid-download.
    pub fn fail_reads_after(&self, after: u64, times: u32) {
        self.knobs.lock().unwrap().read_fault = Some(Fault { after, times });
    }

    /// The next `times` write streams fail after accepting `after` bytes.
    pub fn fail_writes_after(&self, after: u64, times: u32) {
        self.knobs.lock().unwrap().write_fault = Some(Fault { after, times });
    }

    /// How many read streams have been opened so far.
    pub fn reads_opened(&self) -> usize {
        self.reads_opened.load(Ordering::Relaxed)
    }

    /// Create `path` and any missing parents.
    pub fn mkdir_all(&self, path: &str) {
        let mut tree = self.tree.lock().unwrap();
        let mut acc = String::from("/");
        for seg in normalize(path).split('/').filter(|s| !s.is_empty()) {
            acc = child_key(&acc, seg);
            tree.entry(acc.clone()).or_insert(Node::Dir { modified: now_ms() });
        }
    }

    /// Write a file (creating parent folders), with the current time as mtime.
    pub fn put(&self, path: &str, data: impl Into<Vec<u8>>) {
        self.put_with_mtime(path, data, now_ms());
    }

    pub fn put_with_mtime(&self, path: &str, data: impl Into<Vec<u8>>, modified: i64) {
        let k = normalize(path);
        if let Some(p) = parent_key(&k) {
            self.mkdir_all(&p);
        }
        self.tree.lock().unwrap().insert(k, Node::File { data: data.into(), modified });
    }

    /// Contents of a file, `None` if missing or a folder.
    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        match self.tree.lock().unwrap().get(&normalize(path)) {
            Some(Node::File { data, .. }) => Some(data.clone()),
            _ => None,
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        self.tree.lock().unwrap().contains_key(&normalize(path))
    }

    pub fn modified(&self, path: &str) -> Option<i64> {
        match self.tree.lock().unwrap().get(&normalize(path))? {
            Node::Dir { modified } | Node::File { modified, .. } => Some(*modified),
        }
    }

    /// Every path in the tree except the root, sorted.
    pub fn paths(&self) -> Vec<String> {
        self.tree.lock().unwrap().keys().filter(|k| *k != "/").cloned().collect()
    }

    fn take_fault(slot: &mut Option<Fault>) -> Option<u64> {
        let f = slot.as_mut()?;
        let after = f.after;
        f.times = f.times.saturating_sub(1);
        if f.times == 0 {
            *slot = None;
        }
        Some(after)
    }
}

#[async_trait]
impl Provider for MemProvider {
    fn scheme(&self) -> &'static str {
        self.scheme
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: false, polling: true, server_copy: false, trash: false, posix: true, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let k = key(dir);
        let batch = self.knobs.lock().unwrap().list_batch;
        let entries: Vec<Entry> = {
            let tree = self.tree.lock().unwrap();
            require_dir(&tree, &k)?;
            tree.iter()
                .filter(|(p, _)| p.as_str() != "/" && parent_key(p).as_deref() == Some(k.as_str()))
                .map(|(p, n)| entry_for(p.rsplit('/').next().unwrap_or(""), n))
                .collect()
        };
        let total = entries.len();
        for chunk in entries.chunks(batch) {
            if sink.send(chunk.to_vec()).await.is_err() {
                break;
            }
        }
        Ok(total)
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        let k = key(loc);
        let tree = self.tree.lock().unwrap();
        let node = tree.get(&k).ok_or_else(|| CxError::NotFound(k.clone()))?;
        Ok(entry_for(k.rsplit('/').next().unwrap_or(""), node))
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let k = key(dir);
        let mut tree = self.tree.lock().unwrap();
        require_dir(&tree, &k)?;
        let name = match name {
            Some(n) => {
                validate_name(n)?;
                if tree.contains_key(&child_key(&k, n)) {
                    return Err(CxError::AlreadyExists(child_key(&k, n)));
                }
                n.to_string()
            }
            None => (1..)
                .map(|i| if i == 1 { "New folder".to_string() } else { format!("New folder ({i})") })
                .find(|n| !tree.contains_key(&child_key(&k, n)))
                .unwrap(),
        };
        let node = Node::Dir { modified: now_ms() };
        let entry = entry_for(&name, &node);
        tree.insert(child_key(&k, &name), node);
        Ok(entry)
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        let (s, d) = (key(src), key(dst));
        let mut tree = self.tree.lock().unwrap();
        if !tree.contains_key(&s) {
            return Err(CxError::NotFound(s));
        }
        if s == d {
            return Ok(());
        }
        if tree.contains_key(&d) {
            return Err(CxError::AlreadyExists(d));
        }
        if is_under(&d, &s) {
            return Err(CxError::InvalidLocation(format!("cannot move {s} into itself")));
        }
        require_dir(&tree, &parent_key(&d).unwrap_or_else(|| "/".into()))?;
        let moving: Vec<String> = tree.keys().filter(|k| is_under(k, &s)).cloned().collect();
        for old in moving {
            let node = tree.remove(&old).unwrap();
            tree.insert(format!("{d}{}", &old[s.len()..]), node);
        }
        Ok(())
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let k = key(loc);
        if k == "/" {
            return Err(CxError::PermissionDenied("cannot remove the root".into()));
        }
        let mut tree = self.tree.lock().unwrap();
        if !tree.contains_key(&k) {
            return Err(CxError::NotFound(k));
        }
        tree.retain(|p, _| !is_under(p, &k));
        Ok(())
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let k = key(loc);
        let data = match self.tree.lock().unwrap().get(&k) {
            Some(Node::File { data, .. }) => data.clone(),
            Some(Node::Dir { .. }) => return Err(CxError::InvalidLocation(format!("{k} is a folder"))),
            None => return Err(CxError::NotFound(k)),
        };
        self.reads_opened.fetch_add(1, Ordering::Relaxed);
        let mut knobs = self.knobs.lock().unwrap();
        let fail_after = Self::take_fault(&mut knobs.read_fault);
        Ok(Box::pin(MemRead::new(data, offset as usize, knobs.pace.clone(), fail_after)))
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let k = key(loc);
        {
            let mut tree = self.tree.lock().unwrap();
            require_dir(&tree, &parent_key(&k).ok_or_else(|| CxError::InvalidLocation(k.clone()))?)?;
            match (tree.get_mut(&k), mode) {
                (Some(Node::Dir { .. }), _) => return Err(CxError::InvalidLocation(format!("{k} is a folder"))),
                (Some(_), WriteMode::CreateNew) => return Err(CxError::AlreadyExists(k)),
                (Some(Node::File { data, modified }), WriteMode::Truncate) => {
                    data.clear();
                    *modified = now_ms();
                }
                (Some(_), WriteMode::Append) => {}
                (None, _) => {
                    tree.insert(k.clone(), Node::File { data: Vec::new(), modified: now_ms() });
                }
            }
        }
        let mut knobs = self.knobs.lock().unwrap();
        let fail_after = Self::take_fault(&mut knobs.write_fault);
        Ok(Box::pin(MemWrite::new(self.tree.clone(), k, knobs.pace.clone(), fail_after)))
    }

    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        let k = key(loc);
        match self.tree.lock().unwrap().get_mut(&k) {
            Some(Node::Dir { modified } | Node::File { modified, .. }) => {
                *modified = ms;
                Ok(())
            }
            None => Err(CxError::NotFound(k)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cx_core::provider::list_all;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn loc(p: &str) -> Location {
        Location::remote(crate::mem_endpoint("t"), p)
    }

    #[tokio::test]
    async fn list_stat_and_batches() {
        let m = MemProvider::new();
        m.set_list_batch(2);
        m.put("/a/x.txt", b"hello".to_vec());
        m.put("/a/y.txt", b"".to_vec());
        m.mkdir_all("/a/sub/deeper");
        let (tx, mut rx) = mpsc::channel(8);
        let n = m.list(&loc("/a"), tx).await.unwrap();
        assert_eq!(n, 3);
        assert_eq!(rx.recv().await.unwrap().len(), 2);
        let all = list_all(m.as_ref(), &loc("/a")).await.unwrap();
        let names: Vec<_> = all.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["sub", "x.txt", "y.txt"]);
        assert_eq!(m.stat(&loc("/a/x.txt")).await.unwrap().size, 5);
        assert!(m.stat(&loc("/a/sub")).await.unwrap().is_dir);
        assert!(matches!(m.stat(&loc("/nope")).await, Err(CxError::NotFound(_))));
    }

    #[tokio::test]
    async fn write_modes_and_offsets() {
        let m = MemProvider::new();
        let f = loc("/f.bin");
        let mut w = m.open_write(&f, WriteMode::CreateNew).await.unwrap();
        w.write_all(b"hello ").await.unwrap();
        w.shutdown().await.unwrap();
        assert!(matches!(m.open_write(&f, WriteMode::CreateNew).await, Err(CxError::AlreadyExists(_))));
        let mut w = m.open_write(&f, WriteMode::Append).await.unwrap();
        w.write_all(b"world").await.unwrap();
        w.shutdown().await.unwrap();
        let mut s = String::new();
        m.open_read(&f, 6).await.unwrap().read_to_string(&mut s).await.unwrap();
        assert_eq!(s, "world");
        let mut w = m.open_write(&f, WriteMode::Truncate).await.unwrap();
        w.write_all(b"x").await.unwrap();
        w.shutdown().await.unwrap();
        assert_eq!(m.read("/f.bin").unwrap(), b"x");
        assert!(matches!(m.open_write(&loc("/missing/f"), WriteMode::Truncate).await, Err(CxError::NotFound(_))));
    }

    #[tokio::test]
    async fn move_remove_create() {
        let m = MemProvider::new();
        m.put("/a/b/c.txt", b"c".to_vec());
        m.move_to(&loc("/a"), &loc("/z")).await.unwrap();
        assert_eq!(m.read("/z/b/c.txt").unwrap(), b"c");
        assert!(!m.exists("/a"));
        assert!(m.move_to(&loc("/z"), &loc("/z/b/q")).await.is_err());
        m.put("/y", b"y".to_vec());
        assert!(matches!(m.move_to(&loc("/y"), &loc("/z")).await, Err(CxError::AlreadyExists(_))));
        assert_eq!(m.create_dir(&loc("/"), None).await.unwrap().name, "New folder");
        assert_eq!(m.create_dir(&loc("/"), None).await.unwrap().name, "New folder (2)");
        m.remove(&loc("/z")).await.unwrap();
        assert_eq!(m.paths(), ["/New folder", "/New folder (2)", "/y"]);
        m.set_modified(&loc("/y"), 42).await.unwrap();
        assert_eq!(m.stat(&loc("/y")).await.unwrap().modified, Some(42));
        assert!(!m.copy_within(&loc("/y"), &loc("/y2")).await.unwrap());
        assert!(m.trash(&loc("/"), &["y".into()]).await.is_err());
    }

    #[tokio::test]
    async fn injected_read_failure_then_recovery() {
        let m = MemProvider::new();
        m.set_chunk_size(10);
        m.put("/f", vec![7u8; 100]);
        m.fail_reads_after(35, 1);
        let mut buf = Vec::new();
        let err = m.open_read(&loc("/f"), 0).await.unwrap().read_to_end(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::ConnectionReset);
        assert_eq!(buf.len(), 35);
        let mut rest = Vec::new();
        m.open_read(&loc("/f"), 35).await.unwrap().read_to_end(&mut rest).await.unwrap();
        assert_eq!(rest.len(), 65);
        assert_eq!(m.reads_opened(), 2);
    }

    #[tokio::test]
    async fn chunk_delay_slows_streams() {
        let m = MemProvider::new();
        m.set_chunk_size(10);
        m.set_chunk_delay(Duration::from_millis(5));
        m.put("/f", vec![1u8; 50]);
        let t = std::time::Instant::now();
        let mut buf = Vec::new();
        m.open_read(&loc("/f"), 0).await.unwrap().read_to_end(&mut buf).await.unwrap();
        assert!(t.elapsed() >= Duration::from_millis(25));
    }
}
