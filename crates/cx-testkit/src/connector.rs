use crate::MemProvider;
use async_trait::async_trait;
use cx_core::{Connector, Credentials, Endpoint, MemoryCredentials, Provider, Result, Scheme, Vfs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Serves one [`MemProvider`] for every endpoint of a scheme.
pub struct MemConnector {
    scheme: Scheme,
    provider: Arc<MemProvider>,
    connects: AtomicUsize,
}

impl MemConnector {
    pub fn new(scheme: Scheme, provider: Arc<MemProvider>) -> Arc<MemConnector> {
        Arc::new(MemConnector { scheme, provider, connects: AtomicUsize::new(0) })
    }

    /// How many times the Vfs opened a connection through this connector.
    pub fn connects(&self) -> usize {
        self.connects.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl Connector for MemConnector {
    fn scheme(&self) -> Scheme {
        self.scheme
    }

    async fn connect(&self, _ep: &Endpoint, _creds: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        self.connects.fetch_add(1, Ordering::Relaxed);
        Ok(self.provider.clone())
    }
}

/// An SFTP endpoint for tests (`sftp://<host>`); pair it with [`mem_vfs`].
pub fn mem_endpoint(host: &str) -> Endpoint {
    Endpoint { scheme: Scheme::Sftp, user: None, host: host.into(), port: None }
}

/// A Vfs whose local side is `local` and whose `sftp://` endpoints are all
/// served by `remote`.
pub fn mem_vfs(local: Arc<dyn Provider>, remote: Arc<MemProvider>) -> Arc<Vfs> {
    let vfs = Vfs::new(local, Arc::new(MemoryCredentials::default()));
    vfs.register(MemConnector::new(Scheme::Sftp, remote));
    vfs
}

#[cfg(test)]
mod tests {
    use super::*;
    use cx_core::Location;

    #[tokio::test]
    async fn vfs_routes_remote_locations_to_the_mem_provider() {
        let remote = MemProvider::with_scheme("sftp");
        remote.put("/home/a.txt", b"a".to_vec());
        let vfs = mem_vfs(MemProvider::new(), remote.clone());
        let loc = Location::remote(mem_endpoint("nas"), "/home/a.txt");
        let p = vfs.provider(&loc).await.unwrap();
        assert_eq!(p.scheme(), "sftp");
        assert_eq!(p.stat(&loc).await.unwrap().size, 1);
    }
}
