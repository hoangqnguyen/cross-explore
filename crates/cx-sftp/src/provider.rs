use crate::session::{self, ConnectParams, Session};
use async_trait::async_trait;
use cx_core::location::join_posix;
use cx_core::{validate_name, Capabilities, CxError, Entry, EntryKind, Location, Provider, ReadStream, Result, Space, WriteMode, WriteStream};
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::client::RawSftpSession;
use russh_sftp::protocol::{FileAttributes, FileType, OpenFlags, StatusCode};
use std::future::Future;
use std::io::SeekFrom;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::AsyncSeekExt;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

/// A small first batch paints the folder quickly; later batches are larger
/// to keep IPC overhead down.
const FIRST_BATCH: usize = 128;
const NEXT_BATCH: usize = 1024;
/// Parallel requests when resolving symlinks or deleting many files.
const PARALLEL: usize = 16;

/// A connected SFTP server. Cheap to share; operations run concurrently over
/// the SSH connection, which is re-opened transparently if it drops.
pub struct SftpProvider {
    params: ConnectParams,
    current: tokio::sync::Mutex<Option<Arc<Session>>>,
    server_copy: bool,
}

impl SftpProvider {
    pub(crate) async fn connect(params: ConnectParams) -> Result<SftpProvider> {
        let session = Arc::new(session::open(&params).await?);
        let server_copy = session.has_extension("copy-data");
        Ok(SftpProvider { params, current: tokio::sync::Mutex::new(Some(session)), server_copy })
    }

    /// The user's home folder on the server (where `ssh` would start), as a
    /// POSIX path. Handy as the initial folder after connecting.
    pub async fn home(&self) -> Result<String> {
        self.retry(|s| async move { s.sftp.canonicalize(".").await.map_err(|e| map_err(e, ".")) }).await
    }

    /// Create a symbolic link at `link` pointing to `target` (stored as
    /// given, so it may be relative to the link's folder).
    pub async fn symlink(&self, link: &Location, target: &str) -> Result<()> {
        let path = path_of(link)?;
        let p = path.as_str();
        self.retry(|s| async move {
            // OpenSSH implemented SSH_FXP_SYMLINK with the two paths swapped
            // long ago and keeps it for compatibility; every client
            // special-cases it the same way.
            let openssh = s.extensions.keys().any(|k| k.ends_with("@openssh.com"));
            let (a, b) = if openssh { (target, p) } else { (p, target) };
            s.raw.symlink(a, b).await.map(|_| ()).map_err(|e| map_err(e, p))
        })
        .await
    }

    /// Close the connection. The next operation reconnects.
    pub async fn disconnect(&self) {
        if let Some(s) = self.current.lock().await.take() {
            s.disconnect().await;
        }
    }

    async fn session(&self) -> Result<Arc<Session>> {
        let mut cur = self.current.lock().await;
        if let Some(s) = cur.as_ref().filter(|s| !s.is_closed()) {
            return Ok(s.clone());
        }
        let s = Arc::new(session::open(&self.params).await?);
        *cur = Some(s.clone());
        Ok(s)
    }

    async fn forget(&self, s: &Arc<Session>) {
        let mut cur = self.current.lock().await;
        if cur.as_ref().is_some_and(|c| Arc::ptr_eq(c, s)) {
            *cur = None;
        }
    }

    /// Run `op`; if the connection turns out to be dead, reconnect and run it
    /// once more. Servers drop idle sessions and laptops sleep, and the user
    /// should not have to press "retry" for that.
    async fn retry<T, F, Fut>(&self, op: F) -> Result<T>
    where
        F: Fn(Arc<Session>) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let s = self.session().await?;
        match op(s.clone()).await {
            Err(CxError::Connection(_)) => {
                self.forget(&s).await;
                op(self.session().await?).await
            }
            other => other,
        }
    }
}

fn path_of(loc: &Location) -> Result<String> {
    match loc {
        Location::Remote { path, .. } => Ok(path.clone()),
        _ => Err(CxError::InvalidLocation(loc.uri())),
    }
}

/// SFTP status codes → our errors. Transport failures become `Connection`,
/// which is what triggers a reconnect.
fn map_err(e: SftpError, path: &str) -> CxError {
    match e {
        SftpError::Status(s) => match s.status_code {
            StatusCode::NoSuchFile => CxError::NotFound(path.to_string()),
            StatusCode::PermissionDenied => CxError::PermissionDenied(path.to_string()),
            StatusCode::OpUnsupported => CxError::Unsupported(format!("{path}: {}", s.error_message)),
            StatusCode::NoConnection | StatusCode::ConnectionLost => CxError::Connection(s.error_message),
            _ => CxError::Io(format!("{path}: {}", if s.error_message.is_empty() { s.status_code.to_string() } else { s.error_message })),
        },
        SftpError::IO(m) | SftpError::UnexpectedBehavior(m) => CxError::Connection(m),
        SftpError::Timeout => CxError::Connection(format!("{path}: server did not answer")),
        other => CxError::Io(format!("{path}: {other}")),
    }
}

fn is_eof(e: &SftpError) -> bool {
    matches!(e, SftpError::Status(s) if s.status_code == StatusCode::Eof)
}

fn to_ms(secs: Option<u32>) -> Option<i64> {
    secs.map(|s| s as i64 * 1000)
}

/// Build an entry from `lstat`-style attributes; `target` are the followed
/// attributes when the entry is a symlink (`None` if the link is broken).
fn make_entry(name: String, attrs: &FileAttributes, target: Option<&FileAttributes>) -> Entry {
    let kind = match attrs.permissions.map(|_| attrs.file_type()) {
        Some(FileType::Dir) => EntryKind::Dir,
        Some(FileType::Symlink) => EntryKind::Symlink,
        Some(FileType::Other) => EntryKind::Other,
        // Servers that omit permissions still send sizes: call it a file.
        Some(FileType::File) | None => EntryKind::File,
    };
    let (is_dir, size) = match kind {
        EntryKind::Dir => (true, 0),
        EntryKind::Symlink => match target {
            Some(t) if t.is_dir() => (true, 0),
            Some(t) => (false, t.size.unwrap_or(0)),
            None => (false, 0),
        },
        EntryKind::File => (false, attrs.size.unwrap_or(0)),
        EntryKind::Other => (false, 0),
    };
    Entry {
        hidden: name.starts_with('.'),
        // Like std's `readonly()`: nobody may write.
        readonly: attrs.permissions.is_some_and(|p| p & 0o222 == 0),
        modified: to_ms(target.and_then(|t| t.mtime).or(attrs.mtime)),
        created: None,
        name,
        kind,
        is_dir,
        size,
    }
}

fn is_symlink(attrs: &FileAttributes) -> bool {
    attrs.permissions.is_some() && attrs.file_type() == FileType::Symlink
}

/// Turn one `READDIR` reply into entries, following symlinks in parallel.
async fn to_entries(raw: &Arc<RawSftpSession>, dir: &str, files: Vec<russh_sftp::protocol::File>) -> Vec<Entry> {
    let mut links = JoinSet::new();
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        if f.filename == "." || f.filename == ".." {
            continue;
        }
        if is_symlink(&f.attrs) {
            let raw = raw.clone();
            let path = join_posix(dir, &f.filename);
            if links.len() >= PARALLEL {
                if let Some(Ok(e)) = links.join_next().await {
                    out.push(e);
                }
            }
            links.spawn(async move {
                let target = raw.stat(path).await.ok().map(|a| a.attrs);
                make_entry(f.filename, &f.attrs, target.as_ref())
            });
        } else {
            out.push(make_entry(f.filename, &f.attrs, None));
        }
    }
    while let Some(r) = links.join_next().await {
        if let Ok(e) = r {
            out.push(e);
        }
    }
    out
}

/// Every child of `dir` with its `lstat` attributes.
async fn read_dir_all(raw: &RawSftpSession, dir: &str) -> Result<Vec<(String, FileAttributes)>> {
    let handle = raw.opendir(dir).await.map_err(|e| map_err(e, dir))?.handle;
    let mut out = Vec::new();
    let res = loop {
        match raw.readdir(handle.as_str()).await {
            Ok(name) => out.extend(name.files.into_iter().filter(|f| f.filename != "." && f.filename != "..").map(|f| (f.filename, f.attrs))),
            Err(e) if is_eof(&e) => break Ok(out),
            Err(e) => break Err(map_err(e, dir)),
        }
    };
    let _ = raw.close(handle).await;
    res
}

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Depth-first delete. Files in a folder are removed in parallel (each is a
/// round trip); subfolders are handled one after another to bound the
/// number of open directory handles.
fn remove_tree(raw: Arc<RawSftpSession>, path: String, attrs: FileAttributes) -> BoxFut<'static, Result<()>> {
    Box::pin(async move {
        if attrs.permissions.is_none() || attrs.file_type() != FileType::Dir {
            return raw.remove(path.as_str()).await.map(|_| ()).map_err(|e| map_err(e, &path));
        }
        let children = read_dir_all(&raw, &path).await?;
        let mut files = JoinSet::new();
        for (name, a) in children {
            let child = join_posix(&path, &name);
            if a.permissions.is_some() && a.file_type() == FileType::Dir {
                remove_tree(raw.clone(), child, a).await?;
                continue;
            }
            if files.len() >= PARALLEL {
                if let Some(r) = files.join_next().await {
                    r.map_err(|e| CxError::Io(e.to_string()))??;
                }
            }
            let raw = raw.clone();
            files.spawn(async move { raw.remove(child.as_str()).await.map(|_| ()).map_err(|e| map_err(e, &child)) });
        }
        while let Some(r) = files.join_next().await {
            r.map_err(|e| CxError::Io(e.to_string()))??;
        }
        raw.rmdir(path.as_str()).await.map(|_| ()).map_err(|e| map_err(e, &path))
    })
}

/// Encode an SSH `string` (u32 length + bytes).
fn put_string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(&(s.len() as u32).to_be_bytes());
    buf.extend_from_slice(s.as_bytes());
}

impl SftpProvider {
    async fn lstat(&self, path: &str) -> Result<FileAttributes> {
        self.retry(|s| async move { s.raw.lstat(path).await.map(|a| a.attrs).map_err(|e| map_err(e, path)) }).await
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        match self.lstat(path).await {
            Ok(_) => Ok(true),
            Err(CxError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn stat_path(&self, path: &str, name: String) -> Result<Entry> {
        let attrs = self.lstat(path).await?;
        let target = if is_symlink(&attrs) {
            self.retry(|s| async move { Ok(s.raw.stat(path).await.ok().map(|a| a.attrs)) }).await?
        } else {
            None
        };
        Ok(make_entry(name, &attrs, target.as_ref()))
    }

    async fn mkdir(&self, path: &str) -> Result<()> {
        let res = self.retry(|s| async move { s.raw.mkdir(path, FileAttributes::empty()).await.map(|_| ()).map_err(|e| map_err(e, path)) }).await;
        match res {
            // SFTPv3 has no "exists" status: OpenSSH answers a generic failure.
            Err(CxError::Io(_)) if self.exists(path).await.unwrap_or(false) => Err(CxError::AlreadyExists(path.to_string())),
            other => other,
        }
    }

    async fn open_file(&self, path: &str, flags: OpenFlags) -> Result<russh_sftp::client::fs::File> {
        self.retry(|s| async move { s.sftp.open_with_flags(path, flags).await.map_err(|e| map_err(e, path)) }).await
    }
}

#[async_trait]
impl Provider for SftpProvider {
    fn scheme(&self) -> &'static str {
        "sftp"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: false, polling: true, server_copy: self.server_copy, trash: false, posix: true, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let path = path_of(dir)?;
        let p = path.as_str();
        let (s, handle) = self.retry(|s| async move { s.raw.opendir(p).await.map(|h| (s.clone(), h.handle)).map_err(|e| map_err(e, p)) }).await?;
        let raw = s.raw.clone();
        let mut batch = Vec::new();
        let mut limit = FIRST_BATCH;
        let mut total = 0;
        let res = loop {
            match raw.readdir(handle.as_str()).await {
                Ok(name) => {
                    batch.extend(to_entries(&raw, &path, name.files).await);
                    // The first reply goes out as soon as it arrives (OpenSSH
                    // sends ~100 names per reply), later ones are pooled.
                    if batch.len() >= limit || (total == 0 && !batch.is_empty()) {
                        total += batch.len();
                        if sink.send(std::mem::take(&mut batch)).await.is_err() {
                            break Ok(total); // receiver gone: listing cancelled
                        }
                        limit = NEXT_BATCH;
                    }
                }
                Err(e) if is_eof(&e) => {
                    total += batch.len();
                    if !batch.is_empty() {
                        let _ = sink.send(batch).await;
                    }
                    break Ok(total);
                }
                Err(e) => break Err(map_err(e, &path)),
            }
        };
        let _ = raw.close(handle).await;
        res
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        let path = path_of(loc)?;
        self.stat_path(&path, loc.name()).await
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let base = path_of(dir)?;
        if let Some(name) = name {
            validate_name(name)?;
            let path = join_posix(&base, name);
            self.mkdir(&path).await?;
            return self.stat_path(&path, name.to_string()).await;
        }
        for n in 1..10_000 {
            let name = if n == 1 { "New folder".to_string() } else { format!("New folder ({n})") };
            let path = join_posix(&base, &name);
            match self.mkdir(&path).await {
                Ok(()) => return self.stat_path(&path, name).await,
                Err(CxError::AlreadyExists(_)) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(CxError::AlreadyExists("New folder".into()))
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        let (from, to) = (path_of(src)?, path_of(dst)?);
        let case_only = src.parent() == dst.parent() && src.name().to_lowercase() == dst.name().to_lowercase() && from != to;
        // SFTPv3 RENAME semantics vary by server (OpenSSH refuses to replace
        // files but may replace empty folders), so check first.
        if !case_only && self.exists(&to).await? {
            return Err(CxError::AlreadyExists(to));
        }
        let (f, t) = (from.as_str(), to.as_str());
        self.retry(|s| async move { s.raw.rename(f, t).await.map(|_| ()).map_err(|e| map_err(e, f)) }).await
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let path = path_of(loc)?;
        let attrs = self.lstat(&path).await?;
        let p = &path;
        let a = &attrs;
        self.retry(|s| async move { remove_tree(s.raw.clone(), p.clone(), a.clone()).await }).await
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let path = path_of(loc)?;
        let mut f = self.open_file(&path, OpenFlags::READ).await?;
        if offset > 0 {
            f.seek(SeekFrom::Start(offset)).await.map_err(|e| CxError::from_io(e, &path))?;
        }
        Ok(Box::pin(f))
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let path = path_of(loc)?;
        let flags = match mode {
            WriteMode::CreateNew => OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
            WriteMode::Truncate => OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
            // Not OpenFlags::APPEND: the client sends explicit offsets and
            // servers differ in whether O_APPEND overrides them. Seeking to
            // the end is unambiguous.
            WriteMode::Append => OpenFlags::WRITE | OpenFlags::CREATE,
        };
        let mut f = match self.open_file(&path, flags).await {
            Err(CxError::Io(_)) if mode == WriteMode::CreateNew && self.exists(&path).await.unwrap_or(false) => {
                return Err(CxError::AlreadyExists(path));
            }
            other => other?,
        };
        if mode == WriteMode::Append {
            f.seek(SeekFrom::End(0)).await.map_err(|e| CxError::from_io(e, &path))?;
        }
        Ok(Box::pin(f))
    }

    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        let path = path_of(loc)?;
        let mtime = ms.div_euclid(1000).clamp(0, u32::MAX as i64) as u32;
        let current = self.lstat(&path).await?;
        // ACMODTIME sets both times; keep the access time as it was.
        let attrs = FileAttributes { atime: Some(current.atime.unwrap_or(mtime)), mtime: Some(mtime), ..FileAttributes::empty() };
        let p = path.as_str();
        let a = &attrs;
        self.retry(|s| async move { s.raw.setstat(p, a.clone()).await.map(|_| ()).map_err(|e| map_err(e, p)) }).await
    }

    /// Server-side copy through the `copy-data` extension (OpenSSH 9.0+).
    async fn copy_within(&self, src: &Location, dst: &Location) -> Result<bool> {
        if !self.server_copy {
            return Ok(false);
        }
        let (from, to) = (path_of(src)?, path_of(dst)?);
        let attrs = self.lstat(&from).await?;
        if attrs.permissions.is_none() || attrs.file_type() != FileType::File {
            return Ok(false);
        }
        let s = self.session().await?;
        let raw = &s.raw;
        let Ok(rh) = raw.open(from.as_str(), OpenFlags::READ, FileAttributes::empty()).await else { return Ok(false) };
        // Never overwrite: an existing target makes the caller stream instead
        // (and apply its own conflict handling), like a failed reflink locally.
        let wh = match raw.open(to.as_str(), OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE, FileAttributes::empty()).await {
            Ok(h) => h.handle,
            Err(_) => {
                let _ = raw.close(rh.handle).await;
                return Ok(false);
            }
        };
        let mut req = Vec::new();
        put_string(&mut req, &rh.handle);
        req.extend_from_slice(&0u64.to_be_bytes()); // read offset
        req.extend_from_slice(&0u64.to_be_bytes()); // length: 0 = to EOF
        put_string(&mut req, &wh);
        req.extend_from_slice(&0u64.to_be_bytes()); // write offset
        let res = raw.extended("copy-data", req).await;
        let _ = raw.close(rh.handle).await;
        let _ = raw.close(wh).await;
        let ok = matches!(res, Ok(russh_sftp::protocol::Packet::Status(ref st)) if st.status_code == StatusCode::Ok);
        if !ok {
            let _ = raw.remove(to.as_str()).await;
        }
        Ok(ok)
    }

    async fn free_space(&self, loc: &Location) -> Result<Option<Space>> {
        let path = path_of(loc)?;
        let p = path.as_str();
        self.retry(|s| async move {
            if !s.has_extension("statvfs@openssh.com") {
                return Ok(None);
            }
            let v = s.raw.statvfs(p).await.map_err(|e| map_err(e, p))?;
            Ok(Some(Space { free: v.blocks_avail * v.fragment_size, total: v.blocks * v.fragment_size }))
        })
        .await
    }
}
