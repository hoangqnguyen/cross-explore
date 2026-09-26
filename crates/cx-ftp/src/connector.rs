use crate::pool::Params;
use crate::FtpProvider;
use async_trait::async_trait;
use cx_core::{Connector, Credentials, CxError, Endpoint, Provider, Result, Scheme};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Opens `ftp://` or `ftps://` (explicit TLS) connections. Register one
/// connector per scheme.
#[derive(Debug, Clone)]
pub struct FtpConnector {
    scheme: Scheme,
    trust_store: PathBuf,
}

impl FtpConnector {
    /// Plain FTP. `trust_store` is only used by FTPS but kept for symmetry.
    pub fn ftp(trust_store: impl Into<PathBuf>) -> Self {
        FtpConnector { scheme: Scheme::Ftp, trust_store: trust_store.into() }
    }

    /// Explicit FTPS (`AUTH TLS` on the normal port). `trust_store` holds
    /// the certificates the user accepted (see [`trust_host_key`](crate::trust_host_key)).
    pub fn ftps(trust_store: impl Into<PathBuf>) -> Self {
        FtpConnector { scheme: Scheme::Ftps, trust_store: trust_store.into() }
    }

    pub fn trust_store_path(&self) -> &Path {
        &self.trust_store
    }

    /// Connect and return the concrete provider.
    pub async fn open(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<FtpProvider> {
        if ep.scheme != self.scheme {
            return Err(CxError::InvalidLocation(ep.uri()));
        }
        FtpProvider::connect(Params { endpoint: ep.clone(), creds, trust_store: self.trust_store.clone() }).await
    }
}

#[async_trait]
impl Connector for FtpConnector {
    fn scheme(&self) -> Scheme {
        self.scheme
    }

    async fn connect(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(self.open(ep, creds).await?))
    }
}
