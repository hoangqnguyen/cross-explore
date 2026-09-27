//! Uploads as an `AsyncWrite`, cut into chunks.
//!
//! All three services upload big files in pieces (Drive: a resumable
//! session; Dropbox: an upload session; OneDrive: an upload session), and
//! small files in one request. The transfer engine writes a byte stream of
//! unknown length, so the writer here cuts it into fixed-size chunks and
//! hands them over a one-slot channel (backpressure: only a couple of
//! chunks are ever in memory) to a task that drives a service-specific
//! [`Uploader`].
//!
//! The task can't know a chunk is the last until the next message comes, so
//! it keeps one chunk pending (as cx-s3's multipart writer does):
//!
//! - `shutdown()` after at most one chunk: [`Uploader::whole`], one request.
//! - more chunks: [`Uploader::part`] for each but the last, then
//!   [`Uploader::last`] with the total size, which is only now known.
//! - the writer dropped without `shutdown()` (cancelled or failed copy):
//!   [`Uploader::abort`], so no half-written file or session is left behind.

use bytes::Bytes;
use cx_core::{CxError, Result};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;

#[async_trait::async_trait]
pub(crate) trait Uploader: Send + 'static {
    /// The whole file, which fit in one chunk (it may be empty).
    async fn whole(&mut self, data: Bytes) -> Result<()>;
    /// A chunk that is not the last; `offset` is the number of bytes before it.
    async fn part(&mut self, data: Bytes, offset: u64) -> Result<()>;
    /// The last chunk of a file sent in parts; `total` is the file's size.
    async fn last(&mut self, data: Bytes, offset: u64, total: u64) -> Result<()>;
    /// The upload was abandoned after `part` calls: clean up (best effort).
    async fn abort(&mut self) {}
}

enum Msg {
    Chunk(Bytes),
    Finish,
}

pub(crate) fn start(chunk: usize, uploader: impl Uploader) -> Writer {
    let (tx, rx) = mpsc::channel(1);
    let (done_tx, done_rx) = oneshot::channel();
    tokio::spawn(async move {
        let r = run(uploader, rx).await;
        let _ = done_tx.send(r.map_err(io::Error::from));
    });
    Writer { tx: Some(PollSender::new(tx)), buf: Vec::new(), chunk: chunk.max(1), done: done_rx, finishing: false }
}

async fn run(mut up: impl Uploader, mut rx: mpsc::Receiver<Msg>) -> Result<()> {
    let mut pending: Option<Bytes> = None;
    let mut offset = 0u64;
    let mut started = false;
    loop {
        match rx.recv().await {
            Some(Msg::Chunk(data)) => {
                if let Some(prev) = pending.replace(data) {
                    started = true;
                    let len = prev.len() as u64;
                    if let Err(e) = up.part(prev, offset).await {
                        up.abort().await;
                        return Err(e);
                    }
                    offset += len;
                }
            }
            Some(Msg::Finish) => {
                let last = pending.take().unwrap_or_default();
                if !started {
                    return up.whole(last).await;
                }
                let total = offset + last.len() as u64;
                let r = up.last(last, offset, total).await;
                if r.is_err() {
                    up.abort().await;
                }
                return r;
            }
            None => {
                if started {
                    up.abort().await;
                }
                return Err(CxError::Cancelled);
            }
        }
    }
}

/// The `AsyncWrite` side. See the module docs.
pub(crate) struct Writer {
    tx: Option<PollSender<Msg>>,
    buf: Vec<u8>,
    chunk: usize,
    done: oneshot::Receiver<io::Result<()>>,
    finishing: bool,
}

impl Writer {
    fn poll_send(&mut self, cx: &mut Context<'_>, msg: impl FnOnce(&mut Self) -> Msg) -> Poll<io::Result<()>> {
        let Some(tx) = self.tx.as_mut() else {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "stream already shut down")));
        };
        if ready!(tx.poll_reserve(cx)).is_err() {
            return self.poll_task_error(cx);
        }
        let m = msg(self);
        if self.tx.as_mut().is_some_and(|tx| tx.send_item(m).is_err()) {
            return self.poll_task_error(cx);
        }
        Poll::Ready(Ok(()))
    }

    /// The upload task ended early (server error, connection lost): say why.
    fn poll_task_error(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.tx = None;
        match ready!(Pin::new(&mut self.done).poll(cx)) {
            Ok(Err(e)) => Poll::Ready(Err(e)),
            _ => Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "upload stopped"))),
        }
    }

    fn poll_send_chunk(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_send(cx, |s| Msg::Chunk(Bytes::from(std::mem::take(&mut s.buf))))
    }
}

impl AsyncWrite for Writer {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        if self.buf.len() >= self.chunk {
            ready!(self.poll_send_chunk(cx))?;
        }
        if self.tx.is_none() {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "stream already shut down")));
        }
        if self.buf.capacity() == 0 {
            let want = self.chunk.min(data.len().max(64 * 1024));
            self.buf.reserve_exact(want);
        }
        let take = data.len().min(self.chunk - self.buf.len());
        self.buf.extend_from_slice(&data[..take]);
        Poll::Ready(Ok(take))
    }

    /// Chunks go out only when full (the services want fixed-size pieces),
    /// so flushing is a no-op.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.finishing {
            if !self.buf.is_empty() {
                ready!(self.poll_send_chunk(cx))?;
            }
            ready!(self.poll_send(cx, |_| Msg::Finish))?;
            self.finishing = true;
            self.tx = None;
        }
        match ready!(Pin::new(&mut self.done).poll(cx)) {
            Ok(r) => Poll::Ready(r),
            Err(_) => Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "upload task ended"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::AsyncWriteExt;

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<String>>>);

    #[async_trait::async_trait]
    impl Uploader for Log {
        async fn whole(&mut self, data: Bytes) -> Result<()> {
            self.0.lock().unwrap().push(format!("whole {}", data.len()));
            Ok(())
        }
        async fn part(&mut self, data: Bytes, offset: u64) -> Result<()> {
            self.0.lock().unwrap().push(format!("part {offset}+{}", data.len()));
            Ok(())
        }
        async fn last(&mut self, data: Bytes, offset: u64, total: u64) -> Result<()> {
            self.0.lock().unwrap().push(format!("last {offset}+{} of {total}", data.len()));
            Ok(())
        }
        async fn abort(&mut self) {
            self.0.lock().unwrap().push("abort".into());
        }
    }

    #[tokio::test]
    async fn chunking() {
        for (size, expect) in [
            (0usize, vec!["whole 0"]),
            (10, vec!["whole 10"]),
            (16, vec!["whole 16"]),
            (40, vec!["part 0+16", "part 16+16", "last 32+8 of 40"]),
            (32, vec!["part 0+16", "last 16+16 of 32"]),
        ] {
            let log = Log::default();
            let mut w = start(16, log.clone());
            w.write_all(&vec![7u8; size]).await.unwrap();
            w.shutdown().await.unwrap();
            assert_eq!(*log.0.lock().unwrap(), expect, "size {size}");
        }
    }

    #[tokio::test]
    async fn dropped_writer_aborts() {
        let log = Log::default();
        let mut w = start(16, log.clone());
        w.write_all(&[1u8; 40]).await.unwrap();
        drop(w);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(log.0.lock().unwrap().last().map(String::as_str), Some("abort"));
    }
}
