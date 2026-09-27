//! Google Drive, Dropbox and OneDrive: app registrations (client IDs) and
//! browser sign-in. The file operations themselves go through the Vfs like
//! any other server.

use super::AppState;
use cx_cloud::{ClientConfig, CloudConnector, Service};
use cx_core::{CxError, Location, Result, Scheme, Vfs};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

static CONNECTORS: OnceLock<Vec<Arc<CloudConnector>>> = OnceLock::new();
static CONFIG_FILE: OnceLock<PathBuf> = OnceLock::new();

/// A client ID the user pasted in Settings (kept in `cloud.json`; these are
/// public "desktop app" IDs, not account secrets — tokens go to the keychain).
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct SavedClient {
    client_id: String,
    #[serde(default)]
    client_secret: Option<String>,
}

fn service_of(name: &str) -> Result<Service> {
    Service::from_scheme(name).ok_or_else(|| CxError::InvalidLocation(format!("unknown cloud service {name}")))
}

fn scheme_of(s: Service) -> Scheme {
    Scheme::parse(s.scheme()).expect("cloud schemes are core schemes")
}

fn connector(s: Service) -> Option<&'static Arc<CloudConnector>> {
    CONNECTORS.get()?.iter().find(|c| c.service() == s)
}

fn load(path: &Path) -> std::collections::HashMap<String, SavedClient> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn config_for(s: Service, saved: &SavedClient) -> ClientConfig {
    let mut c = ClientConfig::new(s, saved.client_id.trim());
    if let Some(secret) = saved.client_secret.as_deref().filter(|x| !x.trim().is_empty()) {
        c = c.with_secret(secret.trim());
    }
    c
}

/// Register the three connectors, with client IDs from `cloud.json` taking
/// precedence over the `CX_*` environment variables.
pub fn register(vfs: &Vfs, data_dir: &Path) {
    let file = data_dir.join("cloud.json");
    let saved = load(&file);
    let _ = CONFIG_FILE.set(file);
    let list: Vec<Arc<CloudConnector>> = Service::ALL
        .into_iter()
        .map(|s| {
            let c = CloudConnector::new(s, scheme_of(s)).with_store(vfs.credentials().clone());
            if let Some(sc) = saved.get(s.scheme()).filter(|sc| !sc.client_id.trim().is_empty()) {
                c.set_client(Some(config_for(s, sc)));
            }
            Arc::new(c)
        })
        .collect();
    for c in &list {
        vfs.register(c.clone());
    }
    let _ = CONNECTORS.set(list);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudService {
    service: &'static str,
    label: &'static str,
    /// A client ID is set, so sign-in can start.
    configured: bool,
    client_id: Option<String>,
    has_secret: bool,
    /// Env var names that also configure it.
    env: (&'static str, Option<&'static str>),
}

#[tauri::command]
pub fn cloud_services() -> Vec<CloudService> {
    Service::ALL
        .into_iter()
        .map(|s| {
            let client = connector(s).and_then(|c| c.client());
            CloudService {
                service: s.scheme(),
                label: s.label(),
                configured: client.as_ref().is_some_and(|c| !c.client_id.is_empty()),
                client_id: client.as_ref().map(|c| c.client_id.clone()),
                has_secret: client.as_ref().is_some_and(|c| c.client_secret.is_some()),
                env: s.env_vars(),
            }
        })
        .collect()
}

/// Save (or clear, with an empty ID) the app registration for a service.
#[tauri::command]
pub fn cloud_set_client(service: String, client_id: String, client_secret: Option<String>) -> Result<()> {
    let s = service_of(&service)?;
    let file = CONFIG_FILE.get().ok_or_else(|| CxError::Io("cloud not initialised".into()))?;
    let mut saved = load(file);
    let entry = SavedClient { client_id: client_id.trim().to_string(), client_secret: client_secret.filter(|x| !x.trim().is_empty()) };
    let c = connector(s).ok_or_else(|| CxError::Unsupported(service.clone()))?;
    if entry.client_id.is_empty() {
        saved.remove(s.scheme());
        c.set_client(ClientConfig::from_env(s));
    } else {
        c.set_client(Some(config_for(s, &entry)));
        saved.insert(s.scheme().to_string(), entry);
    }
    let bytes = serde_json::to_vec_pretty(&saved).map_err(|e| CxError::Io(e.to_string()))?;
    std::fs::write(file, bytes).map_err(|e| CxError::Io(format!("cannot save {}: {e}", file.display())))
}

/// Sign in through the system browser and return the account's root URI.
/// Cancelled (declined or 5 minutes without an answer) → `cancelled`.
#[tauri::command]
pub async fn cloud_sign_in(service: String, app_handle: AppHandle, app: AppState<'_>) -> Result<String> {
    let s = service_of(&service)?;
    let client = connector(s).and_then(|c| c.client()).ok_or_else(|| {
        CxError::Unsupported(format!("{} needs a client ID first — add one in Settings › Cloud accounts", s.label()))
    })?;
    let req = cx_cloud::start_authorization(s, &client).await?;
    app_handle.opener().open_url(&req.url, None::<&str>).map_err(|e| CxError::Io(format!("cannot open the browser: {e}")))?;
    let signed = req.wait(Duration::from_secs(300)).await?;
    let ep = signed.endpoint(scheme_of(s));
    let creds = signed.credentials();
    app.vfs.credentials().set(&ep, &creds, true)?;
    app.vfs.clear_failure(&ep);
    app.vfs.connect(&ep, Some(creds)).await?;
    Ok(Location::remote(ep, "/").uri())
}

/// Open a web page (the providers' developer consoles) in the system browser.
#[tauri::command]
pub fn open_web_page(url: String, app_handle: AppHandle) -> Result<()> {
    if !url.starts_with("https://") {
        return Err(CxError::InvalidLocation(url));
    }
    app_handle.opener().open_url(&url, None::<&str>).map_err(|e| CxError::Io(format!("cannot open the browser: {e}")))
}
