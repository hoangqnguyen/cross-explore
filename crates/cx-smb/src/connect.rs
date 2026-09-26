//! Signing in: explicit credentials, else a guest attempt, else ask.

use crate::provider::SmbProvider;
use crate::session::Session;
use async_trait::async_trait;
use cx_core::{Connector, Credentials, CxError, Endpoint, Provider, Result, Scheme, Secret};
use smb2::{ClientConfig, ErrorKind};
use std::sync::Arc;
use std::time::Duration;

/// Opens [`SmbProvider`]s for `smb://` endpoints. Register it with
/// [`cx_core::Vfs::register`].
#[derive(Debug, Clone)]
pub struct SmbConnector {
    timeout: Duration,
}

impl Default for SmbConnector {
    fn default() -> Self {
        SmbConnector { timeout: Duration::from_secs(10) }
    }
}

impl SmbConnector {
    pub fn new() -> Self {
        Self::default()
    }

    /// TCP connect budget (default 10 s).
    pub fn with_timeout(timeout: Duration) -> Self {
        SmbConnector { timeout }
    }
}

#[async_trait]
impl Connector for SmbConnector {
    fn scheme(&self) -> Scheme {
        Scheme::Smb
    }

    async fn connect(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(SmbProvider::connect_with(ep, creds, self.timeout).await?))
    }
}

/// Split "DOMAIN\user" into (user, domain). "user@domain" (a UPN) is passed
/// to NTLM as the user name with an empty domain, which servers accept.
pub(crate) fn split_user(user: &str) -> (String, String) {
    match user.split_once('\\') {
        Some((domain, user)) => (user.to_string(), domain.to_string()),
        None => (user.to_string(), String::new()),
    }
}

fn is_guest_name(user: &str) -> bool {
    user.is_empty() || user.eq_ignore_ascii_case("guest") || user.eq_ignore_ascii_case("anonymous")
}

fn addr(ep: &Endpoint) -> String {
    let port = ep.port_or_default();
    if ep.host.contains(':') {
        format!("[{}]:{port}", ep.host)
    } else {
        format!("{}:{port}", ep.host)
    }
}

impl SmbProvider {
    /// Connect to `ep`. With `creds` (a password, or a guest name) that sign-in
    /// is used; without, a guest session is tried. Rejected or missing
    /// credentials give [`CxError::AuthRequired`].
    pub async fn connect(ep: &Endpoint, creds: Option<Credentials>) -> Result<SmbProvider> {
        Self::connect_with(ep, creds, Duration::from_secs(10)).await
    }

    pub(crate) async fn connect_with(ep: &Endpoint, creds: Option<Credentials>, timeout: Duration) -> Result<SmbProvider> {
        let base = ClientConfig { addr: addr(ep), timeout, auto_reconnect: true, ..Default::default() };
        let explicit = creds.as_ref().filter(|c| !(is_guest_name(&c.user) && c.secret == Secret::None));
        let attempts: Vec<(String, String, String)> = match explicit {
            Some(c) => {
                let (user, domain) = split_user(&c.user);
                vec![(user, domain, c.password_str().unwrap_or_default().to_string())]
            }
            // Anonymous first, then the named Guest account: some servers take
            // one but not the other.
            None => vec![(String::new(), String::new(), String::new()), ("Guest".into(), String::new(), String::new())],
        };
        let mut last_auth = None;
        for (username, domain, password) in attempts {
            let config = ClientConfig { username, domain, password, ..base.clone() };
            match Session::connect(config).await {
                Ok(session) => return Ok(SmbProvider::new(ep.clone(), session)),
                Err(e) if matches!(e.kind(), ErrorKind::AuthRequired | ErrorKind::AccessDenied | ErrorKind::SigningRequired) => {
                    last_auth = Some(e)
                }
                Err(e) => return Err(CxError::Connection(format!("{}: {e}", ep.uri()))),
            }
        }
        let user = explicit.map(|c| c.user.clone()).or_else(|| ep.user.clone());
        let reason = match (explicit, last_auth) {
            (Some(_), Some(e)) => format!("the server rejected the user name or password ({e})"),
            (None, _) => "this server does not allow guest access; sign in".into(),
            (Some(_), None) => "sign-in failed".into(),
        };
        Err(CxError::AuthRequired { uri: ep.uri(), user, reason })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_domain_users() {
        assert_eq!(split_user(r"CORP\alice"), ("alice".into(), "CORP".into()));
        assert_eq!(split_user("alice@corp.example"), ("alice@corp.example".into(), String::new()));
        assert_eq!(split_user("bob"), ("bob".into(), String::new()));
    }

    #[test]
    fn formats_addresses_with_ports() {
        let ep = Endpoint { scheme: Scheme::Smb, user: None, host: "::1".into(), port: Some(1445) };
        assert_eq!(addr(&ep), "[::1]:1445");
        let ep = Endpoint { scheme: Scheme::Smb, user: None, host: "nas".into(), port: None };
        assert_eq!(addr(&ep), "nas:445");
    }
}
