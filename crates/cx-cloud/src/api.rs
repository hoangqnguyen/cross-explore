//! The authenticated HTTP client one connection uses: bearer tokens that
//! refresh themselves, backoff on rate limiting and error mapping.
//!
//! Every request is described by a closure that builds it, not by a built
//! request, because a request may have to go out several times: once more
//! after a 401 (with a refreshed token) and again after each 429/503. Bodies
//! are therefore always in memory ([`bytes::Bytes`] clones are cheap);
//! uploads are cut into chunks before they get here (see the `upload` module).
//!
//! Refreshing is serialized: when several requests hit a 401 at once, the
//! first refreshes and the others notice the token changed and just retry.
//! Refreshed tokens are handed to a hook so the app can save them (Microsoft
//! rotates refresh tokens, so a stale stored one would stop working).

use crate::oauth::{self, TokenError};
use crate::service::{ClientConfig, Service};
use crate::tokens::Tokens;
use crate::util::describe;
use cx_core::{Credentials, CxError, Result};
use reqwest::{RequestBuilder, Response, StatusCode};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Called with fresh credentials whenever tokens are refreshed.
pub type TokenHook = Arc<dyn Fn(Credentials) + Send + Sync>;

pub(crate) fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .pool_idle_timeout(Duration::from_secs(90))
        // Redirects are followed by hand: downloads (OneDrive) must not carry
        // the bearer token to the CDN, and Drive's resumable uploads answer
        // 308 meaning "go on", not "go elsewhere".
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("CrossExplore/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| CxError::Connection(format!("http client: {e}")))
}

/// Read a JSON body (reqwest's `json` feature is not enabled workspace-wide).
pub(crate) async fn read_json(resp: Response) -> std::result::Result<serde_json::Value, String> {
    let bytes = resp.bytes().await.map_err(|e| describe(&e))?;
    if bytes.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("bad JSON: {e}"))
}

pub(crate) struct Api {
    pub http: reqwest::Client,
    pub service: Service,
    pub cfg: ClientConfig,
    /// Endpoint URI and account, for `AuthRequired`.
    pub uri: String,
    pub account: String,
    tokens: Mutex<Tokens>,
    refresh_lock: tokio::sync::Mutex<()>,
    hook: Option<TokenHook>,
}

impl Api {
    /// Set up a connection, refreshing right away if the stored access token
    /// has expired (so a revoked grant surfaces as `AuthRequired` at connect).
    pub async fn new(service: Service, cfg: ClientConfig, uri: String, account: String, tokens: Tokens, hook: Option<TokenHook>) -> Result<Api> {
        let api = Api { http: http_client()?, service, cfg, uri, account, tokens: Mutex::new(tokens), refresh_lock: tokio::sync::Mutex::new(()), hook };
        let t = api.tokens.lock().unwrap().clone();
        if t.expired() {
            api.refresh_from(&t.access_token).await?;
        }
        Ok(api)
    }

    pub fn auth_required(&self, reason: impl Into<String>) -> CxError {
        CxError::AuthRequired { uri: self.uri.clone(), user: Some(self.account.clone()), reason: reason.into() }
    }

    fn current(&self) -> Tokens {
        self.tokens.lock().unwrap().clone()
    }

    /// A token that is not about to expire.
    async fn token(&self) -> Result<String> {
        let t = self.current();
        if t.expired() && t.refresh_token.is_some() {
            return self.refresh_from(&t.access_token).await;
        }
        Ok(t.access_token)
    }

    /// Refresh, unless someone else already replaced `stale`.
    async fn refresh_from(&self, stale: &str) -> Result<String> {
        let _guard = self.refresh_lock.lock().await;
        let t = self.current();
        if t.access_token != stale {
            return Ok(t.access_token);
        }
        let label = self.service.label();
        match oauth::refresh(&self.http, &t, &self.cfg).await {
            Ok(new) => {
                *self.tokens.lock().unwrap() = new.clone();
                if let Some(hook) = &self.hook {
                    hook(new.to_credentials(&self.account));
                }
                Ok(new.access_token)
            }
            Err(TokenError::Rejected(m)) => Err(self.auth_required(format!("the {label} session has expired; sign in again ({m})"))),
            Err(TokenError::Other(e)) => Err(e),
        }
    }

    /// Send an authorized request. 401 refreshes the token once; 429/503
    /// (and Drive's rate-limit 403) back off and retry. Any other answer is
    /// returned as is.
    pub async fn send(&self, what: &str, build: impl Fn(&str) -> RequestBuilder + Send + Sync) -> Result<Response> {
        self.send_inner(what, true, &|t| build(t.unwrap_or_default())).await
    }

    /// Send to a pre-authorized URL (upload sessions, download links, copy
    /// monitors) without the bearer token, with the same backoff.
    pub async fn send_plain(&self, what: &str, build: impl Fn() -> RequestBuilder + Send + Sync) -> Result<Response> {
        self.send_inner(what, false, &|_| build()).await
    }

    async fn send_inner(&self, what: &str, auth: bool, build: &(dyn Fn(Option<&str>) -> RequestBuilder + Sync)) -> Result<Response> {
        let policy = self.cfg.retry;
        let mut refreshed = false;
        let mut attempt = 0u32;
        loop {
            let token = if auth { Some(self.token().await?) } else { None };
            let mut rb = build(token.as_deref());
            if let Some(t) = &token {
                rb = rb.bearer_auth(t);
            }
            let resp = rb.send().await.map_err(|e| CxError::Connection(format!("{what}: {}", describe(&e))))?;
            let status = resp.status();
            if status == StatusCode::UNAUTHORIZED && auth {
                if refreshed {
                    return Err(self.error(resp, what).await);
                }
                refreshed = true;
                self.refresh_from(token.as_deref().unwrap_or_default()).await?;
                continue;
            }
            let wait = retry_after(&resp);
            if status == StatusCode::FORBIDDEN && self.service == Service::GDrive {
                // Drive reports per-user throttling as a 403 with a reason.
                let body = resp.text().await.unwrap_or_default();
                if !body.contains("rateLimitExceeded") && !body.contains("userRateLimitExceeded") {
                    return Err(self.map_error(status, &body, what));
                }
            } else if !matches!(status, StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE) {
                return Ok(resp);
            }
            if attempt >= policy.max_retries {
                return Err(CxError::Connection(format!("{what}: {} keeps rate-limiting requests; try again later", self.service.label())));
            }
            let backoff = policy.base_delay.saturating_mul(1u32 << attempt.min(16));
            tokio::time::sleep(wait.unwrap_or(backoff).min(policy.max_delay)).await;
            attempt += 1;
        }
    }

    /// Send and require a 2xx answer.
    pub async fn check(&self, what: &str, build: impl Fn(&str) -> RequestBuilder + Send + Sync) -> Result<Response> {
        let resp = self.send(what, build).await?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        Err(self.error(resp, what).await)
    }

    /// Send, require success and parse the JSON answer.
    pub async fn json(&self, what: &str, build: impl Fn(&str) -> RequestBuilder + Send + Sync) -> Result<serde_json::Value> {
        let resp = self.check(what, build).await?;
        read_json(resp).await.map_err(|e| CxError::Io(format!("{what}: {e}")))
    }

    /// Turn a failed response into an error, reading its body.
    pub async fn error(&self, resp: Response, what: &str) -> CxError {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        self.map_error(status, &body, what)
    }

    /// Map an HTTP failure to a `CxError`. The three services report errors
    /// differently: Drive `{"error":{"errors":[{"reason"}],"message"}}`,
    /// Dropbox `409 {"error_summary":"path/not_found/…"}`, Graph
    /// `{"error":{"code":"itemNotFound","message"}}`.
    pub fn map_error(&self, status: StatusCode, body: &str, what: &str) -> CxError {
        let v: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
        let s = |p: &str| v.pointer(p).and_then(|x| x.as_str()).unwrap_or_default().to_string();
        let (code, message) = match self.service {
            Service::GDrive => (s("/error/errors/0/reason"), s("/error/message")),
            Service::Dropbox => (s("/error_summary"), s("/error_summary")),
            Service::OneDrive => (s("/error/code"), s("/error/message")),
        };
        let detail = if !message.is_empty() {
            message
        } else if !body.trim().is_empty() && body.len() < 300 {
            body.trim().to_string()
        } else {
            format!("HTTP {status}")
        };
        let c = code.as_str();
        let label = self.service.label();
        if status == StatusCode::UNAUTHORIZED || c.starts_with("invalid_access_token") || c.starts_with("expired_access_token") {
            return self.auth_required(format!("{label} did not accept the sign-in: {detail}"));
        }
        if c.contains("not_found") || c == "itemNotFound" || c == "notFound" || status == StatusCode::NOT_FOUND {
            return CxError::NotFound(what.to_string());
        }
        if c.contains("conflict") || c == "nameAlreadyExists" || (status == StatusCode::CONFLICT && self.service != Service::Dropbox) {
            return CxError::AlreadyExists(what.to_string());
        }
        if c.contains("insufficient_space") || c.contains("insufficient_quota") || matches!(c, "storageQuotaExceeded" | "quotaLimitReached" | "quotaExceeded")
            || status == StatusCode::INSUFFICIENT_STORAGE
        {
            return CxError::Io(format!("{what}: not enough space in the {label} account"));
        }
        if c.contains("malformed_path") || c.contains("disallowed_name") {
            return CxError::InvalidName(what.to_string());
        }
        if c.contains("no_write_permission") || c.contains("access_denied") || c.contains("team_folder") || status == StatusCode::FORBIDDEN {
            return CxError::PermissionDenied(format!("{what}: {detail}"));
        }
        CxError::Io(format!("{what}: {detail}"))
    }
}

/// `Retry-After` as seconds or an HTTP date.
fn retry_after(resp: &Response) -> Option<Duration> {
    let v = resp.headers().get("retry-after")?.to_str().ok()?.trim().to_string();
    if let Ok(secs) = v.parse::<f64>() {
        return Some(Duration::from_secs_f64(secs.max(0.0)));
    }
    let at = httpdate::parse_http_date(&v).ok()?;
    Some(at.duration_since(std::time::SystemTime::now()).unwrap_or_default())
}
