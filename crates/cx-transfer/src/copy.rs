//! Copying one file: server-side/clone fast path, else a resumable stream
//! into `<name>.cxpart` that is renamed into place only once complete.

use crate::engine::Ctx;
use cx_core::{CxError, Entry, Location, Provider, Result, WriteMode};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(crate) struct FileTask {
    pub src: Location,
    pub dst: Location,
    pub entry: Entry,
    /// `dst` exists and is replaced once the new copy is complete.
    pub replace: bool,
    /// The file's parent folder was not created by this job, so undo has to
    /// remove the file itself.
    pub top: bool,
}

pub(crate) fn part_location(dst: &Location) -> Location {
    let name = format!("{}.cxpart", dst.name());
    dst.parent().map(|p| p.join(&name)).unwrap_or_else(|| dst.clone())
}

/// Stream errors that look like a dropped link are retried; anything the
/// file system says about the file itself is not.
fn stream_err(err: io::Error, loc: &Location) -> CxError {
    use io::ErrorKind::*;
    match err.kind() {
        ConnectionReset | ConnectionAborted | ConnectionRefused | BrokenPipe | TimedOut | UnexpectedEof | NotConnected | Interrupted => {
            CxError::Connection(format!("{}: {err}", loc.name()))
        }
        _ => CxError::from_io(err, loc.name()),
    }
}

fn retryable(e: &CxError) -> bool {
    matches!(e, CxError::Connection(_))
}

pub(crate) async fn hash(p: &dyn Provider, loc: &Location, chunk: usize) -> Result<blake3::Hash> {
    let mut r = p.open_read(loc, 0).await?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; chunk];
    loop {
        let n = r.read(&mut buf).await.map_err(|e| stream_err(e, loc))?;
        if n == 0 {
            return Ok(h.finalize());
        }
        h.update(&buf[..n]);
    }
}

/// Copy `t.src` to `t.dst`. Bytes are counted into the job's progress as
/// they move and taken back out if the file fails.
pub(crate) async fn transfer(cx: &Ctx, t: &FileTask) -> Result<()> {
    let part = part_location(&t.dst);
    let mut counted = 0u64;
    let result = transfer_inner(cx, t, &part, &mut counted).await;
    if result.is_err() {
        cx.job.counters.sub_bytes(counted);
        if let Ok(p) = cx.mgr.vfs().provider(&part).await {
            let _ = p.remove(&part).await;
        }
    }
    result
}

async fn transfer_inner(cx: &Ctx, t: &FileTask, part: &Location, counted: &mut u64) -> Result<()> {
    let vfs = cx.mgr.vfs();
    let (src_p, dst_p) = (vfs.provider(&t.src).await?, vfs.provider(&t.dst).await?);
    cx.job.ctrl.checkpoint().await?;
    let resume_from = if cx.job.restored { partial_len(dst_p.as_ref(), part, t.entry.size).await } else { 0 };
    let cloned = resume_from == 0 && t.src.same_provider(&t.dst) && matches!(src_p.copy_within(&t.src, part).await, Ok(true));
    if cloned {
        cx.job.counters.add_bytes(t.entry.size);
        *counted = t.entry.size;
    } else {
        stream(cx, t, part, resume_from, counted).await?;
    }
    verify(cx, t, src_p.as_ref(), dst_p.as_ref(), part).await?;
    if t.replace {
        match dst_p.remove(&t.dst).await {
            Ok(()) | Err(CxError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
    }
    dst_p.move_to(part, &t.dst).await?;
    if let Some(ms) = t.entry.modified {
        let _ = dst_p.set_modified(&t.dst, ms).await;
    }
    Ok(())
}

/// Length of a usable partial file, 0 if there is none (or it is bogus).
async fn partial_len(p: &dyn Provider, part: &Location, size: u64) -> u64 {
    match p.stat(part).await {
        Ok(e) if !e.is_dir && e.size <= size => e.size,
        _ => 0,
    }
}

async fn stream(cx: &Ctx, t: &FileTask, part: &Location, mut offset: u64, counted: &mut u64) -> Result<()> {
    let cfg = cx.mgr.config();
    let counters = &cx.job.counters;
    let mut attempt = 0;
    loop {
        // Progress follows what is really in the partial file.
        if offset >= *counted {
            counters.add_bytes(offset - *counted);
        } else {
            counters.sub_bytes(*counted - offset);
        }
        *counted = offset;
        match stream_once(cx, t, part, offset, counted).await {
            Ok(()) => return Ok(()),
            Err(e) if retryable(&e) && attempt < cfg.retries => {
                let wait = cfg.retry_backoff * 2u32.pow(attempt);
                attempt += 1;
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = cx.job.ctrl.cancelled() => return Err(CxError::Cancelled),
                }
                // Providers are looked up again so a dropped connection can be reopened.
                let dst_p = cx.mgr.vfs().provider(part).await?;
                offset = partial_len(dst_p.as_ref(), part, t.entry.size).await;
            }
            Err(e) => return Err(e),
        }
    }
}

async fn stream_once(cx: &Ctx, t: &FileTask, part: &Location, offset: u64, counted: &mut u64) -> Result<()> {
    let vfs = cx.mgr.vfs();
    let (src_p, dst_p) = (vfs.provider(&t.src).await?, vfs.provider(part).await?);
    let mut r = src_p.open_read(&t.src, offset).await?;
    let mode = if offset == 0 { WriteMode::Truncate } else { WriteMode::Append };
    let mut w = dst_p.open_write(part, mode).await?;
    let mut buf = vec![0u8; cx.mgr.config().chunk_size.max(4096)];
    let res: Result<()> = async {
        loop {
            cx.job.ctrl.checkpoint().await?;
            let n = r.read(&mut buf).await.map_err(|e| stream_err(e, &t.src))?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n]).await.map_err(|e| stream_err(e, part))?;
            cx.job.counters.add_bytes(n as u64);
            *counted += n as u64;
        }
        w.shutdown().await.map_err(|e| stream_err(e, part))
    }
    .await;
    if res.is_err() {
        // Flush what was written so a retry can append after it.
        let _ = w.shutdown().await;
    }
    res
}

async fn verify(cx: &Ctx, t: &FileTask, src_p: &dyn Provider, dst_p: &dyn Provider, part: &Location) -> Result<()> {
    let got = dst_p.stat(part).await?.size;
    if got != t.entry.size {
        return Err(CxError::Io(format!("{}: copied {got} of {} bytes", t.entry.name, t.entry.size)));
    }
    if cx.job.req.verify {
        let chunk = cx.mgr.config().chunk_size;
        let (a, b) = tokio::try_join!(hash(src_p, &t.src, chunk), hash(dst_p, part, chunk))?;
        if a != b {
            return Err(CxError::Io(format!("{}: verification failed, the copy differs from the original", t.entry.name)));
        }
    }
    Ok(())
}
