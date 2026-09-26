//! TLS for QUIC without certificate authorities.
//!
//! Both sides present a self-signed certificate that only carries their
//! ed25519 device key. At the TLS layer we check exactly two things: the
//! certificate holds an ed25519 key, and the handshake was signed by that
//! key (so the peer really owns it). *Which* keys are acceptable is decided
//! right after the handshake against the trust store (see
//! [`crate::server`] and [`crate::client`]), in one place, where pairing and
//! tailnet rules live.

use crate::identity::{Identity, PublicKey};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls13_signature_with_raw_key, CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, Error, SignatureScheme};
use rustls_pki_types::{CertificateDer, ServerName, SubjectPublicKeyInfoDer, UnixTime};
use std::sync::Arc;

pub const ALPN: &[u8] = b"cx-peer/1";

/// DER prefix of an ed25519 SubjectPublicKeyInfo (RFC 8410): SEQUENCE {
/// AlgorithmIdentifier { 1.3.101.112 }, BIT STRING (32 bytes) }.
const ED25519_SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn algorithms() -> WebPkiSupportedAlgorithms {
    rustls::crypto::ring::default_provider().signature_verification_algorithms
}

/// Parse the certificate properly (not by searching bytes) and return its
/// SPKI plus the raw key, if it is an ed25519 key.
fn cert_key(cert: &CertificateDer<'_>) -> Result<(SubjectPublicKeyInfoDer<'static>, PublicKey), Error> {
    let bad = || Error::InvalidCertificate(rustls::CertificateError::BadEncoding);
    let ee = webpki::EndEntityCert::try_from(cert).map_err(|_| bad())?;
    let spki = ee.subject_public_key_info();
    let raw = spki.as_ref();
    if raw.len() != 44 || raw[..12] != ED25519_SPKI_PREFIX {
        return Err(Error::InvalidCertificate(rustls::CertificateError::BadEncoding));
    }
    let key: PublicKey = raw[12..].try_into().map_err(|_| bad())?;
    Ok((spki, key))
}

/// The device key of the other side of an established connection.
pub fn peer_key(conn: &quinn::Connection) -> Option<PublicKey> {
    let certs = conn.peer_identity()?.downcast::<Vec<CertificateDer<'static>>>().ok()?;
    cert_key(certs.first()?).ok().map(|(_, k)| k)
}

/// Verify the TLS 1.3 handshake signature with the key taken from the very
/// SPKI we accepted, so the key we later pin is the key that signed.
fn verify13(message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
    if dss.scheme != SignatureScheme::ED25519 {
        return Err(Error::PeerIncompatible(rustls::PeerIncompatible::NoSignatureSchemesInCommon));
    }
    let (spki, _) = cert_key(cert)?;
    verify_tls13_signature_with_raw_key(message, &spki, dss, &algorithms())
}

#[derive(Debug)]
struct PinLater;

impl ServerCertVerifier for PinLater {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, Error> {
        cert_key(end_entity).map(|_| ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::PeerIncompatible(rustls::PeerIncompatible::Tls13RequiredForQuic))
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        verify13(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

impl ClientCertVerifier for PinLater {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(&self, end_entity: &CertificateDer<'_>, _: &[CertificateDer<'_>], _: UnixTime) -> Result<ClientCertVerified, Error> {
        cert_key(end_entity).map(|_| ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &CertificateDer<'_>, _: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::PeerIncompatible(rustls::PeerIncompatible::Tls13RequiredForQuic))
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, Error> {
        verify13(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
}

pub fn server_crypto(id: &Identity) -> Result<rustls::ServerConfig, Error> {
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(PinLater))
        .with_single_cert(vec![id.cert()], id.private_key())?;
    cfg.alpn_protocols = vec![ALPN.to_vec()];
    Ok(cfg)
}

pub fn client_crypto(id: &Identity) -> Result<rustls::ClientConfig, Error> {
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinLater))
        .with_client_auth_cert(vec![id.cert()], id.private_key())?;
    cfg.alpn_protocols = vec![ALPN.to_vec()];
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_the_identity_key_from_its_cert() {
        let dir = tempfile::tempdir().unwrap();
        let id = Identity::load_or_create(dir.path(), "t".into()).unwrap();
        let (_, key) = cert_key(&id.cert()).unwrap();
        assert_eq!(&key, id.public_key());
        assert!(cert_key(&CertificateDer::from(vec![0u8; 10])).is_err());
    }
}
