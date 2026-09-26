//! Maps locations to providers. Local paths go to the local provider; remote
//! endpoints get one cached connection each, opened on first use with
//! credentials from the [`CredentialStore`].

use crate::{CxError, Endpoint, Location, Provider, Result, Scheme};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

/// How long a failed sign-in (or unknown host key) is remembered. Without
/// it, every part of the UI that touches a folder would dial the server
/// again, and servers with brute-force protection (OpenSSH's
/// PerSourcePenalties, fail2ban) then lock us out before the user has even
/// typed a password.
const FAILURE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Secret {
    None,
    Password { password: String },
    /// SSH private key file, optionally encrypted.
    Key { path: String, passphrase: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Credentials {
    pub user: String,
    pub secret: Secret,
}

impl Credentials {
    pub fn password(user: impl Into<String>, password: impl Into<String>) -> Self {
        Credentials { user: user.into(), secret: Secret::Password { password: password.into() } }
    }

    pub fn anonymous() -> Self {
        Credentials { user: "anonymous".into(), secret: Secret::None }
    }

    pub fn password_str(&self) -> Option<&str> {
        match &self.secret {
            Secret::Password { password } => Some(password),
            _ => None,
        }
    }
}

pub trait CredentialStore: Send + Sync {
    fn get(&self, ep: &Endpoint) -> Option<Credentials>;
    /// Remember credentials. `persist` saves them beyond this session (OS keychain).
    fn set(&self, ep: &Endpoint, creds: &Credentials, persist: bool) -> Result<()>;
    fn remove(&self, ep: &Endpoint);
}

/// Session-only credential store (also the base for tests).
#[derive(Default)]
pub struct MemoryCredentials(Mutex<HashMap<Endpoint, Credentials>>);

impl CredentialStore for MemoryCredentials {
    fn get(&self, ep: &Endpoint) -> Option<Credentials> {
        self.0.lock().unwrap().get(ep).cloned()
    }
    fn set(&self, ep: &Endpoint, creds: &Credentials, _persist: bool) -> Result<()> {
        self.0.lock().unwrap().insert(ep.clone(), creds.clone());
        Ok(())
    }
    fn remove(&self, ep: &Endpoint) {
        self.0.lock().unwrap().remove(ep);
    }
}

/// Opens connections for one scheme.
#[async_trait]
pub trait Connector: Send + Sync {
    fn scheme(&self) -> Scheme;

    /// Connect to `ep`. `creds` come from the credential store (or the
    /// sign-in dialog). Return [`CxError::AuthRequired`] when credentials are
    /// missing or rejected, so the UI can ask for them.
    async fn connect(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<Arc<dyn Provider>>;
}

type Slot = Arc<tokio::sync::Mutex<Option<Arc<dyn Provider>>>>;

pub struct Vfs {
    local: Arc<dyn Provider>,
    archive: RwLock<Option<Arc<dyn Provider>>>,
    connectors: RwLock<HashMap<Scheme, Arc<dyn Connector>>>,
    // One async lock per endpoint: a slow server never blocks the others.
    slots: Mutex<HashMap<Endpoint, Slot>>,
    failures: Mutex<HashMap<Endpoint, (Instant, CxError)>>,
    creds: Arc<dyn CredentialStore>,
}

impl Vfs {
    pub fn new(local: Arc<dyn Provider>, creds: Arc<dyn CredentialStore>) -> Arc<Vfs> {
        Arc::new(Vfs {
            local,
            archive: RwLock::new(None),
            connectors: RwLock::new(HashMap::new()),
            slots: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            creds,
        })
    }

    pub fn register(&self, connector: Arc<dyn Connector>) {
        self.connectors.write().unwrap().insert(connector.scheme(), connector);
    }

    pub fn set_archive_provider(&self, provider: Arc<dyn Provider>) {
        *self.archive.write().unwrap() = Some(provider);
    }

    pub fn credentials(&self) -> &Arc<dyn CredentialStore> {
        &self.creds
    }

    pub fn local(&self) -> &Arc<dyn Provider> {
        &self.local
    }

    pub fn supports(&self, scheme: Scheme) -> bool {
        self.connectors.read().unwrap().contains_key(&scheme)
    }

    pub async fn provider(&self, loc: &Location) -> Result<Arc<dyn Provider>> {
        match loc {
            Location::Local(_) => Ok(self.local.clone()),
            Location::Archive { .. } => self
                .archive
                .read()
                .unwrap()
                .clone()
                .ok_or_else(|| CxError::Unsupported("archives".into())),
            Location::Remote { endpoint, .. } => self.connect(endpoint, None).await,
        }
    }

    fn slot(&self, ep: &Endpoint) -> Slot {
        self.slots.lock().unwrap().entry(ep.clone()).or_default().clone()
    }

    /// Get the cached connection or open one. Passing `creds` forces a new
    /// connection with them (after a sign-in prompt).
    pub async fn connect(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        let slot = self.slot(ep);
        let mut guard = slot.lock().await;
        if creds.is_none() {
            if let Some(p) = guard.as_ref() {
                return Ok(p.clone());
            }
            if let Some(err) = self.recent_failure(ep) {
                return Err(err);
            }
        }
        let connector = self
            .connectors
            .read()
            .unwrap()
            .get(&ep.scheme)
            .cloned()
            .ok_or_else(|| CxError::Unsupported(format!("{}:// locations", ep.scheme)))?;
        let creds = creds.or_else(|| self.creds.get(ep)).or_else(|| self.creds.get(&ep.without_user()));
        match connector.connect(ep, creds).await {
            Ok(provider) => {
                self.failures.lock().unwrap().remove(ep);
                *guard = Some(provider.clone());
                Ok(provider)
            }
            Err(e) => {
                if matches!(e, CxError::AuthRequired { .. } | CxError::HostKeyUnknown { .. }) {
                    self.failures.lock().unwrap().insert(ep.clone(), (Instant::now(), e.clone()));
                }
                Err(e)
            }
        }
    }

    fn recent_failure(&self, ep: &Endpoint) -> Option<CxError> {
        let mut failures = self.failures.lock().unwrap();
        match failures.get(ep) {
            Some((t, e)) if t.elapsed() < FAILURE_TTL => Some(e.clone()),
            Some(_) => {
                failures.remove(ep);
                None
            }
            None => None,
        }
    }

    /// Forget a remembered sign-in failure, e.g. after the user trusted the
    /// server's key, so the next attempt really dials the server.
    pub fn clear_failure(&self, ep: &Endpoint) {
        self.failures.lock().unwrap().remove(ep);
    }

    /// Drop a cached connection (it broke, or the user disconnected).
    pub async fn disconnect(&self, ep: &Endpoint) {
        self.clear_failure(ep);
        let slot = self.slots.lock().unwrap().remove(ep);
        if let Some(slot) = slot {
            slot.lock().await.take();
        }
    }

    pub fn connected(&self) -> Vec<Endpoint> {
        let slots: Vec<(Endpoint, Slot)> = self.slots.lock().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        slots
            .into_iter()
            .filter(|(_, s)| s.try_lock().map(|g| g.is_some()).unwrap_or(true))
            .map(|(e, _)| e)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Refusing(AtomicUsize);

    #[async_trait]
    impl Connector for Refusing {
        fn scheme(&self) -> Scheme {
            Scheme::Sftp
        }
        async fn connect(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<Arc<dyn Provider>> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let _ = creds;
            Err(CxError::AuthRequired { uri: ep.uri(), user: None, reason: "Wrong password".into() })
        }
    }

    struct NoLocal;

    #[async_trait]
    impl Provider for NoLocal {
        fn scheme(&self) -> &'static str {
            "file"
        }
        fn capabilities(&self) -> crate::Capabilities {
            Default::default()
        }
        async fn list(&self, _: &Location, _: tokio::sync::mpsc::Sender<Vec<crate::Entry>>) -> Result<usize> {
            unimplemented!()
        }
        async fn stat(&self, _: &Location) -> Result<crate::Entry> {
            unimplemented!()
        }
        async fn create_dir(&self, _: &Location, _: Option<&str>) -> Result<crate::Entry> {
            unimplemented!()
        }
        async fn move_to(&self, _: &Location, _: &Location) -> Result<()> {
            unimplemented!()
        }
        async fn remove(&self, _: &Location) -> Result<()> {
            unimplemented!()
        }
        async fn open_read(&self, _: &Location, _: u64) -> Result<crate::ReadStream> {
            unimplemented!()
        }
        async fn open_write(&self, _: &Location, _: crate::WriteMode) -> Result<crate::WriteStream> {
            unimplemented!()
        }
    }

    #[tokio::test]
    async fn failed_sign_in_is_not_retried_until_new_credentials() {
        let vfs = Vfs::new(Arc::new(NoLocal), Arc::new(MemoryCredentials::default()));
        let conn = Arc::new(Refusing(AtomicUsize::new(0)));
        vfs.register(conn.clone());
        let loc = Location::parse("sftp://host/dir").unwrap();
        for _ in 0..5 {
            assert!(matches!(vfs.provider(&loc).await, Err(CxError::AuthRequired { .. })));
        }
        assert_eq!(conn.0.load(Ordering::SeqCst), 1, "only one real attempt");
        let _ = vfs.connect(loc.endpoint().unwrap(), Some(Credentials::password("u", "p"))).await;
        assert_eq!(conn.0.load(Ordering::SeqCst), 2, "explicit credentials always dial");
        vfs.clear_failure(loc.endpoint().unwrap());
        let _ = vfs.provider(&loc).await;
        assert_eq!(conn.0.load(Ordering::SeqCst), 3, "cleared failures dial again");
    }
}
