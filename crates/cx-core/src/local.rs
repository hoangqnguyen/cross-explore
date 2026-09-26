use crate::{Capabilities, CxError, Entry, EntryKind, Location, Provider, Result};
use async_trait::async_trait;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
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

pub fn validate_name(name: &str) -> Result<()> {
    let bad_char = |c: char| c == '/' || c == '\0' || (cfg!(windows) && "\\:*?\"<>|".contains(c));
    if name.is_empty() || name == "." || name == ".." || name.chars().any(bad_char) {
        return Err(CxError::InvalidName(name.to_string()));
    }
    Ok(())
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

#[async_trait]
impl Provider for LocalProvider {
    fn scheme(&self) -> &'static str {
        "file"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: true, polling: false, server_copy: true, trash: true, posix: cfg!(unix) }
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
                fs::create_dir(&path).map_err(|e| CxError::from_io(e, path.display()))?;
                return Entry::from_path(&path).map_err(|e| CxError::from_io(e, path.display()));
            }
            for n in 1..10_000 {
                let name = if n == 1 { "New folder".to_string() } else { format!("New folder ({n})") };
                let path = dir.join(&name);
                match fs::create_dir(&path) {
                    Ok(()) => return Entry::from_path(&path).map_err(|e| CxError::from_io(e, path.display())),
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(e) => return Err(CxError::from_io(e, path.display())),
                }
            }
            Err(CxError::AlreadyExists("New folder".into()))
        })
        .await
    }

    async fn rename(&self, dir: &Location, from: &str, to: &str) -> Result<Entry> {
        let dir = Self::path(dir)?;
        let (from, to) = (from.to_owned(), to.to_owned());
        blocking(move || {
            validate_name(&to)?;
            let src = dir.join(&from);
            let dst = dir.join(&to);
            // A case-only rename targets the same file on case-insensitive
            // file systems, so the existence check must not block it.
            let case_only = from.to_lowercase() == to.to_lowercase();
            if !case_only && fs::symlink_metadata(&dst).is_ok() {
                return Err(CxError::AlreadyExists(to));
            }
            fs::rename(&src, &dst).map_err(|e| CxError::from_io(e, src.display()))?;
            Entry::from_path(&dst).map_err(|e| CxError::from_io(e, dst.display()))
        })
        .await
    }

    async fn trash(&self, dir: &Location, names: &[String]) -> Result<()> {
        let dir = Self::path(dir)?;
        let paths: Vec<PathBuf> = names.iter().map(|n| dir.join(n)).collect();
        blocking(move || trash::delete_all(&paths).map_err(|e| CxError::Io(format!("move to trash failed: {e}")))).await
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
}
