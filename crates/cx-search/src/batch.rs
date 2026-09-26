//! Hit batching: a small, quick first batch so results appear at once, then
//! larger, less frequent ones to keep IPC overhead low.

use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

const FIRST_LIMIT: usize = 32;
const NEXT_LIMIT: usize = 512;
const FIRST_DELAY: Duration = Duration::from_millis(60);
const NEXT_DELAY: Duration = Duration::from_millis(250);

pub(crate) struct Batcher<T> {
    sink: mpsc::Sender<Vec<T>>,
    buf: Vec<T>,
    sent_any: bool,
    /// When the oldest buffered item arrived.
    since: Instant,
}

/// The receiver was dropped: nobody wants more results.
pub(crate) struct Closed;

impl<T> Batcher<T> {
    pub fn new(sink: mpsc::Sender<Vec<T>>) -> Self {
        Batcher { sink, buf: Vec::new(), sent_any: false, since: Instant::now() }
    }

    pub fn push(&mut self, item: T) {
        if self.buf.is_empty() {
            self.since = Instant::now();
        }
        self.buf.push(item);
    }

    pub fn has_pending(&self) -> bool {
        !self.buf.is_empty()
    }

    /// When the buffered items must go out even if no more arrive.
    pub fn deadline(&self) -> Instant {
        self.since + if self.sent_any { NEXT_DELAY } else { FIRST_DELAY }
    }

    pub fn due(&self) -> bool {
        let limit = if self.sent_any { NEXT_LIMIT } else { FIRST_LIMIT };
        self.has_pending() && (self.buf.len() >= limit || Instant::now() >= self.deadline())
    }

    pub async fn flush(&mut self) -> Result<(), Closed> {
        if self.buf.is_empty() {
            return Ok(());
        }
        self.sent_any = true;
        self.sink.send(std::mem::take(&mut self.buf)).await.map_err(|_| Closed)
    }

    pub async fn flush_if_due(&mut self) -> Result<(), Closed> {
        if self.due() {
            self.flush().await
        } else {
            Ok(())
        }
    }

    pub fn is_closed(&self) -> bool {
        self.sink.is_closed()
    }
}
