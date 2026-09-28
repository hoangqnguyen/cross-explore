//! `archive://` locations as a [`Provider`].
//!
//! The container can live anywhere the [`Vfs`] reaches (even inside another
//! archive), so the provider reaches it back through the Vfs. It holds only
//! a `Weak` reference: the Vfs owns this provider (`set_archive_provider`),
//! and a strong reference back would keep both alive forever.

use crate::cache::materialize;
use crate::format::ArchiveFormat;
use crate::index::{key_of, split_parent, ArchiveIndex};
use crate::stream::{stream_blocking, CommitWriter};
use crate::time::now_ms;
use crate::walk::{build_index, read_member};
use crate::zipedit::{is_under, rewrite, Edit};
use async_trait::async_trait;
use cx_core::{validate_name, Capabilities, CxError, Entry, EntryKind, Location, Provider, ReadStream, Result, Vfs, WriteMode, WriteStream};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::mpsc;

/// How many archive indexes stay in memory. Browsing usually goes back and
/// forth between a couple of archives; each index is a few hundred bytes
/// per member.
const OPEN_ARCHIVES: usize = 8;
const FIRST_BATCH: usize = 128;
const NEXT_BATCH: usize = 2048;

pub struct ArchiveProvider {
    inner: Arc<Inner>,
}

struct Inner {
    vfs: Weak<Vfs>,
    cache_dir: PathBuf,
    /// Least recently used first.
    open: Mutex<Vec<Arc<Opened>>>,
    /// One async lock per container URI: opening and editing the same
    /// archive are serialised, different archives proceed in parallel.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// An indexed archive, valid while the container's size and mtime match.
struct Opened {
    uri: String,
    stamp: (u64, Option<i64>),
    /// The local file with the archive's bytes.
    file: PathBuf,
    /// `file` is the container itself (edits replace it directly).
    local: bool,
    format: ArchiveFormat,
    index: ArchiveIndex,
}

fn split(loc: &Location) -> Result<(&Location, String)> {
    match loc {
        Location::Archive { container, inner } => Ok((container, key_of(inner))),
        _ => Err(CxError::InvalidLocation(format!("{loc} is not inside an archive"))),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| CxError::Io(format!("worker failed: {e}")))?
}

fn join_key(parent: &str, name: &str) -> String {
    if parent.is_empty() { name.to_string() } else { format!("{parent}/{name}") }
}

fn require_dir(index: &ArchiveIndex, key: &str, loc: &dyn std::fmt::Display) -> Result<()> {
    match index.get(key) {
        Some(n) if n.entry.is_dir => Ok(()),
        Some(_) => Err(CxError::InvalidLocation(format!("{loc} is not a folder"))),
        None => Err(CxError::NotFound(loc.to_string())),
    }
}

impl ArchiveProvider {
    /// `cache_dir` holds downloaded copies of remote archives and temporary
    /// files for zip edits.
    pub fn new(vfs: &Arc<Vfs>, cache_dir: impl Into<PathBuf>) -> Arc<ArchiveProvider> {
        Arc::new(ArchiveProvider {
            inner: Arc::new(Inner {
                vfs: Arc::downgrade(vfs),
                cache_dir: cache_dir.into(),
                open: Mutex::new(Vec::new()),
                locks: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Create the provider and register it with `vfs`.
    pub fn install(vfs: &Arc<Vfs>, cache_dir: impl Into<PathBuf>) -> Arc<ArchiveProvider> {
        let p = Self::new(vfs, cache_dir);
        vfs.set_archive_provider(p.clone());
        p
    }

    /// Whether the archive holding `loc` can be edited. `capabilities()` is
    /// per provider and says `writable` (zip is); the UI asks this to grey
    /// out editing commands in read-only formats.
    pub fn is_writable(loc: &Location) -> bool {
        match loc {
            Location::Archive { container, .. } => ArchiveFormat::from_name(&container.name()).is_some_and(ArchiveFormat::writable),
            _ => false,
        }
    }

    /// Drop the cached index of `container` (e.g. after it was replaced).
    pub fn invalidate(&self, container: &Location) {
        self.inner.forget(&container.uri());
    }
}

impl Inner {
    fn vfs(&self) -> Result<Arc<Vfs>> {
        self.vfs.upgrade().ok_or_else(|| CxError::Io("file system is shutting down".into()))
    }

    fn lock(&self, uri: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.lock().unwrap().entry(uri.to_string()).or_default().clone()
    }

    fn forget(&self, uri: &str) {
        self.open.lock().unwrap().retain(|o| o.uri != uri);
    }

    async fn load(&self, container: &Location) -> Result<Arc<Opened>> {
        let lock = self.lock(&container.uri());
        let _guard = lock.lock().await;
        self.load_locked(container).await
    }

    /// Index `container`, or reuse the index while the container is
    /// unchanged. The caller holds the container's lock.
    async fn load_locked(&self, container: &Location) -> Result<Arc<Opened>> {
        let uri = container.uri();
        let vfs = self.vfs()?;
        let stat = vfs.provider(container).await?.stat(container).await?;
        if stat.is_dir {
            return Err(CxError::InvalidLocation(format!("{uri} is a folder, not an archive")));
        }
        let stamp = (stat.size, stat.modified);
        {
            let mut open = self.open.lock().unwrap();
            if let Some(i) = open.iter().position(|o| o.uri == uri) {
                if open[i].stamp == stamp {
                    let o = open.remove(i);
                    open.push(o.clone());
                    return Ok(o);
                }
                open.remove(i);
            }
        }
        let (file, local) = materialize(&vfs, container, &stat, &self.cache_dir).await?;
        let name = container.name();
        let path = file.clone();
        let (format, index) = blocking(move || {
            let format = ArchiveFormat::detect_file(&name, &path)?;
            let root = Entry {
                name,
                kind: EntryKind::Dir,
                is_dir: true,
                size: 0,
                modified: stat.modified,
                created: stat.created,
                hidden: stat.hidden,
                readonly: !format.writable(),
                executable: false,
            };
            Ok((format, build_index(&path, format, root)?))
        })
        .await?;
        let opened = Arc::new(Opened { uri, stamp, file, local, format, index });
        let mut open = self.open.lock().unwrap();
        open.push(opened.clone());
        if open.len() > OPEN_ARCHIVES {
            open.remove(0);
        }
        Ok(opened)
    }

    /// Apply `edit` to a zip container: check it against the current index,
    /// rewrite the archive to a temp file, then replace the container
    /// (in place when local, by upload otherwise).
    async fn edit(&self, container: &Location, check: impl FnOnce(&ArchiveIndex) -> Result<()>, edit: Edit) -> Result<()> {
        let uri = container.uri();
        let lock = self.lock(&uri);
        let _guard = lock.lock().await;
        let opened = self.load_locked(container).await?;
        if !opened.format.writable() {
            return Err(CxError::Unsupported(format!("{} archives are read-only", opened.format.extension())));
        }
        check(&opened.index)?;
        let src = opened.file.clone();
        // Next to a local container so the final rename stays on one volume.
        let tmp_dir = if opened.local { src.parent().map(Path::to_path_buf).unwrap_or_default() } else { self.cache_dir.clone() };
        let tmp = blocking(move || {
            std::fs::create_dir_all(&tmp_dir).map_err(|e| CxError::from_io(e, tmp_dir.display()))?;
            let tmp = tempfile::Builder::new().prefix(".cx-zip-").tempfile_in(&tmp_dir).map_err(|e| CxError::from_io(e, tmp_dir.display()))?;
            let out = tmp.as_file().try_clone().map_err(|e| CxError::from_io(e, tmp.path().display()))?;
            rewrite(&src, out, &edit)?;
            Ok(tmp)
        })
        .await?;
        let result = if opened.local {
            let dst = opened.file.clone();
            blocking(move || {
                if let Ok(meta) = std::fs::metadata(&dst) {
                    let _ = std::fs::set_permissions(tmp.path(), meta.permissions());
                }
                tmp.persist(&dst).map(|_| ()).map_err(|e| CxError::from_io(e.error, dst.display()))
            })
            .await
        } else {
            upload(&*self.vfs()?, tmp.path(), container).await
        };
        self.forget(&uri);
        result
    }
}

/// Replace `dst` with the local file `src` through its provider.
async fn upload(vfs: &Vfs, src: &Path, dst: &Location) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let provider = vfs.provider(dst).await?;
    let mut input = tokio::fs::File::open(src).await.map_err(|e| CxError::from_io(e, src.display()))?;
    let mut out = provider.open_write(dst, WriteMode::Truncate).await?;
    tokio::io::copy(&mut input, &mut out).await.map_err(|e| CxError::io(format!("uploading {dst}"), e))?;
    out.shutdown().await.map_err(|e| CxError::io(format!("uploading {dst}"), e))?;
    Ok(())
}

#[async_trait]
impl Provider for ArchiveProvider {
    fn scheme(&self) -> &'static str {
        "archive"
    }

    fn capabilities(&self) -> Capabilities {
        // Archives don't change behind our back often enough to watch; the
        // index is revalidated against the container on every call instead.
        // `writable` holds for zip only, see `ArchiveProvider::is_writable`.
        Capabilities { live_watch: false, polling: false, server_copy: false, trash: false, posix: false, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let (container, key) = split(dir)?;
        let opened = self.inner.load(container).await?;
        require_dir(&opened.index, &key, dir)?;
        let entries: Vec<Entry> = opened.index.children(&key).map(|it| it.cloned().collect()).unwrap_or_default();
        let total = entries.len();
        let mut rest = entries.as_slice();
        let mut size = FIRST_BATCH;
        while !rest.is_empty() {
            let n = size.min(rest.len());
            if sink.send(rest[..n].to_vec()).await.is_err() {
                break; // listing cancelled
            }
            rest = &rest[n..];
            size = NEXT_BATCH;
        }
        Ok(total)
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        let (container, key) = split(loc)?;
        let opened = self.inner.load(container).await?;
        opened.index.get(&key).map(|n| n.entry.clone()).ok_or_else(|| CxError::NotFound(loc.uri()))
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let (container, parent) = split(dir)?;
        if let Some(n) = name {
            validate_name(n)?;
        }
        let opened = self.inner.load(container).await?;
        require_dir(&opened.index, &parent, dir)?;
        let name = match name {
            Some(n) => n.to_string(),
            None => (1..10_000)
                .map(|n| if n == 1 { "New folder".to_string() } else { format!("New folder ({n})") })
                .find(|n| opened.index.get(&join_key(&parent, n)).is_none())
                .ok_or_else(|| CxError::AlreadyExists("New folder".into()))?,
        };
        let key = join_key(&parent, &name);
        let k = key.clone();
        let check = move |idx: &ArchiveIndex| match idx.get(&k) {
            Some(_) => Err(CxError::AlreadyExists(k.clone())),
            None => Ok(()),
        };
        self.inner.edit(container, check, Edit::AddDir { key, modified: now_ms() }).await?;
        self.stat(&dir.join(&name)).await
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        let (container, from) = split(src)?;
        let (dst_container, to) = split(dst)?;
        if container != dst_container {
            return Err(CxError::Unsupported("moving between archives".into()));
        }
        if from.is_empty() || to.is_empty() {
            return Err(CxError::Unsupported("moving an archive's root".into()));
        }
        for seg in to.split('/') {
            validate_name(seg)?;
        }
        let (src_uri, dst_uri) = (src.uri(), dst.uri());
        let (f, t) = (from.clone(), to.clone());
        let check = move |idx: &ArchiveIndex| {
            if idx.get(&f).is_none() {
                return Err(CxError::NotFound(src_uri));
            }
            if idx.get(&t).is_some() {
                return Err(CxError::AlreadyExists(dst_uri));
            }
            if is_under(&t, &f) {
                return Err(CxError::InvalidLocation(format!("{dst_uri} is inside {src_uri}")));
            }
            require_dir(idx, split_parent(&t).0, &dst_uri)
        };
        self.inner.edit(container, check, Edit::Rename { from, to }).await
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let (container, key) = split(loc)?;
        if key.is_empty() {
            return Err(CxError::Unsupported("removing an archive's root; delete the archive file instead".into()));
        }
        let (k, uri) = (key.clone(), loc.uri());
        let check = move |idx: &ArchiveIndex| idx.get(&k).map(|_| ()).ok_or(CxError::NotFound(uri));
        self.inner.edit(container, check, Edit::Remove { key }).await
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let (container, key) = split(loc)?;
        let opened = self.inner.load(container).await?;
        let node = opened.index.get(&key).ok_or_else(|| CxError::NotFound(loc.uri()))?;
        match node.entry.kind {
            EntryKind::File => {}
            EntryKind::Dir => return Err(CxError::InvalidLocation(format!("{loc} is a folder"))),
            _ => return Err(CxError::Unsupported(format!("reading links inside archives ({loc})"))),
        }
        let ordinal = node.member.ok_or_else(|| CxError::NotFound(loc.uri()))?;
        let (file, format) = (opened.file.clone(), opened.format);
        Ok(stream_blocking(offset, move |w| read_member(&file, format, ordinal, w)))
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let (container, key) = split(loc)?;
        if key.is_empty() {
            return Err(CxError::InvalidLocation(format!("{loc} is the archive's root")));
        }
        validate_name(split_parent(&key).1)?;
        if mode == WriteMode::Append {
            return Err(CxError::Unsupported("resuming a write inside an archive".into()));
        }
        let opened = self.inner.load(container).await?;
        if !opened.format.writable() {
            return Err(CxError::Unsupported(format!("{} archives are read-only", opened.format.extension())));
        }
        let check = {
            let (key, uri) = (key.clone(), loc.uri());
            move |idx: &ArchiveIndex| {
                require_dir(idx, split_parent(&key).0, &uri)?;
                match idx.get(&key) {
                    Some(n) if n.entry.is_dir || mode == WriteMode::CreateNew => Err(CxError::AlreadyExists(uri)),
                    _ => Ok(()),
                }
            }
        };
        // Fail fast; checked again when the archive is rewritten.
        check.clone()(&opened.index)?;
        let dir = self.inner.cache_dir.join("writes");
        std::fs::create_dir_all(&dir).map_err(|e| CxError::from_io(e, dir.display()))?;
        let (file, tmp) = tempfile::NamedTempFile::new_in(&dir).map_err(|e| CxError::from_io(e, dir.display()))?.into_parts();
        let inner = self.inner.clone();
        let container = (*container).clone();
        let src = tmp.to_path_buf();
        let make_commit = Box::new(move || -> crate::stream::Commit {
            Box::pin(async move { inner.edit(&container, check, Edit::AddFile { key, src, modified: now_ms() }).await })
        });
        Ok(Box::pin(CommitWriter::new(tokio::fs::File::from_std(file), tmp, make_commit)))
    }
}
