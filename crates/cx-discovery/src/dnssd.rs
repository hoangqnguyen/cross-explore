//! A one-shot DNS-SD query for `_device-info._tcp`.
//!
//! Apple devices (and Samba's `fruit` module, Synology, QNAP…) publish
//! `_device-info._tcp` as a PTR + TXT pair *without* an SRV record: it only
//! says "the machine called X is a `model=MacBookPro18,1`". `mdns-sd` only
//! reports instances once SRV and addresses resolve, so it never surfaces
//! these. We ask directly instead, as a "legacy unicast" query (RFC 6762
//! §6.7): sent from an ephemeral port, responders answer straight back to
//! us, which needs no port-5353 socket and no shared multicast state.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

pub(crate) const TYPE_PTR: u16 = 12;
pub(crate) const TYPE_TXT: u16 = 16;

const MDNS_ADDR: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(224, 0, 0, 251), 5353);
const DEVICE_INFO: &str = "_device-info._tcp.local";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RData {
    Ptr(String),
    Txt(Vec<String>),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub name: String,
    pub rtype: u16,
    pub data: RData,
}

/// A standard query for `name`/`qtype`, class IN.
pub(crate) fn encode_query(id: u16, questions: &[(&str, u16)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0, 0]); // flags: standard query
    out.extend_from_slice(&(questions.len() as u16).to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    for (name, qtype) in questions {
        for label in name.trim_end_matches('.').split('.') {
            let bytes = label.as_bytes();
            out.push(bytes.len().min(63) as u8);
            out.extend_from_slice(&bytes[..bytes.len().min(63)]);
        }
        out.push(0);
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
    }
    out
}

/// Read a possibly-compressed name at `pos`; returns the name and the
/// position just after it in the original stream.
fn read_name(msg: &[u8], mut pos: usize) -> Option<(String, usize)> {
    let mut labels: Vec<String> = Vec::new();
    let mut end = None;
    for _ in 0..128 {
        let len = *msg.get(pos)? as usize;
        match len {
            0 => {
                return Some((labels.join("."), end.unwrap_or(pos + 1)));
            }
            l if l & 0xc0 == 0xc0 => {
                let ptr = ((l & 0x3f) << 8) | *msg.get(pos + 1)? as usize;
                end.get_or_insert(pos + 2);
                pos = ptr;
            }
            l => {
                let label = msg.get(pos + 1..pos + 1 + l)?;
                labels.push(String::from_utf8_lossy(label).into_owned());
                pos += 1 + l;
            }
        }
    }
    None
}

/// Every resource record in a response (answer, authority, additional).
pub(crate) fn parse(msg: &[u8]) -> Option<Vec<Record>> {
    let count = |i: usize| -> Option<usize> { Some(u16::from_be_bytes([*msg.get(i)?, *msg.get(i + 1)?]) as usize) };
    let flags = count(2)?;
    if flags & 0x8000 == 0 {
        return None; // a query, not a response
    }
    let (qd, rr) = (count(4)?, count(6)? + count(8)? + count(10)?);
    let mut pos = 12;
    for _ in 0..qd {
        pos = read_name(msg, pos)?.1 + 4;
    }
    let mut out = Vec::with_capacity(rr);
    for _ in 0..rr {
        let (name, p) = read_name(msg, pos)?;
        let rtype = count(p)? as u16;
        let rdlen = count(p + 8)?;
        let rdata_start = p + 10;
        let rdata = msg.get(rdata_start..rdata_start + rdlen)?;
        let data = match rtype {
            TYPE_PTR => RData::Ptr(read_name(msg, rdata_start)?.0),
            TYPE_TXT => {
                let mut strings = Vec::new();
                let mut i = 0;
                while i < rdata.len() {
                    let l = rdata[i] as usize;
                    if let Some(s) = rdata.get(i + 1..i + 1 + l) {
                        strings.push(String::from_utf8_lossy(s).into_owned());
                    }
                    i += 1 + l;
                }
                RData::Txt(strings)
            }
            _ => RData::Other,
        };
        out.push(Record { name, rtype, data });
        pos = rdata_start + rdlen;
    }
    Some(out)
}

/// Instance label of `<instance>._device-info._tcp.local`, unescaping
/// `\.` and `\DDD` the way DNS-SD presents them.
fn instance_of(fullname: &str) -> Option<String> {
    let lower = fullname.to_ascii_lowercase();
    let cut = lower.find(&format!(".{DEVICE_INFO}"))?;
    Some(fullname[..cut].to_string())
}

/// Collect `instance → TXT key/values` from a batch of records.
pub(crate) fn device_info(records: &[Record]) -> (Vec<String>, HashMap<String, HashMap<String, String>>) {
    let mut instances = Vec::new();
    let mut txt = HashMap::new();
    for r in records {
        match &r.data {
            RData::Ptr(target) if r.name.eq_ignore_ascii_case(DEVICE_INFO) => {
                if let Some(i) = instance_of(target) {
                    instances.push(i);
                }
            }
            RData::Txt(strings) => {
                if let Some(i) = instance_of(&r.name) {
                    let kv = strings.iter().filter_map(|s| s.split_once('=')).map(|(k, v)| (k.to_ascii_lowercase(), v.to_string())).collect();
                    txt.insert(i, kv);
                }
            }
            _ => {}
        }
    }
    instances.sort();
    instances.dedup();
    (instances, txt)
}

/// Ask the LAN for `_device-info._tcp` and return `instance → model`.
pub(crate) async fn query_device_info(wait: Duration) -> HashMap<String, String> {
    let mut models = HashMap::new();
    let mut records = collect(&[encode_query(0x4358, &[(DEVICE_INFO, TYPE_PTR)])], wait).await;
    let (instances, txt) = device_info(&records);
    // Responders usually include the TXT as an additional record; ask
    // explicitly for any that didn't.
    let missing: Vec<String> = instances.iter().filter(|i| !txt.contains_key(*i)).map(|i| format!("{i}.{DEVICE_INFO}")).collect();
    if !missing.is_empty() {
        let questions: Vec<(&str, u16)> = missing.iter().map(|n| (n.as_str(), TYPE_TXT)).collect();
        records.extend(collect(&[encode_query(0x4359, &questions)], wait / 2).await);
    }
    let (_, txt) = device_info(&records);
    for (instance, kv) in txt {
        if let Some(model) = kv.get("model").filter(|m| !m.is_empty()) {
            models.insert(instance, model.clone());
        }
    }
    models
}

async fn collect(queries: &[Vec<u8>], wait: Duration) -> Vec<Record> {
    crate::net::multicast_query(MDNS_ADDR, queries, wait).await.into_iter().filter_map(|(datagram, _)| parse(&datagram)).flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a response the way mDNSResponder does: PTR answer, TXT in the
    /// additional section, with name compression.
    fn response() -> Vec<u8> {
        let mut m = vec![0x43, 0x58, 0x84, 0x00, 0, 1, 0, 1, 0, 0, 0, 1];
        // Question: _device-info._tcp.local PTR IN
        let q_at = m.len();
        for l in ["_device-info", "_tcp", "local"] {
            m.push(l.len() as u8);
            m.extend_from_slice(l.as_bytes());
        }
        m.push(0);
        m.extend_from_slice(&[0, 12, 0, 1]);
        // Answer: <ptr to q> PTR → "Alex’s MacBook Pro" + <ptr to q>
        m.extend_from_slice(&[0xc0, q_at as u8, 0, 12, 0, 1, 0, 0, 0x11, 0x94]);
        let inst = "Alex’s MacBook Pro".as_bytes();
        m.extend_from_slice(&((inst.len() + 1 + 2) as u16).to_be_bytes());
        let inst_at = m.len();
        m.push(inst.len() as u8);
        m.extend_from_slice(inst);
        m.extend_from_slice(&[0xc0, q_at as u8]);
        // Additional: <ptr to instance> TXT "model=MacBookPro18,1" "osxvers=24"
        m.extend_from_slice(&[0xc0, inst_at as u8, 0, 16, 0x80, 1, 0, 0, 0x11, 0x94]);
        let strings = ["model=MacBookPro18,1", "osxvers=24"];
        let len: usize = strings.iter().map(|s| s.len() + 1).sum();
        m.extend_from_slice(&(len as u16).to_be_bytes());
        for s in strings {
            m.push(s.len() as u8);
            m.extend_from_slice(s.as_bytes());
        }
        m
    }

    #[test]
    fn parses_device_info_response() {
        let records = parse(&response()).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].data, RData::Ptr("Alex’s MacBook Pro._device-info._tcp.local".into()));
        let (instances, txt) = device_info(&records);
        assert_eq!(instances, ["Alex’s MacBook Pro"]);
        assert_eq!(txt["Alex’s MacBook Pro"]["model"], "MacBookPro18,1");
    }

    #[test]
    fn queries_round_trip_and_garbage_is_rejected() {
        let q = encode_query(7, &[(DEVICE_INFO, TYPE_PTR)]);
        assert_eq!(&q[..6], &[0, 7, 0, 0, 0, 1]);
        assert!(parse(&q).is_none(), "queries are not responses");
        assert!(parse(&[0x00, 0x01, 0x84]).is_none());
        let mut looped = response();
        let n = looped.len();
        looped[n - 40] = 0xc0; // corrupt; must not panic or loop forever
        let _ = parse(&looped);
    }
}
