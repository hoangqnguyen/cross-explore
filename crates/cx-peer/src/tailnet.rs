//! Optional zero-config trust inside a Tailscale tailnet.
//!
//! When enabled, a connection from a tailnet address (100.64.0.0/10 or
//! fd7a:115c:a1e0::/48) is accepted without pairing if `tailscale whois`
//! says the remote node belongs to the same Tailscale user as this node.
//! Tailscale has already authenticated that node with WireGuard keys, so the
//! source address is as good as an identity. We use the CLI rather than the
//! local API socket because its location differs per platform.

use serde_json::Value;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

const CANDIDATES: &[&str] = &["/usr/local/bin/tailscale", "/usr/bin/tailscale", "/opt/homebrew/bin/tailscale", "/Applications/Tailscale.app/Contents/MacOS/Tailscale"];

pub fn is_tailnet_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            o[0] == 100 && (o[1] & 0xc0) == 64
        }
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_tailnet_ip(IpAddr::V4(v4)),
            None => v6.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
        },
    }
}

fn find_cli() -> Option<PathBuf> {
    let exe = if cfg!(windows) { "tailscale.exe" } else { "tailscale" };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    if cfg!(windows) {
        let p = PathBuf::from(r"C:\Program Files\Tailscale\tailscale.exe");
        return p.is_file().then_some(p);
    }
    CANDIDATES.iter().map(PathBuf::from).find(|p| p.is_file())
}

async fn run_json(args: &[&str]) -> Option<Value> {
    let cli = find_cli()?;
    let out = tokio::time::timeout(Duration::from_secs(5), Command::new(cli).args(args).kill_on_drop(true).output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

/// Tailscale user ids are numbers in current versions; compare as strings.
fn id_string(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Tagged nodes all share the pseudo-user "tagged-devices"; matching on
/// that would trust every tagged server in the tailnet, so they never count.
fn tagged(node: &Value) -> bool {
    node.get("Tags").and_then(Value::as_array).is_some_and(|t| !t.is_empty())
}

pub fn self_user(status: &Value) -> Option<String> {
    let me = status.get("Self")?;
    if tagged(me) {
        return None;
    }
    id_string(me.get("UserID")?)
}

pub fn whois_user(whois: &Value) -> Option<String> {
    if whois.get("Node").is_some_and(tagged) {
        return None;
    }
    let profile = whois.get("UserProfile");
    if profile.and_then(|p| p.get("LoginName")).and_then(Value::as_str) == Some("tagged-devices") {
        return None;
    }
    profile.and_then(|u| u.get("ID")).and_then(id_string).or_else(|| whois.get("Node")?.get("User").and_then(id_string))
}

/// Answers "is this address a node of my own Tailscale user?".
#[derive(Default)]
pub struct Tailnet {
    self_user: Mutex<Option<String>>,
}

impl Tailnet {
    pub async fn same_user(&self, ip: IpAddr) -> bool {
        if !is_tailnet_ip(ip) {
            return false;
        }
        let me = {
            let mut g = self.self_user.lock().await;
            if g.is_none() {
                *g = run_json(&["status", "--json"]).await.as_ref().and_then(self_user);
            }
            g.clone()
        };
        let Some(me) = me else { return false };
        let ip = match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        };
        let them = run_json(&["whois", "--json", &ip.to_string()]).await;
        them.as_ref().and_then(whois_user).is_some_and(|u| u == me)
    }
}

/// This node's tailnet IPv4 address, for binding only to the tailnet.
pub async fn local_ipv4() -> Option<IpAddr> {
    let cli = find_cli()?;
    let out = Command::new(cli).args(["ip", "-4"]).output().await.ok()?;
    String::from_utf8_lossy(&out.stdout).lines().next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_tailnet_ranges() {
        for ip in ["100.64.0.1", "100.101.102.103", "100.127.255.254", "fd7a:115c:a1e0::1", "::ffff:100.100.1.1"] {
            assert!(is_tailnet_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["100.63.0.1", "100.128.0.1", "192.168.1.2", "127.0.0.1", "fd00::1"] {
            assert!(!is_tailnet_ip(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn parses_cli_json() {
        let status: Value = serde_json::from_str(r#"{"Self":{"UserID":12345,"HostName":"mac"}}"#).unwrap();
        let whois: Value = serde_json::from_str(r#"{"Node":{"User":12345},"UserProfile":{"ID":12345,"LoginName":"me@example.com"}}"#).unwrap();
        assert_eq!(self_user(&status).as_deref(), Some("12345"));
        assert_eq!(whois_user(&whois), self_user(&status));
        let tagged: Value = serde_json::from_str(r#"{"Node":{"User":12345,"Tags":["tag:server"]},"UserProfile":{"ID":12345}}"#).unwrap();
        assert_eq!(whois_user(&tagged), None);
    }
}
