//! Pairing with a 6-digit code.
//!
//! Sending the code itself over the (not yet authenticated) connection
//! would let anyone in the middle read it and pair in our place. Instead
//! both sides run SPAKE2 with the code as the password and both TLS public
//! keys as identities. Only someone who knows the code derives the same
//! session key, a man in the middle has different TLS keys and so derives a
//! different one, and every attempt costs an online guess: the code dies
//! after [`MAX_ATTEMPTS`] tries or [`CODE_TTL`].

use crate::fsutil::now_ms;
use crate::identity::PublicKey;
use cx_core::{CxError, Result};
use serde::Serialize;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const CODE_TTL: Duration = Duration::from_secs(120);
pub const MAX_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PairingCode {
    pub code: String,
    /// Milliseconds since the Unix epoch.
    pub expires_at: i64,
}

struct Active {
    code: String,
    expires: Instant,
    attempts: u32,
}

#[derive(Default)]
pub struct PairingCodes {
    active: Mutex<Option<Active>>,
    ttl: Option<Duration>,
}

impl PairingCodes {
    /// Codes that expire after `ttl` instead of [`CODE_TTL`] (tests).
    pub fn with_ttl(ttl: Duration) -> Self {
        PairingCodes { active: Mutex::new(None), ttl: Some(ttl) }
    }

    /// A fresh code; any previous one stops working.
    pub fn start(&self) -> PairingCode {
        let ttl = self.ttl.unwrap_or(CODE_TTL);
        let code = random_code();
        let expires_at = now_ms() + ttl.as_millis() as i64;
        *self.active.lock().unwrap() = Some(Active { code: code.clone(), expires: Instant::now() + ttl, attempts: 0 });
        PairingCode { code, expires_at }
    }

    pub fn cancel(&self) {
        *self.active.lock().unwrap() = None;
    }

    /// The code to run an attempt with (counts as one of the attempts).
    pub fn attempt(&self) -> Result<String> {
        let mut g = self.active.lock().unwrap();
        let Some(a) = g.as_mut() else { return Err(no_code()) };
        if Instant::now() >= a.expires || a.attempts >= MAX_ATTEMPTS {
            *g = None;
            return Err(no_code());
        }
        a.attempts += 1;
        Ok(a.code.clone())
    }

    /// A pairing succeeded: the code is single-use.
    pub fn consume(&self, code: &str) {
        let mut g = self.active.lock().unwrap();
        if g.as_ref().is_some_and(|a| a.code == code) {
            *g = None;
        }
    }
}

fn no_code() -> CxError {
    CxError::AuthRequired { uri: String::new(), user: None, reason: "no active pairing code on this device (or it expired)".into() }
}

pub fn wrong_code() -> CxError {
    CxError::AuthRequired { uri: String::new(), user: None, reason: "wrong pairing code".into() }
}

fn random_code() -> String {
    use ring::rand::SecureRandom;
    let rng = ring::rand::SystemRandom::new();
    loop {
        let mut b = [0u8; 4];
        rng.fill(&mut b).expect("system random");
        let v = u32::from_le_bytes(b);
        // Rejection sampling keeps all million codes equally likely.
        if v < 4_294_000_000 {
            return format!("{:06}", v % 1_000_000);
        }
    }
}

/// One side of the exchange. `client`/`server` are the TLS keys of the
/// connecting and the accepting device.
pub struct Pake {
    state: Spake2<Ed25519Group>,
}

impl Pake {
    pub fn client(code: &str, client: &PublicKey, server: &PublicKey) -> (Pake, Vec<u8>) {
        let (state, msg) = Spake2::<Ed25519Group>::start_a(&Password::new(code.trim().as_bytes()), &Identity::new(client), &Identity::new(server));
        (Pake { state }, msg)
    }

    pub fn server(code: &str, client: &PublicKey, server: &PublicKey) -> (Pake, Vec<u8>) {
        let (state, msg) = Spake2::<Ed25519Group>::start_b(&Password::new(code.as_bytes()), &Identity::new(client), &Identity::new(server));
        (Pake { state }, msg)
    }

    pub fn finish(self, their_msg: &[u8]) -> Result<Confirm> {
        let key = self.state.finish(their_msg).map_err(|_| wrong_code())?;
        let key: [u8; 32] = blake3::derive_key("cx-peer 2026 pairing", &key);
        Ok(Confirm { key })
    }
}

/// Key confirmation values derived from the shared session key.
pub struct Confirm {
    key: [u8; 32],
}

impl Confirm {
    pub fn server_proof(&self) -> [u8; 32] {
        *blake3::keyed_hash(&self.key, b"server confirms").as_bytes()
    }
    pub fn client_proof(&self) -> [u8; 32] {
        *blake3::keyed_hash(&self.key, b"client confirms").as_bytes()
    }
    /// Constant-time comparison (blake3::Hash equality is constant-time).
    pub fn check(expected: [u8; 32], got: [u8; 32]) -> bool {
        blake3::Hash::from(expected) == blake3::Hash::from(got)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(client_code: &str, server_code: &str, keys_seen_by_client: (PublicKey, PublicKey)) -> bool {
        let (c, s) = ([1u8; 32], [2u8; 32]);
        let (cp, cmsg) = Pake::client(client_code, &keys_seen_by_client.0, &keys_seen_by_client.1);
        let (sp, smsg) = Pake::server(server_code, &c, &s);
        let (Ok(cc), Ok(sc)) = (cp.finish(&smsg), sp.finish(&cmsg)) else { return false };
        Confirm::check(cc.server_proof(), sc.server_proof()) && Confirm::check(sc.client_proof(), cc.client_proof())
    }

    #[test]
    fn only_matching_codes_and_keys_agree() {
        let keys = ([1u8; 32], [2u8; 32]);
        assert!(run("123456", "123456", keys));
        assert!(!run("123457", "123456", keys));
        // A man in the middle presents a different server key to the client.
        assert!(!run("123456", "123456", ([1u8; 32], [3u8; 32])));
    }

    #[test]
    fn codes_are_single_use_limited_and_expire() {
        let codes = PairingCodes::default();
        assert!(codes.attempt().is_err());
        let c = codes.start();
        assert_eq!(c.code.len(), 6);
        for _ in 0..MAX_ATTEMPTS {
            assert_eq!(codes.attempt().unwrap(), c.code);
        }
        assert!(codes.attempt().is_err());
        let c = codes.start();
        codes.consume(&c.code);
        assert!(codes.attempt().is_err());
        let codes = PairingCodes::with_ttl(Duration::from_millis(1));
        codes.start();
        std::thread::sleep(Duration::from_millis(5));
        assert!(codes.attempt().is_err());
    }
}
