//! Opening connections: from stored credentials (tokens) to a provider.

use crate::api::{Api, TokenHook};
use crate::dropbox::DropboxProvider;
use crate::gdrive::{trash_marker_id, GDriveProvider};
use crate::oauth::endpoint_account;
use crate::onedrive::OneDriveProvider;
use crate::service::{ClientConfig, Service};
use crate::tokens::Tokens;
use cx_core::{Connector, CredentialStore, Credentials, CxError, Endpoint, Provider, Result, Scheme, TrashedItem};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock, Weak};

/// An open connection, typed (tests and callers that need the extras, like
/// Drive's trash restore) or as a plain [`Provider`] via [`CloudProvider::into_provider`].
#[derive(Clone)]
pub enum CloudProvider {
    GDrive(Arc<GDriveProvider>),
    Dropbox(Arc<DropboxProvider>),
    OneDrive(Arc<OneDriveProvider>),
}

impl CloudProvider {
    pub fn into_provider(self) -> Arc<dyn Provider> {
        match self {
            CloudProvider::GDrive(p) => p,
            CloudProvider::Dropbox(p) => p,
            CloudProvider::OneDrive(p) => p,
        }
    }
}

/// Open a connection to `service` for the account `ep` stands for.
///
/// `creds` are what [`crate::SignedIn::credentials`] produced (the tokens as
/// JSON). Missing or unreadable credentials, a missing client ID, or a
/// refresh token the service no longer accepts give
/// [`CxError::AuthRequired`], which is the UI's cue to run the sign-in flow.
/// `hook` receives new credentials whenever the tokens are refreshed.
pub async fn open(service: Service, ep: &Endpoint, creds: Option<Credentials>, client: Option<&ClientConfig>, hook: Option<TokenHook>) -> Result<CloudProvider> {
    let account = endpoint_account(ep);
    let label = service.label();
    let auth = |reason: String| CxError::AuthRequired { uri: ep.uri(), user: Some(account.clone()), reason };
    let Some(client) = client else {
        let (var, _) = service.env_vars();
        return Err(auth(format!("no {label} client ID is set up: add one in Settings (or set {var})")));
    };
    let tokens = creds.as_ref().and_then(Tokens::from_credentials).filter(|t| t.service == service);
    let Some(tokens) = tokens else {
        return Err(auth(format!("sign in to {label}")));
    };
    let api = Arc::new(Api::new(service, client.clone(), ep.uri(), account, tokens, hook).await?);
    Ok(match service {
        Service::GDrive => CloudProvider::GDrive(Arc::new(GDriveProvider::new(api, ep.clone()))),
        Service::Dropbox => CloudProvider::Dropbox(Arc::new(DropboxProvider::new(api, ep.clone()))),
        Service::OneDrive => CloudProvider::OneDrive(Arc::new(OneDriveProvider::new(api, ep.clone()))),
    })
}

/// Opens providers for one service under one core [`Scheme`]:
///
/// ```ignore
/// vfs.register(Arc::new(CloudConnector::new(Service::GDrive, Scheme::GDrive).with_store(store.clone())));
/// ```
///
/// The client ID comes from the environment (see [`ClientConfig::from_env`])
/// unless set with [`CloudConnector::with_client`] / [`CloudConnector::set_client`]
/// (for IDs the user pasted into settings).
pub struct CloudConnector {
    service: Service,
    scheme: Scheme,
    client: RwLock<Option<ClientConfig>>,
    store: Option<Arc<dyn CredentialStore>>,
    drives: Mutex<HashMap<String, Weak<GDriveProvider>>>,
}

impl CloudConnector {
    pub fn new(service: Service, scheme: Scheme) -> CloudConnector {
        CloudConnector { service, scheme, client: RwLock::new(ClientConfig::from_env(service)), store: None, drives: Mutex::default() }
    }

    /// The connector for a core scheme whose name is one of ours.
    pub fn for_scheme(scheme: Scheme) -> Option<CloudConnector> {
        Service::from_scheme(scheme.as_str()).map(|s| CloudConnector::new(s, scheme))
    }

    pub fn with_client(self, client: Option<ClientConfig>) -> CloudConnector {
        self.set_client(client);
        self
    }

    /// Save refreshed tokens here (persisted, i.e. to the OS keychain).
    pub fn with_store(mut self, store: Arc<dyn CredentialStore>) -> CloudConnector {
        self.store = Some(store);
        self
    }

    pub fn set_client(&self, client: Option<ClientConfig>) {
        *self.client.write().unwrap() = client;
    }

    pub fn client(&self) -> Option<ClientConfig> {
        self.client.read().unwrap().clone()
    }

    pub fn service(&self) -> Service {
        self.service
    }

    /// Undo a Google Drive `trash` (untrash the files). Items of other
    /// services or accounts not connected are skipped with an error.
    pub async fn restore_trashed(&self, items: &[TrashedItem]) -> Result<()> {
        let drives: Vec<(String, Arc<GDriveProvider>)> =
            self.drives.lock().unwrap().iter().filter_map(|(k, w)| Some((k.clone(), w.upgrade()?))).collect();
        for item in items {
            let marker = item.trashed.as_deref().unwrap_or_default();
            if trash_marker_id(marker).is_none() {
                return Err(CxError::Unsupported(format!("{}: not a Google Drive trash item", item.original)));
            }
            let Some((_, p)) = drives.iter().find(|(uri, _)| marker.starts_with(&format!("{uri}/"))) else {
                return Err(CxError::Unsupported(format!("{}: the account is not connected", item.original)));
            };
            p.restore(std::slice::from_ref(item)).await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Connector for CloudConnector {
    fn scheme(&self) -> Scheme {
        self.scheme
    }

    async fn connect(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        let hook: Option<TokenHook> = self.store.clone().map(|store| {
            let ep = ep.clone();
            Arc::new(move |c: Credentials| {
                let _ = store.set(&ep, &c, true);
            }) as TokenHook
        });
        let client = self.client();
        let p = open(self.service, ep, creds, client.as_ref(), hook).await?;
        if let CloudProvider::GDrive(d) = &p {
            let mut drives = self.drives.lock().unwrap();
            drives.retain(|_, w| w.strong_count() > 0);
            drives.insert(ep.uri(), Arc::downgrade(d));
        }
        Ok(p.into_provider())
    }
}
