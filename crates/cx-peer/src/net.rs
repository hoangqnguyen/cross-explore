//! The QUIC endpoint. One UDP socket serves both directions: incoming
//! connections from peers and our outgoing connections to them. That way
//! peers see our listening port as the source port, which is what they
//! should dial back.

use crate::identity::Identity;
use crate::tls;
use cx_core::{CxError, Result};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, Endpoint, EndpointConfig, IdleTimeout, ServerConfig, TransportConfig, VarInt};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

/// Flow-control windows sized for LAN/tailnet bulk transfer: a single
/// stream may have 32 MiB in flight, a connection 128 MiB, so a transfer
/// is limited by the link rather than by round trips.
const STREAM_WINDOW: u32 = 32 << 20;
const CONN_WINDOW: u32 = 128 << 20;
const MAX_UDP_PAYLOAD: u16 = 8952;
const SOCKET_BUFFER: usize = 8 << 20;

pub const IDLE_TIMEOUT: Duration = Duration::from_secs(20);
pub const KEEP_ALIVE: Duration = Duration::from_secs(5);

fn transport() -> Arc<TransportConfig> {
    let mut t = TransportConfig::default();
    t.max_concurrent_bidi_streams(VarInt::from_u32(4096))
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .stream_receive_window(VarInt::from_u32(STREAM_WINDOW))
        .receive_window(VarInt::from_u32(CONN_WINDOW))
        .send_window(CONN_WINDOW as u64)
        .keep_alive_interval(Some(KEEP_ALIVE))
        .max_idle_timeout(Some(IdleTimeout::try_from(IDLE_TIMEOUT).expect("idle timeout")));
    // Probe up to jumbo-frame sizes: on links that allow them (loopback,
    // many NAS setups) far fewer packets means far less per-packet crypto
    // and syscall overhead. Elsewhere the probes simply fail.
    let mut mtu = quinn::MtuDiscoveryConfig::default();
    mtu.upper_bound(MAX_UDP_PAYLOAD);
    t.mtu_discovery_config(Some(mtu));
    Arc::new(t)
}

/// Bind the UDP socket. Without an explicit address we listen on all
/// interfaces, dual-stack where the OS allows it.
pub fn bind(ip: Option<IpAddr>, port: u16) -> Result<UdpSocket> {
    let err = |e: std::io::Error| CxError::Connection(format!("cannot listen on UDP port {port}: {e}"));
    let make = |addr: SocketAddr, dual: bool| -> std::io::Result<UdpSocket> {
        let sock = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
        if dual {
            sock.set_only_v6(false)?;
        }
        let _ = sock.set_recv_buffer_size(SOCKET_BUFFER);
        let _ = sock.set_send_buffer_size(SOCKET_BUFFER);
        sock.bind(&addr.into())?;
        Ok(sock.into())
    };
    match ip {
        Some(ip) => make(SocketAddr::new(ip, port), false).map_err(err),
        None => make(SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port), true)
            .or_else(|_| make(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port), false))
            .map_err(err),
    }
}

pub fn endpoint(id: &Identity, socket: UdpSocket, listen: bool) -> Result<(Endpoint, ClientConfig)> {
    let tls_err = |e: rustls::Error| CxError::Io(format!("TLS setup: {e}"));
    let server = if listen {
        let crypto = QuicServerConfig::try_from(tls::server_crypto(id).map_err(tls_err)?).map_err(|e| CxError::Io(format!("QUIC setup: {e}")))?;
        let mut cfg = ServerConfig::with_crypto(Arc::new(crypto));
        cfg.transport_config(transport());
        Some(cfg)
    } else {
        None
    };
    let crypto = QuicClientConfig::try_from(tls::client_crypto(id).map_err(tls_err)?).map_err(|e| CxError::Io(format!("QUIC setup: {e}")))?;
    let mut client = ClientConfig::new(Arc::new(crypto));
    client.transport_config(transport());
    let reset_key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &id.reset_key());
    let ep = Endpoint::new(EndpointConfig::new(Arc::new(reset_key)), server, socket, Arc::new(quinn::TokioRuntime))
        .map_err(|e| CxError::Connection(format!("QUIC endpoint: {e}")))?;
    Ok((ep, client))
}

/// Server name sent in the TLS handshake. Certificates are not checked
/// against names (keys are pinned instead), so this is a constant.
pub const SERVER_NAME: &str = "cx-peer";
