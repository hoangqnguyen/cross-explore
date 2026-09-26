use crate::session::ConnectParams;
use crate::SftpProvider;
use async_trait::async_trait;
use cx_core::{Connector, Credentials, CxError, Endpoint, Provider, Result, Scheme};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Opens `sftp://` connections.
#[derive(Debug, Clone)]
pub struct SftpConnector {
    known_hosts: PathBuf,
    ssh_dir: Option<PathBuf>,
    use_agent: bool,
}

impl SftpConnector {
    /// `known_hosts` is the app's own host key store (written by
    /// [`trust_host_key`](crate::trust_host_key)); it need not exist yet.
    /// `~/.ssh/known_hosts` is consulted as well, read-only, and
    /// `~/.ssh/id_*` keys and the ssh-agent are used when no credentials are given.
    pub fn new(known_hosts: impl Into<PathBuf>) -> Self {
        SftpConnector { known_hosts: known_hosts.into(), ssh_dir: dirs::home_dir().map(|h| h.join(".ssh")), use_agent: true }
    }

    /// Use another folder instead of `~/.ssh` for `known_hosts` and default
    /// keys, or none at all.
    pub fn with_ssh_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.ssh_dir = dir;
        self
    }

    /// Whether to offer ssh-agent identities (on by default).
    pub fn with_agent(mut self, on: bool) -> Self {
        self.use_agent = on;
        self
    }

    pub fn known_hosts_path(&self) -> &Path {
        &self.known_hosts
    }

    /// Connect and return the concrete provider (the [`Connector`] impl
    /// returns it type-erased).
    pub async fn open(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<SftpProvider> {
        if ep.scheme != Scheme::Sftp {
            return Err(CxError::InvalidLocation(ep.uri()));
        }
        let mut known_hosts = vec![self.known_hosts.clone()];
        if let Some(dir) = &self.ssh_dir {
            known_hosts.push(dir.join("known_hosts"));
        }
        let params = ConnectParams { endpoint: ep.clone(), creds, known_hosts, ssh_dir: self.ssh_dir.clone(), use_agent: self.use_agent };
        SftpProvider::connect(params).await
    }
}

#[async_trait]
impl Connector for SftpConnector {
    fn scheme(&self) -> Scheme {
        Scheme::Sftp
    }

    async fn connect(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(self.open(ep, creds).await?))
    }
}
