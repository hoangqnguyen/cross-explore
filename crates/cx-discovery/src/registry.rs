//! The merge engine.
//!
//! Sources never edit devices directly. Each one reports *observations*
//! ("mDNS says host X offers SMB", "Tailscale says node N is online at
//! these IPs"), keyed by `(source, key)` so that a source can replace or
//! withdraw exactly what it said before. Devices are then derived by
//! clustering observations that share an address, a host name or a friendly
//! name, and the result is diffed against what the UI was last told.
//!
//! Recomputing from scratch keeps merging and splitting symmetric: when an
//! mDNS record goes away, whatever it bridged simply falls apart again, and
//! nothing has to remember which source contributed which field.

use crate::kind::{self, KindGuess};
use crate::model::{Device, DeviceKind, DiscoveryEvent, Service, ShareHint, Source, TailnetInfo};
use crate::util::{self, address_rank, is_generic_name, is_linkable_ip, normalize_hostname, normalize_name};
use cx_core::Scheme;
use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;

/// How strongly an id identifies a device; the merged device takes the id
/// of its strongest observation.
pub(crate) mod rank {
    pub const WSD: u8 = 1;
    pub const SSDP: u8 = 2;
    pub const MDNS: u8 = 3;
    pub const PEER: u8 = 4;
    pub const TAILSCALE: u8 = 5;
}

/// How much to trust a friendly name.
pub(crate) mod name_rank {
    pub const NETBIOS: u8 = 4;
    pub const MDNS_INSTANCE: u8 = 5;
    pub const SSDP: u8 = 6;
    pub const TAILSCALE: u8 = 8;
    pub const PEER: u8 = 10;
}

/// A service as a source saw it; the URI is built from `host`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedService {
    pub scheme: Scheme,
    pub port: u16,
    /// What goes in the URI authority: a DNS name, an IP, or a peer id.
    pub host: String,
    /// Path segments (e.g. a WebDAV root from TXT `path=`).
    pub path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ObservedShare {
    pub name: String,
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
}

/// One source's statement about one thing it saw.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Observation {
    pub source: Source,
    /// Unique within the source: mDNS full name, Tailscale node id, IP…
    pub key: String,
    /// False for evidence that only decorates a device (port probes,
    /// NetBIOS names, `_device-info` models): a cluster made only of those
    /// is not a device, so a stale probe cannot keep a vanished host alive.
    pub standalone: bool,
    pub identity: Option<(u8, String)>,
    pub name: Option<(u8, String)>,
    pub kind: Option<KindGuess>,
    pub model: Option<String>,
    pub addresses: Vec<IpAddr>,
    /// DNS names (MagicDNS, `.local`), already normalized.
    pub hostnames: Vec<String>,
    /// Friendly names that may be matched across sources (normalized).
    pub link_names: Vec<String>,
    pub tailnet: Option<TailnetInfo>,
    pub services: Vec<ObservedService>,
    pub shares: Vec<ObservedShare>,
    /// Unix ms of the evidence.
    pub seen: i64,
    /// Unix ms after which the observation is dropped unless refreshed.
    pub expires: i64,
}

impl Observation {
    pub fn new(source: Source, key: impl Into<String>, seen: i64) -> Self {
        Observation {
            source,
            key: key.into(),
            standalone: true,
            identity: None,
            name: None,
            kind: None,
            model: None,
            addresses: Vec::new(),
            hostnames: Vec::new(),
            link_names: Vec::new(),
            tailnet: None,
            services: Vec::new(),
            shares: Vec::new(),
            seen,
            expires: i64::MAX,
        }
    }

    /// Equal apart from timestamps: refreshing an unchanged record should
    /// not wake the UI.
    fn same_content(&self, other: &Observation) -> bool {
        let strip = |o: &Observation| Observation { seen: 0, expires: 0, ..o.clone() };
        strip(self) == strip(other)
    }

    /// Keys that, when shared, mean "same machine": addresses, DNS names and
    /// explicit ids.
    fn strong_keys(&self) -> impl Iterator<Item = String> + '_ {
        let ips = self.addresses.iter().filter(|ip| is_linkable_ip(ip)).map(|ip| format!("ip:{ip}"));
        let hosts = self.hostnames.iter().map(|h| format!("host:{h}"));
        let id = self.identity.iter().map(|(_, id)| format!("id:{id}"));
        ips.chain(hosts).chain(id)
    }

    /// Friendly names: "probably the same machine" unless that would fuse
    /// two tailnet nodes (a tailnet can hold two machines called "r38").
    fn name_keys(&self) -> impl Iterator<Item = String> + '_ {
        self.link_names.iter().filter(|n| !is_generic_name(n)).map(|n| format!("name:{n}"))
    }
}

/// All observations plus the device list last reported to the UI.
#[derive(Default)]
pub(crate) struct Registry {
    obs: HashMap<(Source, String), Observation>,
    dirty: bool,
    current: Vec<Device>,
    emitted: HashMap<String, Device>,
}

impl Registry {
    pub fn upsert(&mut self, o: Observation) {
        let k = (o.source, o.key.clone());
        match self.obs.get_mut(&k) {
            Some(existing) if existing.same_content(&o) => {
                existing.seen = existing.seen.max(o.seen);
                existing.expires = o.expires;
            }
            _ => {
                self.obs.insert(k, o);
                self.dirty = true;
            }
        }
    }

    pub fn remove(&mut self, source: Source, key: &str) {
        if self.obs.remove(&(source, key.to_string())).is_some() {
            self.dirty = true;
        }
    }

    /// Replace everything a source said whose key starts with `prefix`
    /// (used by pollers that see the whole picture each time, e.g. Tailscale).
    pub fn replace(&mut self, source: Source, prefix: &str, fresh: Vec<Observation>) {
        let keep: BTreeSet<String> = fresh.iter().map(|o| o.key.clone()).collect();
        let before = self.obs.len();
        self.obs.retain(|(s, k), _| *s != source || !k.starts_with(prefix) || keep.contains(k));
        if self.obs.len() != before {
            self.dirty = true;
        }
        for o in fresh {
            self.upsert(o);
        }
    }

    pub fn expire(&mut self, now: i64) {
        let before = self.obs.len();
        self.obs.retain(|_, o| o.expires > now);
        if self.obs.len() != before {
            self.dirty = true;
        }
    }

    pub fn devices(&self) -> &[Device] {
        &self.current
    }

    /// Rebuild devices if anything changed and return what the UI must hear.
    /// Called on a fixed tick, which is what coalesces bursts of updates.
    pub fn flush(&mut self) -> Vec<DiscoveryEvent> {
        if !self.dirty {
            return Vec::new();
        }
        self.dirty = false;
        let observations: Vec<&Observation> = self.obs.values().collect();
        self.current = build_devices(&observations);
        let mut events = Vec::new();
        let mut next = HashMap::with_capacity(self.current.len());
        for d in &self.current {
            let changed = match self.emitted.get(&d.id) {
                Some(old) => !same_ignoring_seen(old, d),
                None => true,
            };
            if changed {
                events.push(DiscoveryEvent::DeviceUpdated(d.clone()));
            }
            next.insert(d.id.clone(), d.clone());
        }
        let mut lost: Vec<&String> = self.emitted.keys().filter(|id| !next.contains_key(*id)).collect();
        lost.sort();
        // Report losses first so a re-keyed device never exists twice in the UI.
        let lost_events: Vec<DiscoveryEvent> = lost.into_iter().map(|id| DiscoveryEvent::DeviceLost { id: id.clone() }).collect();
        self.emitted = next;
        lost_events.into_iter().chain(events).collect()
    }
}

fn same_ignoring_seen(a: &Device, b: &Device) -> bool {
    Device { last_seen: 0, ..a.clone() } == Device { last_seen: 0, ..b.clone() }
}

/// Union-find over observations linked by shared keys: strong keys first,
/// then friendly names where they don't join two Tailscale nodes. Input
/// order must be deterministic (see `build_devices`).
pub(crate) fn cluster<'a>(obs: &[&'a Observation]) -> Vec<Vec<&'a Observation>> {
    struct Sets {
        parent: Vec<usize>,
        tailnet: Vec<bool>,
    }
    impl Sets {
        fn find(&mut self, mut i: usize) -> usize {
            while self.parent[i] != i {
                self.parent[i] = self.parent[self.parent[i]];
                i = self.parent[i];
            }
            i
        }
        fn union(&mut self, i: usize, j: usize, strong: bool) {
            let (a, b) = (self.find(i), self.find(j));
            if a == b || (!strong && self.tailnet[a] && self.tailnet[b]) {
                return;
            }
            let (lo, hi) = (a.min(b), a.max(b));
            self.parent[hi] = lo;
            self.tailnet[lo] |= self.tailnet[hi];
        }
    }
    let mut sets = Sets { parent: (0..obs.len()).collect(), tailnet: obs.iter().map(|o| o.source == Source::Tailscale).collect() };
    for strong in [true, false] {
        let mut owner: HashMap<String, usize> = HashMap::new();
        for (i, o) in obs.iter().enumerate() {
            let keys: Vec<String> = if strong { o.strong_keys().collect() } else { o.name_keys().collect() };
            for key in keys {
                match owner.get(&key) {
                    Some(&j) => sets.union(i, j, strong),
                    None => {
                        owner.insert(key, i);
                    }
                }
            }
        }
    }
    let mut groups: Vec<Vec<&Observation>> = Vec::new();
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for (i, o) in obs.iter().enumerate() {
        let root = sets.find(i);
        let idx = *slot.entry(root).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[idx].push(*o);
    }
    groups
}

pub(crate) fn build_devices(obs: &[&Observation]) -> Vec<Device> {
    let mut obs = obs.to_vec();
    obs.sort_by(|a, b| (a.source, &a.key).cmp(&(b.source, &b.key)));
    let mut devices: Vec<Device> = cluster(&obs).into_iter().filter_map(|c| merge(&c)).collect();
    devices.sort_by(|a, b| (!a.is_self(), a.name.to_lowercase(), &a.id).cmp(&(!b.is_self(), b.name.to_lowercase(), &b.id)));
    devices
}

/// Fold one cluster into a device. Deterministic regardless of the order
/// observations arrived in, so rebuilding never produces spurious events.
pub(crate) fn merge(cluster: &[&Observation]) -> Option<Device> {
    if !cluster.iter().any(|o| o.standalone) {
        return None;
    }
    let mut obs = cluster.to_vec();
    obs.sort_by(|a, b| (a.source, &a.key).cmp(&(b.source, &b.key)));

    let tailnet = obs.iter().find_map(|o| o.tailnet.clone());
    let mut addresses: Vec<IpAddr> = obs.iter().flat_map(|o| o.addresses.iter().copied()).filter(|ip| !ip.is_loopback() && !ip.is_unspecified()).collect::<BTreeSet<_>>().into_iter().collect();
    addresses.sort_by_key(|ip| (address_rank(ip), *ip));

    let hostnames: BTreeSet<&String> = obs.iter().flat_map(|o| o.hostnames.iter()).collect();
    let hostname = tailnet
        .as_ref()
        .and_then(|t| t.dns_name.clone())
        .or_else(|| hostnames.iter().find(|h| h.ends_with(".local")).map(|h| h.to_string()))
        .or_else(|| hostnames.iter().next().map(|h| h.to_string()));

    let id = obs
        .iter()
        .filter_map(|o| o.identity.as_ref())
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)))
        .map(|(_, id)| id.clone())
        .or_else(|| hostnames.iter().find(|h| h.ends_with(".local")).map(|h| format!("mdns:{h}")))
        .or_else(|| addresses.first().map(|ip| format!("ip:{ip}")))?;

    let name = obs
        .iter()
        .filter_map(|o| o.name.as_ref())
        .filter(|(_, n)| !n.trim().is_empty())
        .max_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)))
        .map(|(_, n)| n.trim().to_string())
        .or_else(|| hostname.as_deref().map(|h| util::host_label(h).to_string()))
        .or_else(|| addresses.first().map(|ip| ip.to_string()))
        .unwrap_or_else(|| id.clone());

    let model = obs.iter().find_map(|o| o.model.clone());
    let kind = kind::best(obs.iter().map(|o| o.kind).chain([kind::from_name(&name)])).map(|(_, k)| k).unwrap_or(DeviceKind::Unknown);
    // A decorating observation that found nothing (a probe with every port
    // closed) is not worth crediting, nor an event.
    let contributed = |o: &&&Observation| o.standalone || !o.services.is_empty() || !o.shares.is_empty() || o.name.is_some() || o.model.is_some();
    let sources: Vec<Source> = obs.iter().filter(contributed).map(|o| o.source).collect::<BTreeSet<_>>().into_iter().collect();

    let mut services: Vec<Service> = Vec::new();
    for o in &obs {
        for s in &o.services {
            let path: Vec<&str> = s.path.iter().map(String::as_str).collect();
            let uri = util::service_uri(s.scheme, &s.host, s.port, &path);
            if !services.iter().any(|x| x.uri == uri) {
                let label = util::service_label(s.scheme, &s.host);
                services.push(Service { scheme: s.scheme, port: s.port, uri, label, source: o.source });
            }
        }
    }
    services.sort_by_key(|s| (scheme_order(s.scheme), s.uri.clone()));

    let mut shares: Vec<ShareHint> = Vec::new();
    for o in &obs {
        for s in &o.shares {
            let uri = util::service_uri(s.scheme, &s.host, s.port, &[&s.name]);
            if !shares.iter().any(|x| x.uri == uri) {
                shares.push(ShareHint { name: s.name.clone(), uri, source: o.source });
            }
        }
    }
    shares.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let last_seen = obs.iter().map(|o| o.seen).max().unwrap_or(0);
    Some(Device { id, name, kind, model, addresses, hostname, sources, tailnet, services, shares, last_seen })
}

/// Services most people want first.
pub(crate) fn scheme_order(s: Scheme) -> u8 {
    match s {
        Scheme::Peer => 0,
        Scheme::Smb => 1,
        Scheme::Sftp => 2,
        Scheme::Davs => 3,
        Scheme::Dav => 4,
        Scheme::Ftps => 5,
        Scheme::Ftp => 6,
        Scheme::S3 => 7,
        Scheme::GDrive | Scheme::Dropbox | Scheme::OneDrive => 8,
    }
}

/// Helper for sources: normalized hostname list without empties.
pub(crate) fn hostnames(names: &[&str]) -> Vec<String> {
    names.iter().map(|h| normalize_hostname(h)).filter(|h| !h.is_empty()).collect()
}

/// Helper for sources: normalized, non-generic link names.
pub(crate) fn link_names(names: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = names.iter().map(|n| normalize_name(n)).filter(|n| !is_generic_name(n)).collect();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn mdns_smb(host: &str, instance: &str, addr: &str) -> Observation {
        let mut o = Observation::new(Source::Mdns, format!("{instance}._smb._tcp.local."), 1000);
        o.identity = Some((rank::MDNS, format!("mdns:{}", normalize_hostname(host))));
        o.name = Some((name_rank::MDNS_INSTANCE, instance.into()));
        o.addresses = vec![ip(addr)];
        o.hostnames = hostnames(&[host]);
        o.link_names = link_names(&[instance]);
        o.services = vec![ObservedService { scheme: Scheme::Smb, port: 445, host: normalize_hostname(host), path: vec![] }];
        o
    }

    fn tailscale(id: &str, host_name: &str, dns: &str, ips: &[&str], online: bool) -> Observation {
        let mut o = Observation::new(Source::Tailscale, format!("node:{id}"), 2000);
        o.identity = Some((rank::TAILSCALE, format!("ts:{id}")));
        o.name = Some((name_rank::TAILSCALE, host_name.into()));
        o.addresses = ips.iter().map(|s| ip(s)).collect();
        o.hostnames = hostnames(&[dns]);
        o.link_names = link_names(&[host_name]);
        o.tailnet = Some(TailnetInfo { online, os: "macOS".into(), owner: None, shared_by: None, is_self: false, dns_name: Some(normalize_hostname(dns)), last_seen: None, tags: vec![] });
        o
    }

    fn probe(addr: &str, host: &str, schemes: &[Scheme]) -> Observation {
        let mut o = Observation::new(Source::Probe, format!("probe:{addr}"), 3000);
        o.standalone = false;
        o.addresses = vec![ip(addr)];
        o.services = schemes.iter().map(|s| ObservedService { scheme: *s, port: s.default_port(), host: host.into(), path: vec![] }).collect();
        o
    }

    #[test]
    fn mdns_records_for_one_host_merge() {
        let smb = mdns_smb("NAS.local.", "nas", "192.168.1.10");
        let mut adisk = Observation::new(Source::Mdns, "nas._adisk._tcp.local.", 1500);
        adisk.hostnames = hostnames(&["nas.local."]);
        adisk.shares = vec![ObservedShare { name: "Time Machine".into(), scheme: Scheme::Smb, host: "nas.local".into(), port: 445 }];
        let devices = build_devices(&[&smb, &adisk]);
        assert_eq!(devices.len(), 1);
        let d = &devices[0];
        assert_eq!(d.id, "mdns:nas.local");
        assert_eq!(d.hostname.as_deref(), Some("nas.local"));
        assert_eq!(d.services[0].uri, "smb://nas.local/");
        assert_eq!(d.shares[0].uri, "smb://nas.local/Time%20Machine/");
        assert_eq!(d.last_seen, 1500);
        assert_eq!(d.kind, DeviceKind::Nas, "name heuristic");
    }

    #[test]
    fn tailscale_and_mdns_merge_by_name_and_ip() {
        let lan = mdns_smb("Alexs-MacBook-Pro.local.", "Alex’s MacBook Pro", "192.168.1.76");
        let mut ts = tailscale("n1", "Alex's MacBook Pro", "alexs-macbook-pro-1.x.ts.net.", &["100.101.1.3"], true);
        let devices = build_devices(&[&lan, &ts]);
        assert_eq!(devices.len(), 1, "linked by friendly name");
        assert_eq!(devices[0].id, "ts:n1");
        assert_eq!(devices[0].hostname.as_deref(), Some("alexs-macbook-pro-1.x.ts.net"));
        assert_eq!(devices[0].addresses, vec![ip("192.168.1.76"), ip("100.101.1.3")]);

        ts.link_names.clear();
        ts.addresses.push(ip("192.168.1.76"));
        assert_eq!(build_devices(&[&lan, &ts]).len(), 1, "linked by LAN address");
    }

    #[test]
    fn unrelated_hosts_stay_apart() {
        let mut a = mdns_smb("a.local.", "Alpha", "192.168.1.2");
        let mut b = mdns_smb("b.local.", "Beta", "192.168.1.3");
        a.addresses.push(ip("172.17.0.1"));
        b.addresses.push(ip("172.17.0.1"));
        assert_eq!(build_devices(&[&a, &b]).len(), 2, "shared docker bridge address must not link");
        let generic1 = { let mut o = mdns_smb("x.local.", "localhost", "10.0.0.1"); o.link_names = link_names(&["localhost"]); o };
        let generic2 = { let mut o = mdns_smb("y.local.", "localhost", "10.0.0.2"); o.link_names = link_names(&["localhost"]); o };
        assert_eq!(build_devices(&[&generic1, &generic2]).len(), 2);
    }

    #[test]
    fn same_named_tailnet_nodes_stay_apart() {
        let win = tailscale("w", "r38", "r38-2.x.ts.net.", &["100.101.1.4"], false);
        let linux = tailscale("l", "r38", "r38.x.ts.net.", &["100.101.1.5"], false);
        let lan = mdns_smb("r38.local.", "r38", "192.168.1.38");
        let devices = build_devices(&[&lan, &win, &linux]);
        assert_eq!(devices.len(), 2, "{devices:#?}");
        // Deterministic whichever order the registry hands them over.
        assert_eq!(build_devices(&[&linux, &win, &lan]), devices);
    }

    #[test]
    fn probes_attach_but_never_create() {
        let ts = tailscale("n2", "hpc", "hpc.x.ts.net.", &["100.101.1.1"], true);
        let p = probe("100.101.1.1", "hpc.x.ts.net", &[Scheme::Smb, Scheme::Sftp]);
        let orphan = probe("192.168.1.99", "192.168.1.99", &[Scheme::Smb]);
        let devices = build_devices(&[&ts, &p, &orphan]);
        assert_eq!(devices.len(), 1);
        let uris: Vec<&str> = devices[0].services.iter().map(|s| s.uri.as_str()).collect();
        assert_eq!(uris, ["smb://hpc.x.ts.net/", "sftp://hpc.x.ts.net/"]);
        assert_eq!(devices[0].services[0].label, "SMB via Tailscale");
        assert_eq!(devices[0].sources, vec![Source::Tailscale, Source::Probe]);
    }

    #[test]
    fn flush_emits_updates_losses_and_coalesces() {
        let mut r = Registry::default();
        r.upsert(mdns_smb("nas.local.", "nas", "192.168.1.10"));
        r.upsert(mdns_smb("pc.local.", "pc", "192.168.1.11"));
        let ev = r.flush();
        assert_eq!(ev.len(), 2);
        assert!(r.flush().is_empty(), "nothing changed");

        // Same content with a newer timestamp is not an event.
        let mut again = mdns_smb("nas.local.", "nas", "192.168.1.10");
        again.seen = 99_999;
        r.upsert(again);
        assert!(r.flush().is_empty());

        // Several changes to one device between ticks → one event.
        r.upsert(probe("192.168.1.10", "nas.local", &[Scheme::Sftp]));
        r.upsert(probe("192.168.1.10", "nas.local", &[Scheme::Sftp, Scheme::Ftp]));
        let ev = r.flush();
        assert_eq!(ev.len(), 1);
        let DiscoveryEvent::DeviceUpdated(d) = &ev[0] else { panic!() };
        assert_eq!(d.services.len(), 3);

        r.remove(Source::Mdns, "pc._smb._tcp.local.");
        assert_eq!(r.flush(), vec![DiscoveryEvent::DeviceLost { id: "mdns:pc.local".into() }]);
    }

    #[test]
    fn stronger_identity_rekeys_device() {
        let mut r = Registry::default();
        r.upsert(mdns_smb("hpc.local.", "hpc", "192.168.1.20"));
        r.flush();
        let mut ts = tailscale("n9", "hpc", "hpc.x.ts.net.", &["100.101.1.1"], true);
        ts.addresses.push(ip("192.168.1.20"));
        r.upsert(ts);
        let ev = r.flush();
        assert_eq!(ev[0], DiscoveryEvent::DeviceLost { id: "mdns:hpc.local".into() });
        assert!(matches!(&ev[1], DiscoveryEvent::DeviceUpdated(d) if d.id == "ts:n9"));
    }

    #[test]
    fn replace_and_expire() {
        let mut r = Registry::default();
        let mut a = tailscale("a", "a", "a.x.ts.net.", &["100.64.0.1"], true);
        a.expires = 5000;
        r.replace(Source::Tailscale, "node:", vec![a, tailscale("b", "b", "b.x.ts.net.", &["100.64.0.2"], true)]);
        assert_eq!(r.flush().len(), 2);
        r.replace(Source::Tailscale, "node:", vec![tailscale("b", "b", "b.x.ts.net.", &["100.64.0.2"], true)]);
        assert_eq!(r.flush(), vec![DiscoveryEvent::DeviceLost { id: "ts:a".into() }]);
        let mut c = tailscale("c", "c", "c.x.ts.net.", &["100.64.0.3"], true);
        c.expires = 5000;
        r.upsert(c);
        r.flush();
        r.expire(6000);
        assert_eq!(r.flush(), vec![DiscoveryEvent::DeviceLost { id: "ts:c".into() }]);
    }

    #[test]
    fn event_json_shape() {
        let d = merge(&[&mdns_smb("nas.local.", "nas", "192.168.1.10")]).unwrap();
        let v = serde_json::to_value(DiscoveryEvent::DeviceUpdated(d)).unwrap();
        assert_eq!(v["type"], "deviceUpdated");
        assert_eq!(v["id"], "mdns:nas.local");
        assert_eq!(v["kind"], "nas");
        assert_eq!(v["services"][0]["scheme"], "smb");
        assert_eq!(v["services"][0]["source"], "mdns");
        assert!(v.get("lastSeen").is_some());
        let v = serde_json::to_value(DiscoveryEvent::DeviceLost { id: "x".into() }).unwrap();
        assert_eq!(v, serde_json::json!({"type": "deviceLost", "id": "x"}));
    }
}
