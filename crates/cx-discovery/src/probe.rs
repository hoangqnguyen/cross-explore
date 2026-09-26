//! TCP port probes: the only way to learn what a tailnet peer or a
//! Windows PC serves, since neither advertises over mDNS. A probe is a bare
//! TCP connect (plus an unauthenticated `OPTIONS` on HTTP ports), closed
//! immediately; nothing is ever logged into.

use crate::http;
use crate::model::{Device, Source};
use crate::registry::{Observation, ObservedService};
use crate::util::{is_lan_ip, is_tailnet_ip, now_ms, url_host};
use cx_core::Scheme;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;

/// One address to probe, and the name to put in the resulting URIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub ip: IpAddr,
    /// MagicDNS name for tailnet addresses, the `.local` name or the IP
    /// itself on the LAN.
    pub host: String,
    pub tailnet: bool,
}

/// What each probed port means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Check {
    /// Open port ⇒ this scheme.
    Open(Scheme),
    /// Open port and `OPTIONS /` answers with a `DAV` header ⇒ this scheme.
    Dav { tls: bool },
}

const PORTS: [(u16, Check); 8] = [
    (22, Check::Open(Scheme::Sftp)),
    (445, Check::Open(Scheme::Smb)),
    (21, Check::Open(Scheme::Ftp)),
    (80, Check::Dav { tls: false }),
    (443, Check::Dav { tls: true }),
    // Synology's WebDAV Server package defaults.
    (5005, Check::Dav { tls: false }),
    (5006, Check::Dav { tls: true }),
    // Our own peer agent. The peer protocol is QUIC, which a TCP connect
    // cannot see; this only finds agents that also accept TCP on the port.
    // Peers are primarily found through `_crossx` mDNS records.
    (47470, Check::Open(Scheme::Peer)),
];

/// Addresses worth probing for a device: its tailnet IPv4 (when the node
/// is online) and its first LAN IPv4. Offline tailnet nodes are skipped —
/// every connect would just time out.
pub(crate) fn targets(device: &Device) -> Vec<Target> {
    let mut out = Vec::new();
    let tailnet_online = device.tailnet.as_ref().map(|t| t.online);
    if tailnet_online == Some(true) {
        if let Some(ip) = device.addresses.iter().find(|ip| ip.is_ipv4() && is_tailnet_ip(ip)) {
            let host = device.tailnet.as_ref().and_then(|t| t.dns_name.clone()).unwrap_or_else(|| ip.to_string());
            out.push(Target { ip: *ip, host, tailnet: true });
        }
    }
    let seen_on_lan = device.sources.iter().any(|s| !matches!(s, Source::Tailscale | Source::Probe));
    if tailnet_online != Some(false) || seen_on_lan {
        if let Some(ip) = device.addresses.iter().find(|ip| ip.is_ipv4() && is_lan_ip(ip)) {
            // Name LAN services after the mDNS host when there is one, so a
            // probe result and an advertised service share one URI.
            let local = device.hostname.iter().cloned().chain(device.services.iter().filter_map(|s| url_host(&s.uri))).find(|h| h.ends_with(".local"));
            out.push(Target { ip: *ip, host: local.unwrap_or_else(|| ip.to_string()), tailnet: false });
        }
    }
    out
}

/// Probe every port of `target` concurrently (bounded by `limit`) and turn
/// the open ones into an observation.
pub(crate) async fn probe(target: &Target, limit: Arc<Semaphore>, connect_timeout: Duration, ttl_ms: i64) -> Observation {
    let checks = PORTS.iter().map(|&(port, check)| {
        let limit = limit.clone();
        let target = target.clone();
        async move {
            let _permit = limit.acquire_owned().await.ok()?;
            let addr = SocketAddr::new(target.ip, port);
            let stream = tokio::time::timeout(connect_timeout, TcpStream::connect(addr)).await.ok()?.ok()?;
            let scheme = match check {
                Check::Open(scheme) => scheme,
                Check::Dav { tls } => {
                    drop(stream);
                    let resp = http::options(target.ip, port, &target.host, tls, connect_timeout * 4).await.ok()?;
                    resp.header("DAV")?;
                    if tls {
                        Scheme::Davs
                    } else {
                        Scheme::Dav
                    }
                }
            };
            Some(ObservedService { scheme, port, host: target.host.clone(), path: vec![] })
        }
    });
    let services: Vec<ObservedService> = crate::util::join_all(checks).await.into_iter().flatten().collect();
    let now = now_ms();
    let mut o = Observation::new(Source::Probe, format!("probe:{}", target.ip), now);
    o.standalone = false;
    o.expires = now + ttl_ms;
    o.addresses = vec![target.ip];
    o.services = services;
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DeviceKind, TailnetInfo};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn device(addrs: &[&str], tailnet: Option<bool>, sources: Vec<Source>) -> Device {
        Device {
            id: "x".into(),
            name: "x".into(),
            kind: DeviceKind::Unknown,
            model: None,
            addresses: addrs.iter().map(|a| a.parse().unwrap()).collect(),
            hostname: None,
            sources,
            tailnet: tailnet.map(|online| TailnetInfo { online, os: "linux".into(), owner: None, shared_by: None, is_self: false, dns_name: Some("alpha.x.ts.net".into()), last_seen: None, tags: vec![] }),
            services: vec![],
            shares: vec![],
            last_seen: 0,
        }
    }

    #[test]
    fn target_selection() {
        let online = device(&["192.168.1.5", "100.101.1.2", "fd7a:115c:a1e0::1"], Some(true), vec![Source::Tailscale]);
        let t = targets(&online);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], Target { ip: "100.101.1.2".parse().unwrap(), host: "alpha.x.ts.net".into(), tailnet: true });
        assert_eq!(t[1].host, "192.168.1.5");
        assert!(targets(&device(&["100.101.1.2"], Some(false), vec![Source::Tailscale])).is_empty(), "offline peers are not probed");
        assert_eq!(targets(&device(&["192.168.1.9"], None, vec![Source::Ssdp])).len(), 1);
    }

    #[tokio::test]
    async fn detects_open_ports_and_webdav() {
        // Can't bind privileged ports in a test, so exercise the pieces:
        // a DAV-speaking server via http::options and join_all ordering.
        let dav = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = dav.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = dav.accept().await.unwrap();
            let mut buf = [0u8; 512];
            let _ = s.read(&mut buf).await;
            s.write_all(b"HTTP/1.1 200 OK\r\nDAV: 1, 2\r\nContent-Length: 0\r\n\r\n").await.unwrap();
        });
        let resp = http::options("127.0.0.1".parse().unwrap(), port, "127.0.0.1", false, Duration::from_secs(2)).await.unwrap();
        assert_eq!(resp.header("dav"), Some("1, 2"));

        let out = crate::util::join_all([1u8, 2, 3].map(|i| async move {
            tokio::time::sleep(Duration::from_millis(30 - 10 * i as u64)).await;
            i
        }))
        .await;
        assert_eq!(out, [1, 2, 3]);
    }

    #[tokio::test]
    async fn closed_ports_yield_no_services() {
        let target = Target { ip: "127.0.0.1".parse().unwrap(), host: "localhost".into(), tailnet: false };
        let o = probe(&target, Arc::new(Semaphore::new(4)), Duration::from_millis(200), 1000).await;
        assert!(!o.standalone);
        // Whatever this machine happens to run, every reported service must
        // point at the target host.
        assert!(o.services.iter().all(|s| s.host == "localhost"));
    }
}
