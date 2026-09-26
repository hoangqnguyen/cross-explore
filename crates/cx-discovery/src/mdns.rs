//! mDNS / DNS-SD: the richest LAN source. Each resolved service instance
//! becomes one observation keyed by its full name, so a goodbye packet
//! removes exactly that service; records of the same host merge into one
//! device through the SRV host name (and the instance name, which macOS
//! uses for all of a machine's services).

use crate::kind;
use crate::model::{Advertisement, Source};
use crate::registry::{self, name_rank, rank, Observation, ObservedService, ObservedShare};
use crate::util::{host_label, normalize_hostname};
use cx_core::Scheme;
use std::collections::HashMap;
use std::net::IpAddr;

/// Service types we browse, without the `.local.` domain.
pub(crate) const BROWSE_TYPES: &[&str] = &[
    "_smb._tcp",
    "_sftp-ssh._tcp",
    "_ssh._tcp",
    "_ftp._tcp",
    "_webdav._tcp",
    "_webdavs._tcp",
    "_afpovertcp._tcp",
    "_adisk._tcp",
    PEER_UDP,
    PEER_TCP,
];

/// Our peer service. QUIC runs over UDP, so `_udp` is the correct DNS-SD
/// type; `_tcp` is announced too for browsers that only look there.
pub(crate) const PEER_UDP: &str = "_crossx._udp";
pub(crate) const PEER_TCP: &str = "_crossx._tcp";

/// A resolved instance, decoupled from `mdns-sd` types so it can be tested.
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    /// `_smb._tcp` (no domain).
    pub service_type: String,
    pub fullname: String,
    /// SRV target, e.g. `NAS.local.`.
    pub host: String,
    pub port: u16,
    pub addresses: Vec<IpAddr>,
    pub txt: HashMap<String, String>,
}

impl Resolved {
    pub fn from_mdns(r: &mdns_sd::ResolvedService) -> Resolved {
        let service_type = r.ty_domain.trim_end_matches('.').trim_end_matches(".local").to_string();
        let txt = r.txt_properties.iter().map(|p| (p.key().to_ascii_lowercase(), p.val_str().to_string())).collect();
        Resolved { service_type, fullname: r.fullname.clone(), host: r.host.clone(), port: r.port, addresses: r.addresses.iter().map(|a| a.to_ip_addr()).collect(), txt }
    }

    /// The human part of the full name ("NAS" in `NAS._smb._tcp.local.`).
    pub fn instance(&self) -> String {
        let suffix = format!(".{}.local.", self.service_type);
        let name = self.fullname.strip_suffix(&suffix).or_else(|| self.fullname.strip_suffix(suffix.trim_end_matches('.'))).unwrap_or(&self.fullname);
        name.replace("\\.", ".").replace("\\\\", "\\")
    }
}

pub(crate) fn observation(r: &Resolved, now: i64) -> Observation {
    let host = normalize_hostname(&r.host);
    let instance = r.instance();
    let mut o = Observation::new(Source::Mdns, r.fullname.clone(), now);
    o.identity = (!host.is_empty()).then(|| (rank::MDNS, format!("mdns:{host}")));
    o.name = Some((name_rank::MDNS_INSTANCE, instance.clone()));
    o.addresses = r.addresses.clone();
    o.hostnames = registry::hostnames(&[&host]);
    o.link_names = registry::link_names(&[&instance, host_label(&host)]);
    o.kind = kind::from_mdns_service(&r.service_type);
    let service = |scheme: Scheme, path: Vec<String>| ObservedService { scheme, port: r.port, host: host.clone(), path };
    let txt_path = || r.txt.get("path").map(|p| p.split('/').filter(|s| !s.is_empty()).map(str::to_string).collect()).unwrap_or_default();
    match r.service_type.as_str() {
        "_smb._tcp" => o.services.push(service(Scheme::Smb, vec![])),
        "_sftp-ssh._tcp" | "_ssh._tcp" => o.services.push(service(Scheme::Sftp, vec![])),
        "_ftp._tcp" => o.services.push(service(Scheme::Ftp, txt_path())),
        "_webdav._tcp" => o.services.push(service(Scheme::Dav, txt_path())),
        "_webdavs._tcp" => o.services.push(service(Scheme::Davs, txt_path())),
        "_adisk._tcp" => o.shares = adisk_shares(&r.txt, &host),
        PEER_UDP | PEER_TCP => {
            let id = r.txt.get("id").filter(|id| is_peer_id(id)).cloned();
            if let Some(name) = r.txt.get("name").filter(|n| !n.is_empty()) {
                o.name = Some((name_rank::PEER, name.clone()));
                o.link_names = registry::link_names(&[name, &instance, host_label(&host)]);
            }
            if let Some(id) = id {
                o.identity = Some((rank::PEER, format!("peer:{id}")));
                o.services.push(ObservedService { scheme: Scheme::Peer, port: Scheme::Peer.default_port(), host: id, path: vec![] });
            } else {
                o.services.push(service(Scheme::Peer, vec![]));
            }
        }
        _ => {}
    }
    if let Some(model) = r.txt.get("model").filter(|m| !m.is_empty()) {
        o.kind = kind::best([o.kind, kind::from_model(model)]);
        o.model = Some(model.clone());
    }
    o
}

/// A peer id ends up as the host part of `peer://<id>/`, so it must be a
/// plain DNS-label-ish token; anything else is ignored rather than trusted.
fn is_peer_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 63 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// `_adisk._tcp` TXT: `dk0=adVN=TimeMachine,adVF=0x82`, `dk1=…`. `adVN` is
/// the SMB share name (Time Machine destinations and other advertised
/// volumes); `sys=` carries flags only.
pub(crate) fn adisk_shares(txt: &HashMap<String, String>, host: &str) -> Vec<ObservedShare> {
    let mut keys: Vec<&String> = txt.keys().filter(|k| k.starts_with("dk")).collect();
    keys.sort();
    keys.into_iter()
        .filter_map(|k| {
            let name = txt[k].split(',').find_map(|kv| kv.split_once('=').filter(|(k, _)| k.eq_ignore_ascii_case("adVN")).map(|(_, v)| v.trim().to_string()))?;
            (!name.is_empty()).then(|| ObservedShare { name, scheme: Scheme::Smb, host: host.to_string(), port: Scheme::Smb.default_port() })
        })
        .collect()
}

/// Observation for a `_device-info._tcp` instance (see `dnssd`). It only
/// decorates: it carries no address, just a name to match and a model.
pub(crate) fn device_info_observation(instance: &str, model: &str, now: i64, ttl_ms: i64) -> Observation {
    let mut o = Observation::new(Source::Mdns, format!("device-info:{instance}"), now);
    o.standalone = false;
    o.expires = now + ttl_ms;
    o.name = Some((name_rank::MDNS_INSTANCE, instance.to_string()));
    o.link_names = registry::link_names(&[instance]);
    o.model = Some(model.to_string());
    o.kind = kind::from_model(model);
    o
}

/// Build the `mdns-sd` registrations for our peer service.
///
/// The records point at `<hostname>-crossx.local`, not the machine's own
/// `.local` name: that one belongs to the OS responder (mDNSResponder,
/// Avahi), and claiming it too makes `mdns-sd` detect a conflict and rename
/// us to `<hostname>-2.local`. Other instances match us by peer id and
/// address anyway.
pub(crate) fn advertisement_infos(ad: &Advertisement) -> Result<Vec<mdns_sd::ServiceInfo>, mdns_sd::Error> {
    let raw = gethostname::gethostname().to_string_lossy().to_string();
    let label: String = host_label(raw.trim_end_matches(".local")).chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' }).collect();
    let host = format!("{}-crossx.local.", if label.is_empty() { "peer" } else { &label });
    let mut txt: Vec<(String, String)> = vec![("id".into(), ad.device_id.clone()), ("name".into(), ad.name.clone())];
    txt.extend(ad.txt.iter().filter(|(k, _)| k != "id" && k != "name").cloned());
    let props: HashMap<String, String> = txt.into_iter().collect();
    [PEER_UDP, PEER_TCP]
        .iter()
        .map(|ty| mdns_sd::ServiceInfo::new(&format!("{ty}.local."), &ad.name, &host, "", ad.port, props.clone()).map(|i| i.enable_addr_auto()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceKind;
    use crate::registry::build_devices;

    fn resolved(ty: &str, instance: &str, host: &str, port: u16, txt: &[(&str, &str)]) -> Resolved {
        Resolved {
            service_type: ty.into(),
            fullname: format!("{instance}.{ty}.local."),
            host: host.into(),
            port,
            addresses: vec!["192.168.1.10".parse().unwrap(), "fe80::1".parse().unwrap()],
            txt: txt.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn synology_records_merge_into_one_nas() {
        let now = 1;
        let smb = observation(&resolved("_smb._tcp", "DiskStation", "DiskStation.local.", 445, &[]), now);
        let afp = observation(&resolved("_afpovertcp._tcp", "DiskStation", "DiskStation.local.", 548, &[]), now);
        let adisk = observation(&resolved("_adisk._tcp", "DiskStation", "DiskStation.local.", 9, &[("sys", "waMa=0,adVF=0x100"), ("dk0", "adVN=TimeMachine,adVF=0x82"), ("dk1", "adVN=Family Backup,adVF=0x82")]), now);
        let dav = observation(&resolved("_webdavs._tcp", "DiskStation", "DiskStation.local.", 5006, &[("path", "/dav")]), now);
        let info = device_info_observation("DiskStation", "Xserve", now, 1000);
        let devices = build_devices(&[&smb, &afp, &adisk, &dav, &info]);
        assert_eq!(devices.len(), 1, "{devices:#?}");
        let d = &devices[0];
        assert_eq!(d.id, "mdns:diskstation.local");
        assert_eq!(d.name, "DiskStation");
        assert_eq!(d.kind, DeviceKind::Nas);
        assert_eq!(d.model.as_deref(), Some("Xserve"));
        let uris: Vec<&str> = d.services.iter().map(|s| s.uri.as_str()).collect();
        assert_eq!(uris, ["smb://diskstation.local/", "davs://diskstation.local:5006/dav/"]);
        let shares: Vec<(&str, &str)> = d.shares.iter().map(|s| (s.name.as_str(), s.uri.as_str())).collect();
        assert_eq!(shares, [("Family Backup", "smb://diskstation.local/Family%20Backup/"), ("TimeMachine", "smb://diskstation.local/TimeMachine/")]);
    }

    #[test]
    fn mac_services_and_device_info_merge_by_instance_name() {
        let ssh = observation(&resolved("_ssh._tcp", "Alex’s MacBook Pro", "Alexs-MacBook-Pro.local.", 22, &[]), 1);
        let info = device_info_observation("Alex’s MacBook Pro", "MacBookPro18,1", 1, 1000);
        let d = &build_devices(&[&ssh, &info])[0];
        assert_eq!(d.kind, DeviceKind::Mac, "model beats the ssh-means-linux hint");
        assert_eq!(d.services[0].uri, "sftp://alexs-macbook-pro.local/");
        assert_eq!(d.addresses[0].to_string(), "192.168.1.10", "IPv4 LAN first, link-local last");
    }

    #[test]
    fn peer_txt_sets_identity_and_name() {
        let r = resolved(PEER_UDP, "hpc", "hpc.local.", 47470, &[("id", "7f3a9c-desk"), ("name", "Alex's Desktop")]);
        let d = &build_devices(&[&observation(&r, 1)])[0];
        assert_eq!(d.id, "peer:7f3a9c-desk");
        assert_eq!(d.name, "Alex's Desktop");
        assert_eq!(d.services[0].uri, "peer://7f3a9c-desk/");
        cx_core::Location::parse(&d.services[0].uri).unwrap();

        let evil = resolved(PEER_UDP, "x", "x.local.", 47470, &[("id", "a/b@c")]);
        let d = &build_devices(&[&observation(&evil, 1)])[0];
        assert_eq!(d.services[0].uri, "peer://x.local/", "untrusted id ignored");
    }

    #[test]
    fn escaped_instance_names() {
        let r = Resolved { fullname: "My\\.Share._smb._tcp.local.".into(), ..resolved("_smb._tcp", "x", "h.local.", 445, &[]) };
        assert_eq!(r.instance(), "My.Share");
    }

    #[test]
    fn advertisement_registers_both_transports() {
        let ad = Advertisement { name: "Alex's MacBook Pro".into(), port: 47470, device_id: "abc".into(), txt: vec![("v".into(), "1".into())] };
        let infos = advertisement_infos(&ad).unwrap();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].get_type(), "_crossx._udp.local.");
        assert_eq!(infos[1].get_property_val_str("id"), Some("abc"));
        assert_eq!(infos[1].get_property_val_str("v"), Some("1"));
        // Never the OS's own .local name: claiming it makes mDNSResponder
        // rename the machine (Foo.local → Foo-2.local) on conflict.
        assert!(infos[0].get_hostname().ends_with("-crossx.local."), "{}", infos[0].get_hostname());
    }
}
