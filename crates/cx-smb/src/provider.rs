use crate::error::map;
use crate::session::Session;
use crate::wire::{self, RawEntry, ATTR_HIDDEN, ATTR_READONLY, ATTR_REPARSE_POINT};
use crate::{io, watch};
use async_trait::async_trait;
use cx_core::{
    validate_name, Capabilities, CxError, Endpoint, Entry, EntryKind, Location, Provider, ReadStream, Result, Space, WatchGuard, WatchSink,
    WriteMode, WriteStream,
};
use smb2::ErrorKind;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Batch sizes for streamed listings, as in the local provider: a small
/// first batch for the first paint, then larger ones.
const FIRST_BATCH: usize = 128;
const NEXT_BATCH: usize = 2048;

/// A connected SMB server. `/` lists its shares; `/Share/dir/file` is a path
/// inside a share.
pub struct SmbProvider {
    endpoint: Endpoint,
    session: Arc<Session>,
}

/// A location split into share and share-relative path ("" = share root).
struct Target {
    share: String,
    path: String,
}

pub(crate) fn to_entry(raw: RawEntry) -> Entry {
    let is_dir = raw.is_dir();
    let kind = if raw.attributes & ATTR_REPARSE_POINT != 0 {
        EntryKind::Symlink
    } else if is_dir {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    Entry {
        hidden: raw.attributes & ATTR_HIDDEN != 0 || raw.name.starts_with('.'),
        // On folders the read-only bit is a legacy "customized folder"
        // marker in Windows, not a permission.
        readonly: !is_dir && raw.attributes & ATTR_READONLY != 0,
        modified: wire::filetime_to_ms(raw.modified),
        created: wire::filetime_to_ms(raw.created),
        size: if is_dir { 0 } else { raw.size },
        name: raw.name,
        kind,
        is_dir,
    }
}

fn share_entry(name: String) -> Entry {
    Entry {
        // Admin/hidden shares (C$, IPC$, …) end in `$` by convention.
        hidden: name.ends_with('$'),
        name,
        kind: EntryKind::Dir,
        is_dir: true,
        size: 0,
        modified: None,
        created: None,
        readonly: false,
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

impl SmbProvider {
    pub(crate) fn new(endpoint: Endpoint, session: Session) -> SmbProvider {
        SmbProvider { endpoint, session: Arc::new(session) }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// `None` for the server root.
    fn target(&self, loc: &Location) -> Result<Option<Target>> {
        match loc {
            Location::Remote { endpoint, path } if endpoint.scheme == cx_core::Scheme::Smb => {
                let _ = endpoint;
                let trimmed = path.trim_matches('/');
                if trimmed.is_empty() {
                    return Ok(None);
                }
                let (share, rest) = trimmed.split_once('/').unwrap_or((trimmed, ""));
                Ok(Some(Target { share: share.to_string(), path: rest.to_string() }))
            }
            _ => Err(CxError::InvalidLocation(loc.uri())),
        }
    }

    fn inside_share(&self, loc: &Location, what: &str) -> Result<Target> {
        self.target(loc)?.ok_or_else(|| CxError::Unsupported(format!("{what} at the server root (it lists shares)")))
    }

    async fn stat_raw(&self, t: &Target) -> smb2::Result<RawEntry> {
        let path = t.path.clone();
        let mut raw = self
            .session
            .run(&t.share, true, |conn, tree| {
                let path = path.clone();
                async move { wire::stat(&conn, &tree, &path).await }
            })
            .await?;
        if t.path.is_empty() {
            raw.name = t.share.clone();
        }
        Ok(raw)
    }

    async fn list_share_root(&self, sink: &mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let shares = self.session.list_shares().await.map_err(|e| map(e, self.endpoint.uri()))?;
        let entries: Vec<Entry> = shares.into_iter().map(|s| share_entry(s.name)).collect();
        let n = entries.len();
        for chunk in entries.chunks(NEXT_BATCH) {
            if sink.send(chunk.to_vec()).await.is_err() {
                break;
            }
        }
        Ok(n)
    }

    async fn remove_tree(&self, share: &str, path: &str) -> smb2::Result<()> {
        // Collect children first so no directory handle stays open while
        // deleting (Samba refuses to delete a folder with an open handle).
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        {
            let p = path.to_string();
            let mut reader = self
                .session
                .run(share, true, |conn, tree| {
                    let p = p.clone();
                    async move { wire::DirReader::open(conn, tree, &p).await }
                })
                .await?;
            let res = async {
                while let Some(page) = reader.next_page().await? {
                    for e in page {
                        let child = join(path, &e.name);
                        // Reparse points (symlinks) are removed, not followed.
                        if e.is_dir() && e.attributes & ATTR_REPARSE_POINT == 0 {
                            dirs.push(child);
                        } else {
                            files.push(child);
                        }
                    }
                }
                Ok::<_, smb2::Error>(())
            }
            .await;
            reader.close().await;
            res?;
        }
        for d in dirs {
            Box::pin(self.remove_tree(share, &d)).await?;
        }
        if !files.is_empty() {
            let (mut conn, tree) = self.session.tree(share).await?;
            let refs: Vec<&str> = files.iter().map(String::as_str).collect();
            for r in tree.delete_files(&mut conn, &refs).await {
                match r {
                    Err(e) if e.kind() != ErrorKind::NotFound => return Err(e),
                    _ => {}
                }
            }
        }
        let p = path.to_string();
        self.session
            .run(share, false, |mut conn, tree| {
                let p = p.clone();
                async move { tree.delete_directory(&mut conn, &p).await }
            })
            .await
    }
}

#[async_trait]
impl Provider for SmbProvider {
    fn scheme(&self) -> &'static str {
        "smb"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: true, polling: false, server_copy: true, trash: false, posix: false, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let Some(t) = self.target(dir)? else {
            return self.list_share_root(&sink).await;
        };
        let path = t.path.clone();
        let mut reader = self
            .session
            .run(&t.share, true, |conn, tree| {
                let path = path.clone();
                async move { wire::DirReader::open(conn, tree, &path).await }
            })
            .await
            .map_err(|e| map(e, dir.uri()))?;
        let mut total = 0;
        let mut limit = FIRST_BATCH;
        let mut batch: Vec<Entry> = Vec::with_capacity(FIRST_BATCH);
        let res: Result<()> = async {
            while let Some(page) = reader.next_page().await.map_err(|e| map(e, dir.uri()))? {
                for raw in page {
                    batch.push(to_entry(raw));
                    if batch.len() >= limit {
                        total += batch.len();
                        if sink.send(std::mem::take(&mut batch)).await.is_err() {
                            return Ok(()); // cancelled
                        }
                        limit = NEXT_BATCH;
                    }
                }
            }
            total += batch.len();
            if !batch.is_empty() {
                let _ = sink.send(std::mem::take(&mut batch)).await;
            }
            Ok(())
        }
        .await;
        reader.close().await;
        res.map(|_| total)
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        match self.target(loc)? {
            None => Ok(share_entry(self.endpoint.host.clone())),
            Some(t) => self.stat_raw(&t).await.map(to_entry).map_err(|e| map(e, loc.uri())),
        }
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let t = &self.inside_share(dir, "creating folders")?;
        let create = |name: String| {
            let path = join(&t.path, &name);
            async move {
                let p = path.clone();
                self.session
                    .run(&t.share, false, |mut conn, tree| {
                        let p = p.clone();
                        async move { tree.create_directory(&mut conn, &p).await }
                    })
                    .await?;
                self.stat_raw(&Target { share: t.share.clone(), path }).await
            }
        };
        if let Some(name) = name {
            validate_name(name)?;
            return create(name.to_string()).await.map(to_entry).map_err(|e| map(e, dir.join(name).uri()));
        }
        for n in 1..10_000 {
            let name = if n == 1 { "New folder".to_string() } else { format!("New folder ({n})") };
            match create(name.clone()).await {
                Ok(raw) => return Ok(to_entry(raw)),
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(map(e, dir.join(&name).uri())),
            }
        }
        Err(CxError::AlreadyExists("New folder".into()))
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        let s = self.inside_share(src, "renaming shares")?;
        let d = self.inside_share(dst, "moving to the server root")?;
        if !s.share.eq_ignore_ascii_case(&d.share) {
            return Err(CxError::Unsupported("moving between shares".into()));
        }
        if s.path.is_empty() || d.path.is_empty() {
            return Err(CxError::Unsupported("renaming a share".into()));
        }
        // SMB rename never replaces (ReplaceIfExists = 0): an existing target
        // answers OBJECT_NAME_COLLISION, i.e. AlreadyExists. A case-only
        // rename targets the same file and is allowed by the server.
        self.session
            .run(&s.share, false, |mut conn, tree| {
                let (from, to) = (s.path.clone(), d.path.clone());
                async move { tree.rename(&mut conn, &from, &to).await }
            })
            .await
            .map_err(|e| match e.kind() {
                ErrorKind::AlreadyExists => CxError::AlreadyExists(dst.uri()),
                _ => map(e, src.uri()),
            })
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let t = self.inside_share(loc, "deleting")?;
        if t.path.is_empty() {
            return Err(CxError::Unsupported("deleting a share".into()));
        }
        let raw = self.stat_raw(&t).await.map_err(|e| map(e, loc.uri()))?;
        let res = if raw.is_dir() && raw.attributes & ATTR_REPARSE_POINT == 0 {
            self.remove_tree(&t.share, &t.path).await
        } else {
            let p = t.path.clone();
            self.session
                .run(&t.share, false, |mut conn, tree| {
                    let p = p.clone();
                    async move { tree.delete_file(&mut conn, &p).await }
                })
                .await
        };
        res.map_err(|e| map(e, loc.uri()))
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let t = self.inside_share(loc, "reading")?;
        let p = t.path.clone();
        let reader = self
            .session
            .run(&t.share, true, |conn, tree| {
                let p = p.clone();
                async move { tree.open_file_reader(conn, &p).await }
            })
            .await
            .map_err(|e| match e.kind() {
                // Opening a folder for reading.
                ErrorKind::IsADirectory => CxError::InvalidLocation(format!("{} is a folder", loc.uri())),
                _ => map(e, loc.uri()),
            })?;
        Ok(Box::pin(io::read_stream(reader, offset)))
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let t = self.inside_share(loc, "writing")?;
        let append_at = match mode {
            WriteMode::Append => match self.stat_raw(&t).await {
                Ok(raw) => raw.size,
                Err(e) if e.kind() == ErrorKind::NotFound => 0,
                Err(e) => return Err(map(e, loc.uri())),
            },
            _ => 0,
        };
        let p = t.path.clone();
        let writer = self
            .session
            .run(&t.share, false, |conn, tree| {
                let p = p.clone();
                async move {
                    match mode {
                        WriteMode::CreateNew => tree.create_file_writer_exclusive(conn, &p).await,
                        WriteMode::Truncate => tree.create_file_writer(conn, &p).await,
                        WriteMode::Append => tree.create_file_writer_at(conn, &p, append_at).await,
                    }
                }
            })
            .await
            .map_err(|e| match e.kind() {
                ErrorKind::AlreadyExists => CxError::AlreadyExists(loc.uri()),
                _ => map(e, loc.uri()),
            })?;
        Ok(Box::pin(io::write_stream(writer)))
    }

    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        let t = self.inside_share(loc, "setting times")?;
        let ft = wire::ms_to_filetime(ms);
        self.session
            .run(&t.share, true, |mut conn, tree| {
                let p = t.path.clone();
                async move { wire::set_modified(&mut conn, &tree, &p, ft).await }
            })
            .await
            .map_err(|e| map(e, loc.uri()))
    }

    async fn copy_within(&self, src: &Location, dst: &Location) -> Result<bool> {
        let (Some(s), Some(d)) = (self.target(src)?, self.target(dst)?) else {
            return Ok(false);
        };
        // FSCTL_SRV_COPYCHUNK only works between files on one share.
        if !s.share.eq_ignore_ascii_case(&d.share) || s.path.is_empty() || d.path.is_empty() {
            return Ok(false);
        }
        let src_raw = self.stat_raw(&s).await.map_err(|e| map(e, src.uri()))?;
        if src_raw.is_dir() {
            return Ok(false);
        }
        match self.stat_raw(&d).await {
            Ok(_) => return Err(CxError::AlreadyExists(dst.uri())),
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => return Err(map(e, dst.uri())),
        }
        let res = self
            .session
            .run(&s.share, false, |mut conn, tree| {
                let (from, to) = (s.path.clone(), d.path.clone());
                async move { tree.server_side_copy_file(&mut conn, &from, &to).await }
            })
            .await;
        match res {
            Ok(_) => Ok(true),
            Err(e) => {
                // Don't leave an empty or partial destination behind: the
                // caller falls back to streaming it.
                let _ = self.remove(dst).await;
                if e.kind() == ErrorKind::Unsupported {
                    Ok(false)
                } else {
                    Err(map(e, src.uri()))
                }
            }
        }
    }

    async fn watch(&self, dir: &Location, sink: WatchSink) -> Result<Option<WatchGuard>> {
        // The share list has no change notification.
        let Some(t) = self.target(dir)? else {
            return Ok(None);
        };
        match watch::start(self.session.clone(), t.share, t.path, sink).await {
            Ok(stop) => Ok(Some(WatchGuard::new(stop))),
            Err(e) if e.kind() == ErrorKind::Unsupported => Ok(None),
            Err(e) => Err(map(e, dir.uri())),
        }
    }

    async fn free_space(&self, loc: &Location) -> Result<Option<Space>> {
        let Some(t) = self.target(loc)? else {
            return Ok(None);
        };
        let info = self
            .session
            .run(&t.share, true, |mut conn, tree| async move { tree.fs_info(&mut conn).await })
            .await
            .map_err(|e| map(e, loc.uri()))?;
        Ok(Some(Space { free: info.free_bytes, total: info.total_bytes }))
    }
}
