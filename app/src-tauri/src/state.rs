//! Everything the backend keeps alive for the app's lifetime.

use crate::credentials::KeychainCredentials;
use crate::events::Events;
use crate::jobs::Jobs;
use crate::tags::Tags;
use cx_core::Vfs;
use cx_discovery::Discovery;
use cx_peer::PeerService;
use cx_thumbs::Thumbnailer;
use cx_transfer::TransferManager;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// Peer-mode preferences the peer crate doesn't persist itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerPrefs {
    pub enabled: bool,
    pub tailnet_auto_trust: bool,
}

pub struct App {
    pub vfs: Arc<Vfs>,
    pub events: Arc<Events>,
    pub jobs: Arc<Jobs>,
    pub transfers: Arc<TransferManager>,
    pub thumbs: Thumbnailer,
    pub tags: Tags,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub discovery: Mutex<Option<Discovery>>,
    pub peer: tokio::sync::RwLock<Option<Arc<PeerService>>>,
    pub peer_prefs: Mutex<PeerPrefs>,
    tasks: Mutex<HashMap<u64, CancellationToken>>,
    next_task: AtomicU64,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        vfs: Arc<Vfs>,
        events: Arc<Events>,
        jobs: Arc<Jobs>,
        transfers: Arc<TransferManager>,
        thumbs: Thumbnailer,
        data_dir: PathBuf,
        cache_dir: PathBuf,
    ) -> App {
        let peer_prefs = std::fs::read(data_dir.join("peer.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        App {
            tags: Tags::new(data_dir.join("tags.json")),
            vfs,
            events,
            jobs,
            transfers,
            thumbs,
            data_dir,
            cache_dir,
            discovery: Mutex::new(None),
            peer: tokio::sync::RwLock::new(None),
            peer_prefs: Mutex::new(peer_prefs),
            tasks: Mutex::new(HashMap::new()),
            // Archive jobs and tasks get ids far away from transfer-engine ids.
            next_task: AtomicU64::new(1 << 40),
        }
    }

    pub fn save_peer_prefs(&self) {
        let prefs = self.peer_prefs.lock().unwrap().clone();
        if let Ok(b) = serde_json::to_vec_pretty(&prefs) {
            let _ = std::fs::write(self.data_dir.join("peer.json"), b);
        }
    }

    /// Register a cancellable task; returns its id and token.
    pub fn task(&self) -> (u64, CancellationToken) {
        let id = self.next_task.fetch_add(1, Ordering::Relaxed);
        let token = CancellationToken::new();
        self.tasks.lock().unwrap().insert(id, token.clone());
        (id, token)
    }

    pub fn finish_task(&self, id: u64) {
        self.tasks.lock().unwrap().remove(&id);
    }

    pub fn cancel_task(&self, id: u64) -> bool {
        match self.tasks.lock().unwrap().remove(&id) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    pub fn credentials() -> Arc<KeychainCredentials> {
        Arc::new(KeychainCredentials::default())
    }
}
