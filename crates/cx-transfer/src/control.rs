//! Pause/resume/cancel signal checked by every chunk of every file.

use cx_core::{CxError, Result};
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Run {
    Running,
    Paused,
    Cancelled,
}

pub(crate) struct Control(watch::Sender<Run>);

impl Control {
    pub fn new(initial: Run) -> Self {
        Control(watch::Sender::new(initial))
    }

    pub fn get(&self) -> Run {
        *self.0.borrow()
    }

    /// Returns whether anything changed. Cancelling is final.
    pub fn set(&self, run: Run) -> bool {
        self.0.send_if_modified(|cur| {
            if *cur == run || *cur == Run::Cancelled {
                return false;
            }
            *cur = run;
            true
        })
    }

    /// Wait while paused; fail once cancelled.
    pub async fn checkpoint(&self) -> Result<()> {
        let mut rx = self.0.subscribe();
        loop {
            match *rx.borrow_and_update() {
                Run::Running => return Ok(()),
                Run::Cancelled => return Err(CxError::Cancelled),
                Run::Paused => {}
            }
            if rx.changed().await.is_err() {
                return Err(CxError::Cancelled);
            }
        }
    }

    /// Resolves when the job is cancelled.
    pub async fn cancelled(&self) {
        let mut rx = self.0.subscribe();
        let _ = rx.wait_for(|r| *r == Run::Cancelled).await;
    }
}
