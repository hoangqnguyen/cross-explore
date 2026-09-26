//! WS-Discovery: how Windows machines announce themselves on a LAN since
//! SMB1/NetBIOS browsing was retired (the "Network" folder in Explorer).
//! A SOAP-over-UDP `Probe` to 239.255.255.250:3702 makes them answer with
//! `ProbeMatches`; their `Types` say "Computer" and `XAddrs` carry an
//! address. Windows PCs almost always serve SMB, which the prober then
//! confirms on port 445.

use crate::kind;
use crate::model::Source;
use crate::registry::{rank, Observation};
use crate::ssdp::xml_text;
use crate::util::url_host;
use std::collections::HashMap;
use crate::net::multicast_query;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::time::Duration;

/// A WS-Discovery 1.1 (2005/04 namespace, what Windows speaks) Probe.
pub(crate) fn probe_message(message_id: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?><soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope" xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:wsd="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:wsdp="http://schemas.xmlsoap.org/ws/2006/02/devprof"><soap:Header><wsa:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</wsa:To><wsa:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</wsa:Action><wsa:MessageID>urn:uuid:{message_id}</wsa:MessageID></soap:Header><soap:Body><wsd:Probe><wsd:Types>wsdp:Device</wsd:Types></wsd:Probe></soap:Body></soap:Envelope>"#
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProbeMatch {
    /// Endpoint reference, e.g. `urn:uuid:…`; stable per device.
    pub endpoint: String,
    pub types: String,
    pub xaddrs: Vec<String>,
}

/// Parse a ProbeMatches message. Returns `None` for anything else
/// (including our own Probe looping back).
pub(crate) fn parse_probe_matches(xml: &str) -> Option<ProbeMatch> {
    if !xml.contains("ProbeMatches") {
        return None;
    }
    let endpoint = xml_text(xml, "Address")?;
    let types = xml_text(xml, "Types").unwrap_or_default();
    let xaddrs = xml_text(xml, "XAddrs").map(|x| x.split_whitespace().map(str::to_string).collect()).unwrap_or_default();
    Some(ProbeMatch { endpoint, types, xaddrs })
}

pub(crate) fn observation(from: IpAddr, m: &ProbeMatch, now: i64, ttl_ms: i64) -> Observation {
    let id = m.endpoint.trim_start_matches("urn:").trim_start_matches("uuid:").to_ascii_lowercase();
    let mut o = Observation::new(Source::WsDiscovery, format!("wsd:{id}"), now);
    o.expires = now + ttl_ms;
    o.identity = Some((rank::WSD, format!("wsd:{id}")));
    let mut addrs = vec![from];
    for ip in m.xaddrs.iter().filter_map(|x| url_host(x)?.parse::<IpAddr>().ok()) {
        if !addrs.contains(&ip) {
            addrs.push(ip);
        }
    }
    o.addresses = addrs;
    o.kind = kind::from_wsd_types(&m.types);
    o
}

/// Probe and collect matches for `wait`, keyed by endpoint.
pub(crate) async fn discover(wait: Duration) -> HashMap<String, (IpAddr, ProbeMatch)> {
    let mut out = HashMap::new();
    let group = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 3702);
    let msg = probe_message(&message_id()).into_bytes();
    // UDP is lossy and the spec suggests repeating the probe.
    for (datagram, from) in multicast_query(group, &[msg.clone(), msg], wait).await {
        if let Some(m) = std::str::from_utf8(&datagram).ok().and_then(parse_probe_matches) {
            out.insert(m.endpoint.clone(), (from.ip(), m));
        }
    }
    out
}

/// A random-enough UUID for `MessageID` without a uuid crate.
fn message_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_i64(crate::util::now_ms());
    let a = h.finish();
    h.write_u64(a);
    let b = h.finish();
    format!("{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}", a >> 32, (a >> 16) & 0xffff, a & 0xfff, (b >> 48) & 0xfff, b & 0xffff_ffff_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceKind;

    const WINDOWS_MATCH: &str = r#"<?xml version="1.0" encoding="utf-8" ?><soap:Envelope xmlns:soap="http://www.w3.org/2003/05/soap-envelope" xmlns:wsa="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:wsd="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:wsdp="http://schemas.xmlsoap.org/ws/2006/02/devprof" xmlns:pub="http://schemas.microsoft.com/windows/pub/2005/07"><soap:Header><wsa:To>http://schemas.xmlsoap.org/ws/2004/08/addressing/role/anonymous</wsa:To><wsa:Action>http://schemas.xmlsoap.org/ws/2005/04/discovery/ProbeMatches</wsa:Action><wsa:MessageID>urn:uuid:2b8c2e2f-d6d8-4a5c-9f55-0c3a1c0f9e61</wsa:MessageID><wsa:RelatesTo>urn:uuid:0d8c1d6e-1111-4222-8333-444455556666</wsa:RelatesTo><wsd:AppSequence InstanceId="12" SequenceId="urn:uuid:aaaa" MessageNumber="3"></wsd:AppSequence></soap:Header><soap:Body><wsd:ProbeMatches><wsd:ProbeMatch><wsa:EndpointReference><wsa:Address>urn:uuid:4c4c4544-0051-3510-8053-b4c04f4d3232</wsa:Address></wsa:EndpointReference><wsd:Types>wsdp:Device pub:Computer</wsd:Types><wsd:XAddrs>http://192.168.1.40:5357/4c4c4544-0051-3510-8053-b4c04f4d3232/ http://[fe80::1234%12]:5357/x/</wsd:XAddrs><wsd:MetadataVersion>2</wsd:MetadataVersion></wsd:ProbeMatch></wsd:ProbeMatches></soap:Body></soap:Envelope>"#;

    #[test]
    fn parses_windows_probe_matches() {
        let m = parse_probe_matches(WINDOWS_MATCH).unwrap();
        assert_eq!(m.endpoint, "urn:uuid:4c4c4544-0051-3510-8053-b4c04f4d3232");
        assert_eq!(m.types, "wsdp:Device pub:Computer");
        assert_eq!(m.xaddrs.len(), 2);
        let o = observation("192.168.1.40".parse().unwrap(), &m, 0, 1000);
        assert_eq!(o.kind.unwrap().1, DeviceKind::Pc);
        assert_eq!(o.addresses, vec!["192.168.1.40".parse::<IpAddr>().unwrap()], "XAddr duplicate and scoped v6 dropped");
        assert_eq!(o.identity.unwrap().1, "wsd:4c4c4544-0051-3510-8053-b4c04f4d3232");
    }

    #[test]
    fn ignores_our_own_probe() {
        assert!(parse_probe_matches(&probe_message(&message_id())).is_none());
        assert_eq!(message_id().len(), 36);
    }
}
