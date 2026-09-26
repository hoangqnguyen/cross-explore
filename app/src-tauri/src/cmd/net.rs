//! Server connections: sign-in with credentials, disconnect, SSH host keys.

use super::AppState;
use cx_core::{Credentials, CxError, Location, Result};

fn endpoint_of(uri: &str) -> Result<cx_core::Endpoint> {
    Location::parse(uri)?.endpoint().cloned().ok_or_else(|| CxError::InvalidLocation(format!("{uri} is not a server")))
}

/// Connect (or reconnect) to the server in `uri`. With credentials, a fresh
/// connection is made with them and, on success, they're remembered — in the
/// keychain when `remember` is set, otherwise for this session.
#[tauri::command]
pub async fn connect_server(uri: String, credentials: Option<Credentials>, remember: bool, app: AppState<'_>) -> Result<()> {
    let mut ep = endpoint_of(&uri)?;
    if let Some(c) = &credentials {
        // Sign-in dialogs may supply a user the URI didn't have.
        if ep.user.is_none() && !c.user.is_empty() {
            let _ = app.vfs.credentials().set(&ep, c, false);
        }
    }
    let provider = app.vfs.connect(&ep, credentials.clone()).await?;
    // Make sure the connection really works before reporting success.
    let root = Location::parse(&uri)?;
    match provider.stat(&root).await {
        Ok(_) | Err(CxError::NotFound(_)) => {}
        Err(e) => return Err(e),
    }
    if let Some(c) = credentials {
        app.vfs.credentials().set(&ep, &c, remember)?;
        if ep.user.is_none() {
            ep.user = Some(c.user.clone());
            app.vfs.credentials().set(&ep, &c, remember)?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn disconnect_server(uri: String, app: AppState<'_>) -> Result<()> {
    app.vfs.disconnect(&endpoint_of(&uri)?).await;
    Ok(())
}

#[tauri::command]
pub fn connections(app: AppState<'_>) -> Vec<String> {
    app.vfs.connected().into_iter().filter(|e| e.scheme != cx_core::Scheme::Peer).map(|e| format!("{}/", e.uri())).collect()
}

#[tauri::command]
pub fn trust_host_key(uri: String, key_type: String, fingerprint: String, app: AppState<'_>) -> Result<()> {
    crate::sftp::trust(&app, &uri, &key_type, &fingerprint)?;
    app.vfs.clear_failure(&endpoint_of(&uri)?);
    Ok(())
}
