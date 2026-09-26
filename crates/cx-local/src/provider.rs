use cx_core::{validate_name, Capabilities, CxError, Entry, EntryKind, Location, Provider, ReadStream, Result, Space, TrashedItem, WatchGuard, WatchSink, WriteMode, WriteStream};
use async_trait::async_trait;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tokio::io::AsyncSeekExt;
use tokio::sync::mpsc;
use tokio::task::spawn_blocking;
/// Batch sizes for streamed listings: a tiny first batch for the first paint,
/// then larger ones to keep IPC overhead low.
const FIRST_BATCH: usize = 128;
const NEXT_BATCH: usize = 2048;

#[derive(Debug, Default, Clone, Copy)]
pub struct LocalProvider;

impl LocalProvider {
    fn path(loc: &Location) -> Result<PathBuf> {
        loc.local_path()
            .map(Path::to_path_buf)
            .ok_or_else(|| CxError::InvalidLocation(loc.uri()))
    }
}

fn list_blocking(dir: &Path, sink: &mpsc::Sender<Vec<Entry>>) -> Result<usize> {
    let rd = fs::read_dir(dir).map_err(|e| CxError::from_io(e, dir.display()))?;
    let mut batch = Vec::with_capacity(FIRST_BATCH);
    let mut limit = FIRST_BATCH;
    let mut total = 0;
    for de in rd {
        let Ok(de) = de else { continue };
        let name = de.file_name().to_string_lossy().into_owned();
        let path = de.path();
        let entry = match de.metadata() {
            Ok(meta) => Entry::from_metadata(name, &path, &meta),
            // Unreadable entries still show up, just without details.
            Err(_) => Entry {
                hidden: name.starts_with('.'),
                is_dir: de.file_type().map(|t| t.is_dir()).unwrap_or(false),
                kind: EntryKind::Other,
                name,
                size: 0,
                modified: None,
                created: None,
                readonly: false,
            },
        };
        batch.push(entry);
        if batch.len() >= limit {
            total += batch.len();
            if sink.blocking_send(std::mem::take(&mut batch)).is_err() {
                return Ok(total); // receiver gone: listing was cancelled
            }
            limit = NEXT_BATCH;
            batch.reserve(limit);
        }
    }
    total += batch.len();
    if !batch.is_empty() {
        let _ = sink.blocking_send(batch);
    }
    Ok(total)
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    spawn_blocking(f)
        .await
        .map_err(|e| CxError::Io(format!("worker failed: {e}")))?
}

fn is_cross_device(e: &io::Error) -> bool {
    // EXDEV on Unix, ERROR_NOT_SAME_DEVICE on Windows.
    matches!(e.raw_os_error(), Some(18) if cfg!(unix)) || matches!(e.raw_os_error(), Some(17) if cfg!(windows))
}

fn io_err(path: &Path) -> impl Fn(io::Error) -> CxError + '_ {
    move |e| CxError::from_io(e, path.display())
}

#[async_trait]
impl Provider for LocalProvider {
    fn scheme(&self) -> &'static str {
        "file"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: true, polling: false, server_copy: true, trash: crate::trash::AVAILABLE, posix: cfg!(unix), writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let dir = Self::path(dir)?;
        blocking(move || list_blocking(&dir, &sink)).await
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        let path = Self::path(loc)?;
        blocking(move || Entry::from_path(&path).map_err(|e| CxError::from_io(e, path.display()))).await
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let dir = Self::path(dir)?;
        let name = name.map(str::to_owned);
        blocking(move || {
            if let Some(name) = name {
                validate_name(&name)?;
                let path = dir.join(&name);
                fs::create_dir(&path).map_err(io_err(&path))?;
                return Entry::from_path(&path).map_err(io_err(&path));
            }
            for n in 1..10_000 {
                let name = if n == 1 { "New folder".to_string() } else { format!("New folder ({n})") };
                let path = dir.join(&name);
                match fs::create_dir(&path) {
                    Ok(()) => return Entry::from_path(&path).map_err(io_err(&path)),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(CxError::from_io(e, path.display())),
                }
            }
            Err(CxError::AlreadyExists("New folder".into()))
        })
        .await
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        let (src, dst) = (Self::path(src)?, Self::path(dst)?);
        blocking(move || {
            let same_file = src.parent() == dst.parent()
                && src.file_name().map(|n| n.to_string_lossy().to_lowercase()) == dst.file_name().map(|n| n.to_string_lossy().to_lowercase());
            // A case-only rename targets the same file on case-insensitive
            // file systems, so the existence check must not block it.
            if !same_file && fs::symlink_metadata(&dst).is_ok() {
                return Err(CxError::AlreadyExists(dst.display().to_string()));
            }
            fs::rename(&src, &dst).map_err(|e| {
                if is_cross_device(&e) {
                    CxError::Unsupported("moving across volumes".into())
                } else {
                    CxError::from_io(e, src.display())
                }
            })
        })
        .await
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let path = Self::path(loc)?;
        blocking(move || {
            let meta = fs::symlink_metadata(&path).map_err(io_err(&path))?;
            if meta.is_dir() {
                fs::remove_dir_all(&path).map_err(io_err(&path))
            } else {
                fs::remove_file(&path).map_err(io_err(&path))
            }
        })
        .await
    }

    async fn trash(&self, dir: &Location, names: &[String]) -> Result<Vec<TrashedItem>> {
        let dir = Self::path(dir)?;
        let paths: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
        blocking(move || crate::trash::trash_paths(&paths)).await
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let path = Self::path(loc)?;
        let mut f = tokio::fs::File::open(&path).await.map_err(io_err(&path))?;
        if offset > 0 {
            f.seek(io::SeekFrom::Start(offset)).await.map_err(io_err(&path))?;
        }
        Ok(Box::pin(f))
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let path = Self::path(loc)?;
        let mut opts = tokio::fs::OpenOptions::new();
        opts.write(true);
        match mode {
            WriteMode::CreateNew => opts.create_new(true),
            WriteMode::Truncate => opts.create(true).truncate(true),
            WriteMode::Append => opts.create(true).append(true),
        };
        let f = opts.open(&path).await.map_err(io_err(&path))?;
        Ok(Box::pin(f))
    }

    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        let path = Self::path(loc)?;
        blocking(move || {
            let t = if ms >= 0 {
                std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64)
            } else {
                std::time::UNIX_EPOCH - std::time::Duration::from_millis(ms.unsigned_abs())
            };
            let f = fs::File::options().write(true).open(&path).or_else(|_| fs::File::open(&path)).map_err(io_err(&path))?;
            f.set_modified(t).map_err(io_err(&path))
        })
        .await
    }

    async fn copy_within(&self, src: &Location, dst: &Location) -> Result<bool> {
        let (src, dst) = (Self::path(src)?, Self::path(dst)?);
        // Copy-on-write clones (APFS, Btrfs, XFS, ReFS) are instant and take
        // no extra space. Anything else is streamed by the transfer engine
        // so it can report progress.
        blocking(move || Ok(fs::metadata(&src).map(|m| m.is_file()).unwrap_or(false) && reflink_copy::reflink(&src, &dst).is_ok())).await
    }

    async fn watch(&self, dir: &Location, sink: WatchSink) -> Result<Option<WatchGuard>> {
        let path = Self::path(dir)?;
        let w = crate::watch::watch_dir(&path, move |c| sink(c)).map_err(|e| CxError::io(format!("cannot watch {}", path.display()), e))?;
        Ok(Some(WatchGuard::new(w)))
    }

    async fn free_space(&self, loc: &Location) -> Result<Option<Space>> {
        let path = Self::path(loc)?;
        blocking(move || {
            let disks = sysinfo::Disks::new_with_refreshed_list();
            Ok(disks
                .list()
                .iter()
                .filter(|d| path.starts_with(d.mount_point()))
                .max_by_key(|d| d.mount_point().as_os_str().len())
                .map(|d| Space { free: d.available_space(), total: d.total_space() }))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn list_all(dir: &Path) -> (Vec<Vec<Entry>>, usize) {
        let (tx, mut rx) = mpsc::channel(16);
        let loc = Location::local(dir);
        let handle = tokio::spawn(async move { LocalProvider.list(&loc, tx).await });
        let mut batches = Vec::new();
        while let Some(b) = rx.recv().await {
            batches.push(b);
        }
        (batches, handle.await.unwrap().unwrap())
    }

    #[tokio::test]
    async fn lists_in_small_first_batch_then_larger_ones() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..(FIRST_BATCH + 10) {
            fs::write(tmp.path().join(format!("f{i}.txt")), b"hi").unwrap();
        }
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let (batches, total) = list_all(tmp.path()).await;
        assert_eq!(total, FIRST_BATCH + 11);
        assert_eq!(batches[0].len(), FIRST_BATCH);
        let all: Vec<_> = batches.concat();
        let sub = all.iter().find(|e| e.name == "sub").unwrap();
        assert!(sub.is_dir && sub.kind == EntryKind::Dir);
        let f = all.iter().find(|e| e.name == "f0.txt").unwrap();
        assert_eq!(f.size, 2);
        assert!(f.modified.is_some());
    }

    #[tokio::test]
    async fn create_dir_picks_free_names() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = Location::local(tmp.path());
        let a = LocalProvider.create_dir(&loc, None).await.unwrap();
        let b = LocalProvider.create_dir(&loc, None).await.unwrap();
        assert_eq!(a.name, "New folder");
        assert_eq!(b.name, "New folder (2)");
    }

    #[tokio::test]
    async fn rename_never_overwrites() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = Location::local(tmp.path());
        fs::write(tmp.path().join("a"), b"a").unwrap();
        fs::write(tmp.path().join("b"), b"b").unwrap();
        assert!(matches!(LocalProvider.rename(&loc, "a", "b").await, Err(CxError::AlreadyExists(_))));
        assert!(matches!(LocalProvider.rename(&loc, "a", "x/y").await, Err(CxError::InvalidName(_))));
        let e = LocalProvider.rename(&loc, "a", "c").await.unwrap();
        assert_eq!(e.name, "c");
        assert_eq!(fs::read(tmp.path().join("b")).unwrap(), b"b");
    }

    #[tokio::test]
    async fn missing_dir_is_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, _rx) = mpsc::channel(1);
        let r = LocalProvider.list(&Location::local(tmp.path().join("nope")), tx).await;
        assert!(matches!(r, Err(CxError::NotFound(_))));
    }

    #[tokio::test]
    async fn streams_read_write_append() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let tmp = tempfile::tempdir().unwrap();
        let loc = Location::local(tmp.path().join("f.bin"));
        let mut w = LocalProvider.open_write(&loc, WriteMode::CreateNew).await.unwrap();
        w.write_all(b"hello ").await.unwrap();
        w.shutdown().await.unwrap();
        assert!(matches!(LocalProvider.open_write(&loc, WriteMode::CreateNew).await, Err(CxError::AlreadyExists(_))));
        let mut w = LocalProvider.open_write(&loc, WriteMode::Append).await.unwrap();
        w.write_all(b"world").await.unwrap();
        w.shutdown().await.unwrap();
        let mut r = LocalProvider.open_read(&loc, 6).await.unwrap();
        let mut s = String::new();
        r.read_to_string(&mut s).await.unwrap();
        assert_eq!(s, "world");
        LocalProvider.set_modified(&loc, 1_000_000).await.unwrap();
        assert_eq!(LocalProvider.stat(&loc).await.unwrap().modified, Some(1_000_000));
    }

    #[tokio::test]
    async fn move_and_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Location::local(tmp.path());
        fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        fs::write(tmp.path().join("a/b/c.txt"), b"x").unwrap();
        LocalProvider.move_to(&dir.join("a"), &dir.join("z")).await.unwrap();
        assert!(tmp.path().join("z/b/c.txt").exists());
        LocalProvider.remove(&dir.join("z")).await.unwrap();
        assert!(!tmp.path().join("z").exists());
    }
}
