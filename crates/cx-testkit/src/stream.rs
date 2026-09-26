//! Byte streams over the in-memory tree, with pacing and injected faults.

use crate::provider::{now_ms, Node, Tree};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

#[derive(Debug, Clone)]
pub(crate) struct Pace {
    pub chunk: usize,
    pub delay: Duration,
    pub bytes_per_sec: Option<u64>,
}

impl Default for Pace {
    fn default() -> Self {
        Pace { chunk: 64 * 1024, delay: Duration::ZERO, bytes_per_sec: None }
    }
}

impl Pace {
    fn wait_for(&self, bytes: usize) -> Duration {
        let throttle = self.bytes_per_sec.map(|bps| Duration::from_secs_f64(bytes as f64 / bps.max(1) as f64)).unwrap_or_default();
        self.delay + throttle
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Fault {
    pub after: u64,
    pub times: u32,
}

/// Waits before each chunk; `Ready` once the wait for the current chunk is over.
struct Gate {
    sleep: Option<Pin<Box<Sleep>>>,
}

impl Gate {
    fn poll(&mut self, cx: &mut Context<'_>, wait: Duration) -> Poll<()> {
        if wait.is_zero() {
            return Poll::Ready(());
        }
        let sleep = self.sleep.get_or_insert_with(|| Box::pin(tokio::time::sleep(wait)));
        match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => {
                self.sleep = None;
                Poll::Ready(())
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

fn injected() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionReset, "injected connection reset")
}

pub(crate) struct MemRead {
    data: Vec<u8>,
    pos: usize,
    pace: Pace,
    gate: Gate,
    /// Absolute position at which the stream breaks.
    fail_at: Option<usize>,
}

impl MemRead {
    pub fn new(data: Vec<u8>, offset: usize, pace: Pace, fail_after: Option<u64>) -> Self {
        let pos = offset.min(data.len());
        MemRead { data, pos, pace, gate: Gate { sleep: None }, fail_at: fail_after.map(|n| pos + n as usize) }
    }
}

impl AsyncRead for MemRead {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let mut end = (me.pos + me.pace.chunk).min(me.data.len()).min(me.pos + buf.remaining());
        if let Some(f) = me.fail_at {
            if me.pos >= f {
                return Poll::Ready(Err(injected()));
            }
            end = end.min(f);
        }
        if me.gate.poll(cx, me.pace.wait_for(end - me.pos)).is_pending() {
            return Poll::Pending;
        }
        buf.put_slice(&me.data[me.pos..end]);
        me.pos = end;
        Poll::Ready(Ok(()))
    }
}

pub(crate) struct MemWrite {
    tree: Arc<Mutex<Tree>>,
    key: String,
    pace: Pace,
    gate: Gate,
    written: u64,
    fail_after: Option<u64>,
}

impl MemWrite {
    pub fn new(tree: Arc<Mutex<Tree>>, key: String, pace: Pace, fail_after: Option<u64>) -> Self {
        MemWrite { tree, key, pace, gate: Gate { sleep: None }, written: 0, fail_after }
    }
}

impl AsyncWrite for MemWrite {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        let mut n = buf.len().min(me.pace.chunk);
        if let Some(f) = me.fail_after {
            if me.written >= f {
                return Poll::Ready(Err(injected()));
            }
            n = n.min((f - me.written) as usize);
        }
        if me.gate.poll(cx, me.pace.wait_for(n)).is_pending() {
            return Poll::Pending;
        }
        match me.tree.lock().unwrap().get_mut(&me.key) {
            Some(Node::File { data, modified }) => {
                data.extend_from_slice(&buf[..n]);
                *modified = now_ms();
            }
            _ => return Poll::Ready(Err(io::Error::new(io::ErrorKind::NotFound, format!("{} vanished", me.key)))),
        }
        me.written += n as u64;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
