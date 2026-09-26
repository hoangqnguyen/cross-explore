//! NetBIOS node-status (NBSTAT) query: asks a Windows or Samba host for
//! its computer name over UDP 137. WS-Discovery and port probes find
//! Windows PCs by address only; this turns `192.168.1.40` into `HPC`.

use crate::model::Source;
use crate::registry::{self, name_rank, Observation};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::net::UdpSocket;

/// Node-status request for the wildcard name `*`.
pub(crate) fn nbstat_request(id: u16) -> Vec<u8> {
    let mut m = Vec::with_capacity(50);
    m.extend_from_slice(&id.to_be_bytes());
    m.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    // "*" padded with NULs to 16 bytes, first-level encoded: each nibble + 'A'.
    let mut name = [0u8; 16];
    name[0] = b'*';
    m.push(32);
    for b in name {
        m.push(b'A' + (b >> 4));
        m.push(b'A' + (b & 0x0f));
    }
    m.push(0);
    m.extend_from_slice(&[0x00, 0x21, 0x00, 0x01]); // NBSTAT, IN
    m
}

/// The unique workstation name (suffix 0x00, not a group) from a node
/// status response.
pub(crate) fn parse_nbstat(resp: &[u8]) -> Option<String> {
    if resp.len() < 12 || resp[2] & 0x80 == 0 {
        return None;
    }
    let mut pos = 12;
    // Answer name: either a pointer or a length-prefixed encoded name.
    match *resp.get(pos)? {
        l if l & 0xc0 == 0xc0 => pos += 2,
        _ => {
            while *resp.get(pos)? != 0 {
                pos += 1 + *resp.get(pos)? as usize;
            }
            pos += 1;
        }
    }
    pos += 2 + 2 + 4 + 2; // type, class, ttl, rdlength
    let count = *resp.get(pos)? as usize;
    pos += 1;
    for i in 0..count {
        let entry = resp.get(pos + i * 18..pos + i * 18 + 18)?;
        let (name, suffix, flags) = (&entry[..15], entry[15], u16::from_be_bytes([entry[16], entry[17]]));
        let is_group = flags & 0x8000 != 0;
        if suffix == 0x00 && !is_group {
            let name = String::from_utf8_lossy(name).trim_end().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

pub(crate) async fn query(ip: IpAddr, wait: Duration) -> Option<String> {
    let bind: SocketAddr = if ip.is_ipv4() { "0.0.0.0:0".parse().ok()? } else { return None };
    let sock = UdpSocket::bind(bind).await.ok()?;
    sock.send_to(&nbstat_request(0x4358), (ip, 137)).await.ok()?;
    let mut buf = [0u8; 1024];
    let (n, _) = tokio::time::timeout(wait, sock.recv_from(&mut buf)).await.ok()?.ok()?;
    parse_nbstat(&buf[..n])
}

pub(crate) fn observation(ip: IpAddr, name: &str, now: i64, ttl_ms: i64) -> Observation {
    let mut o = Observation::new(Source::NetBios, format!("netbios:{ip}"), now);
    o.standalone = false;
    o.expires = now + ttl_ms;
    o.addresses = vec![ip];
    o.name = Some((name_rank::NETBIOS, name.to_string()));
    o.link_names = registry::link_names(&[name]);
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(entries: &[(&str, u8, u16)]) -> Vec<u8> {
        let mut m = vec![0x43, 0x58, 0x84, 0x00, 0, 0, 0, 1, 0, 0, 0, 0];
        m.extend_from_slice(&nbstat_request(0)[12..12 + 34]);
        m.extend_from_slice(&[0x00, 0x21, 0x00, 0x01, 0, 0, 0, 0]);
        let rdlen = 1 + entries.len() * 18 + 46;
        m.extend_from_slice(&(rdlen as u16).to_be_bytes());
        m.push(entries.len() as u8);
        for (name, suffix, flags) in entries {
            m.extend_from_slice(format!("{name:<15}").as_bytes());
            m.push(*suffix);
            m.extend_from_slice(&flags.to_be_bytes());
        }
        m.extend_from_slice(&[0u8; 46]);
        m
    }

    #[test]
    fn request_encodes_wildcard() {
        let r = nbstat_request(1);
        assert_eq!(r.len(), 50);
        assert_eq!(&r[13..15], b"CK", "'*' = 0x2A");
    }

    #[test]
    fn picks_unique_workstation_name() {
        let resp = response(&[("WORKGROUP", 0x00, 0x8400), ("HPC", 0x20, 0x0400), ("HPC", 0x00, 0x0400)]);
        assert_eq!(parse_nbstat(&resp).as_deref(), Some("HPC"));
        assert_eq!(parse_nbstat(&response(&[("WORKGROUP", 0x00, 0x8400)])), None);
        assert_eq!(parse_nbstat(&resp[..30]), None);
    }
}
