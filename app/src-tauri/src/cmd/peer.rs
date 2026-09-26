//! Peer mode (other Cross Explore devices) and nearby-device discovery.

use super::AppState;
use crate::jobs::UiJob;
use crate::state::App;
use cx_core::{CxError, Location, Result};
use cx_discovery::{Advertisement, Device, Discovery, DiscoveryConfig, DiscoveryEvent};
use cx_peer::{PeerConfig, PeerEvent, PeerService, Share};
use serde::Serialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

pub const PEER_PORT: u16 = 47470;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedView {
    id: String,
    name: String,
    added_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerStatus {
    enabled: bool,
    device_id: String,
    name: String,
    port: u16,
    shares: Vec<Share>,
    trusted: Vec<TrustedView>,
    tailnet_auto_trust: bool,
}

async fn status(app: &App) -> Result<PeerStatus> {
    let guard = app.peer.read().await;
    let svc = guard.as_ref().ok_or_else(|| CxError::Unsupported("peer mode".into()))?;
    let id = svc.identity();
    let prefs = app.peer_prefs.lock().unwrap().clone();
    Ok(PeerStatus {
        enabled: prefs.enabled,
        device_id: id.device_id,
        name: id.name,
        port: svc.local_addr().map(|a| a.port()).unwrap_or(PEER_PORT),
        shares: svc.shares(),
        trusted: svc.trusted_devices().into_iter().map(|t| TrustedView { id: t.device_id, name: t.name, added_at: t.added_at }).collect(),
        tailnet_auto_trust: prefs.tailnet_auto_trust,
    })
}

/// (Re)start the peer service. It always runs as a client so other devices
/// can be browsed; it only listens (and advertises itself) when sharing is on.
pub async fn start_peer(app: Arc<App>) -> Result<()> {
    if let Some(old) = app.peer.write().await.take() {
        old.stop().await;
    }
    let prefs = app.peer_prefs.lock().unwrap().clone();
    let mut cfg = PeerConfig::new(app.data_dir.join("peer"));
    cfg.listen = prefs.enabled;
    cfg.port = if prefs.enabled { PEER_PORT } else { 0 };
    cfg.tailnet_auto_trust = prefs.tailnet_auto_trust;
    let events_app = app.clone();
    let svc = match PeerService::start(cfg.clone(), Arc::new(move |e| on_peer_event(&events_app, e))).await {
        Ok(s) => s,
        // Port taken (another instance?): fall back to any free port.
        Err(_) if prefs.enabled => {
            cfg.port = 0;
            let events_app = app.clone();
            PeerService::start(cfg, Arc::new(move |e| on_peer_event(&events_app, e))).await?
        }
        Err(e) => return Err(e),
    };
    app.vfs.register(svc.connector());
    let svc = Arc::new(svc);
    if let Some(d) = app.discovery.lock().unwrap().as_ref() {
        if prefs.enabled {
            let id = svc.identity();
            let port = svc.local_addr().map(|a| a.port()).unwrap_or(PEER_PORT);
            let _ = d.advertise(Advertisement { name: id.name, port, device_id: id.device_id, txt: vec![] });
        } else {
            d.stop_advertising();
        }
        feed_directory(&svc, &d.devices());
    }
    *app.peer.write().await = Some(svc);
    if let Ok(s) = status(&app).await {
        app.events.emit("peer", serde_json::json!({ "status": s }));
    }
    Ok(())
}

/// Tell the peer client where discovered Cross Explore devices are.
fn feed_directory(svc: &PeerService, devices: &[Device]) {
    for d in devices {
        for s in d.services.iter().filter(|s| s.scheme == cx_core::Scheme::Peer) {
            let Ok(loc) = Location::parse(&s.uri) else { continue };
            let Some(ep) = loc.endpoint() else { continue };
            let addrs: Vec<SocketAddr> = d.addresses.iter().map(|ip| SocketAddr::new(*ip, s.port)).collect();
            if !addrs.is_empty() {
                svc.directory().set(&ep.host, addrs);
            }
        }
    }
}

pub fn start_discovery(app: Arc<App>) {
    let events_app = app.clone();
    let d = Discovery::start(DiscoveryConfig::default(), move |e| {
        let app = &events_app;
        if let DiscoveryEvent::DeviceUpdated(dev) = &e {
            if let Ok(guard) = app.peer.try_read() {
                if let Some(svc) = guard.as_ref() {
                    feed_directory(svc, std::slice::from_ref(dev));
                }
            }
        }
        let devices = app.discovery.lock().ok().and_then(|d| d.as_ref().map(|d| d.devices())).unwrap_or_default();
        app.events.emit("devices", serde_json::json!({ "devices": devices }));
    });
    *app.discovery.lock().unwrap() = Some(d);
}

fn offer_jobs() -> &'static Mutex<HashMap<String, u64>> {
    static MAP: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    MAP.get_or_init(Default::default)
}

fn on_peer_event(app: &Arc<App>, e: PeerEvent) {
    match e {
        PeerEvent::IncomingOffer { offer_id, from, files, total } => {
            app.events.emit("offer", serde_json::json!({ "offer": { "id": offer_id, "from": { "id": from.device_id, "name": from.name }, "files": files, "total": total } }));
        }
        PeerEvent::OfferProgress { offer_id, direction, peer, state, bytes, total, file, error, .. } => {
            let state = serde_json::to_value(&state).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
            let outgoing = serde_json::to_value(&direction).ok().and_then(|v| v.as_str().map(|s| s == "outgoing")).unwrap_or(false);
            let id = *offer_jobs().lock().unwrap().entry(offer_id).or_insert_with(|| {
                let (id, _) = app.task();
                app.finish_task(id);
                let label = format!("peer://{}/", peer.device_id);
                app.jobs.insert(UiJob::new(id, if outgoing { "send" } else { "receive" }, vec![label], None));
                id
            });
            app.jobs.update(id, |j| {
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
                    j.errors.push(cx_transfer::FileError { uri: String::new(), message: e });
                } else if state == "declined" {
                    j.errors.push(cx_transfer::FileError { uri: String::new(), message: format!("{} declined", peer.name) });
                }
            });
        }
        PeerEvent::PairingCompleted { .. } | PeerEvent::PeerConnected { .. } | PeerEvent::PeerDisconnected { .. } => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(s) = status(&app).await {
                    app.events.emit("peer", serde_json::json!({ "status": s }));
                }
            });
        }
        PeerEvent::RemoteAccess(_) => {}
    }
}

#[tauri::command]
pub async fn peer_status(app: AppState<'_>) -> Result<PeerStatus> {
    status(&app).await
}

#[tauri::command]
pub async fn peer_set_enabled(enabled: bool, app: AppState<'_>) -> Result<PeerStatus> {
    app.peer_prefs.lock().unwrap().enabled = enabled;
    app.save_peer_prefs();
    start_peer(app.inner().clone()).await?;
    status(&app).await
}

#[tauri::command]
pub async fn peer_set_shares(shares: Vec<Share>, app: AppState<'_>) -> Result<PeerStatus> {
    app.peer.read().await.as_ref().ok_or_else(|| CxError::Unsupported("peer mode".into()))?.set_shares(shares)?;
    status(&app).await
}

#[tauri::command]
pub async fn peer_set_auto_trust(on: bool, app: AppState<'_>) -> Result<PeerStatus> {
    app.peer_prefs.lock().unwrap().tailnet_auto_trust = on;
    app.save_peer_prefs();
    if let Some(svc) = app.peer.read().await.as_ref() {
        svc.set_tailnet_auto_trust(on);
    }
    status(&app).await
}

#[tauri::command]
pub async fn peer_pair_code(app: AppState<'_>) -> Result<String> {
    let guard = app.peer.read().await;
    let code = guard.as_ref().ok_or_else(|| CxError::Unsupported("peer mode".into()))?.start_pairing().code;
    Ok(format!("{} {}", &code[..3.min(code.len())], &code[3.min(code.len())..]))
}

#[derive(Serialize)]
pub struct Paired {
    id: String,
    name: String,
}

#[tauri::command]
pub async fn peer_pair(address: String, code: String, app: AppState<'_>) -> Result<Paired> {
    let svc = app.peer.read().await.clone().ok_or_else(|| CxError::Unsupported("peer mode".into()))?;
    let addr = if address.contains(':') && !address.contains("::") { address } else { format!("{address}:{PEER_PORT}") };
    let d = svc.pair(&addr, &code.replace(' ', "")).await?;
    Ok(Paired { id: d.device_id, name: d.name })
}

#[tauri::command]
pub async fn peer_forget(id: String, app: AppState<'_>) -> Result<PeerStatus> {
    app.peer.read().await.as_ref().ok_or_else(|| CxError::Unsupported("peer mode".into()))?.remove_trusted(&id)?;
    status(&app).await
}

#[tauri::command]
pub async fn peer_send(device: String, uris: Vec<String>, app: AppState<'_>) -> Result<String> {
    let svc = app.peer.read().await.clone().ok_or_else(|| CxError::Unsupported("peer mode".into()))?;
    let paths = uris
        .iter()
        .map(|u| Location::parse(u).ok().and_then(|l| l.local_path().map(PathBuf::from)).ok_or_else(|| CxError::Unsupported("sending remote files".into())))
        .collect::<Result<Vec<_>>>()?;
    // Accept a peer URI (peer://<id>/), a discovery id ("peer:<id>") or a bare id.
    let target = match Location::parse(&device).ok().and_then(|l| l.endpoint().cloned()) {
        Some(ep) => ep.host,
        None => device.strip_prefix("peer:").unwrap_or(&device).to_string(),
    };
    svc.send_offer(&target, paths).await
}

#[tauri::command]
pub async fn peer_respond(offer_id: String, accept: bool, dest: Option<String>, app: AppState<'_>) -> Result<()> {
    let svc = app.peer.read().await.clone().ok_or_else(|| CxError::Unsupported("peer mode".into()))?;
    if accept {
        let dest = dest.as_deref().map(Location::parse).transpose()?.and_then(|l| l.local_path().map(PathBuf::from)).or_else(dirs::download_dir).ok_or_else(|| CxError::InvalidLocation("no download folder".into()))?;
        svc.accept_offer(&offer_id, dest)
    } else {
        svc.decline_offer(&offer_id)
    }
}

#[tauri::command]
pub fn discovery_devices(app: AppState<'_>) -> Vec<Device> {
    app.discovery.lock().unwrap().as_ref().map(|d| d.devices()).unwrap_or_default()
}

#[tauri::command]
pub fn discovery_refresh(app: AppState<'_>) {
    if let Some(d) = app.discovery.lock().unwrap().as_ref() {
        d.refresh();
    }
}
