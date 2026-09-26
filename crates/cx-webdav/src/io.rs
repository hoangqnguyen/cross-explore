//! Uploads as an `AsyncWrite`.
//!
//! A PUT carries the whole file in one request body, but the transfer engine
//! pushes bytes through an `AsyncWrite`. The two meet in a bounded channel:
//! the request is started right away with a streaming (chunked) body that
//! pulls from the channel, and `shutdown()` ends the body and waits for the
//! server's answer. The channel bound gives backpressure, so a slow server
//! slows the writer down instead of buffering the file in memory.

use bytes::Bytes;
use futures_util::stream;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;

/// Writes are coalesced into chunks of this size.
const CHUNK: usize = 256 * 1024;

pub(crate) enum Msg {
    Data(Bytes),
    /// `shutdown()` was called: end the body normally.
    Finish,
}

/// The body side: yields the written chunks, ends on `Finish`, and fails
/// (aborting the request) if the writer is dropped without shutting down.
pub(crate) fn body(rx: mpsc::Receiver<Msg>) -> reqwest::Body {
    let s = stream::unfold(Some(rx), |rx| async move {
        let mut rx = rx?;
        match rx.recv().await {
            Some(Msg::Data(b)) => Some((Ok(b), Some(rx))),
            Some(Msg::Finish) => None,
            None => Some((Err(io::Error::new(io::ErrorKind::Interrupted, "upload abandoned")), None)),
        }
    });
    reqwest::Body::wrap_stream(s)
}

pub(crate) struct ChannelWriter {
    tx: Option<PollSender<Msg>>,
    buf: Vec<u8>,
    done: oneshot::Receiver<io::Result<()>>,
    finishing: bool,
}

pub(crate) fn channel() -> (mpsc::Receiver<Msg>, oneshot::Sender<io::Result<()>>, ChannelWriter) {
    let (tx, rx) = mpsc::channel(4);
    let (done_tx, done_rx) = oneshot::channel();
    (rx, done_tx, ChannelWriter { tx: Some(PollSender::new(tx)), buf: Vec::new(), done: done_rx, finishing: false })
}

impl ChannelWriter {
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

    /// The request ended early (server error, connection lost): report why.
    fn poll_task_error(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.tx = None;
        match ready!(Pin::new(&mut self.done).poll(cx)) {
            Ok(Err(e)) => Poll::Ready(Err(e)),
            _ => Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "upload stopped"))),
        }
    }

    fn poll_flush_buf(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.buf.is_empty() {
            return Poll::Ready(Ok(()));
        }
        self.poll_send(cx, |s| Msg::Data(Bytes::from(std::mem::take(&mut s.buf))))
    }
}

impl AsyncWrite for ChannelWriter {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        if self.buf.len() >= CHUNK {
            ready!(self.poll_flush_buf(cx))?;
        }
        let take = data.len().min(CHUNK - self.buf.len());
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
            Err(_) => Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "upload task ended"))),
        }
    }
}
