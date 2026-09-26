//! Device-kind heuristics. Every source gives a guess with a confidence
//! (0–10); the merged device takes the most confident one. That lets an
//! explicit signal (a UPnP `deviceType`, an Apple model identifier) beat a
//! vague one (Tailscale saying "linux" about a Synology box, a name
//! containing "nas").

use crate::model::DeviceKind;

/// A kind guess and how sure we are of it.
pub(crate) type KindGuess = (u8, DeviceKind);

pub(crate) fn best(guesses: impl IntoIterator<Item = Option<KindGuess>>) -> Option<KindGuess> {
    guesses.into_iter().flatten().max_by_key(|(c, _)| *c)
}

/// Tailscale's `OS` field, refined by the host name (an iPad reports "iOS").
pub(crate) fn from_tailscale_os(os: &str, name: &str) -> Option<KindGuess> {
    let by_name = from_name(name);
    let guess = match os.to_ascii_lowercase().as_str() {
        "macos" => (8, DeviceKind::Mac),
        "windows" => (8, DeviceKind::Pc),
        // Plenty of NAS boxes and routers run Tailscale as "linux": keep this
        // weak so a name like "synology" wins.
        "linux" | "freebsd" | "openbsd" | "illumos" => (2, DeviceKind::Linux),
        "ios" | "android" => match by_name {
            Some((_, DeviceKind::Tablet)) => (7, DeviceKind::Tablet),
            _ => (6, DeviceKind::Phone),
        },
        _ => return by_name,
    };
    best([Some(guess), by_name])
}

/// Apple model identifiers from `_device-info._tcp` TXT `model=`.
pub(crate) fn from_model(model: &str) -> Option<KindGuess> {
    let m = model.to_ascii_lowercase();
    let starts = |p: &[&str]| p.iter().any(|p| m.starts_with(p));
    Some(if starts(&["macbook", "imac", "macmini", "macpro", "mac1", "mac2", "mac3"]) {
        (9, DeviceKind::Mac)
    } else if starts(&["iphone", "ipod"]) {
        (9, DeviceKind::Phone)
    } else if starts(&["ipad"]) {
        (9, DeviceKind::Tablet)
    } else if starts(&["timecapsule", "airport"]) {
        (7, DeviceKind::Router)
    } else if starts(&["xserve", "rackmac"]) {
        // Nobody runs an Xserve any more; Synology, QNAP and Samba's
        // fruit module all claim to be one so Finder shows a server icon.
        (6, DeviceKind::Nas)
    } else if starts(&["appletv", "audioaccessory"]) {
        return None;
    } else {
        return from_name(model);
    })
}

/// Words in a friendly name or host name. Weak on purpose.
pub(crate) fn from_name(name: &str) -> Option<KindGuess> {
    let n = name.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    let kind = if has(&["synology", "diskstation", "rackstation", "qnap", "truenas", "freenas", "unraid", "readynas", "asustor", "terramaster", "mycloud", "nas"]) {
        DeviceKind::Nas
    } else if has(&["macbook", "imac", "mac mini", "mac-mini", "macmini", "mac studio", "mac-studio", "mac pro"]) {
        DeviceKind::Mac
    } else if has(&["ipad", "galaxy tab", "tablet"]) {
        DeviceKind::Tablet
    } else if has(&["iphone", "pixel", "galaxy", "android", "oneplus", "xiaomi", "redmi"]) {
        DeviceKind::Phone
    } else if has(&["router", "gateway", "openwrt", "ubiquiti", "unifi", "mikrotik", "fritz", "archer", "tp-link", "netgear"]) {
        DeviceKind::Router
    } else if has(&["printer", "laserjet", "officejet", "deskjet", "epson", "brother", "canon", "kyocera"]) {
        DeviceKind::Printer
    } else if has(&["desktop-", "laptop-", "-pc", "windows"]) {
        DeviceKind::Pc
    } else if has(&["raspberrypi", "ubuntu", "debian", "fedora", "linux"]) {
        DeviceKind::Linux
    } else {
        return None;
    };
    Some((3, kind))
}

/// UPnP device description (`deviceType`, `manufacturer`, `modelName`) and
/// the SSDP `SERVER` header.
pub(crate) fn from_upnp(device_type: &str, manufacturer: &str, model: &str, server: &str) -> Option<KindGuess> {
    let t = device_type.to_ascii_lowercase();
    let all = format!("{manufacturer} {model} {server}").to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| all.contains(w));
    // The device type is explicit, so it wins over the vendor: Synology and
    // QNAP also make routers.
    if t.contains("internetgatewaydevice") || t.contains("wfadevice") {
        return Some((9, DeviceKind::Router));
    }
    if t.contains("print") {
        return Some((9, DeviceKind::Printer));
    }
    if has(&["synology", "qnap", "asustor", "terramaster", "readynas", "western digital", "wd my cloud", "buffalo", "truenas", "unraid"]) {
        return Some((8, DeviceKind::Nas));
    }
    if has(&["windows"]) && !t.contains("mediarenderer") {
        return Some((5, DeviceKind::Pc));
    }
    best([from_name(model), from_name(manufacturer)])
}

/// WS-Discovery `Types` (e.g. `wsdp:Device pub:Computer`).
pub(crate) fn from_wsd_types(types: &str) -> Option<KindGuess> {
    let t = types.to_ascii_lowercase();
    if t.contains("computer") {
        Some((6, DeviceKind::Pc))
    } else if t.contains("print") || t.contains("scan") {
        Some((8, DeviceKind::Printer))
    } else {
        None
    }
}

/// Which mDNS service types a host offers. SSH alone suggests a Unix box.
pub(crate) fn from_mdns_service(service_type: &str) -> Option<KindGuess> {
    match service_type {
        "_ssh._tcp" | "_sftp-ssh._tcp" => Some((1, DeviceKind::Linux)),
        "_afpovertcp._tcp" | "_adisk._tcp" => Some((1, DeviceKind::Mac)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use DeviceKind::*;

    fn kind(g: Option<KindGuess>) -> DeviceKind {
        g.map(|g| g.1).unwrap_or(Unknown)
    }

    #[test]
    fn tailscale_os() {
        assert_eq!(kind(from_tailscale_os("macOS", "Alex's MacBook Pro")), Mac);
        assert_eq!(kind(from_tailscale_os("windows", "hpc")), Pc);
        assert_eq!(kind(from_tailscale_os("linux", "alpha")), Linux);
        assert_eq!(kind(from_tailscale_os("linux", "synology")), Nas);
        assert_eq!(kind(from_tailscale_os("android", "Alex's Phone")), Phone);
        assert_eq!(kind(from_tailscale_os("iOS", "localhost")), Phone);
        assert_eq!(kind(from_tailscale_os("iOS", "Alex's iPad")), Tablet);
    }

    #[test]
    fn apple_models() {
        assert_eq!(kind(from_model("MacBookPro18,1")), Mac);
        assert_eq!(kind(from_model("Macmini9,1")), Mac);
        assert_eq!(kind(from_model("Mac14,13")), Mac);
        assert_eq!(kind(from_model("iPhone15,2")), Phone);
        assert_eq!(kind(from_model("iPad13,4")), Tablet);
        assert_eq!(kind(from_model("Xserve")), Nas);
        assert_eq!(kind(from_model("AppleTV14,1")), Unknown);
    }

    #[test]
    fn upnp() {
        assert_eq!(kind(from_upnp("urn:schemas-upnp-org:device:InternetGatewayDevice:1", "TP-Link", "Archer C6", "")), Router);
        assert_eq!(kind(from_upnp("urn:schemas-upnp-org:device:MediaServer:1", "Synology Inc", "DS920+", "")), Nas);
        assert_eq!(kind(from_upnp("urn:schemas-upnp-org:device:Basic:1", "", "", "Linux/4.4 UPnP/1.0 QNAP/5.1")), Nas);
        assert_eq!(kind(from_upnp("urn:schemas-upnp-org:device:MediaRenderer:1", "Google Inc.", "Chromecast", "")), Unknown);
    }

    #[test]
    fn explicit_beats_vague() {
        let merged = best([from_tailscale_os("linux", "box"), from_upnp("", "Synology Inc", "DS220+", "")]);
        assert_eq!(kind(merged), Nas);
        let merged = best([from_mdns_service("_ssh._tcp"), from_tailscale_os("macOS", "x")]);
        assert_eq!(kind(merged), Mac);
    }
}
