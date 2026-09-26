//! SSDP / UPnP: routers, NAS boxes, media servers, printers, TVs. An
//! `M-SEARCH ssdp:all` makes every UPnP device answer with a `LOCATION`
//! pointing at its description XML, where `friendlyName`, `manufacturer`
//! and `modelName` tell a Synology from a router.

use crate::kind;
use crate::model::Source;
use crate::registry::{name_rank, rank, Observation};
use crate::util::url_host;
use std::collections::HashMap;
use crate::net::multicast_query;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::time::Duration;

const MSEARCH: &str = "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: ssdp:all\r\nUSER-AGENT: CrossExplore/1 UPnP/1.1\r\n\r\n";

/// The interesting headers of one M-SEARCH response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SsdpResponse {
    pub location: String,
    pub usn: String,
    pub st: String,
    pub server: String,
    /// `CACHE-CONTROL: max-age`, seconds.
    pub max_age: u64,
}

pub(crate) fn parse_response(text: &str) -> Option<SsdpResponse> {
    let mut lines = text.split("\r\n").flat_map(|l| l.split('\n'));
    let status = lines.next()?;
    if !(status.starts_with("HTTP/1.1 200") || status.starts_with("HTTP/1.0 200") || status.starts_with("NOTIFY")) {
        return None;
    }
    let mut h: HashMap<String, String> = HashMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            h.insert(k.trim().to_ascii_uppercase(), v.trim().to_string());
        }
    }
    let max_age = h.get("CACHE-CONTROL").and_then(|c| c.split(',').find_map(|p| p.trim().strip_prefix("max-age").map(|v| v.trim_start_matches([' ', '=']).trim().parse().ok()))).flatten().unwrap_or(1800);
    Some(SsdpResponse {
        location: h.remove("LOCATION")?,
        usn: h.remove("USN").unwrap_or_default(),
        st: h.remove("ST").or_else(|| h.remove("NT")).unwrap_or_default(),
        server: h.remove("SERVER").unwrap_or_default(),
        max_age,
    })
}

/// The root device of a UPnP description document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Description {
    pub device_type: String,
    pub friendly_name: String,
    pub manufacturer: String,
    pub model_name: String,
    pub model_number: String,
    pub udn: String,
}

/// Text of the first `<tag>` (any namespace prefix) inside `xml`.
pub(crate) fn xml_text(xml: &str, tag: &str) -> Option<String> {
    let mut search = 0;
    while let Some(rel) = xml[search..].find('<') {
        let start = search + rel + 1;
        let end = start + xml[start..].find('>')?;
        let open = &xml[start..end];
        search = end + 1;
        let name = open.split_whitespace().next().unwrap_or("");
        let local = name.rsplit(':').next().unwrap_or(name);
        if local != tag || open.starts_with('/') || open.ends_with('/') {
            continue;
        }
        let close = xml[search..].find(&format!("</{name}>"))?;
        return Some(unescape(xml[search..search + close].trim()));
    }
    None
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

pub(crate) fn parse_description(xml: &str) -> Option<Description> {
    // The first <device> is the root; embedded devices follow inside it and
    // must not override its fields, so only look before <deviceList>.
    let device_start = xml.find("<device>").or_else(|| xml.find("<device "))?;
    let root = &xml[device_start..];
    let root = root.find("<deviceList").map(|i| &root[..i]).unwrap_or(root);
    let get = |t: &str| xml_text(root, t).unwrap_or_default();
    Some(Description {
        device_type: get("deviceType"),
        friendly_name: get("friendlyName"),
        manufacturer: get("manufacturer"),
        model_name: get("modelName"),
        model_number: get("modelNumber"),
        udn: get("UDN"),
    })
}

/// `uuid:1234::urn:…` → `1234`.
fn usn_uuid(usn: &str) -> Option<String> {
    let u = usn.split("::").next()?.trim();
    let u = u.strip_prefix("uuid:").unwrap_or(u);
    (!u.is_empty()).then(|| u.to_ascii_lowercase())
}

pub(crate) fn observation(ip: IpAddr, resp: &SsdpResponse, desc: Option<&Description>, now: i64, ttl_ms: i64) -> Observation {
    let mut o = Observation::new(Source::Ssdp, format!("ssdp:{ip}"), now);
    o.expires = now + ttl_ms.max(resp.max_age as i64 * 1000);
    o.addresses = vec![ip];
    let uuid = desc.and_then(|d| usn_uuid(&d.udn)).or_else(|| usn_uuid(&resp.usn));
    o.identity = uuid.map(|u| (rank::SSDP, format!("ssdp:{u}")));
    match desc {
        Some(d) => {
            if !d.friendly_name.is_empty() {
                o.name = Some((name_rank::SSDP, d.friendly_name.clone()));
            }
            let model = [d.model_name.as_str(), d.model_number.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
            o.model = (!model.is_empty()).then_some(model);
            o.kind = kind::best([kind::from_upnp(&d.device_type, &d.manufacturer, &d.model_name, &resp.server), kind::from_name(&d.friendly_name)]);
        }
        None => o.kind = kind::from_upnp(&resp.st, "", "", &resp.server),
    }
    o
}

/// Send M-SEARCH and gather responses for `wait`, one per responding IP
/// (preferring a root-device response, whose LOCATION is the full tree).
pub(crate) async fn search(wait: Duration) -> HashMap<IpAddr, SsdpResponse> {
    let mut out: HashMap<IpAddr, SsdpResponse> = HashMap::new();
    let group = SocketAddrV4::new(Ipv4Addr::new(239, 255, 255, 250), 1900);
    // Sent twice: UDP multicast is lossy and the spec suggests repeating.
    let msg = MSEARCH.as_bytes().to_vec();
    for (datagram, from) in multicast_query(group, &[msg.clone(), msg], wait).await {
        let Some(resp) = std::str::from_utf8(&datagram).ok().and_then(parse_response) else { continue };
        let is_root = resp.st.eq_ignore_ascii_case("upnp:rootdevice");
        match out.get(&from.ip()) {
            Some(existing) if !is_root || existing.st.eq_ignore_ascii_case("upnp:rootdevice") => {}
            _ => {
                out.insert(from.ip(), resp);
            }
        }
    }
    out
}

/// Fetch a description, but only from the device that sent the response:
/// a LAN device must not be able to make us request arbitrary URLs.
pub(crate) async fn fetch_description(from: IpAddr, location: &str) -> Option<Description> {
    let host: IpAddr = url_host(location)?.parse().ok()?;
    if host != from {
        return None;
    }
    let resp = crate::http::get(location, Duration::from_secs(2), 256 * 1024).await.ok()?;
    if resp.status != 200 {
        return None;
    }
    parse_description(&String::from_utf8_lossy(&resp.body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceKind;

    const RESPONSE: &str = "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=120\r\nDATE: Sat, 26 Sep 2026 12:00:00 GMT\r\nEXT:\r\nLOCATION: http://192.168.1.20:5000/ssdp/desc-DSM-eth0.xml\r\nSERVER: Synology/DSM/192.168.1.20\r\nST: upnp:rootdevice\r\nUSN: uuid:73796E6F-6473-6D00-0000-0011322f0a3b::upnp:rootdevice\r\n\r\n";

    const DESCRIPTION: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0">
  <specVersion><major>1</major><minor>0</minor></specVersion>
  <device>
    <deviceType>urn:schemas-upnp-org:device:Basic:1</deviceType>
    <friendlyName>DiskStation (DS920+)</friendlyName>
    <manufacturer>Synology</manufacturer>
    <manufacturerURL>http://www.synology.com</manufacturerURL>
    <modelDescription>Synology NAS</modelDescription>
    <modelName>DS920+</modelName>
    <modelNumber>DS920+ 7.2-64570</modelNumber>
    <UDN>uuid:73796E6F-6473-6D00-0000-0011322f0a3b</UDN>
    <deviceList>
      <device><deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType><friendlyName>Embedded &amp; ignored</friendlyName></device>
    </deviceList>
  </device>
</root>"#;

    #[test]
    fn parses_msearch_response() {
        let r = parse_response(RESPONSE).unwrap();
        assert_eq!(r.location, "http://192.168.1.20:5000/ssdp/desc-DSM-eth0.xml");
        assert_eq!(r.st, "upnp:rootdevice");
        assert_eq!(r.max_age, 120);
        assert_eq!(usn_uuid(&r.usn).as_deref(), Some("73796e6f-6473-6d00-0000-0011322f0a3b"));
        assert!(parse_response("M-SEARCH * HTTP/1.1\r\n\r\n").is_none(), "our own query echoed back");
    }

    #[test]
    fn parses_description_root_only() {
        let d = parse_description(DESCRIPTION).unwrap();
        assert_eq!(d.friendly_name, "DiskStation (DS920+)");
        assert_eq!(d.manufacturer, "Synology");
        assert_eq!(d.model_name, "DS920+");
        let o = observation("192.168.1.20".parse().unwrap(), &parse_response(RESPONSE).unwrap(), Some(&d), 0, 1000);
        assert_eq!(o.kind.unwrap().1, DeviceKind::Nas);
        assert_eq!(o.identity.unwrap().1, "ssdp:73796e6f-6473-6d00-0000-0011322f0a3b");
        assert_eq!(o.expires, 120_000);
    }

    #[test]
    fn router_description_with_prefixes() {
        let xml = r#"<root><device><s:deviceType xmlns:s="x">urn:schemas-upnp-org:device:InternetGatewayDevice:1</s:deviceType><s:friendlyName xmlns:s="x">Archer AX55</s:friendlyName><manufacturer>TP-Link</manufacturer><modelName>AX55</modelName></device></root>"#;
        let d = parse_description(xml).unwrap();
        assert_eq!(d.device_type, "urn:schemas-upnp-org:device:InternetGatewayDevice:1");
        let o = observation("192.168.1.1".parse().unwrap(), &parse_response(RESPONSE).unwrap(), Some(&d), 0, 1000);
        assert_eq!(o.kind.unwrap().1, DeviceKind::Router);
        assert_eq!(o.name.unwrap().1, "Archer AX55");
        assert_eq!(xml_text("<a><b/><b>x &amp; y</b></a>", "b").as_deref(), Some("x & y"));
    }
}
