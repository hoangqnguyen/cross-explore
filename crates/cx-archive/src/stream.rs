//! Bridges between the blocking archive codecs and the async `Provider`
//! streams.
//!
//! Decoders are synchronous `Read`ers, so a member is decoded on the
//! blocking pool and handed over in chunks through a small channel. The
//! channel bound gives back-pressure, and dropping the reader stops the
//! decoder at its next chunk.

use cx_core::{CxError, ReadStream, Result};
use std::future::Future;
use std::io::{self, Write};
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::mpsc;

const CHUNK: usize = 64 * 1024;

type Chunk = io::Result<Vec<u8>>;

/// The async end: an `AsyncRead` over chunks sent by a blocking producer.
pub(crate) struct ChannelReader {
    rx: mpsc::Receiver<Chunk>,
    buf: Vec<u8>,
    pos: usize,
}

impl AsyncRead for ChannelReader {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        loop {
            if self.pos < self.buf.len() {
                let n = out.remaining().min(self.buf.len() - self.pos);
                out.put_slice(&self.buf[self.pos..self.pos + n]);
                self.pos += n;
                return Poll::Ready(Ok(()));
            }
            match ready!(self.rx.poll_recv(cx)) {
                Some(Ok(chunk)) => {
                    self.buf = chunk;
                    self.pos = 0;
                }
                Some(Err(e)) => return Poll::Ready(Err(e)),
                None => return Poll::Ready(Ok(())), // producer done: EOF
            }
        }
    }
}

/// The blocking end: a `Write` that drops the first `skip` bytes (read
/// offsets: archive members can't seek) and sends the rest in chunks.
pub(crate) struct ChunkWriter {
    tx: mpsc::Sender<Chunk>,
    skip: u64,
    buf: Vec<u8>,
}

impl ChunkWriter {
    fn send(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK));
        self.tx.blocking_send(Ok(chunk)).map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))
    }
}

impl Write for ChunkWriter {
    fn write(&mut self, mut data: &[u8]) -> io::Result<usize> {
        let len = data.len();
        if self.skip > 0 {
            let s = (self.skip as usize).min(data.len());
            self.skip -= s as u64;
            data = &data[s..];
        }
        self.buf.extend_from_slice(data);
        if self.buf.len() >= CHUNK {
            self.send()?;
        }
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.send()
    }
}

/// Run `produce` on the blocking pool and read what it writes. Errors after
/// the stream started surface as read errors; a reader that went away
/// (broken pipe) is not an error.
pub(crate) fn stream_blocking(offset: u64, produce: impl FnOnce(&mut ChunkWriter) -> Result<()> + Send + 'static) -> ReadStream {
    let (tx, rx) = mpsc::channel(4);
    tokio::task::spawn_blocking(move || {
        let mut w = ChunkWriter { tx: tx.clone(), skip: offset, buf: Vec::with_capacity(CHUNK) };
        let res = produce(&mut w).and_then(|_| w.flush().map_err(|e| CxError::from_io(e, "archive member")));
        if let Err(e) = res {
            if !tx.is_closed() {
                let _ = tx.blocking_send(Err(e.into()));
            }
        }
    });
    Box::pin(ChannelReader { rx, buf: Vec::new(), pos: 0 })
}

pub(crate) type Commit = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

/// A write stream into a temporary file whose `shutdown` runs a commit step
/// (for zip members: rewrite the archive with the new file in it). Dropped
/// without `shutdown`, nothing is committed and the temp file is removed.
pub(crate) struct CommitWriter {
    file: tokio::fs::File,
    // Keeps the temp file alive until the commit has read it.
    _tmp: tempfile::TempPath,
    make_commit: Option<Box<dyn FnOnce() -> Commit + Send>>,
    commit: Option<Commit>,
    done: bool,
}

impl CommitWriter {
    pub fn new(file: tokio::fs::File, tmp: tempfile::TempPath, make_commit: Box<dyn FnOnce() -> Commit + Send>) -> Self {
        CommitWriter { file, _tmp: tmp, make_commit: Some(make_commit), commit: None, done: false }
    }
}

impl AsyncWrite for CommitWriter {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.file).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.file).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.done {
            return Poll::Ready(Ok(()));
        }
        if self.commit.is_none() {
            ready!(Pin::new(&mut self.file).poll_shutdown(cx))?;
            let make = self.make_commit.take().expect("commit not started twice");
            self.commit = Some(make());
        }
        let res = ready!(self.commit.as_mut().expect("commit set").as_mut().poll(cx));
        self.done = true;
        self.commit = None;
        Poll::Ready(res.map_err(io::Error::from))
    }
}
