//! [`PeerService`]: the one object the app (or `cx serve`) keeps. It owns
//! the QUIC endpoint, serves the configured shares, holds client
//! connections to other devices, the trust store and pending offers, and
//! reports everything through a single event callback.

use crate::audit::AuditLog;
use crate::client::{PeerClient, PeerProvider};
use crate::connector::{PeerConnector, PeerDirectory};
use crate::events::{EventHandler, PeerEvent};
use crate::identity::{default_name, Identity, IdentityInfo};
use crate::offer::PendingOffer;
use crate::pairing::{PairingCode, PairingCodes, CODE_TTL};
use crate::shares::{self, Share, ShareRoot};
use crate::tailnet::Tailnet;
use crate::trust::{TrustStore, TrustedDevice};
use cx_core::{CxError, Endpoint, Result, Scheme};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct PeerConfig {
    /// Holds the device key, trusted devices, shares and the audit log.
    pub state_dir: PathBuf,
    /// Friendly name shown to other devices (default: host name).
    pub name: Option<String>,
    /// UDP port (default 47470, 0 picks a free one).
    pub port: u16,
    /// Listen on this address only (default: all interfaces).
    pub bind: Option<IpAddr>,
    /// Listen only on this machine's Tailscale address.
    pub tailscale_only: bool,
    /// Accept incoming connections. `false` makes a client-only instance
    /// (the CLI's `ls`/`get`/`put`).
    pub listen: bool,
    /// Shares to serve; `None` loads the ones saved by [`PeerService::set_shares`].
    pub shares: Option<Vec<Share>>,
    pub tailnet_auto_trust: bool,
    pub pairing_code_ttl: Duration,
}

impl PeerConfig {
    pub fn new(state_dir: impl Into<PathBuf>) -> PeerConfig {
        PeerConfig {
            state_dir: state_dir.into(),
            name: None,
            port: Scheme::Peer.default_port(),
            bind: None,
            tailscale_only: false,
            listen: true,
            shares: None,
            tailnet_auto_trust: false,
            pairing_code_ttl: CODE_TTL,
        }
    }

    /// Default state directory: `<data dir>/cross-explore/peer`.
    pub fn default_state_dir() -> PathBuf {
        dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("cross-explore").join("peer")
    }
}

/// Shared state behind the service, the server tasks, clients and the connector.
pub(crate) struct Inner {
    pub identity: Identity,
    pub state_dir: PathBuf,
    pub endpoint: quinn::Endpoint,
    pub client_config: quinn::ClientConfig,
    pub trust: TrustStore,
    pub shares: RwLock<Vec<ShareRoot>>,
    pub pairing: PairingCodes,
    pub audit: AuditLog,
    events: RwLock<EventHandler>,
    pub directory: PeerDirectory,
    pub tailnet: Tailnet,
    pub tailnet_auto_trust: AtomicBool,
    /// Outgoing connections, keyed by "host:port" as the user addressed them.
    pub clients: Mutex<HashMap<String, Arc<PeerClient>>>,
    /// Incoming connections by QUIC connection id, so revoking trust can
    /// drop them at once.
    pub incoming: Mutex<HashMap<usize, (String, quinn::Connection)>>,
    pub offers: Mutex<HashMap<String, PendingOffer>>,
    pub stopped: AtomicBool,
}

impl Inner {
    pub fn emit(&self, e: PeerEvent) {
        let h = self.events.read().unwrap().clone();
        h(e);
    }

    pub fn shares(&self) -> Vec<ShareRoot> {
        self.shares.read().unwrap().clone()
    }

    pub fn tailnet_auto_trust(&self) -> bool {
        self.tailnet_auto_trust.load(Ordering::Relaxed)
    }

    /// The client for `host:port`, created on first use.
    pub fn client(self: &Arc<Self>, host: &str, port: u16) -> Arc<PeerClient> {
        let key = format!("{}:{port}", host.to_ascii_lowercase());
        let mut map = self.clients.lock().unwrap();
        map.entry(key).or_insert_with(|| Arc::new(PeerClient::new(Arc::downgrade(self), host.to_string(), port))).clone()
    }
}

pub struct PeerService {
    inner: Arc<Inner>,
    accept_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl PeerService {
    /// Open (or create) the identity, bind the endpoint and start serving.
    pub async fn start(config: PeerConfig, events: EventHandler) -> Result<PeerService> {
        let name = config.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(default_name);
        let identity = Identity::load_or_create(&config.state_dir, name)?;
        let trust = TrustStore::load(&config.state_dir)?;
        let share_list = match &config.shares {
            Some(s) => s.clone(),
            None => shares::load(&config.state_dir),
        };
        let roots = shares::prepare(&share_list)?;
        let bind_ip = match (config.bind, config.tailscale_only) {
            (Some(ip), _) => Some(ip),
            (None, true) => Some(crate::tailnet::local_ipv4().await.ok_or_else(|| CxError::Connection("Tailscale is not running (no tailnet address)".into()))?),
            (None, false) => None,
        };
        let socket = crate::net::bind(bind_ip, config.port)?;
        let (endpoint, client_config) = crate::net::endpoint(&identity, socket, config.listen)?;
        let inner = Arc::new(Inner {
            audit: AuditLog::new(&config.state_dir),
            identity,
            state_dir: config.state_dir.clone(),
            endpoint,
            client_config,
            trust,
            shares: RwLock::new(roots),
            pairing: PairingCodes::with_ttl(config.pairing_code_ttl),
            events: RwLock::new(events),
            directory: PeerDirectory::default(),
            tailnet: Tailnet::default(),
            tailnet_auto_trust: AtomicBool::new(config.tailnet_auto_trust),
            clients: Mutex::new(HashMap::new()),
            incoming: Mutex::new(HashMap::new()),
            offers: Mutex::new(HashMap::new()),
            stopped: AtomicBool::new(false),
        });
        let task = config.listen.then(|| tokio::spawn(crate::server::accept_loop(inner.clone())));
        Ok(PeerService { inner, accept_task: Mutex::new(task) })
    }

    /// Close every connection and stop listening. Peers are told right away
    /// (not left to time out), so their clients reconnect promptly later.
    pub async fn stop(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
        self.inner.endpoint.close(0u32.into(), b"shutting down");
        if let Some(t) = self.accept_task.lock().unwrap().take() {
            t.abort();
        }
        self.inner.clients.lock().unwrap().clear();
        self.inner.offers.lock().unwrap().clear();
        let _ = tokio::time::timeout(Duration::from_secs(1), self.inner.endpoint.wait_idle()).await;
        // Handles to the endpoint may outlive the service (a connector kept
        // by the Vfs), and quinn holds the socket as long as they exist.
        // Swap in a throwaway socket so the port is free for a restart now.
        // quinn keeps the previous socket around after a rebind (for packets
        // still in flight), so swap twice to really let go of the port.
        for _ in 0..2 {
            if let Ok(s) = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)) {
                let _ = self.inner.endpoint.rebind(s);
            }
        }
    }

    pub fn identity(&self) -> IdentityInfo {
        self.inner.identity.info()
    }

    /// The bound UDP address (useful with port 0).
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.inner.endpoint.local_addr().map_err(|e| CxError::Connection(e.to_string()))
    }

    pub fn set_event_handler(&self, handler: EventHandler) {
        *self.inner.events.write().unwrap() = handler;
    }

    pub fn shares(&self) -> Vec<Share> {
        self.inner.shares().into_iter().map(|s| s.share).collect()
    }

    /// Replace the shares (validated first) and save them in the state dir.
    pub fn set_shares(&self, shares: Vec<Share>) -> Result<()> {
        let roots = shares::prepare(&shares)?;
        shares::save(&self.inner.state_dir, &shares)?;
        *self.inner.shares.write().unwrap() = roots;
        Ok(())
    }

    pub fn trusted_devices(&self) -> Vec<TrustedDevice> {
        self.inner.trust.list()
    }

    /// Forget a device and drop its connections. It must pair again.
    pub fn remove_trusted(&self, device_id: &str) -> Result<bool> {
        let removed = self.inner.trust.remove(device_id)?;
        for (_, (id, conn)) in self.inner.incoming.lock().unwrap().iter() {
            if id == device_id {
                conn.close(1u32.into(), b"no longer trusted");
            }
        }
        self.inner.clients.lock().unwrap().retain(|_, c| c.device_id().as_deref() != Some(device_id));
        Ok(removed)
    }

    /// Limit which shares a paired device may use (`None` = all).
    pub fn set_device_shares(&self, device_id: &str, shares: Option<Vec<String>>) -> Result<bool> {
        self.inner.trust.update(device_id, |d| d.shares = shares)
    }

    /// Show this code on screen; the other device enters it in `pair`.
    pub fn start_pairing(&self) -> PairingCode {
        self.inner.pairing.start()
    }

    pub fn cancel_pairing(&self) {
        self.inner.pairing.cancel();
    }

    /// Pair with the device at `addr` ("host", "host:port" or a device id
    /// known to the directory) using the code it displays.
    pub async fn pair(&self, addr: &str, code: &str) -> Result<TrustedDevice> {
        crate::client::pair(&self.inner, addr, code).await
    }

    /// Ask a running service that uses the same state directory (so the
    /// same device key; e.g. a headless `cx serve`) to show a new pairing
    /// code. Only a connection authenticated with that key is allowed to.
    pub async fn request_pairing_code(&self, addr: &str) -> Result<PairingCode> {
        crate::client::request_pairing_code(&self.inner, addr).await
    }

    pub fn set_tailnet_auto_trust(&self, on: bool) {
        self.inner.tailnet_auto_trust.store(on, Ordering::Relaxed);
    }

    pub fn tailnet_auto_trust(&self) -> bool {
        self.inner.tailnet_auto_trust()
    }

    /// Where discovery (mDNS, tailnet) reports devices; the connector
    /// resolves `peer://<device id>/…` through it.
    pub fn directory(&self) -> &PeerDirectory {
        &self.inner.directory
    }

    /// The `peer://` connector to register with the [`cx_core::Vfs`].
    pub fn connector(&self) -> Arc<PeerConnector> {
        Arc::new(PeerConnector::new(self.inner.clone()))
    }

    /// A provider for `peer://host[:port]` without going through a Vfs.
    pub async fn provider(&self, host: &str, port: Option<u16>) -> Result<Arc<PeerProvider>> {
        let ep = Endpoint { scheme: Scheme::Peer, user: None, host: host.to_string(), port };
        PeerConnector::new(self.inner.clone()).provider(&ep).await
    }

    /// Offer local files to a device (AirDrop-style). Returns the offer id
    /// once the receiver has it; progress arrives as `OfferProgress` events.
    pub async fn send_offer(&self, peer: &str, files: Vec<PathBuf>) -> Result<String> {
        let (host, port) = crate::connector::split_host_port(peer);
        let client = self.inner.client(&host, port);
        crate::offer::send(&self.inner, client, files).await
    }

    pub fn accept_offer(&self, offer_id: &str, dest_dir: PathBuf) -> Result<()> {
        crate::offer::decide(&self.inner, offer_id, Some(dest_dir))
    }

    pub fn decline_offer(&self, offer_id: &str) -> Result<()> {
        crate::offer::decide(&self.inner, offer_id, None)
    }

    #[cfg(test)]
    pub(crate) fn inner(&self) -> &Arc<Inner> {
        &self.inner
    }

    /// Path of the append-only audit log.
    pub fn audit_log_path(&self) -> PathBuf {
        self.inner.audit.path().to_path_buf()
    }
}

impl Drop for PeerService {
    fn drop(&mut self) {
        if !self.inner.stopped.load(Ordering::SeqCst) {
            self.inner.endpoint.close(0u32.into(), b"shutting down");
            if let Some(t) = self.accept_task.lock().unwrap().take() {
                t.abort();
            }
        }
    }
}
