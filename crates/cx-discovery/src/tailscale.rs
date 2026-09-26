//! Tailscale: every node on the tailnet (and nodes shared into it), with
//! OS, owner, MagicDNS name and online state.
//!
//! We read the same status document two ways: Tailscale's LocalAPI unix
//! socket on Linux (no process spawn, works when the CLI is not on PATH),
//! and `tailscale status --json` everywhere else. On macOS the GUI app
//! ships the CLI inside its bundle, and the App Store variant has no
//! socket at all, so the CLI is the portable path.

use crate::kind;
use crate::model::{Source, TailnetInfo};
use crate::registry::{self, name_rank, rank, Observation};
use crate::util::{is_lan_ip, is_tailnet_ip, normalize_hostname, normalize_name, parse_rfc3339_ms};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Key prefix of every Tailscale observation (see `Registry::replace`).
pub(crate) const KEY_PREFIX: &str = "node:";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct Status {
    backend_state: String,
    #[serde(rename = "Self")]
    self_node: Option<Node>,
    peer: Option<HashMap<String, Node>>,
    user: Option<HashMap<String, User>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct Node {
    #[serde(rename = "ID")]
    id: String,
    host_name: String,
    #[serde(rename = "DNSName")]
    dns_name: String,
    #[serde(rename = "OS")]
    os: String,
    #[serde(rename = "UserID")]
    user_id: serde_json::Value,
    #[serde(rename = "AltSharerUserID")]
    alt_sharer_user_id: serde_json::Value,
    #[serde(rename = "TailscaleIPs")]
    tailscale_ips: Option<Vec<IpAddr>>,
    addrs: Option<Vec<String>>,
    cur_addr: String,
    online: bool,
    last_seen: String,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase", default)]
struct User {
    login_name: String,
}

/// Turn `tailscale status --json` into one observation per node. Returns an
/// empty list when Tailscale is installed but not running or logged out,
/// which makes all tailnet devices disappear, as they should.
pub(crate) fn parse_status(json: &str, now: i64, ttl_ms: i64) -> Result<Vec<Observation>, serde_json::Error> {
    let status: Status = serde_json::from_str(json)?;
    if status.backend_state != "Running" {
        return Ok(Vec::new());
    }
    let users = status.user.unwrap_or_default();
    let login = |v: &serde_json::Value| -> Option<String> {
        let key = match v {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s.clone(),
            _ => return None,
        };
        users.get(&key).map(|u| u.login_name.clone()).filter(|l| !l.is_empty())
    };
    let mut out = Vec::new();
    if let Some(me) = &status.self_node {
        out.push(node_observation(me, true, &login, now, ttl_ms));
    }
    let mut peers: Vec<&Node> = status.peer.as_ref().map(|p| p.values().collect()).unwrap_or_default();
    peers.sort_by(|a, b| a.id.cmp(&b.id));
    out.extend(peers.into_iter().filter(|n| !n.id.is_empty()).map(|n| node_observation(n, false, &login, now, ttl_ms)));
    Ok(out)
}

/// The name people gave the machine. `HostName` is the OS host name
/// ("Alex's MacBook Pro", but also "localhost" for iPhones), while the
/// first label of the MagicDNS name is what the admin console shows and may
/// have been renamed ("alpha" for a box whose OS calls itself
/// "edge-box-f942"). Prefer `HostName` when the DNS label is just its slug.
fn friendly_name(node: &Node) -> String {
    let label = normalize_hostname(&node.dns_name);
    let label = label.split('.').next().unwrap_or("");
    let host = node.host_name.trim();
    if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
        return if label.is_empty() { host.to_string() } else { label.to_string() };
    }
    let (h, l) = (normalize_name(host), normalize_name(label));
    if label.is_empty() || l.starts_with(&h) {
        host.to_string()
    } else {
        label.to_string()
    }
}

fn node_observation(node: &Node, is_self: bool, login: &dyn Fn(&serde_json::Value) -> Option<String>, now: i64, ttl_ms: i64) -> Observation {
    let seen = if node.online || is_self { now } else { parse_rfc3339_ms(&node.last_seen).unwrap_or(0) };
    let mut o = Observation::new(Source::Tailscale, format!("{KEY_PREFIX}{}", node.id), seen);
    o.expires = now + ttl_ms;
    o.identity = Some((rank::TAILSCALE, format!("ts:{}", node.id)));
    let name = friendly_name(node);
    o.name = Some((name_rank::TAILSCALE, name.clone()));
    o.kind = kind::from_tailscale_os(&node.os, &name);
    let dns = normalize_hostname(&node.dns_name);
    o.hostnames = registry::hostnames(&[&dns]);
    let host_name = if node.host_name.eq_ignore_ascii_case("localhost") { "" } else { node.host_name.as_str() };
    o.link_names = registry::link_names(&[host_name, &name]);
    o.addresses = node.tailscale_ips.clone().unwrap_or_default();
    // LAN addresses let us merge with what mDNS/SSDP see, but only when we
    // know they are on *our* LAN: our own endpoints, or a peer Tailscale is
    // currently talking to directly over a private address. Another site's
    // 192.168.1.40 is somebody else's machine.
    let parse_ip = |s: &str| s.parse::<SocketAddr>().ok().map(|a| a.ip());
    if is_self {
        o.addresses.extend(node.addrs.iter().flatten().filter_map(|a| parse_ip(a)).filter(is_lan_ip));
    } else if let Some(cur) = parse_ip(&node.cur_addr).filter(|ip| is_lan_ip(ip) && !is_tailnet_ip(ip)) {
        o.addresses.push(cur);
    }
    let owner = login(&node.user_id);
    let shared_by = login(&node.alt_sharer_user_id);
    o.tailnet = Some(TailnetInfo {
        online: node.online || is_self,
        os: node.os.clone(),
        owner,
        shared_by,
        is_self,
        dns_name: (!dns.is_empty()).then_some(dns),
        last_seen: if node.online || is_self { None } else { parse_rfc3339_ms(&node.last_seen) },
        tags: node.tags.clone().unwrap_or_default(),
    });
    o
}

/// Where the CLI usually lives. PATH first so a user's choice wins.
pub(crate) fn find_binary(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    let exe = if cfg!(windows) { "tailscale.exe" } else { "tailscale" };
    let on_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|d| d.join(exe));
    let known = [
        "/usr/local/bin/tailscale",
        "/opt/homebrew/bin/tailscale",
        "/usr/bin/tailscale",
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        r"C:\Program Files\Tailscale\tailscale.exe",
        r"C:\Program Files (x86)\Tailscale\tailscale.exe",
    ]
    .into_iter()
    .map(PathBuf::from);
    on_path.chain(known).find(|p| p.is_file())
}

/// Fetch the status JSON: LocalAPI socket if there is one, else the CLI.
pub(crate) async fn fetch_status(binary: Option<&Path>, limit: Duration) -> Option<String> {
    #[cfg(unix)]
    if let Some(json) = local_api(limit).await {
        return Some(json);
    }
    let bin = find_binary(binary)?;
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(["status", "--json"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    {
        // Don't flash a console window from the GUI app.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = tokio::time::timeout(limit, cmd.output()).await.ok()?.ok()?;
    // `tailscale status` exits non-zero when stopped but still prints JSON.
    let text = String::from_utf8(out.stdout).ok()?;
    text.trim_start().starts_with('{').then_some(text)
}

#[cfg(unix)]
async fn local_api(limit: Duration) -> Option<String> {
    const SOCKETS: [&str; 2] = ["/var/run/tailscale/tailscaled.sock", "/run/tailscale/tailscaled.sock"];
    let path = SOCKETS.iter().find(|p| Path::new(p).exists())?;
    let deadline = tokio::time::Instant::now() + limit;
    let stream = tokio::time::timeout_at(deadline, tokio::net::UnixStream::connect(path)).await.ok()?.ok()?;
    let resp = crate::http::exchange(stream, "GET", "local-tailscaled.sock", "/localapi/v0/status", 8 << 20, deadline).await.ok()?;
    (resp.status == 200).then(|| String::from_utf8(resp.body).ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::DeviceKind;
    use crate::registry::build_devices;

    const FIXTURE: &str = include_str!("../tests/fixtures/tailscale-status.json");

    #[test]
    fn parses_realistic_status() {
        let obs = parse_status(FIXTURE, 1_000_000, 60_000).unwrap();
        assert_eq!(obs.len(), 7);
        let refs: Vec<&Observation> = obs.iter().collect();
        let devices = build_devices(&refs);
        assert_eq!(devices.len(), 7);
        let by_name = |n: &str| devices.iter().find(|d| d.name == n).unwrap_or_else(|| panic!("{n} missing: {devices:#?}"));

        let me = &devices[0];
        assert_eq!(me.name, "Alex's MacBook Pro");
        assert_eq!(me.kind, DeviceKind::Mac);
        let t = me.tailnet.as_ref().unwrap();
        assert!(t.is_self && t.online);
        assert_eq!(t.owner.as_deref(), Some("owner@example.com"));
        assert_eq!(me.hostname.as_deref(), Some("alexs-macbook-pro-1.example-tail.ts.net"));
        let addrs: Vec<String> = me.addresses.iter().map(|a| a.to_string()).collect();
        assert_eq!(addrs, ["172.27.48.176", "192.168.1.76", "100.101.1.3", "fd7a:115c:a1e0::3701:9961"], "own LAN endpoints, not the public one");

        let hpc = by_name("hpc");
        assert_eq!(hpc.id, "ts:nHpc11CNTRL");
        assert_eq!(hpc.kind, DeviceKind::Pc);
        assert!(hpc.addresses.contains(&"192.168.1.40".parse().unwrap()), "direct LAN path");
        assert!(!hpc.addresses.contains(&"198.51.100.20".parse().unwrap()));

        let alpha = by_name("alpha");
        assert_eq!(alpha.kind, DeviceKind::Linux);
        let t = alpha.tailnet.as_ref().unwrap();
        assert_eq!(t.owner.as_deref(), Some("tagged-devices"));
        assert_eq!(t.shared_by.as_deref(), Some("friend@example.com"));
        assert_eq!(t.tags, ["tag:server"]);

        assert_eq!(by_name("Alex's Phone").kind, DeviceKind::Phone);
        let iphone = by_name("alexs-iphone");
        assert_eq!(iphone.kind, DeviceKind::Phone);
        let t = iphone.tailnet.as_ref().unwrap();
        assert!(!t.online);
        assert_eq!(t.last_seen, parse_rfc3339_ms("2026-09-26T12:10:00.1Z"));
        assert_eq!(iphone.last_seen, t.last_seen.unwrap());

        assert_eq!(by_name("r38").kind, DeviceKind::Pc, "r38-2 is just the DNS de-dup suffix");
        assert_eq!(by_name("Sam’s Mac mini").kind, DeviceKind::Mac);
    }

    #[test]
    fn stopped_backend_yields_nothing() {
        let obs = parse_status(r#"{"BackendState":"Stopped","Self":{"ID":"x"},"Peer":null}"#, 0, 0).unwrap();
        assert!(obs.is_empty());
        assert!(parse_status("not json", 0, 0).is_err());
    }
}
