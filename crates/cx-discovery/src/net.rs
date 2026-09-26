//! Multicast "who's there" queries on every LAN interface.
//!
//! A socket that merely sends to a multicast group goes out of whichever
//! interface the routing table picks, so on a multi-homed machine (Wi-Fi
//! plus Ethernet, a VPN or overlay adapter, a VM bridge) half the network
//! would never hear us. We open one socket per interface address, pinned
//! with `IP_MULTICAST_IF`, and gather replies from all of them.

use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;
use tokio::net::UdpSocket;

/// IPv4 addresses of interfaces that can carry LAN multicast. Tailscale's
/// interface is skipped: it does not forward multicast.
pub(crate) fn lan_ipv4s() -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local() && !crate::util::is_tailnet_ip(&IpAddr::V4(v4)) => Some(v4),
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn socket_on(iface: Option<Ipv4Addr>) -> std::io::Result<UdpSocket> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    s.set_nonblocking(true)?;
    s.set_multicast_ttl_v4(2)?;
    let bind = iface.unwrap_or(Ipv4Addr::UNSPECIFIED);
    if let Some(ip) = iface {
        s.set_multicast_if_v4(&ip)?;
    }
    s.bind(&SocketAddr::from((bind, 0)).into())?;
    UdpSocket::from_std(s.into())
}

/// Send each payload to `group` from every LAN interface, then collect
/// every datagram that comes back within `wait`.
pub(crate) async fn multicast_query(group: SocketAddrV4, payloads: &[Vec<u8>], wait: Duration) -> Vec<(Vec<u8>, SocketAddr)> {
    let ifaces = lan_ipv4s();
    let mut sockets: Vec<UdpSocket> = ifaces.iter().filter_map(|ip| socket_on(Some(*ip)).ok()).collect();
    if sockets.is_empty() {
        sockets.extend(socket_on(None).ok());
    }
    let deadline = tokio::time::Instant::now() + wait;
    let per_socket = sockets.into_iter().map(|sock| async move {
        for p in payloads {
            let _ = sock.send_to(p, group).await;
        }
        let mut got = Vec::new();
        let mut buf = vec![0u8; 16 * 1024];
        while let Ok(Ok((n, from))) = tokio::time::timeout_at(deadline, sock.recv_from(&mut buf)).await {
            got.push((buf[..n].to_vec(), from));
        }
        got
    });
    crate::util::join_all(per_socket).await.into_iter().flatten().collect()
}
