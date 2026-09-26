//! "Compress" and "Extract" commands.
//!
//! Both work between any providers: sources are read and destinations
//! written through the [`Vfs`], so zipping an SFTP folder straight onto an
//! SMB share works. The codecs are blocking, so each job is two halves
//! talking over a bounded channel: an async side doing provider I/O and a
//! blocking side running the zip writer or the archive walker. The bound
//! keeps memory flat and the slower side sets the pace.

use crate::format::{strip_archive_ext, ArchiveFormat};
use crate::index::{sanitize, MemberKind};
use crate::walk::{walk_all, Flow};
use crate::zipedit::{dir_options, finish, member_options};
use cx_core::provider::list_all;
use cx_core::{validate_name, CxError, Entry, EntryKind, Location, Provider, Result, Vfs, WriteMode, WriteStream};
use serde::Serialize;
use std::collections::HashSet;
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const CHUNK: usize = 256 * 1024;

/// Progress of a compress/extract job, for the transfer shelf.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// Compress: source bytes read. Extract: archive bytes consumed (the
    /// uncompressed total of a tar.gz is unknown until the end, while the
    /// archive's own size is known up front).
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: u64,
    /// 0 when unknown (extracting).
    pub files_total: u64,
    /// Path of the item being processed, relative to the job.
    pub current: String,
}

enum Cmd {
    Dir { name: String, modified: Option<i64> },
    File { name: String, modified: Option<i64>, size: u64 },
    Data(Vec<u8>),
    Finish,
}

struct Item {
    name: String,
    loc: Location,
    entry: Entry,
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| CxError::Io(format!("worker failed: {e}")))?
}

fn unique(taken: &mut HashSet<String>, name: &str) -> String {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    let mut n = 1;
    loop {
        let candidate = if n == 1 { name.to_string() } else { format!("{stem} ({n}){ext}") };
        if taken.insert(candidate.to_lowercase()) {
            return candidate;
        }
        n += 1;
    }
}

/// Everything under the sources, parents before children. Symlinked
/// folders are stored as nothing rather than followed (loops).
async fn scan(vfs: &Vfs, sources: &[Location], cancel: &CancellationToken) -> Result<Vec<Item>> {
    let mut out = Vec::new();
    let mut top = HashSet::new();
    for src in sources {
        let provider = vfs.provider(src).await?;
        let entry = provider.stat(src).await?;
        let name = unique(&mut top, &src.name());
        let mut stack = vec![(name, src.clone(), entry)];
        while let Some((name, loc, entry)) = stack.pop() {
            if cancel.is_cancelled() {
                return Err(CxError::Cancelled);
            }
            let is_real_dir = entry.kind == EntryKind::Dir;
            if entry.is_dir && !is_real_dir {
                continue;
            }
            if is_real_dir {
                let mut children = list_all(provider.as_ref(), &loc).await?;
                children.sort_by(|a, b| b.name.cmp(&a.name)); // popped in name order
                for c in children {
                    stack.push((format!("{name}/{}", c.name), loc.join(&c.name), c));
                }
            } else if entry.kind == EntryKind::Other {
                continue;
            }
            out.push(Item { name, loc, entry });
        }
    }
    Ok(out)
}

/// Zip `sources` (files or folders from any provider) into `dest_zip`,
/// which must not exist yet. Folders are recursed, mtimes kept, and
/// already-compressed media is stored instead of deflated. On error or
/// cancellation nothing is left at `dest_zip`.
pub async fn compress<P>(vfs: &Arc<Vfs>, sources: Vec<Location>, dest_zip: Location, progress: P, cancel: CancellationToken) -> Result<Entry>
where
    P: Fn(&Progress) + Send + Sync,
{
    let dest = vfs.provider(&dest_zip).await?;
    if dest.stat(&dest_zip).await.is_ok() {
        return Err(CxError::AlreadyExists(dest_zip.uri()));
    }
    let items = scan(vfs, &sources, &cancel).await?;
    let mut p = Progress {
        bytes_total: items.iter().filter(|i| !i.entry.is_dir).map(|i| i.entry.size).sum(),
        files_total: items.iter().filter(|i| !i.entry.is_dir).count() as u64,
        ..Default::default()
    };
    progress(&p);

    // Build the zip in a local temp file: next to a local destination (so
    // it can be renamed into place), in the temp folder otherwise.
    let tmp_dir = dest_zip.local_path().and_then(|p| p.parent()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let tmp = tempfile::Builder::new().prefix(".cx-zip-").tempfile_in(&tmp_dir).map_err(|e| CxError::from_io(e, tmp_dir.display()))?;
    let out = tmp.as_file().try_clone().map_err(|e| CxError::from_io(e, tmp.path().display()))?;
    let (tx, mut rx) = mpsc::channel::<Cmd>(8);
    let writer = tokio::task::spawn_blocking(move || -> Result<bool> {
        let ctx = "new zip";
        let mut zip = zip::ZipWriter::new(BufWriter::new(out));
        let zerr = |e| crate::walk::zip_err(e, ctx);
        while let Some(cmd) = rx.blocking_recv() {
            match cmd {
                Cmd::Dir { name, modified } => zip.add_directory(format!("{name}/"), dir_options(modified)).map_err(zerr)?,
                Cmd::File { name, modified, size } => zip.start_file(name.as_str(), member_options(&name, modified, size)).map_err(zerr)?,
                Cmd::Data(d) => zip.write_all(&d).map_err(|e| CxError::from_io(e, ctx))?,
                Cmd::Finish => {
                    finish(zip, &ctx)?;
                    return Ok(true);
                }
            }
        }
        Ok(false) // sender dropped: cancelled or failed
    });

    let fed = feed(vfs, &items, &tx, &mut p, &progress, &cancel).await;
    if fed.is_ok() {
        let _ = tx.send(Cmd::Finish).await;
    }
    drop(tx);
    let written = writer.await.map_err(|e| CxError::Io(format!("zip writer failed: {e}")))?;
    fed?;
    if !written? {
        return Err(CxError::Cancelled);
    }

    match &dest_zip {
        Location::Local(path) => {
            let path = path.clone();
            blocking(move || {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o644));
                }
                tmp.persist_noclobber(&path).map(|_| ()).map_err(|e| CxError::from_io(e.error, path.display()))
            })
            .await?
        }
        _ => {
            let mut input = tokio::fs::File::open(tmp.path()).await.map_err(|e| CxError::from_io(e, tmp.path().display()))?;
            let mut w = dest.open_write(&dest_zip, WriteMode::CreateNew).await?;
            let res = async {
                tokio::io::copy(&mut input, &mut w).await?;
                w.shutdown().await
            }
            .await;
            if let Err(e) = res {
                drop(w);
                let _ = dest.remove(&dest_zip).await;
                return Err(CxError::io(format!("writing {dest_zip}"), e));
            }
        }
    }
    dest.stat(&dest_zip).await
}

async fn feed<P: Fn(&Progress)>(vfs: &Vfs, items: &[Item], tx: &mpsc::Sender<Cmd>, p: &mut Progress, progress: &P, cancel: &CancellationToken) -> Result<()> {
    let gone = || CxError::Io("zip writer stopped".into());
    let mut buf = vec![0u8; CHUNK];
    for item in items {
        if cancel.is_cancelled() {
            return Err(CxError::Cancelled);
        }
        p.current = item.name.clone();
        if item.entry.is_dir {
            tx.send(Cmd::Dir { name: item.name.clone(), modified: item.entry.modified }).await.map_err(|_| gone())?;
            continue;
        }
        let provider = vfs.provider(&item.loc).await?;
        let mut r = provider.open_read(&item.loc, 0).await?;
        tx.send(Cmd::File { name: item.name.clone(), modified: item.entry.modified, size: item.entry.size }).await.map_err(|_| gone())?;
        loop {
            let n = tokio::select! {
                n = r.read(&mut buf) => n.map_err(|e| CxError::io(&item.loc, e))?,
                _ = cancel.cancelled() => return Err(CxError::Cancelled),
            };
            if n == 0 {
                break;
            }
            tx.send(Cmd::Data(buf[..n].to_vec())).await.map_err(|_| gone())?;
            p.bytes_done += n as u64;
            progress(p);
        }
        p.files_done += 1;
        progress(p);
    }
    Ok(())
}

/// A free name in `dir`: `name`, `name (2)`, `name (3)`, …
async fn free_name(provider: &dyn Provider, dir: &Location, name: &str) -> Result<String> {
    for n in 1..10_000 {
        let candidate = if n == 1 { name.to_string() } else { format!("{name} ({n})") };
        match provider.stat(&dir.join(&candidate)).await {
            Err(CxError::NotFound(_)) => return Ok(candidate),
            Err(e) => return Err(e),
            Ok(_) => continue,
        }
    }
    Err(CxError::AlreadyExists(name.to_string()))
}

enum Out {
    Dir { key: String },
    File { key: String, modified: Option<i64> },
    Data(Vec<u8>),
}

/// Extract `archive` (an archive file anywhere) into a new folder in
/// `dest_dir`, named after the archive ("photos.zip" → "photos", or
/// "photos (2)" when taken). Returns the new folder.
///
/// Members whose path would escape the folder (`..`, absolute paths) abort
/// the extraction ("zip-slip"); links are skipped rather than recreated, so
/// they can't point outside either. On error or cancellation the partial
/// folder is removed.
pub async fn extract<P>(vfs: &Arc<Vfs>, archive: Location, dest_dir: Location, progress: P, cancel: CancellationToken) -> Result<Location>
where
    P: Fn(&Progress) + Send + Sync,
{
    let src = vfs.provider(&archive).await?;
    let stat = src.stat(&archive).await?;
    let name = archive.name();
    // Remote archives are downloaded to a temp file for the walk.
    let _download;
    let file = match archive.local_path() {
        Some(p) => p.to_path_buf(),
        None => {
            let tmp = tempfile::Builder::new().prefix(".cx-extract-").tempfile().map_err(|e| CxError::from_io(e, "temp file"))?.into_temp_path();
            crate::cache::download(vfs, &archive, &tmp).await?;
            let p = tmp.to_path_buf();
            _download = tmp;
            p
        }
    };
    let format = {
        let (n, f) = (name.clone(), file.clone());
        blocking(move || ArchiveFormat::detect_file(&n, &f)).await?
    };

    let dest = vfs.provider(&dest_dir).await?;
    let base = match strip_archive_ext(&name) {
        "" => "Archive".to_string(),
        s => s.to_string(),
    };
    let target = loop {
        let n = free_name(dest.as_ref(), &dest_dir, &base).await?;
        match dest.create_dir(&dest_dir, Some(&n)).await {
            Ok(_) => break dest_dir.join(&n),
            Err(CxError::AlreadyExists(_)) => continue, // lost a race
            Err(e) => return Err(e),
        }
    };

    let counter = Arc::new(AtomicU64::new(0));
    let (tx, rx) = mpsc::channel::<Out>(8);
    let walker = {
        let (file, counter) = (file.clone(), counter.clone());
        tokio::task::spawn_blocking(move || walk_members(&file, format, counter, tx))
    };
    let p = Progress { bytes_total: stat.size, ..Default::default() };
    let written = write_members(dest.as_ref(), &target, rx, p, &counter, &progress, &cancel).await;
    let walked = walker.await.map_err(|e| CxError::Io(format!("archive reader failed: {e}")))?;
    let result = match (written, walked) {
        (Err(e), _) | (Ok(()), Err(e)) => Err(e),
        (Ok(()), Ok(())) => Ok(target.clone()),
    };
    if result.is_err() {
        let _ = dest.remove(&target).await;
    }
    result
}

/// The blocking half of `extract`: walk the archive and send its members.
fn walk_members(file: &std::path::Path, format: ArchiveFormat, counter: Arc<AtomicU64>, tx: mpsc::Sender<Out>) -> Result<()> {
    let send = |m: Out| tx.blocking_send(m).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe));
    let res = walk_all(file, format, counter, &mut |h, r: &mut dyn Read| {
        let Some(key) = sanitize(&h.raw_name) else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("unsafe path in archive: {}", h.raw_name)));
        };
        if key.split('/').any(|seg| validate_name(seg).is_err()) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("invalid name in archive: {}", h.raw_name)));
        }
        match h.kind {
            MemberKind::Dir => send(Out::Dir { key })?,
            MemberKind::File => {
                send(Out::File { key, modified: h.modified })?;
                let mut buf = vec![0u8; CHUNK];
                loop {
                    let n = r.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    send(Out::Data(buf[..n].to_vec()))?;
                }
            }
            MemberKind::Symlink | MemberKind::Other => {}
        }
        Ok(Flow::Continue)
    });
    match res {
        // The writer side stopped first; it reports why.
        Err(_) if tx.is_closed() => Ok(()),
        Err(CxError::Io(m)) if m.contains("unsafe path") || m.contains("invalid name") => Err(CxError::InvalidName(m)),
        other => other,
    }
}

/// The async half of `extract`: create folders and files at the destination.
async fn write_members<P: Fn(&Progress)>(
    dest: &dyn Provider,
    target: &Location,
    mut rx: mpsc::Receiver<Out>,
    mut p: Progress,
    counter: &AtomicU64,
    progress: &P,
    cancel: &CancellationToken,
) -> Result<()> {
    let mut made: HashSet<String> = HashSet::new();
    let mut current: Option<(WriteStream, Location, Option<i64>)> = None;
    loop {
        let msg = tokio::select! {
            m = rx.recv() => m,
            _ = cancel.cancelled() => return Err(CxError::Cancelled),
        };
        let Some(msg) = msg else { break };
        match msg {
            Out::Data(d) => {
                if let Some((w, loc, _)) = current.as_mut() {
                    w.write_all(&d).await.map_err(|e| CxError::io(&*loc, e))?;
                }
                p.bytes_done = counter.load(Ordering::Relaxed);
                progress(&p);
                continue;
            }
            Out::Dir { key } => {
                close(dest, current.take(), &mut p, progress).await?;
                ensure_dir(dest, target, &key, &mut made).await?;
            }
            Out::File { key, modified } => {
                close(dest, current.take(), &mut p, progress).await?;
                let (parent, _) = crate::index::split_parent(&key);
                ensure_dir(dest, target, parent, &mut made).await?;
                let loc = key.split('/').fold(target.clone(), |l, s| l.join(s));
                // Truncate: a later duplicate member wins, as in tar.
                let w = dest.open_write(&loc, WriteMode::Truncate).await?;
                p.current = key;
                current = Some((w, loc, modified));
            }
        }
    }
    close(dest, current.take(), &mut p, progress).await
}

async fn close<P: Fn(&Progress)>(dest: &dyn Provider, current: Option<(WriteStream, Location, Option<i64>)>, p: &mut Progress, progress: &P) -> Result<()> {
    let Some((mut w, loc, modified)) = current else { return Ok(()) };
    w.shutdown().await.map_err(|e| CxError::io(&loc, e))?;
    drop(w);
    if let Some(ms) = modified {
        let _ = dest.set_modified(&loc, ms).await;
    }
    p.files_done += 1;
    progress(p);
    Ok(())
}

/// Create `key` (and its missing ancestors) under `target`.
async fn ensure_dir(dest: &dyn Provider, target: &Location, key: &str, made: &mut HashSet<String>) -> Result<()> {
    let mut acc = String::new();
    let mut loc = target.clone();
    for seg in key.split('/').filter(|s| !s.is_empty()) {
        acc = if acc.is_empty() { seg.to_string() } else { format!("{acc}/{seg}") };
        let parent = loc.clone();
        loc = loc.join(seg);
        if made.contains(&acc) {
            continue;
        }
        match dest.create_dir(&parent, Some(seg)).await {
            Ok(_) => {}
            Err(CxError::AlreadyExists(_)) if dest.stat(&loc).await.map(|e| e.is_dir).unwrap_or(false) => {}
            Err(e) => return Err(e),
        }
        made.insert(acc.clone());
    }
    Ok(())
}
