//! SFTP and FTP(S) connectors, and trusting a server's key or certificate
//! after the user reviewed its fingerprint.

use crate::state::App;
use cx_core::{CxError, Location, Result, Scheme, Vfs};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn ssh_store(data_dir: &Path) -> PathBuf {
    data_dir.join("known_hosts")
}

fn tls_store(data_dir: &Path) -> PathBuf {
    data_dir.join("ftps_trust")
}

pub fn register(vfs: &Vfs, data_dir: &Path) {
    vfs.register(Arc::new(cx_sftp::SftpConnector::new(ssh_store(data_dir))));
    vfs.register(Arc::new(cx_ftp::FtpConnector::ftp(tls_store(data_dir))));
    vfs.register(Arc::new(cx_ftp::FtpConnector::ftps(tls_store(data_dir))));
}

pub fn trust(app: &App, uri: &str, key_type: &str, fingerprint: &str) -> Result<()> {
    let loc = Location::parse(uri)?;
    let ep = loc.endpoint().ok_or_else(|| CxError::InvalidLocation(uri.into()))?;
    let port = ep.port_or_default();
    match ep.scheme {
        Scheme::Sftp => cx_sftp::trust_host_key(&ssh_store(&app.data_dir), &ep.host, port, key_type, fingerprint),
        Scheme::Ftp | Scheme::Ftps => cx_ftp::trust_host_key(&tls_store(&app.data_dir), &ep.host, port, key_type, fingerprint),
        other => Err(CxError::Unsupported(format!("trusting {other} keys"))),
    }
}
