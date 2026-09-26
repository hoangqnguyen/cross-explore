//! Ranking discovered services into "you probably want to open this"
//! suggestions for the Nearby banner.
//!
//! The ordering encodes a few convictions: another Cross Explore instance
//! is the best experience; a NAS exists to share files, so its SMB shares
//! come next; a Linux box on the tailnet is usually reached over SFTP; a
//! Mac's or PC's SMB is often just the admin share. Our own machine and
//! offline tailnet nodes are never suggested.

use crate::model::{Device, DeviceKind};
use cx_core::Scheme;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub device_id: String,
    /// Location to open; always parseable by `cx_core::Location::parse`.
    pub uri: String,
    /// "DiskStation", "Time Machine on DiskStation".
    pub title: String,
    /// "SMB · diskstation.local".
    pub subtitle: String,
    pub scheme: Scheme,
    /// Higher is better; only meaningful relative to other suggestions.
    pub score: i32,
}

fn service_score(kind: DeviceKind, scheme: Scheme, on_tailnet: bool) -> i32 {
    use DeviceKind::*;
    match (scheme, kind) {
        (Scheme::Peer, _) => 100,
        (Scheme::Smb, Nas) => 90,
        (Scheme::Davs, Nas) => 70,
        (Scheme::Dav, Nas) => 65,
        (Scheme::Sftp, Linux) if on_tailnet => 80,
        (Scheme::Sftp, Linux) => 72,
        (Scheme::Sftp, Nas) => 60,
        (Scheme::Smb, Pc) => 68,
        (Scheme::Smb, Mac) => 58,
        (Scheme::Smb, _) => 55,
        (Scheme::Sftp, Mac) => 50,
        (Scheme::Sftp, _) => 52,
        (Scheme::Davs, _) => 48,
        (Scheme::Dav, _) => 45,
        (Scheme::Ftps, _) => 35,
        (Scheme::Ftp, _) => 30,
        (Scheme::S3, _) => 40,
    }
}

/// Rank the services and share hints of `devices`, best first, one entry
/// per URI.
pub fn suggestions(devices: &[Device]) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = Vec::new();
    for d in devices {
        if d.is_self() || !d.is_online() {
            continue;
        }
        let on_tailnet = d.tailnet.is_some();
        for s in &d.services {
            let host = crate::util::url_host(&s.uri).unwrap_or_default();
            let host = if s.scheme == Scheme::Peer { d.hostname.clone().unwrap_or(host) } else { host };
            out.push(Suggestion {
                device_id: d.id.clone(),
                uri: s.uri.clone(),
                title: d.name.clone(),
                subtitle: format!("{} · {host}", s.label),
                scheme: s.scheme,
                score: service_score(d.kind, s.scheme, on_tailnet),
            });
        }
        for share in &d.shares {
            // A named share beats the bare server root it lives on.
            let base = d.services.iter().find(|s| share.uri.starts_with(&s.uri)).map(|s| service_score(d.kind, s.scheme, on_tailnet)).unwrap_or(service_score(d.kind, Scheme::Smb, on_tailnet));
            let host = crate::util::url_host(&share.uri).unwrap_or_default();
            out.push(Suggestion {
                device_id: d.id.clone(),
                uri: share.uri.clone(),
                title: format!("{} on {}", share.name, d.name),
                subtitle: format!("Shared folder · {host}"),
                scheme: Scheme::Smb,
                score: base + 3,
            });
        }
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase())).then_with(|| a.uri.cmp(&b.uri)));
    let mut seen = std::collections::HashSet::new();
    out.retain(|s| seen.insert(s.uri.clone()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Service, ShareHint, Source, TailnetInfo};

    fn device(id: &str, name: &str, kind: DeviceKind, services: &[(Scheme, &str)], tailnet: Option<(bool, bool)>) -> Device {
        Device {
            id: id.into(),
            name: name.into(),
            kind,
            model: None,
            addresses: vec![],
            hostname: None,
            sources: vec![if tailnet.is_some() { Source::Tailscale } else { Source::Mdns }],
            tailnet: tailnet.map(|(online, is_self)| TailnetInfo { online, os: String::new(), owner: None, shared_by: None, is_self, dns_name: None, last_seen: None, tags: vec![] }),
            services: services.iter().map(|(scheme, uri)| Service { scheme: *scheme, port: scheme.default_port(), uri: uri.to_string(), label: scheme.label().into(), source: Source::Probe }).collect(),
            shares: vec![],
            last_seen: 0,
        }
    }

    #[test]
    fn ranks_useful_things_first() {
        let mut nas = device("mdns:nas.local", "DiskStation", DeviceKind::Nas, &[(Scheme::Smb, "smb://nas.local/"), (Scheme::Ftp, "ftp://nas.local/")], None);
        nas.shares.push(ShareHint { name: "TimeMachine".into(), uri: "smb://nas.local/TimeMachine/".into(), source: Source::Mdns });
        let devices = vec![
            device("ts:me", "My Mac", DeviceKind::Mac, &[(Scheme::Smb, "smb://me/")], Some((true, true))),
            device("ts:off", "r38", DeviceKind::Pc, &[(Scheme::Smb, "smb://r38/")], Some((false, false))),
            device("ts:hpc", "hpc", DeviceKind::Pc, &[(Scheme::Smb, "smb://hpc.x.ts.net/"), (Scheme::Sftp, "sftp://hpc.x.ts.net/")], Some((true, false))),
            device("ts:alpha", "alpha", DeviceKind::Linux, &[(Scheme::Sftp, "sftp://alpha.x.ts.net/")], Some((true, false))),
            device("peer:p", "Laptop", DeviceKind::Mac, &[(Scheme::Peer, "peer://p/")], None),
            nas,
        ];
        let s = suggestions(&devices);
        let uris: Vec<&str> = s.iter().map(|s| s.uri.as_str()).collect();
        assert_eq!(uris, ["peer://p/", "smb://nas.local/TimeMachine/", "smb://nas.local/", "sftp://alpha.x.ts.net/", "smb://hpc.x.ts.net/", "sftp://hpc.x.ts.net/", "ftp://nas.local/"]);
        assert_eq!(s[1].title, "TimeMachine on DiskStation");
        assert_eq!(s[2].subtitle, "SMB · nas.local");
        let json = serde_json::to_value(&s[0]).unwrap();
        assert_eq!(json["deviceId"], "peer:p");
        assert_eq!(json["scheme"], "peer");
    }
}
