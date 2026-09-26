//! Server credentials: kept in memory for the session and, when the user
//! asks to remember them, in the OS keychain (macOS Keychain, Windows
//! Credential Manager, Secret Service on Linux). Never written to disk by us.

use cx_core::{CredentialStore, Credentials, CxError, Endpoint, MemoryCredentials, Result};

const SERVICE: &str = "dev.crossexplore.app";

#[derive(Default)]
pub struct KeychainCredentials {
    session: MemoryCredentials,
}

fn account(ep: &Endpoint) -> String {
    ep.uri()
}

impl CredentialStore for KeychainCredentials {
    fn get(&self, ep: &Endpoint) -> Option<Credentials> {
        if let Some(c) = self.session.get(ep) {
            return Some(c);
        }
        let stored = keychain_get(&account(ep))?;
        let creds: Credentials = serde_json::from_str(&stored).ok()?;
        let _ = self.session.set(ep, &creds, false);
        Some(creds)
    }

    fn set(&self, ep: &Endpoint, creds: &Credentials, persist: bool) -> Result<()> {
        self.session.set(ep, creds, false)?;
        // Credentials entered without a user in the URI are stored for the
        // bare host too, so `smb://nas/...` finds them next time.
        if ep.user.is_some() {
            self.session.set(&ep.without_user(), creds, false)?;
        }
        if persist {
            let json = serde_json::to_string(creds).map_err(|e| CxError::Io(e.to_string()))?;
            keychain_set(&account(ep), &json)?;
            if ep.user.is_some() {
                keychain_set(&account(&ep.without_user()), &json)?;
            }
        }
        Ok(())
    }

    fn remove(&self, ep: &Endpoint) {
        self.session.remove(ep);
        keychain_delete(&account(ep));
    }
}

#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
fn keychain_get(account: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, account).ok()?.get_password().ok()
}

#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
fn keychain_set(account: &str, secret: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, account)
        .and_then(|e| e.set_password(secret))
        .map_err(|e| CxError::Io(format!("couldn't save to the keychain: {e}")))
}

#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
fn keychain_delete(account: &str) {
    if let Ok(e) = keyring::Entry::new(SERVICE, account) {
        let _ = e.delete_credential();
    }
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn keychain_get(_account: &str) -> Option<String> {
    None
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn keychain_set(_account: &str, _secret: &str) -> Result<()> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn keychain_delete(_account: &str) {}
