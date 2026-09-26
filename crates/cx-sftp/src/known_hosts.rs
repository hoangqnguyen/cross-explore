//! Host key verification against OpenSSH `known_hosts` files.
//!
//! Two files are consulted: the user's `~/.ssh/known_hosts` (read-only: we
//! never rewrite a file other tools own) and the app's own store, where keys
//! the user accepted in the UI are recorded by [`trust_host_key`].
//!
//! The app store uses the OpenSSH line format, except that the key column may
//! hold a `SHA256:` fingerprint instead of the base64 key blob. The UI only
//! ever sees fingerprints (that is what it shows the user), and a SHA-256 of
//! the key blob identifies the key just as well as the blob itself.

use cx_core::{CxError, Result};
use russh::keys::{HashAlg, PublicKey};
use std::fs;
use std::io::Write;
use std::path::Path;

/// What the known-hosts files say about a presented key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    Trusted,
    /// No key of this type is recorded for the host.
    Unknown,
    /// A different key of the same type is recorded: possibly an attack, or
    /// the server was reinstalled.
    Changed,
}

/// OpenSSH-style fingerprint, e.g. `SHA256:uNiVztksCsDhcc0u9e8BujQXVUpKZIDTMczCvj3tD2s`.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// Key type name as OpenSSH writes it (`ssh-ed25519`, `ecdsa-sha2-nistp256`, `ssh-rsa`).
pub fn key_type(key: &PublicKey) -> String {
    key.algorithm().as_str().to_string()
}

/// The host column form OpenSSH uses: `host` on port 22, `[host]:port` otherwise.
pub fn host_pattern(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// Check `key` for `host:port` in the given files. A match in any file wins
/// over a mismatch in another, so a key accepted in the app overrides a stale
/// entry in `~/.ssh/known_hosts`.
pub fn check(files: &[&Path], host: &str, port: u16, key: &PublicKey) -> HostKeyStatus {
    let mut changed = false;
    for file in files {
        let Ok(text) = fs::read_to_string(file) else { continue };
        match check_text(&text, host, port, key) {
            HostKeyStatus::Trusted => return HostKeyStatus::Trusted,
            HostKeyStatus::Changed => changed = true,
            HostKeyStatus::Unknown => {}
        }
    }
    if changed {
        HostKeyStatus::Changed
    } else {
        HostKeyStatus::Unknown
    }
}

/// Check one known_hosts file's contents.
pub fn check_text(text: &str, host: &str, port: u16, key: &PublicKey) -> HostKeyStatus {
    let target = host_pattern(host, port);
    let presented_type = key_type(key);
    let presented_fp = fingerprint(key);
    let mut changed = false;
    for line in text.lines() {
        let Some(entry) = parse_line(line) else { continue };
        if !entry.hosts_match(&target) {
            continue;
        }
        if entry.key_type != presented_type {
            // OpenSSH treats another key type as "not known", not "changed".
            continue;
        }
        let same = entry.fingerprint().as_deref() == Some(presented_fp.as_str());
        match entry.marker {
            Marker::Revoked if same => return HostKeyStatus::Changed,
            Marker::Revoked | Marker::CertAuthority => {}
            Marker::None if same => return HostKeyStatus::Trusted,
            Marker::None => changed = true,
        }
    }
    if changed {
        HostKeyStatus::Changed
    } else {
        HostKeyStatus::Unknown
    }
}

/// Record that the user trusts `fingerprint` (as shown in the host-key
/// prompt) for `host:port`. Replaces any earlier key of the same type for
/// that host in the store, so accepting a changed key works too.
pub fn trust_host_key(store: &Path, host: &str, port: u16, key_type: &str, fingerprint: &str) -> Result<()> {
    if key_type.is_empty() || key_type.contains(char::is_whitespace) || !fingerprint.starts_with("SHA256:") || fingerprint.contains(char::is_whitespace) {
        return Err(CxError::InvalidName(format!("{key_type} {fingerprint}")));
    }
    let target = host_pattern(host, port);
    let existing = fs::read_to_string(store).unwrap_or_default();
    let mut out = String::new();
    for line in existing.lines() {
        let drop = parse_line(line).is_some_and(|e| e.marker == Marker::None && e.key_type == key_type && e.hosts_match(&target));
        if !drop {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(&format!("{target} {key_type} {fingerprint}\n"));
    if let Some(dir) = store.parent() {
        fs::create_dir_all(dir).map_err(|e| CxError::from_io(e, dir.display()))?;
    }
    // Write-then-rename so a crash never leaves a half-written store.
    let tmp = store.with_extension("tmp");
    let mut f = fs::File::create(&tmp).map_err(|e| CxError::from_io(e, tmp.display()))?;
    f.write_all(out.as_bytes()).map_err(|e| CxError::from_io(e, tmp.display()))?;
    f.sync_all().ok();
    fs::rename(&tmp, store).map_err(|e| CxError::from_io(e, store.display()))
}

#[derive(Debug, PartialEq, Eq)]
enum Marker {
    None,
    CertAuthority,
    Revoked,
}

struct Line<'a> {
    marker: Marker,
    hosts: &'a str,
    key_type: &'a str,
    key: &'a str,
}

fn parse_line(line: &str) -> Option<Line<'_>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let mut cols = line.split_whitespace();
    let mut first = cols.next()?;
    let marker = match first {
        "@cert-authority" => Marker::CertAuthority,
        "@revoked" => Marker::Revoked,
        m if m.starts_with('@') => return None,
        _ => Marker::None,
    };
    if marker != Marker::None {
        first = cols.next()?;
    }
    Some(Line { marker, hosts: first, key_type: cols.next()?, key: cols.next()? })
}

impl Line<'_> {
    /// SHA256 fingerprint of the recorded key (parsed from the blob, or
    /// taken as-is from an app-store line).
    fn fingerprint(&self) -> Option<String> {
        if self.key.starts_with("SHA256:") {
            return Some(self.key.to_string());
        }
        russh::keys::parse_public_key_base64(self.key).ok().map(|k| fingerprint(&k))
    }

    /// OpenSSH semantics: comma-separated patterns, `!pattern` negates (a
    /// negated match rejects the line outright), `|1|salt|hash` is a hashed
    /// host, `*` and `?` are wildcards.
    fn hosts_match(&self, target: &str) -> bool {
        let mut matched = false;
        for pat in self.hosts.split(',') {
            if let Some(neg) = pat.strip_prefix('!') {
                if pattern_match(neg, target) {
                    return false;
                }
            } else if pat.starts_with("|1|") {
                matched |= hashed_match(pat, target);
            } else {
                matched |= pattern_match(pat, target);
            }
        }
        matched
    }
}

fn pattern_match(pat: &str, target: &str) -> bool {
    glob(&pat.to_ascii_lowercase().chars().collect::<Vec<_>>(), &target.to_ascii_lowercase().chars().collect::<Vec<_>>())
}

fn glob(p: &[char], t: &[char]) -> bool {
    match p.first() {
        None => t.is_empty(),
        Some('*') => (0..=t.len()).any(|i| glob(&p[1..], &t[i..])),
        Some('?') => !t.is_empty() && glob(&p[1..], &t[1..]),
        Some(c) => t.first() == Some(c) && glob(&p[1..], &t[1..]),
    }
}

/// `|1|base64(salt)|base64(HMAC-SHA1(salt, host))`, as written with `HashKnownHosts yes`.
fn hashed_match(pat: &str, target: &str) -> bool {
    use base64::Engine;
    use hmac::{KeyInit, Mac};
    let mut parts = pat.split('|').skip(2);
    let (Some(salt), Some(hash)) = (parts.next(), parts.next()) else { return false };
    let b64 = base64::engine::general_purpose::STANDARD;
    let (Ok(salt), Ok(hash)) = (b64.decode(salt), b64.decode(hash)) else { return false };
    let Ok(mac) = hmac::Hmac::<sha1::Sha1>::new_from_slice(&salt) else { return false };
    mac.chain_update(target.as_bytes()).verify_slice(&hash).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use hmac::{KeyInit, Mac};

    // Throwaway ed25519 public keys made with ssh-keygen.
    const ED: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJ1oZattw9YURYjqNmwzI3iEeGqwwEeWYfjYYBbkHTsr";
    const ED2: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIIBFyU62mPN9/M/JoNf6hRFukDrxWovPYZtngQMQQSkW";

    fn key(b64: &str) -> PublicKey {
        russh::keys::parse_public_key_base64(b64).unwrap()
    }

    fn hashed(host: &str) -> String {
        let salt = [7u8; 20];
        let mac = hmac::Hmac::<sha1::Sha1>::new_from_slice(&salt).unwrap().chain_update(host.as_bytes()).finalize().into_bytes();
        let b64 = base64::engine::general_purpose::STANDARD;
        format!("|1|{}|{}", b64.encode(salt), b64.encode(mac))
    }

    #[test]
    fn plain_hosts_and_ports() {
        let text = format!("# comment\nnas,10.0.0.2 ssh-ed25519 {ED}\n[nas]:2222 ssh-ed25519 {ED2} some comment\n");
        assert_eq!(check_text(&text, "nas", 22, &key(ED)), HostKeyStatus::Trusted);
        assert_eq!(check_text(&text, "10.0.0.2", 22, &key(ED)), HostKeyStatus::Trusted);
        assert_eq!(check_text(&text, "NAS", 22, &key(ED)), HostKeyStatus::Trusted);
        assert_eq!(check_text(&text, "nas", 2222, &key(ED2)), HostKeyStatus::Trusted);
        assert_eq!(check_text(&text, "nas", 2222, &key(ED)), HostKeyStatus::Changed);
        assert_eq!(check_text(&text, "nas", 22, &key(ED2)), HostKeyStatus::Changed);
        assert_eq!(check_text(&text, "other", 22, &key(ED)), HostKeyStatus::Unknown);
        // Port 22 entries don't cover other ports.
        assert_eq!(check_text(&text, "10.0.0.2", 2200, &key(ED)), HostKeyStatus::Unknown);
    }

    #[test]
    fn hashed_hosts() {
        let text = format!("{} ssh-ed25519 {ED}\n{} ssh-ed25519 {ED}\n", hashed("nas"), hashed("[nas]:2222"));
        assert_eq!(check_text(&text, "nas", 22, &key(ED)), HostKeyStatus::Trusted);
        assert_eq!(check_text(&text, "nas", 2222, &key(ED)), HostKeyStatus::Trusted);
        assert_eq!(check_text(&text, "nas", 22, &key(ED2)), HostKeyStatus::Changed);
        assert_eq!(check_text(&text, "nas2", 22, &key(ED)), HostKeyStatus::Unknown);
    }

    #[test]
    fn wildcards_negation_and_markers() {
        let text = format!("*.lan,!evil.lan ssh-ed25519 {ED}\n@revoked * ssh-ed25519 {ED2}\n@cert-authority *.corp ssh-ed25519 {ED}\n");
        assert_eq!(check_text(&text, "box.lan", 22, &key(ED)), HostKeyStatus::Trusted);
        assert_eq!(check_text(&text, "evil.lan", 22, &key(ED)), HostKeyStatus::Unknown);
        // A revoked key is never accepted.
        assert_eq!(check_text(&text, "box.lan", 22, &key(ED2)), HostKeyStatus::Changed);
        // CA lines don't vouch for plain keys.
        assert_eq!(check_text(&text, "x.corp", 22, &key(ED)), HostKeyStatus::Unknown);
    }

    #[test]
    fn other_key_types_are_unknown_not_changed() {
        let text = "nas ecdsa-sha2-nistp256 AAAA\nnas ssh-rsa garbage\n";
        assert_eq!(check_text(text, "nas", 22, &key(ED)), HostKeyStatus::Unknown);
    }

    #[test]
    fn trust_store_round_trip_and_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("sub/known_hosts");
        let (k1, k2) = (key(ED), key(ED2));
        assert_eq!(check(&[&store], "127.0.0.1", 2222, &k1), HostKeyStatus::Unknown);
        trust_host_key(&store, "127.0.0.1", 2222, &key_type(&k1), &fingerprint(&k1)).unwrap();
        trust_host_key(&store, "other", 22, &key_type(&k1), &fingerprint(&k1)).unwrap();
        assert_eq!(check(&[&store], "127.0.0.1", 2222, &k1), HostKeyStatus::Trusted);
        assert_eq!(check(&[&store], "127.0.0.1", 2222, &k2), HostKeyStatus::Changed);
        // Accepting the new key replaces the old one.
        trust_host_key(&store, "127.0.0.1", 2222, &key_type(&k2), &fingerprint(&k2)).unwrap();
        assert_eq!(check(&[&store], "127.0.0.1", 2222, &k2), HostKeyStatus::Trusted);
        assert_eq!(check(&[&store], "127.0.0.1", 2222, &k1), HostKeyStatus::Changed);
        assert_eq!(check(&[&store], "other", 22, &k1), HostKeyStatus::Trusted);
        assert!(trust_host_key(&store, "h", 22, "ssh-ed25519", "md5:xx").is_err());
    }

    #[test]
    fn app_store_overrides_stale_user_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let user = tmp.path().join("user");
        let app = tmp.path().join("app");
        fs::write(&user, format!("nas ssh-ed25519 {ED}\n")).unwrap();
        let k2 = key(ED2);
        assert_eq!(check(&[&app, &user], "nas", 22, &k2), HostKeyStatus::Changed);
        trust_host_key(&app, "nas", 22, &key_type(&k2), &fingerprint(&k2)).unwrap();
        assert_eq!(check(&[&app, &user], "nas", 22, &k2), HostKeyStatus::Trusted);
    }

    #[test]
    fn fingerprint_matches_openssh_format() {
        // `ssh-keygen -lf` output for ED.
        assert_eq!(fingerprint(&key(ED)), "SHA256:Hq19tEbBDpYEGxXUCjMCRj+lgTmHp78J8qft+u1mEtE");
        assert_eq!(key_type(&key(ED)), "ssh-ed25519");
    }
}
