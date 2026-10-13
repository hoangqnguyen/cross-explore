//! Peer mode (other Cross Explore devices) and nearby-device discovery.
//!
//! The peer service always runs as a client, so paired devices can be
//! browsed; it only listens (and advertises itself over mDNS) while sharing
//! is on. Discovered Cross Explore devices are fed into the peer client's
//! [`cx_peer::PeerDirectory`], so `peer://<device id>/` resolves without the
//! user ever typing an address.

use crate::events::{EngineEvent, IncomingOffer, OfferPeer};
use crate::jobs::{name_of, JobView};
use crate::Engine;
use cx_core::{CxError, Location, Result};
use cx_discovery::{Advertisement, Device, Discovery, DiscoveryConfig, DiscoveryEvent};
use cx_peer::{PeerConfig, PeerEvent, PeerService, Share};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Weak};

/// The port peers listen on (and are dialled on when an address has none).
pub const PEER_PORT: u16 = 47470;

/// Peer-mode preferences the peer crate doesn't persist itself.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PeerPrefs {
    pub enabled: bool,
    pub tailnet_auto_trust: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrustedView {
    pub id: String,
    pub name: String,
    pub added_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PeerStatus {
    pub enabled: bool,
    pub device_id: String,
    pub name: String,
    pub port: u16,
    pub shares: Vec<Share>,
    pub trusted: Vec<TrustedView>,
    pub tailnet_auto_trust: bool,
}

/// A device we just paired with.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Paired {
    pub id: String,
    pub name: String,
}

fn no_peer() -> CxError {
    CxError::Unsupported("peer mode".into())
}

/// Tell the peer client where discovered Cross Explore devices are.
pub fn feed_directory(svc: &PeerService, devices: &[Device]) {
    for d in devices {
        for s in d
            .services
            .iter()
            .filter(|s| s.scheme == cx_core::Scheme::Peer)
        {
            let Ok(loc) = Location::parse(&s.uri) else {
                continue;
            };
            let Some(ep) = loc.endpoint() else { continue };
            let addrs: Vec<SocketAddr> = d
                .addresses
                .iter()
                .map(|ip| SocketAddr::new(*ip, s.port))
                .collect();
            if !addrs.is_empty() {
                svc.directory().set(&ep.host, addrs);
            }
        }
    }
}

/// Give devices that are also paired Cross Explore peers a `peer://` service.
/// mDNS doesn't cross Tailscale and the port probe is TCP-only while peers
/// speak QUIC, so a paired device seen only as a tailnet node would otherwise
/// show up with just SMB/SFTP (or nothing) and its share list stay out of reach.
/// A device matches when an address it was last reached at is one of its own.
pub fn add_paired_peers(devices: &mut [Device], trusted: &[cx_peer::TrustedDevice]) {
    for d in devices.iter_mut() {
        if d.services.iter().any(|s| s.scheme == cx_core::Scheme::Peer) {
            continue;
        }
        let paired = trusted.iter().find(|t| {
            t.last_addrs
                .iter()
                .filter_map(|a| a.parse::<SocketAddr>().ok())
                .any(|a| d.addresses.contains(&a.ip().to_canonical()))
        });
        if let Some(t) = paired {
            d.services.push(cx_discovery::Service {
                scheme: cx_core::Scheme::Peer,
                port: cx_core::Scheme::Peer.default_port(),
                uri: format!("peer://{}/", t.device_id),
                label: "Cross Explore".into(),
                source: cx_discovery::Source::Probe,
            });
        }
    }
}

/// "123 456" for display; codes are typed with or without the space.
pub fn format_pair_code(code: &str) -> String {
    let i = 3.min(code.len());
    format!("{} {}", &code[..i], &code[i..])
}

/// `host` or `host:port` → an address with the peer port filled in.
pub fn peer_address(address: &str) -> String {
    let address = address.trim();
    if address.contains(':') && !address.contains("::") {
        address.to_string()
    } else {
        format!("{address}:{PEER_PORT}")
    }
}

impl Engine {
    pub(crate) fn load_peer_prefs(data_dir: &std::path::Path) -> PeerPrefs {
        std::fs::read(data_dir.join("peer.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save_peer_prefs(&self) {
        let prefs = self.peer_prefs.lock().unwrap().clone();
        if let Ok(b) = serde_json::to_vec_pretty(&prefs) {
            let _ = std::fs::write(self.data_dir.join("peer.json"), b);
        }
    }

    pub fn peer_prefs(&self) -> PeerPrefs {
        self.peer_prefs.lock().unwrap().clone()
    }

    /// The running peer service, if it started.
    pub async fn peer_service(&self) -> Option<Arc<PeerService>> {
        self.peer.read().await.clone()
    }

    pub async fn peer_status(&self) -> Result<PeerStatus> {
        let guard = self.peer.read().await;
        let svc = guard.as_ref().ok_or_else(no_peer)?;
        let id = svc.identity();
        let prefs = self.peer_prefs();
        Ok(PeerStatus {
            enabled: prefs.enabled,
            device_id: id.device_id,
            name: id.name,
            port: svc.local_addr().map(|a| a.port()).unwrap_or(PEER_PORT),
            shares: svc.shares(),
            trusted: svc
                .trusted_devices()
                .into_iter()
                .map(|t| TrustedView {
                    id: t.device_id,
                    name: t.name,
                    added_at: t.added_at,
                })
                .collect(),
            tailnet_auto_trust: prefs.tailnet_auto_trust,
        })
    }

    /// (Re)start the peer service with the current preferences. It always
    /// runs as a client so other devices can be browsed; it only listens
    /// (and advertises itself) when sharing is on.
    pub async fn start_peer(self: &Arc<Self>) -> Result<()> {
        if let Some(old) = self.peer.write().await.take() {
            old.stop().await;
        }
        let prefs = self.peer_prefs();
        let mut cfg = PeerConfig::new(self.data_dir.join("peer"));
        cfg.listen = prefs.enabled;
        cfg.port = if prefs.enabled {
            self.config.peer_port
        } else {
            0
        };
        cfg.tailnet_auto_trust = prefs.tailnet_auto_trust;
        let weak = Arc::downgrade(self);
        let handler =
            move |w: Weak<Engine>| Arc::new(move |e| on_peer_event(&w, e)) as cx_peer::EventHandler;
        let svc = match PeerService::start(cfg.clone(), handler(weak.clone())).await {
            Ok(s) => s,
            // Port taken (another instance?): fall back to any free port.
            Err(_) if prefs.enabled => {
                cfg.port = 0;
                PeerService::start(cfg, handler(weak)).await?
            }
            Err(e) => return Err(e),
        };
        self.vfs.register(svc.connector());
        let svc = Arc::new(svc);
        if let Some(d) = self.discovery.lock().unwrap().as_ref() {
            if prefs.enabled {
                let id = svc.identity();
                let port = svc.local_addr().map(|a| a.port()).unwrap_or(PEER_PORT);
                let _ = d.advertise(Advertisement {
                    name: id.name,
                    port,
                    device_id: id.device_id,
                    txt: vec![],
                });
            } else {
                d.stop_advertising();
            }
            feed_directory(&svc, &d.devices());
        }
        *self.peer.write().await = Some(svc);
        if let Ok(status) = self.peer_status().await {
            self.events.emit(EngineEvent::Peer { status });
        }
        Ok(())
    }

    /// Start browsing for nearby devices (mDNS, Tailscale, SSDP, …).
    pub fn start_discovery(self: &Arc<Self>) {
        self.start_discovery_with(DiscoveryConfig::default());
    }

    pub fn start_discovery_with(self: &Arc<Self>, cfg: DiscoveryConfig) {
        let weak = Arc::downgrade(self);
        let d = Discovery::start(cfg, move |e| {
            let Some(engine) = weak.upgrade() else { return };
            if let DiscoveryEvent::DeviceUpdated(dev) = &e {
                if let Ok(guard) = engine.peer.try_read() {
                    if let Some(svc) = guard.as_ref() {
                        feed_directory(svc, std::slice::from_ref(dev));
                    }
                }
            }
            engine.events.emit(EngineEvent::Devices {
                devices: engine.devices(),
            });
        });
        *self.discovery.lock().unwrap() = Some(d);
    }

    /// Nearby devices found so far (empty until discovery runs).
    pub fn devices(&self) -> Vec<Device> {
        let mut devices = self
            .discovery
            .lock()
            .ok()
            .and_then(|d| d.as_ref().map(|d| d.devices()))
            .unwrap_or_default();
        if let Ok(guard) = self.peer.try_read() {
            if let Some(svc) = guard.as_ref() {
                add_paired_peers(&mut devices, &svc.trusted_devices());
            }
        }
        devices
    }

    /// Scan again now.
    pub fn refresh_discovery(&self) {
        if let Some(d) = self.discovery.lock().unwrap().as_ref() {
            d.refresh();
        }
    }

    /// Turn sharing on or off (restarts the service).
    pub async fn peer_set_enabled(self: &Arc<Self>, enabled: bool) -> Result<PeerStatus> {
        self.peer_prefs.lock().unwrap().enabled = enabled;
        self.save_peer_prefs();
        self.start_peer().await?;
        self.peer_status().await
    }

    pub async fn peer_set_shares(&self, shares: Vec<Share>) -> Result<PeerStatus> {
        self.peer
            .read()
            .await
            .as_ref()
            .ok_or_else(no_peer)?
            .set_shares(shares)?;
        self.peer_status().await
    }

    /// Trust devices signed in to the same Tailscale account without pairing.
    pub async fn peer_set_auto_trust(&self, on: bool) -> Result<PeerStatus> {
        self.peer_prefs.lock().unwrap().tailnet_auto_trust = on;
        self.save_peer_prefs();
        if let Some(svc) = self.peer.read().await.as_ref() {
            svc.set_tailnet_auto_trust(on);
        }
        self.peer_status().await
    }

    /// A fresh 6-digit pairing code for another device to type, "123 456".
    pub async fn peer_pair_code(&self) -> Result<String> {
        let guard = self.peer.read().await;
        let code = guard.as_ref().ok_or_else(no_peer)?.start_pairing().code;
        Ok(format_pair_code(&code))
    }

    /// Pair with the device at `address` (host or host:port) using the code
    /// it shows.
    pub async fn peer_pair(&self, address: &str, code: &str) -> Result<Paired> {
        let svc = self.peer.read().await.clone().ok_or_else(no_peer)?;
        let d = svc
            .pair(&peer_address(address), &code.replace(' ', ""))
            .await?;
        Ok(Paired {
            id: d.device_id,
            name: d.name,
        })
    }

    pub async fn peer_forget(&self, id: &str) -> Result<PeerStatus> {
        self.peer
            .read()
            .await
            .as_ref()
            .ok_or_else(no_peer)?
            .remove_trusted(id)?;
        self.peer_status().await
    }

    /// Offer local files to a device. `device` is a peer URI (`peer://<id>/`),
    /// a discovery id (`peer:<id>`) or a bare device id. Returns the offer id.
    pub async fn peer_send(&self, device: &str, uris: &[String]) -> Result<String> {
        let svc = self.peer.read().await.clone().ok_or_else(no_peer)?;
        let paths = uris
            .iter()
            .map(|u| {
                Location::parse(u)
                    .ok()
                    .and_then(|l| l.local_path().map(PathBuf::from))
                    .ok_or_else(|| CxError::Unsupported("sending remote files".into()))
            })
            .collect::<Result<Vec<_>>>()?;
        let target = match Location::parse(device)
            .ok()
            .and_then(|l| l.endpoint().cloned())
        {
            Some(ep) => ep.host,
            None => device.strip_prefix("peer:").unwrap_or(device).to_string(),
        };
        svc.send_offer(&target, paths).await
    }

    /// Accept (into `dest`, default Downloads) or decline an incoming offer.
    pub async fn peer_respond(
        &self,
        offer_id: &str,
        accept: bool,
        dest: Option<&str>,
    ) -> Result<()> {
        let svc = self.peer.read().await.clone().ok_or_else(no_peer)?;
        if accept {
            let dest = dest
                .map(Location::parse)
                .transpose()?
                .and_then(|l| l.local_path().map(PathBuf::from))
                .or_else(dirs::download_dir)
                .ok_or_else(|| CxError::InvalidLocation("no download folder".into()))?;
            svc.accept_offer(offer_id, dest)
        } else {
            svc.decline_offer(offer_id)
        }
    }
}

fn on_peer_event(engine: &Weak<Engine>, e: PeerEvent) {
    let Some(engine) = engine.upgrade() else {
        return;
    };
    match e {
        PeerEvent::IncomingOffer {
            offer_id,
            from,
            files,
            total,
        } => {
            engine.events.emit(EngineEvent::Offer {
                offer: IncomingOffer {
                    id: offer_id,
                    from: OfferPeer {
                        id: from.device_id,
                        name: from.name,
                    },
                    files,
                    total,
                },
            });
        }
        PeerEvent::OfferProgress {
            offer_id,
            direction,
            peer,
            state,
            bytes,
            total,
            file,
            error,
            ..
        } => {
            let state = name_of(&state);
            let outgoing = name_of(&direction) == "outgoing";
            let id = *engine
                .offer_jobs
                .lock()
                .unwrap()
                .entry(offer_id)
                .or_insert_with(|| {
                    let id = engine.tasks.next_id();
                    let label = format!("peer://{}/", peer.device_id);
                    engine.jobs.insert(JobView::new(
                        id,
                        if outgoing { "send" } else { "receive" },
                        vec![label],
                        None,
                    ));
                    id
                });
            engine.jobs.update(id, |j| {
                j.bytes_done = bytes;
                j.bytes_total = total;
                j.current = file;
                j.state = match state.as_str() {
                    "completed" => "done",
                    "failed" | "declined" => "failed",
                    "cancelled" => "cancelled",
                    "pending" => "queued",
                    _ => "running",
                }
                .into();
                if let Some(e) = error {
                    j.errors.push(cx_transfer::FileError {
                        uri: String::new(),
                        message: e,
                    });
                } else if state == "declined" {
                    j.errors.push(cx_transfer::FileError {
                        uri: String::new(),
                        message: format!("{} declined", peer.name),
                    });
                }
            });
        }
        PeerEvent::PairingCompleted { .. }
        | PeerEvent::PeerConnected { .. }
        | PeerEvent::PeerDisconnected { .. } => {
            let e2 = engine.clone();
            engine.rt.spawn(async move {
                if let Ok(status) = e2.peer_status().await {
                    e2.events.emit(EngineEvent::Peer { status });
                }
            });
        }
        PeerEvent::RemoteAccess(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_and_codes() {
        assert_eq!(peer_address("10.0.0.2"), "10.0.0.2:47470");
        assert_eq!(peer_address("host:5000"), "host:5000");
        assert_eq!(peer_address("fe80::1"), "fe80::1:47470");
        assert_eq!(format_pair_code("123456"), "123 456");
    }

    fn device(id: &str, ip: &str) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            kind: cx_discovery::DeviceKind::Pc,
            model: None,
            addresses: vec![ip.parse().unwrap()],
            hostname: None,
            sources: vec![cx_discovery::Source::Tailscale],
            tailnet: None,
            services: vec![],
            shares: vec![],
            last_seen: 0,
        }
    }

    #[test]
    fn paired_devices_get_a_peer_service() {
        let mut paired = cx_peer::TrustedDevice::new(&[7u8; 32], "hpc".into(), "windows".into());
        paired.last_addrs = vec!["[::ffff:100.85.18.101]:47470".into()];
        let mut devices = vec![device("ts:hpc", "100.85.18.101"), device("ts:other", "100.1.1.1")];
        add_paired_peers(&mut devices, &[paired.clone()]);
        let uri = format!("peer://{}/", paired.device_id);
        assert_eq!(devices[0].services.len(), 1);
        assert_eq!(devices[0].services[0].uri, uri);
        assert!(devices[1].services.is_empty());
        // Idempotent, and an advertised peer service is left alone.
        add_paired_peers(&mut devices, &[paired]);
        assert_eq!(devices[0].services.len(), 1);
    }
}
