//! Uploads as an `AsyncWrite`: one PUT for small files, a multipart upload
//! for big ones.
//!
//! The writer cuts the byte stream into parts and hands them to a task over
//! a one-slot channel, which gives backpressure (at most a few parts are in
//! memory). The task can't know a part is the last until the next message
//! arrives, so it keeps one part pending:
//!
//! - `shutdown()` with only one part (or none) seen: a single `PutObject`.
//!   Most files take this path and never start a multipart upload.
//! - a second part arrives: `CreateMultipartUpload`, then each part goes up
//!   as `UploadPart`; `shutdown()` sends the last part and completes.
//! - the writer is dropped without `shutdown()` (cancelled or failed copy):
//!   the channel closes without `Finish`, and a started multipart upload is
//!   aborted so the service doesn't keep (and bill for) orphaned parts.
//!
//! S3 caps an upload at 10 000 parts of at least 5 MiB (except the last).
//! Parts start at [`PART`] and double every 1000 parts, so the size of the
//! file doesn't need to be known up front.

use crate::client::{Request, S3Client};
use crate::xml;
use bytes::Bytes;
use cx_core::CxError;
use reqwest::Method;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use tokio::io::AsyncWrite;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::PollSender;

/// First part size (S3's minimum is 5 MiB).
pub(crate) const PART: usize = 8 * 1024 * 1024;

fn part_size(parts_sent: usize) -> usize {
    PART << (parts_sent / 1000).min(8)
}

enum Msg {
    Part(Bytes),
    Finish,
}

pub(crate) struct Target {
    pub bucket: String,
    pub key: String,
    /// `If-None-Match: *` on the final PUT (CreateNew).
    pub create_new: bool,
    /// URI for error messages.
    pub uri: String,
}

pub(crate) fn start(client: Arc<S3Client>, target: Target) -> Writer {
    let (tx, rx) = mpsc::channel(1);
    let (done_tx, done_rx) = oneshot::channel();
    tokio::spawn(async move {
        let r = run(&client, &target, rx).await;
        let _ = done_tx.send(r.map_err(io::Error::from));
    });
    Writer { tx: Some(PollSender::new(tx)), buf: Vec::new(), parts: 0, done: done_rx, finishing: false }
}

async fn run(client: &S3Client, t: &Target, mut rx: mpsc::Receiver<Msg>) -> Result<(), CxError> {
    let mut pending: Option<Bytes> = None;
    let mut upload: Option<Multipart> = None;
    loop {
        match rx.recv().await {
            Some(Msg::Part(data)) => {
                if let Some(prev) = pending.replace(data) {
                    if upload.is_none() {
                        upload = Some(Multipart::create(client, t).await?);
                    }
                    let up = upload.as_mut().expect("just created");
                    if let Err(e) = up.put_part(client, t, prev).await {
                        up.abort(client, t).await;
                        return Err(e);
                    }
                }
            }
            Some(Msg::Finish) => {
                let last = pending.take().unwrap_or_default();
                let Some(mut up) = upload else {
                    let mut req = Request::new(Method::PUT, Some(&t.bucket), &t.key).body(last);
                    if t.create_new {
                        req = req.header("if-none-match", "*");
                    }
                    client.check(&req, &t.uri).await?;
                    return Ok(());
                };
                let r = async {
                    up.put_part(client, t, last).await?;
                    up.complete(client, t).await
                }
                .await;
                if r.is_err() {
                    up.abort(client, t).await;
                }
                return r;
            }
            None => {
                if let Some(up) = upload {
                    up.abort(client, t).await;
                }
                return Err(CxError::Cancelled);
            }
        }
    }
}

struct Multipart {
    id: String,
    etags: Vec<String>,
}

impl Multipart {
    async fn create(client: &S3Client, t: &Target) -> Result<Multipart, CxError> {
        let req = Request::new(Method::POST, Some(&t.bucket), &t.key).query("uploads", "");
        let root = client.xml_ok(&req, &t.uri).await?;
        let id = root.text("UploadId").filter(|s| !s.is_empty()).ok_or_else(|| CxError::Io(format!("{}: no UploadId in answer", t.uri)))?;
        Ok(Multipart { id: id.to_string(), etags: Vec::new() })
    }

    async fn put_part(&mut self, client: &S3Client, t: &Target, data: Bytes) -> Result<(), CxError> {
        let n = self.etags.len() + 1;
        let req = Request::new(Method::PUT, Some(&t.bucket), &t.key).query("partNumber", n.to_string()).query("uploadId", &self.id).body(data);
        let resp = client.check(&req, &t.uri).await?;
        let etag = resp.headers().get("etag").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        self.etags.push(etag);
        Ok(())
    }

    async fn complete(&self, client: &S3Client, t: &Target) -> Result<(), CxError> {
        let mut body = String::from("<CompleteMultipartUpload xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
        for (i, etag) in self.etags.iter().enumerate() {
            body.push_str(&format!("<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>", i + 1, xml::escape(etag)));
        }
        body.push_str("</CompleteMultipartUpload>");
        let req = Request::new(Method::POST, Some(&t.bucket), &t.key).query("uploadId", &self.id).header("content-type", "application/xml").body(body);
        client.xml_ok(&req, &t.uri).await.map(|_| ())
    }

    /// Best effort: the upload failed or was abandoned.
    async fn abort(&self, client: &S3Client, t: &Target) {
        let req = Request::new(Method::DELETE, Some(&t.bucket), &t.key).query("uploadId", &self.id);
        let _ = client.send(&req).await;
    }
}

/// The `AsyncWrite` side. See the module docs.
pub(crate) struct Writer {
    tx: Option<PollSender<Msg>>,
    buf: Vec<u8>,
    parts: usize,
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

    fn poll_send_part(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.poll_send(cx, |s| Msg::Part(Bytes::from(std::mem::take(&mut s.buf)))))?;
        self.parts += 1;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Writer {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let size = part_size(self.parts);
        if self.buf.len() >= size {
            ready!(self.poll_send_part(cx))?;
        }
        if self.tx.is_none() {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, "stream already shut down")));
        }
        if self.buf.capacity() == 0 {
            self.buf.reserve_exact(size.min(data.len().max(64 * 1024)));
        }
        let take = data.len().min(size - self.buf.len());
        self.buf.extend_from_slice(&data[..take]);
        Poll::Ready(Ok(take))
    }

    /// Parts are only sent when full: S3 has no way to store less than a
    /// part before the end, so flushing is a no-op.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.finishing {
            if !self.buf.is_empty() {
                ready!(self.poll_send_part(cx))?;
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
