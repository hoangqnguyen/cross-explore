//! "Nearby": background discovery of devices and file services on the LAN
//! and the Tailscale tailnet, for the sidebar and the suggestions banner.
//!
//! Sources, each reporting *observations* into a shared registry:
//!
//! - **mDNS / DNS-SD** — `_smb`, `_sftp-ssh`, `_ssh`, `_ftp`, `_webdav(s)`,
//!   `_afpovertcp`, `_adisk` (Time Machine share names) and our own
//!   `_crossx` peers, plus a direct `_device-info` query for Apple model
//!   identifiers.
//! - **Tailscale** — `tailscale status --json` (or the LocalAPI socket on
//!   Linux): every node with its OS, owner, MagicDNS name and online state.
//! - **Port probes** — TCP 22/445/21/47470 and WebDAV (`OPTIONS` must
//!   return `DAV:`) on 80/443/5005/5006, for tailnet peers and LAN hosts.
//! - **SSDP / UPnP** — routers, NAS boxes, media devices, with their
//!   description XML.
//! - **WS-Discovery** — Windows PCs; **NetBIOS** node status for their names.
//!
//! The registry clusters observations that share an address, DNS name or
//! friendly name into [`Device`]s and reports changes as
//! [`DiscoveryEvent`]s, coalesced to at most ~4 per second per device.
//! [`suggestions`] ranks the results into things worth opening.
//!
//! Nothing here authenticates or opens a session with anything; discovery
//! only listens, asks who is there, and checks which ports answer.
//!
//! ```no_run
//! use cx_discovery::{Discovery, DiscoveryConfig, DiscoveryEvent};
//! let discovery = Discovery::start(DiscoveryConfig::default(), |event| match event {
//!     DiscoveryEvent::DeviceUpdated(d) => println!("{} ({:?}): {} services", d.name, d.kind, d.services.len()),
//!     DiscoveryEvent::DeviceLost { id } => println!("lost {id}"),
//! });
//! discovery.refresh();
//! ```

mod dnssd;
mod http;
mod kind;
mod mdns;
mod model;
mod net;
mod netbios;
mod probe;
mod registry;
mod service;
mod ssdp;
mod suggest;
mod tailscale;
mod util;
mod wsd;

pub use model::{Advertisement, Device, DeviceKind, DiscoveryConfig, DiscoveryEvent, Service, ShareHint, Source, TailnetInfo};
pub use service::Discovery;
pub use suggest::{suggestions, Suggestion};
