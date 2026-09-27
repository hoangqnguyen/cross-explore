//! Server connections: every connector, sign-in with credentials,
//! disconnect, and trusting an SSH host key or FTPS certificate after the
//! user reviewed its fingerprint.

use crate::Engine;
use cx_core::{Credentials, CxError, Endpoint, Location, Result, Scheme, Vfs};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where trusted SSH host keys live (OpenSSH known_hosts format).
pub fn ssh_store(data_dir: &Path) -> PathBuf {
    data_dir.join("known_hosts")
}

/// Where trusted FTPS certificates live.
pub fn tls_store(data_dir: &Path) -> PathBuf {
    data_dir.join("ftps_trust")
}

/// Register every remote connector except peer (which comes with the peer
/// service): SMB, WebDAV over http and https, S3, SFTP, FTP and FTPS.
pub fn register_connectors(vfs: &Vfs, data_dir: &Path) {
    vfs.register(Arc::new(cx_smb::SmbConnector::new()));
    vfs.register(Arc::new(cx_webdav::DavConnector::http()));
    vfs.register(Arc::new(cx_webdav::DavConnector::https()));
    vfs.register(Arc::new(cx_s3::S3Connector));
    vfs.register(Arc::new(cx_sftp::SftpConnector::new(ssh_store(data_dir))));
    vfs.register(Arc::new(cx_ftp::FtpConnector::ftp(tls_store(data_dir))));
    vfs.register(Arc::new(cx_ftp::FtpConnector::ftps(tls_store(data_dir))));
}

/// Record a server key as trusted (what the host-key prompt's "Trust" does).
pub fn trust_host_key(data_dir: &Path, uri: &str, key_type: &str, fingerprint: &str) -> Result<()> {
    let loc = Location::parse(uri)?;
    let ep = loc.endpoint().ok_or_else(|| CxError::InvalidLocation(uri.into()))?;
    let port = ep.port_or_default();
    match ep.scheme {
        Scheme::Sftp => cx_sftp::trust_host_key(&ssh_store(data_dir), &ep.host, port, key_type, fingerprint),
        Scheme::Ftp | Scheme::Ftps => cx_ftp::trust_host_key(&tls_store(data_dir), &ep.host, port, key_type, fingerprint),
        other => Err(CxError::Unsupported(format!("trusting {other} keys"))),
    }
}

pub fn endpoint_of(uri: &str) -> Result<Endpoint> {
    Location::parse(uri)?.endpoint().cloned().ok_or_else(|| CxError::InvalidLocation(format!("{uri} is not a server")))
}

impl Engine {
    /// Connect (or reconnect) to the server in `uri`. With credentials, a
    /// fresh connection is made with them and, on success, they're
    /// remembered — in the keychain when `remember` is set, otherwise for
    /// this session. Fails with `AuthRequired` / `HostKeyUnknown` for the
    /// front end to prompt and retry.
    pub async fn connect_server(&self, uri: &str, credentials: Option<Credentials>, remember: bool) -> Result<()> {
        let mut ep = endpoint_of(uri)?;
        if let Some(c) = &credentials {
            // Sign-in dialogs may supply a user the URI didn't have.
            if ep.user.is_none() && !c.user.is_empty() {
                let _ = self.vfs.credentials().set(&ep, c, false);
            }
        }
        let provider = self.vfs.connect(&ep, credentials.clone()).await?;
        // Make sure the connection really works before reporting success.
        let root = Location::parse(uri)?;
        match provider.stat(&root).await {
            Ok(_) | Err(CxError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        if let Some(c) = credentials {
            self.vfs.credentials().set(&ep, &c, remember)?;
            if ep.user.is_none() {
                ep.user = Some(c.user.clone());
                self.vfs.credentials().set(&ep, &c, remember)?;
            }
        }
        Ok(())
    }

    pub async fn disconnect_server(&self, uri: &str) -> Result<()> {
        self.vfs.disconnect(&endpoint_of(uri)?).await;
        Ok(())
    }

    /// Open server connections as `scheme://[user@]host[:port]/` (peers excluded).
    pub fn connections(&self) -> Vec<String> {
        self.vfs.connected().into_iter().filter(|e| e.scheme != Scheme::Peer).map(|e| format!("{}/", e.uri())).collect()
    }

    /// Trust the key the server presented, and forget the remembered
    /// failure so the next attempt really dials again.
    pub fn trust_host_key(&self, uri: &str, key_type: &str, fingerprint: &str) -> Result<()> {
        trust_host_key(&self.data_dir, uri, key_type, fingerprint)?;
        self.vfs.clear_failure(&endpoint_of(uri)?);
        Ok(())
    }
}
