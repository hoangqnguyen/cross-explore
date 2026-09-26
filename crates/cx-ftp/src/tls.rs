//! FTPS certificate checking with trust-on-first-use.
//!
//! FTP servers on NAS boxes and home servers almost always use self-signed
//! certificates, so failing hard on them would make FTPS useless. Instead a
//! certificate that the web PKI can't vouch for is treated like an unknown
//! SSH host key: the connect fails with
//! [`CxError::HostKeyUnknown`](cx_core::CxError::HostKeyUnknown) (key type
//! `x509`, SHA-256 fingerprint of the certificate), the UI asks the user, and
//! [`trust_host_key`] records the answer. The TLS handshake signature is
//! still verified against the pinned certificate's key, so pinning is as
//! strong as SSH's.
//!
//! The store uses the same line format as the SFTP one
//! (`[host]:port key-type SHA256:…`).

use base64::Engine;
use cx_core::{CxError, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use suppaftp::tokio_rustls::rustls;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};

pub const KEY_TYPE: &str = "x509";

/// `SHA256:<base64>` of the DER certificate, formatted like SSH fingerprints.
pub fn fingerprint(der: &[u8]) -> String {
    format!("SHA256:{}", base64::engine::general_purpose::STANDARD_NO_PAD.encode(Sha256::digest(der)))
}

pub fn host_pattern(host: &str, port: u16) -> String {
    format!("[{host}]:{port}")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    Unknown,
    Changed,
}

pub fn check(store: &Path, host: &str, port: u16, fp: &str) -> Trust {
    let target = host_pattern(host, port);
    let text = fs::read_to_string(store).unwrap_or_default();
    let mut changed = false;
    for (hosts, kt, key) in text.lines().filter_map(parse_line) {
        if kt != KEY_TYPE || !hosts.split(',').any(|h| h.eq_ignore_ascii_case(&target)) {
            continue;
        }
        if key == fp {
            return Trust::Trusted;
        }
        changed = true;
    }
    if changed {
        Trust::Changed
    } else {
        Trust::Unknown
    }
}

fn parse_line(line: &str) -> Option<(&str, &str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with('@') {
        return None;
    }
    let mut cols = line.split_whitespace();
    Some((cols.next()?, cols.next()?, cols.next()?))
}

/// Record that the user trusts the certificate with `fingerprint` (as shown
/// in the prompt, from `HostKeyUnknown`) for `host:port`, replacing any
/// earlier one. Same signature as `cx_sftp::trust_host_key` so the UI can
/// treat both alike.
pub fn trust_host_key(store: &Path, host: &str, port: u16, key_type: &str, fingerprint: &str) -> Result<()> {
    if key_type != KEY_TYPE || !fingerprint.starts_with("SHA256:") || fingerprint.contains(char::is_whitespace) {
        return Err(CxError::InvalidName(format!("{key_type} {fingerprint}")));
    }
    let target = host_pattern(host, port);
    let existing = fs::read_to_string(store).unwrap_or_default();
    let mut out = String::new();
    for line in existing.lines() {
        let replaced = parse_line(line).is_some_and(|(h, kt, _)| kt == key_type && h.split(',').any(|h| h.eq_ignore_ascii_case(&target)));
        if !replaced {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(&format!("{target} {key_type} {fingerprint}\n"));
    if let Some(dir) = store.parent() {
        fs::create_dir_all(dir).map_err(|e| CxError::from_io(e, dir.display()))?;
    }
    let tmp = store.with_extension("tmp");
    let mut f = fs::File::create(&tmp).map_err(|e| CxError::from_io(e, tmp.display()))?;
    f.write_all(out.as_bytes()).map_err(|e| CxError::from_io(e, tmp.display()))?;
    fs::rename(&tmp, store).map_err(|e| CxError::from_io(e, store.display()))
}

/// What the verifier saw when it refused a certificate.
#[derive(Debug, Clone)]
pub struct Refused {
    pub fingerprint: String,
    pub changed: bool,
}

/// Web PKI first, then the user's pinned certificates.
#[derive(Debug)]
struct TofuVerifier {
    webpki: Arc<WebPkiServerVerifier>,
    provider: Arc<rustls::crypto::CryptoProvider>,
    store: PathBuf,
    host: String,
    port: u16,
    refused: Arc<Mutex<Option<Refused>>>,
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let err = match self.webpki.verify_server_cert(end_entity, intermediates, server_name, ocsp, now) {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        let fp = fingerprint(end_entity.as_ref());
        match check(&self.store, &self.host, self.port, &fp) {
            Trust::Trusted => Ok(ServerCertVerified::assertion()),
            t => {
                *self.refused.lock().unwrap() = Some(Refused { fingerprint: fp, changed: t == Trust::Changed });
                Err(err)
            }
        }
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// A TLS client config for one server; `refused` is filled in when the
/// server's certificate is rejected.
pub fn client_config(store: &Path, host: &str, port: u16, refused: Arc<Mutex<Option<Refused>>>) -> Result<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let webpki = WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(|e| CxError::Connection(format!("TLS setup: {e}")))?;
    let verifier = TofuVerifier { webpki, provider: provider.clone(), store: store.to_path_buf(), host: host.to_string(), port, refused };
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| CxError::Connection(format!("TLS setup: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trust_store_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("hosts");
        let (a, b) = (fingerprint(b"cert a"), fingerprint(b"cert b"));
        assert!(a.starts_with("SHA256:") && !a.ends_with('='));
        assert_eq!(check(&store, "nas", 21, &a), Trust::Unknown);
        trust_host_key(&store, "nas", 21, KEY_TYPE, &a).unwrap();
        assert_eq!(check(&store, "nas", 21, &a), Trust::Trusted);
        assert_eq!(check(&store, "nas", 990, &a), Trust::Unknown);
        assert_eq!(check(&store, "nas", 21, &b), Trust::Changed);
        trust_host_key(&store, "nas", 21, KEY_TYPE, &b).unwrap();
        assert_eq!(check(&store, "nas", 21, &b), Trust::Trusted);
        assert_eq!(fs::read_to_string(&store).unwrap().lines().count(), 1);
        assert!(trust_host_key(&store, "nas", 21, "ssh-ed25519", &b).is_err());
    }
}
