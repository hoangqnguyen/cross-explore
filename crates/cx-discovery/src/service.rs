//! The running service: one background thread with its own small tokio
//! runtime, a task per source, and a fixed-rate flush that turns registry
//! changes into events.
//!
//! Running on a private thread (rather than the app's runtime) keeps
//! discovery's sockets, timers and child processes from ever competing with
//! listing or transfer work, and lets `start` be called from any context,
//! async or not.

use crate::model::{Advertisement, Device, DiscoveryConfig, DiscoveryEvent};
use crate::registry::Registry;
use crate::suggest::{suggestions, Suggestion};
use crate::util::now_ms;
use crate::{dnssd, mdns, netbios, probe, ssdp, tailscale, wsd};
use cx_core::{CxError, Result};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::{watch, Semaphore};
use tokio::time::Instant;

type EventSink = Arc<dyn Fn(DiscoveryEvent) + Send + Sync>;

/// How often registry changes are turned into events. Updates to a device
/// arriving faster than this are coalesced into one event, so no device is
/// reported more than ~4 times a second.
const FLUSH_EVERY: Duration = Duration::from_millis(250);

/// State shared by the source tasks and the public handle.
struct Hub {
    registry: Mutex<Registry>,
    /// Bumped by `refresh()`; every periodic source wakes on change.
    refresh: watch::Sender<u64>,
}

impl Hub {
    fn registry(&self) -> MutexGuard<'_, Registry> {
        // A panic in a source task must not take discovery down with it.
        self.registry.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Handle to the background discovery service. Discovery runs until
/// [`Discovery::stop`] is called or the handle is dropped.
///
/// Discovery only *finds* things: it listens to announcements, asks
/// "who's there" on the LAN, reads `tailscale status`, and opens TCP
/// connections to see which ports answer. It never signs in anywhere;
/// connecting is the user's decision.
pub struct Discovery {
    hub: Arc<Hub>,
    shutdown: watch::Sender<bool>,
    mdns: Option<mdns_sd::ServiceDaemon>,
    advertised: Mutex<Vec<String>>,
}

impl Discovery {
    /// Start every enabled source in the background. `on_event` is called
    /// from the discovery thread, at most once per device per 250 ms; keep
    /// it cheap (forward to the UI, don't block).
    pub fn start(cfg: DiscoveryConfig, on_event: impl Fn(DiscoveryEvent) + Send + Sync + 'static) -> Discovery {
        let hub = Arc::new(Hub { registry: Mutex::new(Registry::default()), refresh: watch::channel(0).0 });
        let (shutdown, shutdown_rx) = watch::channel(false);
        // Created even when browsing is off: it is also what advertises us.
        let mdns = mdns_sd::ServiceDaemon::new().ok();
        let sink: EventSink = Arc::new(on_event);
        let (hub2, daemon) = (hub.clone(), mdns.clone());
        let spawned = std::thread::Builder::new().name("cx-discovery".into()).spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
            rt.block_on(run(cfg, hub2, daemon, sink, shutdown_rx));
        });
        if spawned.is_err() {
            let _ = shutdown.send(true);
        }
        Discovery { hub, shutdown, mdns, advertised: Mutex::new(Vec::new()) }
    }

    /// Current merged device list (self first, then by name). At most one
    /// flush interval behind the event stream.
    pub fn devices(&self) -> Vec<Device> {
        self.hub.registry().devices().to_vec()
    }

    /// Ranked suggestions for the current devices; see [`suggestions`].
    pub fn suggestions(&self) -> Vec<Suggestion> {
        suggestions(self.hub.registry().devices())
    }

    /// Re-query everything now (the user pressed "Scan"): Tailscale, SSDP,
    /// WS-Discovery and `_device-info` run immediately and every known
    /// address is re-probed. mDNS browsing is continuous and needs no kick.
    pub fn refresh(&self) {
        self.hub.refresh.send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Announce our own peer service over mDNS as `_crossx._udp` and
    /// `_crossx._tcp`, replacing any previous advertisement.
    pub fn advertise(&self, service: Advertisement) -> Result<()> {
        let daemon = self.mdns.as_ref().ok_or_else(|| CxError::Unsupported("mDNS is not available on this system".into()))?;
        let infos = mdns::advertisement_infos(&service).map_err(|e| CxError::io("advertise", e))?;
        let mut advertised = self.advertised.lock().unwrap_or_else(|e| e.into_inner());
        for fullname in advertised.drain(..) {
            let _ = daemon.unregister(&fullname);
        }
        for info in infos {
            let fullname = info.get_fullname().to_string();
            daemon.register(info).map_err(|e| CxError::io("advertise", e))?;
            advertised.push(fullname);
        }
        Ok(())
    }

    /// Withdraw our advertisement (sends mDNS goodbye packets).
    pub fn stop_advertising(&self) {
        let Some(daemon) = &self.mdns else { return };
        for fullname in self.advertised.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
            let _ = daemon.unregister(&fullname);
        }
    }

    /// Stop all sources. Idempotent; also happens on drop.
    pub fn stop(&self) {
        if self.shutdown.send_replace(true) {
            return;
        }
        self.stop_advertising();
        if let Some(daemon) = &self.mdns {
            let _ = daemon.shutdown();
        }
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.stop();
    }
}

async fn run(cfg: DiscoveryConfig, hub: Arc<Hub>, daemon: Option<mdns_sd::ServiceDaemon>, sink: EventSink, mut shutdown: watch::Receiver<bool>) {
    tokio::spawn(flush_loop(hub.clone(), sink));
    if cfg.mdns {
        if let Some(daemon) = &daemon {
            for ty in mdns::BROWSE_TYPES {
                if let Ok(rx) = daemon.browse(&format!("{ty}.local.")) {
                    tokio::spawn(mdns_loop(hub.clone(), rx));
                }
            }
        }
        tokio::spawn(device_info_loop(hub.clone(), cfg.scan_interval));
    }
    if cfg.tailscale {
        tokio::spawn(tailscale_loop(hub.clone(), cfg.clone()));
    }
    if cfg.ssdp {
        tokio::spawn(ssdp_loop(hub.clone(), cfg.scan_interval));
    }
    if cfg.ws_discovery {
        tokio::spawn(wsd_loop(hub.clone(), cfg.scan_interval));
    }
    if cfg.probe || cfg.netbios {
        tokio::spawn(probe_loop(hub.clone(), cfg.clone()));
    }
    while !*shutdown.borrow_and_update() {
        if shutdown.changed().await.is_err() {
            break;
        }
    }
    // Returning ends `block_on`; dropping the runtime cancels every task.
}

/// Sleep for `interval`, or until someone calls `refresh()`.
async fn wait(interval: Duration, refresh: &mut watch::Receiver<u64>) {
    tokio::select! {
        _ = tokio::time::sleep(interval) => {}
        _ = refresh.changed() => {}
    }
}

fn ttl(interval: Duration, rounds: u32) -> i64 {
    (interval * rounds).as_millis() as i64
}

async fn flush_loop(hub: Arc<Hub>, sink: EventSink) {
    let mut tick = tokio::time::interval(FLUSH_EVERY);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let events = {
            let mut r = hub.registry();
            r.expire(now_ms());
            r.flush()
        };
        for e in events {
            sink(e);
        }
    }
}

async fn mdns_loop(hub: Arc<Hub>, rx: mdns_sd::Receiver<mdns_sd::ServiceEvent>) {
    while let Ok(event) = rx.recv_async().await {
        match event {
            mdns_sd::ServiceEvent::ServiceResolved(r) => {
                let o = mdns::observation(&mdns::Resolved::from_mdns(&r), now_ms());
                hub.registry().upsert(o);
            }
            mdns_sd::ServiceEvent::ServiceRemoved(_, fullname) => hub.registry().remove(crate::Source::Mdns, &fullname),
            _ => {}
        }
    }
}

async fn device_info_loop(hub: Arc<Hub>, every: Duration) {
    let mut refresh = hub.refresh.subscribe();
    loop {
        let models = dnssd::query_device_info(Duration::from_millis(1500)).await;
        let now = now_ms();
        {
            let mut r = hub.registry();
            for (instance, model) in models {
                r.upsert(mdns::device_info_observation(&instance, &model, now, ttl(every, 3)));
            }
        }
        wait(every, &mut refresh).await;
    }
}

async fn tailscale_loop(hub: Arc<Hub>, cfg: DiscoveryConfig) {
    let mut refresh = hub.refresh.subscribe();
    loop {
        if let Some(json) = tailscale::fetch_status(cfg.tailscale_binary.as_deref(), Duration::from_secs(5)).await {
            // A failed poll changes nothing; entries expire after a few
            // missed polls, so a crashed tailscaled eventually clears.
            if let Ok(obs) = tailscale::parse_status(&json, now_ms(), ttl(cfg.tailscale_interval, 4)) {
                hub.registry().replace(crate::Source::Tailscale, tailscale::KEY_PREFIX, obs);
            }
        }
        wait(cfg.tailscale_interval, &mut refresh).await;
    }
}

async fn ssdp_loop(hub: Arc<Hub>, every: Duration) {
    let mut refresh = hub.refresh.subscribe();
    // Descriptions rarely change; fetch each LOCATION once per half hour.
    let mut cache: HashMap<String, (Instant, Option<ssdp::Description>)> = HashMap::new();
    loop {
        let found = ssdp::search(Duration::from_secs(3)).await;
        for (ip, resp) in found {
            let fresh = cache.get(&resp.location).filter(|(at, _)| at.elapsed() < Duration::from_secs(1800)).map(|(_, d)| d.clone());
            let desc = match fresh {
                Some(d) => d,
                None => {
                    let d = ssdp::fetch_description(ip, &resp.location).await;
                    cache.insert(resp.location.clone(), (Instant::now(), d.clone()));
                    d
                }
            };
            let o = ssdp::observation(ip, &resp, desc.as_ref(), now_ms(), ttl(every, 3));
            hub.registry().upsert(o);
        }
        wait(every, &mut refresh).await;
    }
}

async fn wsd_loop(hub: Arc<Hub>, every: Duration) {
    let mut refresh = hub.refresh.subscribe();
    loop {
        let found = wsd::discover(Duration::from_secs(2)).await;
        let now = now_ms();
        {
            let mut r = hub.registry();
            for (from, m) in found.values() {
                r.upsert(wsd::observation(*from, m, now, ttl(every, 3)));
            }
        }
        wait(every, &mut refresh).await;
    }
}

/// Probe every address of every known device, once per `probe_interval`
/// (or right away on refresh), with bounded concurrency. New devices are
/// picked up within a few seconds of appearing.
async fn probe_loop(hub: Arc<Hub>, cfg: DiscoveryConfig) {
    let mut refresh = hub.refresh.subscribe();
    let limit = Arc::new(Semaphore::new(cfg.probe_concurrency.max(1)));
    let mut last: HashMap<IpAddr, Instant> = HashMap::new();
    let in_flight: Arc<Mutex<HashSet<IpAddr>>> = Arc::default();
    let probe_ttl = ttl(cfg.probe_interval, 3);
    loop {
        let targets: Vec<probe::Target> = hub.registry().devices().iter().flat_map(probe::targets).collect();
        for target in targets {
            let due = last.get(&target.ip).is_none_or(|at| at.elapsed() >= cfg.probe_interval);
            if !due || !in_flight.lock().unwrap_or_else(|e| e.into_inner()).insert(target.ip) {
                continue;
            }
            last.insert(target.ip, Instant::now());
            let (hub, limit, in_flight, cfg) = (hub.clone(), limit.clone(), in_flight.clone(), cfg.clone());
            tokio::spawn(async move {
                if cfg.probe {
                    let timeout = if target.tailnet { cfg.probe_timeout_tailnet } else { cfg.probe_timeout };
                    let mut o = probe::probe(&target, limit.clone(), timeout, probe_ttl).await;
                    if o.services.is_empty() && target.tailnet {
                        // The first packets to a relayed peer set up the
                        // path; by now a direct or warmed DERP route exists.
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        o = probe::probe(&target, limit.clone(), timeout, probe_ttl).await;
                    }
                    hub.registry().upsert(o);
                }
                if cfg.netbios && !target.tailnet {
                    let permit = limit.acquire_owned().await;
                    if let (Ok(_permit), Some(name)) = (permit, netbios::query(target.ip, Duration::from_millis(400)).await) {
                        hub.registry().upsert(netbios::observation(target.ip, &name, now_ms(), probe_ttl));
                    }
                }
                in_flight.lock().unwrap_or_else(|e| e.into_inner()).remove(&target.ip);
            });
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = refresh.changed() => last.clear(),
        }
    }
}
