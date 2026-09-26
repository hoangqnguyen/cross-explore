//! Byte streams over SMB handles.
//!
//! The transfer engine wants plain `AsyncRead` / `AsyncWrite`, while SMB is
//! fastest with many large requests in flight. Both directions therefore run
//! a background task that owns the smb2 handle and talks to the stream
//! through a small bounded channel: the reader keeps [`READ_DEPTH`] READs of
//! [`CHUNK`] on the wire ahead of the consumer, the writer hands chunks to
//! smb2's pipelined `FileWriter` (which keeps its own window of WRITEs). The
//! channel bound is the backpressure: a slow consumer or a slow link never
//! makes either side buffer without limit.

use futures_util::stream::{self, StreamExt};
use smb2::{FileReader, FileWriter};
use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot};

/// Bytes per READ request and per chunk handed to the writer.
pub(crate) const CHUNK: u64 = 1024 * 1024;
/// READs kept in flight ahead of the consumer.
const READ_DEPTH: usize = 8;

/// Stream `reader` from `offset` to its end.
pub(crate) fn read_stream(reader: FileReader, offset: u64) -> ChannelReader {
    let (tx, rx) = mpsc::channel::<io::Result<Vec<u8>>>(4);
    tokio::spawn(async move {
        let size = reader.size();
        {
            let offsets = (offset..size).step_by(CHUNK as usize);
            let mut chunks = stream::iter(offsets).map(|o| reader.read_at(o, CHUNK)).buffered(READ_DEPTH);
            while let Some(res) = chunks.next().await {
                let item = res.map_err(|e| io::Error::other(e.to_string()));
                let failed = item.is_err();
                // A closed channel means the consumer dropped the stream:
                // stop reading (the in-flight READs are discarded by smb2).
                if tx.send(item).await.is_err() || failed {
                    break;
                }
            }
        }
        let _ = reader.close().await;
    });
    ChannelReader { rx, buf: Vec::new(), pos: 0 }
}

pub(crate) struct ChannelReader {
    rx: mpsc::Receiver<io::Result<Vec<u8>>>,
    buf: Vec<u8>,
    pos: usize,
}

impl AsyncRead for ChannelReader {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, out: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        loop {
            if self.pos < self.buf.len() {
                let n = out.remaining().min(self.buf.len() - self.pos);
                let pos = self.pos;
                out.put_slice(&self.buf[pos..pos + n]);
                self.pos += n;
                return Poll::Ready(Ok(()));
            }
            match ready!(self.rx.poll_recv(cx)) {
                Some(Ok(chunk)) => {
                    self.buf = chunk;
                    self.pos = 0;
                }
                Some(Err(e)) => return Poll::Ready(Err(e)),
                None => return Poll::Ready(Ok(())), // EOF
            }
        }
    }
}

enum Msg {
    Data(Vec<u8>),
    /// `shutdown()` was called: flush, close, report.
    Finish,
}

/// Write through `writer`. Dropping the stream without `shutdown()` aborts
/// the write (the handle is closed, the partial file stays).
pub(crate) fn write_stream(mut writer: FileWriter) -> ChannelWriter {
    let (tx, mut rx) = mpsc::channel::<Msg>(4);
    let (done_tx, done_rx) = oneshot::channel();
    tokio::spawn(async move {
        let result = loop {
            match rx.recv().await {
                Some(Msg::Data(d)) => {
                    if let Err(e) = writer.write_chunk(&d).await {
                        let _ = writer.abort().await;
                        break Err(io::Error::other(e.to_string()));
                    }
                }
                Some(Msg::Finish) => break writer.finish().await.map(|_| ()).map_err(|e| io::Error::other(e.to_string())),
                None => {
                    let _ = writer.abort().await;
                    break Err(io::Error::new(io::ErrorKind::Interrupted, "write abandoned"));
                }
            }
        };
        let _ = done_tx.send(result);
    });
    ChannelWriter { tx: Some(tokio_util::sync::PollSender::new(tx)), buf: Vec::new(), done: done_rx, finishing: false }
}

pub(crate) struct ChannelWriter {
    tx: Option<tokio_util::sync::PollSender<Msg>>,
    buf: Vec<u8>,
    done: oneshot::Receiver<io::Result<()>>,
    finishing: bool,
}

impl ChannelWriter {
    /// Send `msg` once the channel has room. If the task already stopped,
    /// surface the error it stopped with.
    fn poll_send(&mut self, cx: &mut Context<'_>, msg: impl FnOnce(&mut Self) -> Msg) -> Poll<io::Result<()>> {
        let Some(tx) = self.tx.as_mut() else {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "stream already shut down")));
        };
        if ready!(tx.poll_reserve(cx)).is_err() {
            return self.poll_task_error(cx);
        }
        let m = msg(self);
        if self.tx.as_mut().unwrap().send_item(m).is_err() {
            return self.poll_task_error(cx);
        }
        Poll::Ready(Ok(()))
    }

    fn poll_task_error(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.tx = None;
        match ready!(Pin::new(&mut self.done).poll(cx)) {
            Ok(Err(e)) => Poll::Ready(Err(e)),
            _ => Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "writer stopped"))),
        }
    }

    fn poll_flush_buf(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.buf.is_empty() {
            return Poll::Ready(Ok(()));
        }
        self.poll_send(cx, |s| Msg::Data(std::mem::take(&mut s.buf)))
    }
}

use std::future::Future;

impl AsyncWrite for ChannelWriter {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        // Coalesce small writes into CHUNK-sized messages.
        if self.buf.len() as u64 >= CHUNK {
            ready!(self.poll_flush_buf(cx))?;
        }
        let take = data.len().min(CHUNK as usize - self.buf.len());
        self.buf.extend_from_slice(&data[..take]);
        Poll::Ready(Ok(take))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush_buf(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.finishing {
            ready!(self.poll_flush_buf(cx))?;
            ready!(self.poll_send(cx, |_| Msg::Finish))?;
            self.finishing = true;
            self.tx = None;
        }
        match ready!(Pin::new(&mut self.done).poll(cx)) {
            Ok(r) => Poll::Ready(r),
            Err(_) => Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "writer task ended"))),
        }
    }
}
