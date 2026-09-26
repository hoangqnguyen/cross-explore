//! This device's long-term identity: an ed25519 key pair kept in the state
//! directory. Everything else is derived from it: the device id (a hash of
//! the public key, so ids are self-certifying), the self-signed TLS
//! certificate QUIC presents, and the stateless-reset key.
//!
//! Peers never trust a certificate authority; they pin each other's public
//! key after pairing (see [`crate::trust`]).

use cx_core::{CxError, Result};
use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::Serialize;
use std::path::Path;

pub type PublicKey = [u8; 32];

const KEY_FILE: &str = "identity.key";

/// Length of a device id in base32 characters (80 bits of the key hash).
pub const DEVICE_ID_LEN: usize = 16;

pub struct Identity {
    pkcs8: Vec<u8>,
    public: PublicKey,
    device_id: String,
    cert: CertificateDer<'static>,
    pub(crate) name: String,
}

/// What the UI shows about this device.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IdentityInfo {
    pub device_id: String,
    pub name: String,
    pub os: String,
    /// Hex of the raw ed25519 public key.
    pub public_key: String,
    /// Short human-comparable form of the key ("abcd-efgh-...").
    pub fingerprint: String,
}

impl Identity {
    /// Load the key from `state_dir`, creating it on first run.
    pub fn load_or_create(state_dir: &Path, name: String) -> Result<Identity> {
        std::fs::create_dir_all(state_dir).map_err(|e| CxError::from_io(e, state_dir.display()))?;
        let path = state_dir.join(KEY_FILE);
        let pkcs8 = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let kp = KeyPair::generate_for(&PKCS_ED25519).map_err(|e| CxError::io("generating device key", e))?;
                let der = kp.serialize_der();
                crate::fsutil::write_private(&path, &der)?;
                der
            }
            Err(e) => return Err(CxError::from_io(e, path.display())),
        };
        Self::from_pkcs8(pkcs8, name)
    }

    pub fn from_pkcs8(pkcs8: Vec<u8>, name: String) -> Result<Identity> {
        let kp = KeyPair::from_pkcs8_der_and_sign_algo(&PrivatePkcs8KeyDer::from(pkcs8.clone()), &PKCS_ED25519)
            .map_err(|e| CxError::io("reading device key", e))?;
        let public: PublicKey = kp.public_key_raw().try_into().map_err(|_| CxError::Io("device key is not ed25519".into()))?;
        let device_id = device_id(&public);
        // The certificate is only a container for the key: peers ignore names
        // and validity and compare the key itself.
        let mut params = CertificateParams::new(vec!["cx-peer".to_string()]).map_err(|e| CxError::io("certificate", e))?;
        params.distinguished_name.push(rcgen::DnType::CommonName, device_id.clone());
        let cert = params.self_signed(&kp).map_err(|e| CxError::io("certificate", e))?;
        Ok(Identity { pkcs8, public, device_id, cert: cert.der().clone(), name })
    }

    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    pub fn cert(&self) -> CertificateDer<'static> {
        self.cert.clone()
    }

    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.pkcs8.clone()))
    }

    /// Stateless-reset key derived from the device key, so a restarted
    /// server can still tell clients of its previous run to reconnect at once
    /// instead of making them wait for the idle timeout.
    pub fn reset_key(&self) -> [u8; 32] {
        blake3::derive_key("cx-peer 2026 stateless reset", &self.pkcs8)
    }

    pub fn info(&self) -> IdentityInfo {
        IdentityInfo {
            device_id: self.device_id.clone(),
            name: self.name.clone(),
            os: std::env::consts::OS.to_string(),
            public_key: hex(&self.public),
            fingerprint: fingerprint(&self.public),
        }
    }
}

/// Device id: lowercase base32 of the first 80 bits of BLAKE3(public key).
/// Trust decisions always compare the full key, never just the id.
pub fn device_id(key: &PublicKey) -> String {
    let hash = blake3::hash(key);
    let mut s = data_encoding::BASE32_NOPAD.encode(&hash.as_bytes()[..10]).to_ascii_lowercase();
    s.truncate(DEVICE_ID_LEN);
    s
}

/// Whether `s` has the shape of a device id (it may still be a host name).
pub fn looks_like_device_id(s: &str) -> bool {
    s.len() == DEVICE_ID_LEN && s.bytes().all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b))
}

/// A short form users can compare out loud: 4 groups of 4 base32 chars of
/// a separate hash domain.
pub fn fingerprint(key: &PublicKey) -> String {
    let h = blake3::derive_key("cx-peer 2026 fingerprint", key);
    let s = data_encoding::BASE32_NOPAD.encode(&h[..10]);
    s.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

pub fn hex(bytes: &[u8]) -> String {
    data_encoding::HEXLOWER.encode(bytes)
}

pub fn parse_hex_key(s: &str) -> Option<PublicKey> {
    data_encoding::HEXLOWER_PERMISSIVE.decode(s.as_bytes()).ok()?.try_into().ok()
}

pub fn default_name() -> String {
    hostname::get()
        .ok()
        .map(|h| h.to_string_lossy().trim_end_matches(".local").to_string())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "Cross Explore device".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_persists_and_ids_are_stable() {
        let dir = tempfile::tempdir().unwrap();
        let a = Identity::load_or_create(dir.path(), "a".into()).unwrap();
        let b = Identity::load_or_create(dir.path(), "a".into()).unwrap();
        assert_eq!(a.public_key(), b.public_key());
        assert_eq!(a.device_id(), b.device_id());
        assert!(looks_like_device_id(a.device_id()), "{}", a.device_id());
        let other = Identity::load_or_create(tempfile::tempdir().unwrap().path(), "b".into()).unwrap();
        assert_ne!(a.device_id(), other.device_id());
        assert_eq!(parse_hex_key(&a.info().public_key), Some(*a.public_key()));
    }
}
