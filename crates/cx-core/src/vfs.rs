//! Maps locations to providers. Local paths go to the local provider; remote
//! endpoints get one cached connection each, opened on first use with
//! credentials from the [`CredentialStore`].

use crate::{CxError, Endpoint, Location, Provider, Result, Scheme};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

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
    creds: Arc<dyn CredentialStore>,
}

impl Vfs {
    pub fn new(local: Arc<dyn Provider>, creds: Arc<dyn CredentialStore>) -> Arc<Vfs> {
        Arc::new(Vfs {
            local,
            archive: RwLock::new(None),
            connectors: RwLock::new(HashMap::new()),
            slots: Mutex::new(HashMap::new()),
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
        }
        let connector = self
            .connectors
            .read()
            .unwrap()
            .get(&ep.scheme)
            .cloned()
            .ok_or_else(|| CxError::Unsupported(format!("{}:// locations", ep.scheme)))?;
        let creds = creds.or_else(|| self.creds.get(ep)).or_else(|| self.creds.get(&ep.without_user()));
        let provider = connector.connect(ep, creds).await?;
        *guard = Some(provider.clone());
        Ok(provider)
    }

    /// Drop a cached connection (it broke, or the user disconnected).
    pub async fn disconnect(&self, ep: &Endpoint) {
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
