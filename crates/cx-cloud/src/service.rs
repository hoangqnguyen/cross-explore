//! Which cloud service, and how to reach it.
//!
//! [`Service`] is this crate's own notion of the three services. Providers
//! and the OAuth helpers are keyed by it rather than by `cx_core::Scheme`,
//! so they work (and are tested) without the core knowing about cloud
//! schemes; the [`crate::CloudConnector`] is what ties a `Scheme` to a
//! `Service`.
//!
//! [`ClientConfig`] carries the OAuth client the user registered (client
//! IDs are per-developer; see the crate docs) plus every URL the crate
//! talks to, so tests can point a provider at a local mock server.

use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const GDRIVE_SCHEME: &str = "gdrive";
pub const DROPBOX_SCHEME: &str = "dropbox";
pub const ONEDRIVE_SCHEME: &str = "onedrive";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Service {
    GDrive,
    Dropbox,
    OneDrive,
}

impl Service {
    pub const ALL: [Service; 3] = [Service::GDrive, Service::Dropbox, Service::OneDrive];

    /// The URI scheme: `gdrive`, `dropbox` or `onedrive`.
    pub fn scheme(self) -> &'static str {
        match self {
            Service::GDrive => GDRIVE_SCHEME,
            Service::Dropbox => DROPBOX_SCHEME,
            Service::OneDrive => ONEDRIVE_SCHEME,
        }
    }

    /// The service for a URI scheme (a few aliases accepted).
    pub fn from_scheme(s: &str) -> Option<Service> {
        Some(match s.to_ascii_lowercase().as_str() {
            "gdrive" | "googledrive" | "google-drive" => Service::GDrive,
            "dropbox" => Service::Dropbox,
            "onedrive" | "msgraph" => Service::OneDrive,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Service::GDrive => "Google Drive",
            Service::Dropbox => "Dropbox",
            Service::OneDrive => "OneDrive",
        }
    }

    /// Environment variables read by [`ClientConfig::from_env`]:
    /// (client id, client secret).
    pub fn env_vars(self) -> (&'static str, Option<&'static str>) {
        match self {
            Service::GDrive => ("CX_GDRIVE_CLIENT_ID", Some("CX_GDRIVE_CLIENT_SECRET")),
            Service::Dropbox => ("CX_DROPBOX_APP_KEY", Some("CX_DROPBOX_APP_SECRET")),
            Service::OneDrive => ("CX_ONEDRIVE_CLIENT_ID", None),
        }
    }

    /// OAuth scopes requested at sign-in.
    pub(crate) fn scopes(self) -> Option<&'static str> {
        match self {
            // Full Drive access: a file manager must see files it didn't create.
            Service::GDrive => Some("https://www.googleapis.com/auth/drive"),
            // Dropbox scopes are set on the app in the App Console; asking
            // for none grants all of them.
            Service::Dropbox => None,
            // offline_access is what yields a refresh token.
            Service::OneDrive => Some("offline_access Files.ReadWrite.All User.Read"),
        }
    }

    /// Dropbox wants an exact redirect URI match (port included), so it
    /// gets a fixed port that is registered in its App Console. Google and
    /// Microsoft accept any port on a loopback redirect.
    pub(crate) fn default_redirect_port(self) -> Option<u16> {
        match self {
            Service::Dropbox => Some(DROPBOX_REDIRECT_PORT),
            _ => None,
        }
    }
}

/// The loopback port registered for Dropbox (`http://127.0.0.1:47480/callback`).
pub const DROPBOX_REDIRECT_PORT: u16 = 47480;

/// Where each service lives. Only tests change these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceUrls {
    /// Browser sign-in page.
    pub authorize: String,
    /// Token endpoint (code exchange and refresh).
    pub token: String,
    /// JSON API root: Drive v3, Dropbox's RPC host, Microsoft Graph v1.0.
    pub api: String,
    /// Upload / content root: Drive's upload host, Dropbox's content host.
    /// Graph serves content from `api`.
    pub content: String,
}

impl ServiceUrls {
    pub fn for_service(service: Service) -> ServiceUrls {
        let s = |v: &str| v.to_string();
        match service {
            Service::GDrive => ServiceUrls {
                authorize: s("https://accounts.google.com/o/oauth2/v2/auth"),
                token: s("https://oauth2.googleapis.com/token"),
                api: s("https://www.googleapis.com/drive/v3"),
                content: s("https://www.googleapis.com/upload/drive/v3"),
            },
            Service::Dropbox => ServiceUrls {
                authorize: s("https://www.dropbox.com/oauth2/authorize"),
                token: s("https://api.dropboxapi.com/oauth2/token"),
                api: s("https://api.dropboxapi.com/2"),
                content: s("https://content.dropboxapi.com/2"),
            },
            Service::OneDrive => ServiceUrls {
                // "common" takes both personal Microsoft accounts and
                // work/school (OneDrive for Business) accounts.
                authorize: s("https://login.microsoftonline.com/common/oauth2/v2.0/authorize"),
                token: s("https://login.microsoftonline.com/common/oauth2/v2.0/token"),
                api: s("https://graph.microsoft.com/v1.0"),
                content: s("https://graph.microsoft.com/v1.0"),
            },
        }
    }

    /// Every URL under one origin (a mock server), laid out as the mock
    /// servers in the tests expect: `/authorize`, `/token`, `/api`, `/content`.
    pub fn local(origin: &str) -> ServiceUrls {
        let o = origin.trim_end_matches('/');
        ServiceUrls { authorize: format!("{o}/authorize"), token: format!("{o}/token"), api: format!("{o}/api"), content: format!("{o}/content") }
    }
}

/// How rate limiting (429, 503, Drive's 403 `rateLimitExceeded`) is ridden out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// First backoff when the server gives no `Retry-After`; doubles each time.
    pub base_delay: Duration,
    /// Upper bound for any single wait, `Retry-After` included.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy { max_retries: 6, base_delay: Duration::from_secs(1), max_delay: Duration::from_secs(64) }
    }
}

/// An OAuth client registered by the user (or the app's distributor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientConfig {
    pub client_id: String,
    /// Google "Desktop app" clients have a secret (not really secret in an
    /// installed app, but the token endpoint wants it). Dropbox and
    /// Microsoft public clients use PKCE alone.
    pub client_secret: Option<String>,
    /// Fixed loopback port for the redirect; `None` picks a free one.
    pub redirect_port: Option<u16>,
    pub urls: ServiceUrls,
    pub retry: RetryPolicy,
}

impl ClientConfig {
    pub fn new(service: Service, client_id: impl Into<String>) -> ClientConfig {
        ClientConfig {
            client_id: client_id.into(),
            client_secret: None,
            redirect_port: service.default_redirect_port(),
            urls: ServiceUrls::for_service(service),
            retry: RetryPolicy::default(),
        }
    }

    pub fn with_secret(mut self, secret: impl Into<String>) -> ClientConfig {
        let s = secret.into();
        self.client_secret = (!s.is_empty()).then_some(s);
        self
    }

    pub fn with_urls(mut self, urls: ServiceUrls) -> ClientConfig {
        self.urls = urls;
        self
    }

    pub fn with_redirect_port(mut self, port: Option<u16>) -> ClientConfig {
        self.redirect_port = port;
        self
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> ClientConfig {
        self.retry = retry;
        self
    }

    /// The client from `CX_GDRIVE_CLIENT_ID` / `CX_GDRIVE_CLIENT_SECRET`,
    /// `CX_DROPBOX_APP_KEY` (/ `CX_DROPBOX_APP_SECRET`) or
    /// `CX_ONEDRIVE_CLIENT_ID`; `None` when the id is not set.
    pub fn from_env(service: Service) -> Option<ClientConfig> {
        let (id_var, secret_var) = service.env_vars();
        let id = std::env::var(id_var).ok().filter(|v| !v.trim().is_empty())?;
        let mut cfg = ClientConfig::new(service, id.trim());
        if let Some(secret) = secret_var.and_then(|v| std::env::var(v).ok()) {
            cfg = cfg.with_secret(secret.trim());
        }
        Some(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemes_round_trip() {
        for s in Service::ALL {
            assert_eq!(Service::from_scheme(s.scheme()), Some(s));
        }
        assert_eq!(Service::from_scheme("GDrive"), Some(Service::GDrive));
        assert_eq!(Service::from_scheme("sftp"), None);
    }
}
