//! `peer://` for the [`cx_core::Vfs`].
//!
//! The endpoint host is either a device id (resolved through the
//! [`PeerDirectory`] that discovery fills, then the addresses a trusted
//! device was last reached at) or a plain host name / IP address.

use crate::client::PeerProvider;
use crate::service::Inner;
use async_trait::async_trait;
use cx_core::{Connector, Credentials, Endpoint, Provider, Result, Scheme};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

/// Device id → current addresses, as seen by discovery (mDNS, tailnet
/// peers). Cloning shares the same map.
#[derive(Clone, Default)]
pub struct PeerDirectory(Arc<RwLock<HashMap<String, Vec<SocketAddr>>>>);

impl PeerDirectory {
    pub fn set(&self, device_id: &str, addrs: Vec<SocketAddr>) {
        let mut m = self.0.write().unwrap();
        if addrs.is_empty() {
            m.remove(device_id);
        } else {
            m.insert(device_id.to_string(), addrs);
        }
    }

    pub fn get(&self, device_id: &str) -> Vec<SocketAddr> {
        self.0.read().unwrap().get(device_id).cloned().unwrap_or_default()
    }

    pub fn remove(&self, device_id: &str) {
        self.0.write().unwrap().remove(device_id);
    }

    pub fn all(&self) -> HashMap<String, Vec<SocketAddr>> {
        self.0.read().unwrap().clone()
    }
}

pub struct PeerConnector {
    inner: Arc<Inner>,
}

impl PeerConnector {
    pub(crate) fn new(inner: Arc<Inner>) -> PeerConnector {
        PeerConnector { inner }
    }

    /// Like [`Connector::connect`], with the concrete provider type (for
    /// peer-only extras such as `hash` and `list_shares`).
    pub async fn provider(&self, ep: &Endpoint) -> Result<Arc<PeerProvider>> {
        let client = self.inner.client(&ep.host, ep.port_or_default());
        // Connect now so an untrusted or unreachable peer fails here, where
        // the UI expects sign-in / host-key errors.
        client.connection().await?;
        Ok(Arc::new(PeerProvider::new(client)))
    }
}

#[async_trait]
impl Connector for PeerConnector {
    fn scheme(&self) -> Scheme {
        Scheme::Peer
    }

    /// Credentials are ignored: peers authenticate with pinned device keys.
    async fn connect(&self, ep: &Endpoint, _creds: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        Ok(self.provider(ep).await?)
    }
}

/// "host", "host:port", "[v6]:port", "v6" → (host, port).
pub fn split_host_port(s: &str) -> (String, u16) {
    let default = Scheme::Peer.default_port();
    let s = s.trim().trim_start_matches("peer://").trim_end_matches('/');
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return (sa.ip().to_string(), sa.port());
    }
    if s.parse::<std::net::IpAddr>().is_ok() {
        return (s.to_string(), default);
    }
    match s.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => match p.parse() {
            Ok(port) => (h.to_string(), port),
            Err(_) => (s.to_string(), default),
        },
        _ => (s.trim_start_matches('[').trim_end_matches(']').to_string(), default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_hosts_and_ports() {
        assert_eq!(split_host_port("nas"), ("nas".into(), 47470));
        assert_eq!(split_host_port("nas.local:5000"), ("nas.local".into(), 5000));
        assert_eq!(split_host_port("127.0.0.1:9"), ("127.0.0.1".into(), 9));
        assert_eq!(split_host_port("[::1]:9"), ("::1".into(), 9));
        assert_eq!(split_host_port("fe80::1"), ("fe80::1".into(), 47470));
        assert_eq!(split_host_port("peer://abcdefghijklmnop/"), ("abcdefghijklmnop".into(), 47470));
    }
}
