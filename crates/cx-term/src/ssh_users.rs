//! User names that successfully signed in over SSH, per host and port.
//!
//! The embedded terminal asks for the remote user the first time (the local
//! account name is often wrong) and writes it here only after OpenSSH reports
//! that authentication succeeded. The next SSH to that server skips the question.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// One file, so the desktop app and the terminal UI remember the same names.
const FILE: &str = "ssh-users.json";

fn gate() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn map_key(host: &str, port: u16) -> String {
    // A newline cannot appear in a host, so this doesn't collide with `host:port`
    // when the host is an IPv6 address.
    format!("{host}\n{port}")
}

#[derive(Debug, Clone)]
pub struct SshUsers {
    path: PathBuf,
}

impl SshUsers {
    pub fn new(data_dir: impl AsRef<Path>) -> SshUsers {
        SshUsers {
            path: data_dir.as_ref().join(FILE),
        }
    }

    pub fn get(&self, host: &str, port: u16) -> Option<String> {
        let _g = gate();
        load(&self.path).get(&map_key(host, port)).cloned()
    }

    pub fn set(&self, host: &str, port: u16, user: &str) {
        let _g = gate();
        let mut map = load(&self.path);
        map.insert(map_key(host, port), user.to_string());
        save(&self.path, &map);
    }

    /// Forget a name that failed to sign in. Returns whether one was stored.
    pub fn forget(&self, host: &str, port: u16) -> bool {
        let _g = gate();
        let mut map = load(&self.path);
        let removed = map.remove(&map_key(host, port)).is_some();
        if removed {
            save(&self.path, &map);
        }
        removed
    }
}

fn load(path: &Path) -> HashMap<String, String> {
    let Ok(bytes) = fs::read(path) else {
        return HashMap::new();
    };
    serde_json::from_slice::<Stored>(&bytes)
        .map(|s| s.users)
        .unwrap_or_default()
}

fn save(path: &Path, users: &HashMap<String, String>) {
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let bytes = serde_json::to_vec_pretty(&Stored {
        users: users.clone(),
    })
    .unwrap_or_default();
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, bytes).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    users: HashMap<String, String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_per_host_and_port() {
        let dir = tempfile::tempdir().unwrap();
        let users = SshUsers::new(dir.path());
        assert!(users.get("nas", 22).is_none());
        users.set("nas", 22, "pi");
        users.set("nas", 2222, "admin");
        users.set("::1", 22, "me");
        assert_eq!(users.get("nas", 22).as_deref(), Some("pi"));
        assert_eq!(users.get("nas", 2222).as_deref(), Some("admin"));
        assert_eq!(users.get("::1", 22).as_deref(), Some("me"));
        assert!(users.forget("nas", 22));
        assert!(users.get("nas", 22).is_none());
        assert!(!users.forget("nas", 22));
        assert_eq!(users.get("nas", 2222).as_deref(), Some("admin"));
    }
}
