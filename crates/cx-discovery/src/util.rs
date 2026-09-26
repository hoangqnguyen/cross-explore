//! Small helpers shared by the sources: address classes, name
//! normalization, URI building and time.

use cx_core::{Endpoint, Scheme};
use percent_encoding::{utf8_percent_encode, AsciiSet, CONTROLS};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Tailscale's CGNAT range (100.64.0.0/10) and its IPv6 ULA prefix.
pub(crate) fn is_tailnet_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (64..128).contains(&o[1])
        }
        IpAddr::V6(v6) => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

/// RFC 1918 / unique-local: addresses that only mean something on a LAN.
pub(crate) fn is_lan_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xfe00) == 0xfc00 && !is_tailnet_ip(ip),
    }
}

fn is_link_local_v6(v6: &Ipv6Addr) -> bool {
    (v6.segments()[0] & 0xffc0) == 0xfe80
}

/// Whether two observations sharing this address are the same machine.
/// Excludes addresses that many unrelated hosts carry at once: loopback,
/// link-local (fe80::1 exists on every utun), and the default bridge
/// addresses of Docker, libvirt and VirtualBox NAT.
pub(crate) fn is_linkable_ip(ip: &IpAddr) -> bool {
    const SHARED_V4: [Ipv4Addr; 3] = [Ipv4Addr::new(172, 17, 0, 1), Ipv4Addr::new(192, 168, 122, 1), Ipv4Addr::new(10, 0, 2, 15)];
    match ip {
        IpAddr::V4(v4) => !(v4.is_loopback() || v4.is_unspecified() || v4.is_link_local() || v4.is_multicast() || SHARED_V4.contains(v4)),
        IpAddr::V6(v6) => !(v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() || is_link_local_v6(v6)),
    }
}

/// Display order: LAN IPv4, tailnet IPv4, other IPv4, then IPv6 in the same
/// order. The first entry is what probes and URIs use by default.
pub(crate) fn address_rank(ip: &IpAddr) -> u8 {
    let class = if is_lan_ip(ip) {
        0
    } else if is_tailnet_ip(ip) {
        1
    } else if matches!(ip, IpAddr::V6(v6) if is_link_local_v6(v6)) {
        3
    } else {
        2
    };
    class + if ip.is_ipv6() { 4 } else { 0 }
}

/// Lowercase alphanumerics only, so "Alex’s MacBook Pro" (mDNS, curly
/// apostrophe), "Alex's MacBook Pro" (Tailscale) and "alexs-macbook-pro"
/// (a DNS label) all compare equal.
pub(crate) fn normalize_name(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Names too common to identify a machine by.
pub(crate) fn is_generic_name(normalized: &str) -> bool {
    normalized.len() < 3 || matches!(normalized, "localhost" | "android" | "iphone" | "ipad" | "raspberrypi" | "ubuntu" | "debian" | "unknown" | "computer" | "desktop" | "laptop")
}

/// DNS names are case-insensitive; drop the trailing root dot too.
pub(crate) fn normalize_hostname(s: &str) -> String {
    s.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// First DNS label: "hpc.tailnet.ts.net" → "hpc".
pub(crate) fn host_label(host: &str) -> &str {
    host.split('.').next().unwrap_or(host)
}

const SEGMENT: &AsciiSet = &CONTROLS.add(b' ').add(b'"').add(b'#').add(b'%').add(b'<').add(b'>').add(b'?').add(b'`').add(b'{').add(b'}').add(b'/').add(b'\\');

/// `scheme://host[:port]/path`, omitting the port when it is the scheme's
/// default so URIs stay canonical across sources.
pub(crate) fn service_uri(scheme: Scheme, host: &str, port: u16, path_segments: &[&str]) -> String {
    let port = (port != scheme.default_port() && port != 0).then_some(port);
    let endpoint = Endpoint { scheme, user: None, host: host.to_string(), port };
    let mut uri = endpoint.uri();
    uri.push('/');
    let path: Vec<String> = path_segments.iter().filter(|s| !s.is_empty()).map(|s| utf8_percent_encode(s, SEGMENT).to_string()).collect();
    uri.push_str(&path.join("/"));
    if !path.is_empty() {
        uri.push('/');
    }
    uri
}

/// UI label for a service on `host`.
pub(crate) fn service_label(scheme: Scheme, host: &str) -> String {
    let via_tailnet = host.parse::<IpAddr>().map(|ip| is_tailnet_ip(&ip)).unwrap_or(false) || host.ends_with(".ts.net");
    let base = match scheme {
        Scheme::Davs => "WebDAV (HTTPS)",
        other => other.label(),
    };
    if via_tailnet {
        format!("{base} via Tailscale")
    } else {
        base.to_string()
    }
}

/// Parse Tailscale's RFC 3339 UTC timestamps ("2026-07-31T14:48:38.1Z") to
/// Unix ms without pulling in a date crate. Go's zero time ("0001-01-01…")
/// means "never" and yields `None`.
pub(crate) fn parse_rfc3339_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 20 || s.starts_with("0001-") {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let rest = &s[19..];
    let (frac, tz) = match rest.strip_prefix('.') {
        Some(r) => {
            let end = r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len());
            let digits = &r[..end.min(3)];
            let ms = format!("{digits:0<3}").parse::<i64>().ok()?;
            (ms, &r[end..])
        }
        None => (0, rest),
    };
    let offset_min = match tz {
        "Z" | "z" => 0,
        tz if tz.len() == 6 => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            sign * (tz[1..3].parse::<i64>().ok()? * 60 + tz[4..6].parse::<i64>().ok()?)
        }
        _ => return None,
    };
    // Days from civil (Howard Hinnant's algorithm).
    let y = if mo <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((((days * 24 + h) * 60 + mi - offset_min) * 60 + sec) * 1000 + frac)
}

/// Host part of an `http://host[:port]/…` URL.
pub(crate) fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map(|(_, a)| a).unwrap_or(authority);
    if let Some(v6) = authority.strip_prefix('[') {
        return Some(v6.split(']').next()?.to_string());
    }
    Some(authority.split(':').next()?.to_string()).filter(|h| !h.is_empty())
}

/// `join_all` without the `futures` crate: spawn-free, polls in place.
pub(crate) async fn join_all<F: std::future::Future>(futs: impl IntoIterator<Item = F>) -> Vec<F::Output> {
    let mut set = Vec::new();
    for f in futs {
        set.push(Box::pin(f));
    }
    let mut out: Vec<Option<F::Output>> = (0..set.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, f) in set.iter_mut().enumerate() {
            if out[i].is_none() {
                match f.as_mut().poll(cx) {
                    std::task::Poll::Ready(v) => out[i] = Some(v),
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    out.into_iter().map(|o| o.expect("all futures completed")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_classes() {
        let ts: IpAddr = "100.101.1.1".parse().unwrap();
        let ts6: IpAddr = "fd7a:115c:a1e0::a039:1266".parse().unwrap();
        let lan: IpAddr = "192.168.1.76".parse().unwrap();
        assert!(is_tailnet_ip(&ts) && is_tailnet_ip(&ts6) && !is_tailnet_ip(&lan));
        assert!(is_lan_ip(&lan) && !is_lan_ip(&ts6));
        assert!(!is_linkable_ip(&"172.17.0.1".parse().unwrap()));
        assert!(!is_linkable_ip(&"fe80::1".parse().unwrap()));
        assert!(address_rank(&lan) < address_rank(&ts) && address_rank(&ts) < address_rank(&ts6));
    }

    #[test]
    fn names_normalize_across_sources() {
        assert_eq!(normalize_name("Alex’s MacBook Pro"), normalize_name("Alex's MacBook Pro"));
        assert_eq!(normalize_name("alexs-macbook-pro"), "alexsmacbookpro");
        assert_eq!(normalize_hostname("HPC.dog-snares.ts.net."), "hpc.dog-snares.ts.net");
        assert!(is_generic_name("localhost"));
    }

    #[test]
    fn uris_are_canonical_and_parse() {
        assert_eq!(service_uri(Scheme::Smb, "nas.local", 445, &[]), "smb://nas.local/");
        assert_eq!(service_uri(Scheme::Sftp, "hpc", 2222, &[]), "sftp://hpc:2222/");
        assert_eq!(service_uri(Scheme::Smb, "nas.local", 445, &["Time Machine"]), "smb://nas.local/Time%20Machine/");
        assert_eq!(service_uri(Scheme::Dav, "fe80::1", 5005, &[]), "dav://[fe80::1]:5005/");
        for uri in ["smb://nas.local/Time%20Machine/", "peer://abc-123/", "dav://[fd7a:115c:a1e0::1]:5005/"] {
            cx_core::Location::parse(uri).unwrap();
        }
        let loc = cx_core::Location::parse(&service_uri(Scheme::Smb, "nas.local", 445, &["Time Machine"])).unwrap();
        assert_eq!(loc.posix_path(), Some("/Time Machine"));
    }

    #[test]
    fn labels_mention_tailscale() {
        assert_eq!(service_label(Scheme::Smb, "hpc.dog-snares.ts.net"), "SMB via Tailscale");
        assert_eq!(service_label(Scheme::Sftp, "192.168.1.4"), "SFTP");
    }

    #[test]
    fn rfc3339() {
        assert_eq!(parse_rfc3339_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339_ms("2026-07-31T14:48:38.1Z"), Some(1_785_509_318_100));
        assert_eq!(parse_rfc3339_ms("2026-07-31T16:48:38+02:00"), Some(1_785_509_318_000));
        assert_eq!(parse_rfc3339_ms("0001-01-01T00:00:00Z"), None);
    }

    #[test]
    fn url_hosts() {
        assert_eq!(url_host("http://192.168.1.1:1900/rootDesc.xml").as_deref(), Some("192.168.1.1"));
        assert_eq!(url_host("http://[fe80::1]:5357/x").as_deref(), Some("fe80::1"));
        assert_eq!(url_host("http://nas.local/").as_deref(), Some("nas.local"));
    }
}
