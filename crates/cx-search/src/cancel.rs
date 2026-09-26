use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// A cheap, clonable cancellation token.
///
/// Blocking workers poll [`Cancel::is_cancelled`]; async loops `select!` on
/// [`Cancel::cancelled`] so a stuck remote listing does not delay the stop.
#[derive(Clone, Default, Debug)]
pub struct Cancel(Arc<Inner>);

#[derive(Default, Debug)]
struct Inner {
    flag: AtomicBool,
    notify: Notify,
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.flag.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.flag.load(Ordering::SeqCst)
    }

    /// Resolves once [`Cancel::cancel`] has been called (immediately if it already was).
    pub async fn cancelled(&self) {
        loop {
            let notified = self.0.notify.notified();
            tokio::pin!(notified);
            // Register before checking the flag so a concurrent cancel can't be missed.
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}
