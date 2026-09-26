//! Devices this one has paired with. A device is trusted by its full ed25519
//! public key (pinned when pairing, trust-on-first-use); the device id is
//! only an index. Stored as JSON in the state directory.

use crate::fsutil::{now_ms, write_atomic};
use crate::identity::{device_id, fingerprint, hex, parse_hex_key, PublicKey};
use cx_core::{CxError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::RwLock;

const FILE: &str = "trusted.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrustedDevice {
    pub device_id: String,
    pub name: String,
    #[serde(default)]
    pub os: String,
    /// Hex of the raw ed25519 public key.
    pub public_key: String,
    pub fingerprint: String,
    /// Milliseconds since the Unix epoch.
    pub added_at: i64,
    /// Shares this device may use on this machine; `None` means all.
    #[serde(default)]
    pub shares: Option<Vec<String>>,
    /// Where this device was last reached ("ip:port"), used to reconnect by
    /// device id when discovery has nothing fresher.
    #[serde(default)]
    pub last_addrs: Vec<String>,
}

impl TrustedDevice {
    pub fn new(key: &PublicKey, name: String, os: String) -> TrustedDevice {
        TrustedDevice {
            device_id: device_id(key),
            name,
            os,
            public_key: hex(key),
            fingerprint: fingerprint(key),
            added_at: now_ms(),
            shares: None,
            last_addrs: Vec::new(),
        }
    }

    pub fn key(&self) -> Option<PublicKey> {
        parse_hex_key(&self.public_key)
    }

    pub fn may_use_share(&self, share: &str) -> bool {
        self.shares.as_ref().is_none_or(|s| s.iter().any(|n| n == share))
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    devices: Vec<TrustedDevice>,
}

pub struct TrustStore {
    path: PathBuf,
    devices: RwLock<BTreeMap<String, TrustedDevice>>,
}

impl TrustStore {
    pub fn load(state_dir: &std::path::Path) -> Result<TrustStore> {
        let path = state_dir.join(FILE);
        let file: File = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| CxError::io(path.display(), e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
            Err(e) => return Err(CxError::from_io(e, path.display())),
        };
        // Re-derive ids from keys so a hand-edited file can't alias a device.
        let devices = file
            .devices
            .into_iter()
            .filter_map(|d| {
                let key = d.key()?;
                Some((device_id(&key), TrustedDevice { device_id: device_id(&key), ..d }))
            })
            .collect();
        Ok(TrustStore { path, devices: RwLock::new(devices) })
    }

    fn save(&self, devices: &BTreeMap<String, TrustedDevice>) -> Result<()> {
        let file = File { devices: devices.values().cloned().collect() };
        let json = serde_json::to_vec_pretty(&file).map_err(|e| CxError::io("trust store", e))?;
        write_atomic(&self.path, &json)
    }

    pub fn list(&self) -> Vec<TrustedDevice> {
        self.devices.read().unwrap().values().cloned().collect()
    }

    pub fn get(&self, device_id: &str) -> Option<TrustedDevice> {
        self.devices.read().unwrap().get(device_id).cloned()
    }

    /// The trusted device holding exactly this key.
    pub fn by_key(&self, key: &PublicKey) -> Option<TrustedDevice> {
        self.get(&device_id(key)).filter(|d| d.key().as_ref() == Some(key))
    }

    /// Add or refresh a device (re-pairing keeps its share restrictions).
    pub fn insert(&self, mut dev: TrustedDevice) -> Result<TrustedDevice> {
        let mut map = self.devices.write().unwrap();
        if let Some(old) = map.get(&dev.device_id) {
            if old.public_key == dev.public_key {
                dev.shares = old.shares.clone();
                dev.added_at = old.added_at;
            }
        }
        map.insert(dev.device_id.clone(), dev.clone());
        self.save(&map)?;
        Ok(dev)
    }

    pub fn remove(&self, device_id: &str) -> Result<bool> {
        let mut map = self.devices.write().unwrap();
        let removed = map.remove(device_id).is_some();
        if removed {
            self.save(&map)?;
        }
        Ok(removed)
    }

    pub fn update(&self, device_id: &str, f: impl FnOnce(&mut TrustedDevice)) -> Result<bool> {
        let mut map = self.devices.write().unwrap();
        let Some(d) = map.get_mut(device_id) else { return Ok(false) };
        let before = d.clone();
        f(d);
        if *d != before {
            self.save(&map)?;
        }
        Ok(true)
    }

    /// Remember where a device was reached (most recent first, a few kept).
    pub fn note_addr(&self, device_id: &str, addr: String) {
        let _ = self.update(device_id, |d| {
            d.last_addrs.retain(|a| *a != addr);
            d.last_addrs.insert(0, addr);
            d.last_addrs.truncate(4);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_matches_by_full_key() {
        let dir = tempfile::tempdir().unwrap();
        let key = [7u8; 32];
        let store = TrustStore::load(dir.path()).unwrap();
        let mut d = TrustedDevice::new(&key, "nas".into(), "linux".into());
        d.shares = Some(vec!["Media".into()]);
        store.insert(d).unwrap();
        store.note_addr(&device_id(&key), "10.0.0.2:47470".into());

        let store = TrustStore::load(dir.path()).unwrap();
        let d = store.by_key(&key).unwrap();
        assert!(d.may_use_share("Media") && !d.may_use_share("Docs"));
        assert_eq!(d.last_addrs, ["10.0.0.2:47470"]);
        assert!(store.by_key(&[8u8; 32]).is_none());
        assert!(store.remove(&d.device_id).unwrap());
        assert!(TrustStore::load(dir.path()).unwrap().list().is_empty());
    }
}
