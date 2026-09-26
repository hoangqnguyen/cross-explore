//! Public data model. Everything here is `Serialize` (camelCase) because the
//! UI receives devices and events verbatim through Tauri.

use cx_core::Scheme;
use serde::Serialize;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

/// What a device probably is, used for the sidebar icon and for ranking
/// suggestions. Derived from weak signals, so treat it as a hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum DeviceKind {
    Mac,
    Pc,
    Linux,
    Nas,
    Phone,
    Tablet,
    Router,
    Printer,
    #[default]
    Unknown,
}

/// Which mechanism told us about a device (or one of its services).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Mdns,
    Tailscale,
    Ssdp,
    WsDiscovery,
    NetBios,
    /// A TCP port probe we ran ourselves.
    Probe,
}

/// Tailnet facts about a device, straight from `tailscale status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailnetInfo {
    pub online: bool,
    /// As Tailscale reports it: "macOS", "windows", "linux", "android", "iOS"…
    pub os: String,
    /// Login name of the node's owner ("tagged-devices" for tagged nodes).
    pub owner: Option<String>,
    /// Login name of whoever shared this node into our tailnet, if it was shared.
    pub shared_by: Option<String>,
    /// This machine.
    pub is_self: bool,
    /// MagicDNS name without the trailing dot, e.g. `hpc.tailnet-x.ts.net`.
    pub dns_name: Option<String>,
    /// Unix ms; `None` when Tailscale has no record (never seen or online now).
    pub last_seen: Option<i64>,
    /// ACL tags such as `tag:server`.
    pub tags: Vec<String>,
}

/// A file service we believe a device offers. `uri` is a Cross Explore
/// location that `cx_core::Location::parse` accepts; opening it may still ask
/// for credentials — discovery never tries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub scheme: Scheme,
    pub port: u16,
    pub uri: String,
    /// Short UI label, e.g. "SMB" or "SFTP via Tailscale".
    pub label: String,
    pub source: Source,
}

/// A named share we learned about without signing in (Time Machine volumes
/// from `_adisk` TXT records, and the like).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareHint {
    pub name: String,
    pub uri: String,
    pub source: Source,
}

/// One physical (or virtual) machine, merged from every source that saw it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Stable while the device keeps its strongest identity, prefixed by
    /// origin: `ts:<node id>`, `peer:<id>`, `mdns:<host>.local`,
    /// `ssdp:<uuid>`, `wsd:<uuid>` or `ip:<addr>`. If a stronger identity
    /// shows up later (say the Tailscale node of a host first seen over
    /// mDNS) the old id is reported lost and the device re-announced.
    pub id: String,
    /// Friendly name ("Alex's MacBook Pro", "synology", "hpc").
    pub name: String,
    pub kind: DeviceKind,
    /// Hardware model when a source told us ("MacBookPro18,1", "DS920+").
    pub model: Option<String>,
    /// LAN addresses first, then tailnet, then the rest; IPv4 before IPv6.
    pub addresses: Vec<IpAddr>,
    /// Best DNS name: MagicDNS if on the tailnet, else the `.local` name.
    pub hostname: Option<String>,
    pub sources: Vec<Source>,
    pub tailnet: Option<TailnetInfo>,
    pub services: Vec<Service>,
    pub shares: Vec<ShareHint>,
    /// Unix ms of the most recent evidence that the device is around.
    pub last_seen: i64,
}

impl Device {
    /// Online as far as we can tell: tailnet devices report it explicitly,
    /// anything seen on the LAN is online by construction.
    pub fn is_online(&self) -> bool {
        match &self.tailnet {
            Some(t) => t.online || self.sources.iter().any(|s| *s != Source::Tailscale && *s != Source::Probe),
            None => true,
        }
    }

    pub fn is_self(&self) -> bool {
        self.tailnet.as_ref().is_some_and(|t| t.is_self)
    }
}

/// Emitted on the `on_event` callback. Serialized with an inline `type` tag:
/// `{"type":"deviceUpdated","id":…,"name":…}` / `{"type":"deviceLost","id":…}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
// Events are created a few times a second and moved straight to the UI;
// boxing the device would only add an allocation and a less direct API.
#[allow(clippy::large_enum_variant)]
pub enum DiscoveryEvent {
    /// A device appeared or something visible about it changed. Carries the
    /// full device: the UI replaces its copy by `id`.
    DeviceUpdated(Device),
    DeviceLost { id: String },
}

/// Our own peer service, announced over mDNS as `_crossx._udp` (the QUIC
/// transport) and `_crossx._tcp` (for browsers that only look at TCP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advertisement {
    /// Instance name other machines show, usually the computer name.
    pub name: String,
    pub port: u16,
    /// Our stable peer id; other instances put it in `peer://<id>/`.
    pub device_id: String,
    /// Extra TXT entries (protocol version, capabilities…). `id` and `name`
    /// are added automatically.
    pub txt: Vec<(String, String)>,
}

/// Which sources run and how often. The defaults are what the app uses.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub mdns: bool,
    pub tailscale: bool,
    pub ssdp: bool,
    pub ws_discovery: bool,
    pub netbios: bool,
    pub probe: bool,
    /// How often `tailscale status` is polled.
    pub tailscale_interval: Duration,
    /// How often SSDP, WS-Discovery and `_device-info` are re-queried.
    pub scan_interval: Duration,
    /// How often an address is re-probed when nothing asked for it.
    pub probe_interval: Duration,
    /// TCP connect timeout for LAN addresses.
    pub probe_timeout: Duration,
    /// TCP connect timeout for tailnet addresses: the first packet to a peer
    /// may go through a DERP relay while the direct path is negotiated.
    pub probe_timeout_tailnet: Duration,
    /// Upper bound on concurrent probe connections.
    pub probe_concurrency: usize,
    /// Explicit path to the `tailscale` CLI; searched for when `None`.
    pub tailscale_binary: Option<PathBuf>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        DiscoveryConfig {
            mdns: true,
            tailscale: true,
            ssdp: true,
            ws_discovery: true,
            netbios: true,
            probe: true,
            tailscale_interval: Duration::from_secs(15),
            scan_interval: Duration::from_secs(60),
            probe_interval: Duration::from_secs(180),
            probe_timeout: Duration::from_millis(300),
            probe_timeout_tailnet: Duration::from_millis(1500),
            probe_concurrency: 48,
            tailscale_binary: None,
        }
    }
}
