//! One SMB session per endpoint, with tree connects cached per share.
//!
//! smb2's `Connection` is a cheap `Arc` clone that multiplexes every request
//! over one TCP connection, so operations take a clone and run without any
//! lock held: a big download never blocks a listing in the other pane. The
//! only lock guards the `SmbClient` itself, which is needed for the rare
//! stateful steps (tree connect, share enumeration, reconnect).
//!
//! Reconnects happen at two levels. smb2 revives a dead socket in place under
//! every connection clone (`auto_reconnect`), but tree ids belong to the old
//! session, so trees are cached with the connection *generation* they were
//! made in and re-connected when it moves. If revival itself gave up, the
//! client is rebuilt from scratch on the next operation.

use crate::error;
use smb2::client::connection::Connection;
use smb2::{ClientConfig, ShareInfo, SmbClient, Tree};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) struct Session {
    config: ClientConfig,
    state: Mutex<State>,
}

struct State {
    client: SmbClient,
    /// Keyed by lower-cased share name (share names are case-insensitive).
    trees: HashMap<String, (u64, Arc<Tree>)>,
}

impl Session {
    pub(crate) async fn connect(config: ClientConfig) -> smb2::Result<Session> {
        let client = SmbClient::connect(config.clone()).await?;
        Ok(Session { config, state: Mutex::new(State { client, trees: HashMap::new() }) })
    }

    /// Make sure the client has a live session, rebuilding it if smb2 gave up
    /// on reviving the old one.
    async fn ensure_alive(&self, state: &mut State) -> smb2::Result<()> {
        if !state.client.is_disconnected() {
            return Ok(());
        }
        state.trees.clear();
        if state.client.reconnect().await.is_ok() {
            return Ok(());
        }
        state.client = SmbClient::connect(self.config.clone()).await?;
        Ok(())
    }

    /// A connection clone plus the (cached) tree connect for `share`.
    pub(crate) async fn tree(&self, share: &str) -> smb2::Result<(Connection, Arc<Tree>)> {
        let mut state = self.state.lock().await;
        self.ensure_alive(&mut state).await?;
        let generation = state.client.connection().generation();
        let key = share.to_lowercase();
        if let Some((g, tree)) = state.trees.get(&key) {
            if *g == generation {
                return Ok((state.client.connection().clone(), tree.clone()));
            }
        }
        let tree = Arc::new(state.client.connect_share(share).await?);
        let generation = state.client.connection().generation();
        state.trees.insert(key, (generation, tree.clone()));
        Ok((state.client.connection().clone(), tree))
    }

    async fn forget(&self, share: &str) {
        let mut state = self.state.lock().await;
        state.trees.remove(&share.to_lowercase());
        // A connection that looks alive but answered "session deleted" is
        // not: make the next `ensure_alive` rebuild it.
        if state.client.is_disconnected() {
            state.trees.clear();
        }
    }

    /// Run `op` against `share`, retrying once on a fresh tree connect if the
    /// cached one went stale. `idempotent` ops are also retried after a lost
    /// connection; others (create, rename, delete) may already have happened
    /// on the server, so that error is surfaced instead of guessed about.
    pub(crate) async fn run<T, F, Fut>(&self, share: &str, idempotent: bool, op: F) -> smb2::Result<T>
    where
        F: Fn(Connection, Arc<Tree>) -> Fut,
        Fut: Future<Output = smb2::Result<T>>,
    {
        let (conn, tree) = self.tree(share).await?;
        match op(conn, tree).await {
            Err(e) if error::is_stale(&e) && (idempotent || rejected_before_running(&e)) => {
                self.forget(share).await;
                let (conn, tree) = self.tree(share).await?;
                op(conn, tree).await
            }
            other => other,
        }
    }

    pub(crate) async fn list_shares(&self) -> smb2::Result<Vec<ShareInfo>> {
        let mut state = self.state.lock().await;
        self.ensure_alive(&mut state).await?;
        state.client.list_shares().await
    }
}

/// The server refused the request outright because the tree or session id
/// was unknown, so nothing ran and a retry is safe.
fn rejected_before_running(e: &smb2::Error) -> bool {
    use smb2::types::status::NtStatus;
    matches!(error::status(e), Some(s) if s == NtStatus::NETWORK_NAME_DELETED || s == NtStatus::USER_SESSION_DELETED)
}
