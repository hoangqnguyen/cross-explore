//! Server credentials: kept in memory for the session and, when the user
//! asks to remember them, in the OS keychain (macOS Keychain, Windows
//! Credential Manager, Secret Service on Linux). Never written to disk by us.

use cx_core::{CredentialStore, Credentials, CxError, Endpoint, MemoryCredentials, Result};

/// Keychain service name. Shared by the desktop app and the terminal UI, so
/// a server remembered in one is known to the other.
pub const SERVICE: &str = "dev.crossexplore.explorer";

pub struct KeychainCredentials {
    session: MemoryCredentials,
    /// False keeps everything in memory (tests, or a machine without a
    /// keychain daemon where every lookup would stall).
    keychain: bool,
}

impl Default for KeychainCredentials {
    fn default() -> Self {
        KeychainCredentials { session: MemoryCredentials::default(), keychain: true }
    }
}

impl KeychainCredentials {
    /// A store that never touches the OS keychain.
    pub fn session_only() -> Self {
        KeychainCredentials { session: MemoryCredentials::default(), keychain: false }
    }
}

fn account(ep: &Endpoint) -> String {
    ep.uri()
}

impl CredentialStore for KeychainCredentials {
    fn get(&self, ep: &Endpoint) -> Option<Credentials> {
        if let Some(c) = self.session.get(ep) {
            return Some(c);
        }
        if !self.keychain {
            return None;
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
        if persist && self.keychain {
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
        if self.keychain {
            keychain_delete(&account(ep));
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios", windows, target_os = "linux"))]
fn keychain_get(account: &str) -> Option<String> {
    keyring::Entry::new(SERVICE, account).ok()?.get_password().ok()
}

#[cfg(any(target_os = "macos", target_os = "ios", windows, target_os = "linux"))]
fn keychain_set(account: &str, secret: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, account)
        .and_then(|e| e.set_password(secret))
        .map_err(|e| CxError::Io(format!("couldn't save to the keychain: {e}")))
}

#[cfg(any(target_os = "macos", target_os = "ios", windows, target_os = "linux"))]
fn keychain_delete(account: &str) {
    if let Ok(e) = keyring::Entry::new(SERVICE, account) {
        let _ = e.delete_credential();
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios", windows, target_os = "linux")))]
fn keychain_get(_account: &str) -> Option<String> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "ios", windows, target_os = "linux")))]
fn keychain_set(_account: &str, _secret: &str) -> Result<()> {
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "ios", windows, target_os = "linux")))]
fn keychain_delete(_account: &str) {}

#[cfg(test)]
mod tests {
    use super::*;
    use cx_core::Location;

    #[test]
    fn session_store_finds_credentials_by_bare_host() {
        let store = KeychainCredentials::session_only();
        let loc = Location::parse("sftp://pi@nas/home").unwrap();
        let ep = loc.endpoint().unwrap();
        store.set(ep, &Credentials::password("pi", "pw"), true).unwrap();
        assert_eq!(store.get(ep).unwrap().password_str(), Some("pw"));
        assert_eq!(store.get(&ep.without_user()).unwrap().user, "pi");
        store.remove(ep);
        assert!(store.get(ep).is_none());
    }
}
