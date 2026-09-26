//! Peer mode: Cross Explore instances talking directly to each other over
//! QUIC, on a LAN or a Tailscale tailnet, so a folder on another machine
//! browses like a local one (streamed listings, pushed live changes, fast
//! transfers) and files can be sent AirDrop-style.
//!
//! - [`PeerService`] is the entry point: it owns the device identity, the
//!   QUIC endpoint (server and client on one UDP socket), the trust store,
//!   the shares and pending offers, and reports [`PeerEvent`]s.
//! - [`PeerConnector`] plugs `peer://<device id or host>/<Share>/…` into the
//!   [`cx_core::Vfs`]; its [`PeerProvider`] implements the whole
//!   [`cx_core::Provider`] trait over the [`protocol`].
//! - Security: devices are identified by ed25519 keys pinned at pairing
//!   (SPAKE2 over a 6-digit code) or accepted as the same Tailscale user;
//!   peers see only configured shares, with traversal and symlink escapes
//!   refused ([`shares`]); every remote operation is audited.

mod audit;
mod client;
mod connector;
pub mod events;
mod fsutil;
pub mod identity;
mod net;
mod offer;
pub mod pairing;
pub mod protocol;
mod server;
mod service;
pub mod shares;
pub mod tailnet;
mod tls;
pub mod trust;

pub use client::PeerProvider;
pub use connector::{split_host_port, PeerConnector, PeerDirectory};
pub use events::{AuditRecord, Direction, EventHandler, OfferFileInfo, OfferState, PeerEvent, PeerRef};
pub use identity::IdentityInfo;
pub use pairing::PairingCode;
pub use protocol::ShareInfo;
pub use service::{PeerConfig, PeerService};
pub use shares::Share;
pub use trust::TrustedDevice;
