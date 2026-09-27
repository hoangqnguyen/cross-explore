//! OAuth 2.0 for installed apps: authorization code + PKCE (RFC 7636) with a
//! loopback redirect (RFC 8252 §7.3).
//!
//! The flow the app drives:
//!
//! 1. [`start_authorization`] binds `127.0.0.1:<port>` and builds the
//!    sign-in URL (`redirect_uri = http://127.0.0.1:<port>/callback`).
//! 2. The app opens [`AuthRequest::url`] in the system browser.
//! 3. [`AuthRequest::wait`] serves the one request the browser makes to the
//!    callback, checks `state`, exchanges the code (with the PKCE verifier)
//!    for tokens and asks the service who signed in.
//! 4. The app stores [`SignedIn::credentials`] for [`SignedIn::endpoint`]
//!    in its credential store and connects.
//!
//! Where a loopback listener is impossible (a phone app that uses a custom
//! URL scheme and the OS's web-auth session), build the request with
//! [`start_authorization_with_redirect`] and hand the URL the OS returns to
//! [`AuthRequest::complete`].
//!
//! PKCE is why no client secret has to be shipped: the verifier never leaves
//! this process, so an intercepted code is useless to anyone else. `state`
//! ties the callback to this attempt (CSRF).

use crate::api::{http_client, read_json};
use crate::service::{ClientConfig, Service};
use crate::tokens::Tokens;
use crate::util::{describe, encode, query, random_bytes};
use base64::Engine;
use cx_core::{Credentials, CxError, Endpoint, Result, Scheme};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Path the loopback listener answers on.
pub const CALLBACK_PATH: &str = "/callback";

fn b64url(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

/// A PKCE verifier (43 chars of base64url, 256 bits) and its S256 challenge.
pub(crate) fn pkce_pair() -> (String, String) {
    let verifier = b64url(&random_bytes(32));
    let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// A sign-in in progress.
pub struct AuthRequest {
    /// Open this in the browser.
    pub url: String,
    /// Opaque value the callback must echo.
    pub state: String,
    pub redirect_uri: String,
    pub service: Service,
    client: ClientConfig,
    verifier: String,
    listener: Option<TcpListener>,
}

impl std::fmt::Debug for AuthRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthRequest").field("url", &self.url).field("redirect_uri", &self.redirect_uri).field("service", &self.service).finish_non_exhaustive()
    }
}

/// Who signed in, and the tokens to keep.
#[derive(Debug, Clone)]
pub struct SignedIn {
    pub service: Service,
    /// The account's e-mail address (what goes into the URI).
    pub account: String,
    pub display_name: Option<String>,
    pub tokens: Tokens,
}

impl SignedIn {
    /// Credentials to store for [`SignedIn::endpoint`].
    pub fn credentials(&self) -> Credentials {
        self.tokens.to_credentials(&self.account)
    }

    /// The endpoint for this account under `scheme` (the core scheme the
    /// app registered for this service).
    pub fn endpoint(&self, scheme: Scheme) -> Endpoint {
        account_endpoint(scheme, &self.account)
    }
}

/// `me@example.com` → `scheme://me@example.com`: the local part becomes the
/// endpoint user and the domain its host, so the URI reads as the address.
/// Accounts without an `@` become the host alone.
pub fn account_endpoint(scheme: Scheme, account: &str) -> Endpoint {
    match account.rsplit_once('@') {
        Some((user, host)) if !user.is_empty() && !host.is_empty() => {
            Endpoint { scheme, user: Some(user.to_string()), host: host.to_ascii_lowercase(), port: None }
        }
        _ => Endpoint { scheme, user: None, host: account.to_ascii_lowercase(), port: None },
    }
}

/// The account an endpoint stands for (inverse of [`account_endpoint`]).
pub fn endpoint_account(ep: &Endpoint) -> String {
    match &ep.user {
        Some(u) => format!("{u}@{}", ep.host),
        None => ep.host.clone(),
    }
}

/// Bind the loopback listener and build the sign-in URL.
pub async fn start_authorization(service: Service, client: &ClientConfig) -> Result<AuthRequest> {
    let port = client.redirect_port.unwrap_or(0);
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| CxError::Connection(format!("can't listen on 127.0.0.1:{port} for the sign-in callback: {e}")))?;
    let port = listener.local_addr().map_err(|e| CxError::io("sign-in listener", e))?.port();
    let mut req = start_authorization_with_redirect(service, client, &format!("http://127.0.0.1:{port}{CALLBACK_PATH}"))?;
    req.listener = Some(listener);
    Ok(req)
}

/// Build a sign-in URL for a redirect URI the caller handles itself.
pub fn start_authorization_with_redirect(service: Service, client: &ClientConfig, redirect_uri: &str) -> Result<AuthRequest> {
    if client.client_id.trim().is_empty() {
        return Err(CxError::InvalidLocation(format!("no {} client ID is configured", service.label())));
    }
    let (verifier, challenge) = pkce_pair();
    let state = b64url(&random_bytes(16));
    let mut params: Vec<(&str, &str)> = vec![
        ("response_type", "code"),
        ("client_id", &client.client_id),
        ("redirect_uri", redirect_uri),
        ("state", &state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
    ];
    if let Some(scope) = service.scopes() {
        params.push(("scope", scope));
    }
    match service {
        // offline + consent: Google only hands out a refresh token on the
        // first consent unless asked again.
        Service::GDrive => params.extend([("access_type", "offline"), ("prompt", "consent")]),
        Service::Dropbox => params.push(("token_access_type", "offline")),
        Service::OneDrive => params.push(("prompt", "select_account")),
    }
    let sep = if client.urls.authorize.contains('?') { '&' } else { '?' };
    let url = format!("{}{sep}{}", client.urls.authorize, query(&params));
    Ok(AuthRequest { url, state, redirect_uri: redirect_uri.to_string(), service, client: client.clone(), verifier, listener: None })
}

impl AuthRequest {
    /// Serve the loopback callback until the browser comes back (or
    /// `timeout` passes), then exchange the code for tokens.
    pub async fn wait(mut self, timeout: Duration) -> Result<SignedIn> {
        let listener = self.listener.take().ok_or_else(|| CxError::Unsupported("this sign-in has no loopback listener; use complete()".into()))?;
        let code = tokio::time::timeout(timeout, wait_for_code(&listener, &self.state, self.service))
            .await
            .map_err(|_| CxError::Cancelled)??;
        drop(listener);
        complete_authorization(&self, &code).await
    }

    /// Finish with the redirect URL the browser was sent to (for callers
    /// that handle the redirect themselves).
    pub async fn complete(&self, callback_url: &str) -> Result<SignedIn> {
        let url = url::Url::parse(callback_url).map_err(|e| CxError::InvalidLocation(format!("{callback_url}: {e}")))?;
        let code = code_from_query(url.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())), &self.state)?;
        complete_authorization(self, &code).await
    }
}

/// Pick `code` out of callback parameters, checking `state` and errors.
fn code_from_query(pairs: impl Iterator<Item = (String, String)>, state: &str) -> Result<String> {
    let (mut code, mut got_state, mut error, mut desc) = (None, None, None, None);
    for (k, v) in pairs {
        match k.as_str() {
            "code" => code = Some(v),
            "state" => got_state = Some(v),
            "error" => error = Some(v),
            "error_description" => desc = Some(v),
            _ => {}
        }
    }
    if got_state.as_deref() != Some(state) {
        return Err(CxError::PermissionDenied("sign-in callback with a wrong state (not from this sign-in)".into()));
    }
    if let Some(e) = error {
        if e == "access_denied" {
            return Err(CxError::Cancelled);
        }
        return Err(CxError::PermissionDenied(format!("sign-in failed: {}", desc.unwrap_or(e))));
    }
    code.filter(|c| !c.is_empty()).ok_or_else(|| CxError::Io("sign-in callback without a code".into()))
}

/// Exchange an authorization code for tokens and look up the account.
pub async fn complete_authorization(req: &AuthRequest, code: &str) -> Result<SignedIn> {
    let http = http_client()?;
    let c = &req.client;
    let mut form: Vec<(&str, &str)> = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", &req.redirect_uri),
        ("client_id", &c.client_id),
        ("code_verifier", &req.verifier),
    ];
    if let Some(secret) = &c.client_secret {
        form.push(("client_secret", secret));
    }
    let v = post_form(&http, &c.urls.token, &form).await.map_err(|e| match e {
        TokenError::Rejected(m) => CxError::PermissionDenied(format!("{} refused the sign-in: {m}", req.service.label())),
        TokenError::Other(e) => e,
    })?;
    let tokens = Tokens::from_response(req.service, &c.client_id, &v, None)
        .ok_or_else(|| CxError::Io(format!("{}: no access token in the token answer", req.service.label())))?;
    let (account, display_name) = fetch_account(&http, req.service, c, &tokens.access_token).await?;
    Ok(SignedIn { service: req.service, account, display_name, tokens })
}

pub(crate) enum TokenError {
    /// The token endpoint said no (`invalid_grant`, revoked, wrong client).
    Rejected(String),
    Other(CxError),
}

async fn post_form(http: &reqwest::Client, url: &str, form: &[(&str, &str)]) -> std::result::Result<serde_json::Value, TokenError> {
    let resp = http
        .post(url)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .body(query(form))
        .send()
        .await
        .map_err(|e| TokenError::Other(CxError::Connection(format!("{url}: {}", describe(&e)))))?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| TokenError::Other(CxError::Connection(format!("{url}: {}", describe(&e)))))?;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
    if status.is_success() {
        return Ok(v);
    }
    let reason = v
        .get("error_description")
        .and_then(|d| d.as_str())
        .or_else(|| v.get("error").and_then(|d| d.as_str()))
        .map(str::to_owned)
        .unwrap_or_else(|| format!("HTTP {status}"));
    if status.is_client_error() {
        Err(TokenError::Rejected(reason))
    } else {
        Err(TokenError::Other(CxError::Connection(format!("{url}: {reason}"))))
    }
}

/// Trade a refresh token for a new access token.
pub(crate) async fn refresh(http: &reqwest::Client, tokens: &Tokens, client: &ClientConfig) -> std::result::Result<Tokens, TokenError> {
    let Some(refresh_token) = tokens.refresh_token.as_deref() else {
        return Err(TokenError::Rejected("no refresh token".into()));
    };
    // A refresh token only works with the client it was issued to.
    let client_id = tokens.client_id.as_deref().unwrap_or(&client.client_id);
    let mut form = vec![("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", client_id)];
    if let Some(secret) = client.client_secret.as_deref().filter(|_| client_id == client.client_id) {
        form.push(("client_secret", secret));
    }
    let v = post_form(http, &client.urls.token, &form).await?;
    Tokens::from_response(tokens.service, client_id, &v, tokens.refresh_token.clone())
        .ok_or_else(|| TokenError::Other(CxError::Io("no access token in the refresh answer".into())))
}

/// Ask the service who the token belongs to: (e-mail, display name).
async fn fetch_account(http: &reqwest::Client, service: Service, c: &ClientConfig, token: &str) -> Result<(String, Option<String>)> {
    let api = &c.urls.api;
    let rb = match service {
        Service::GDrive => http.get(format!("{api}/about?fields={}", encode("user(emailAddress,displayName)"))),
        Service::Dropbox => http.post(format!("{api}/users/get_current_account")).header("content-type", "application/json").body("null"),
        Service::OneDrive => http.get(format!("{api}/me?$select=userPrincipalName,mail,displayName")),
    };
    let what = format!("{} account", service.label());
    let resp = rb.bearer_auth(token).send().await.map_err(|e| CxError::Connection(format!("{what}: {}", describe(&e))))?;
    let status = resp.status();
    let v = read_json(resp).await.map_err(|e| CxError::Connection(format!("{what}: {e}")))?;
    if !status.is_success() {
        return Err(CxError::PermissionDenied(format!("{what}: HTTP {status}")));
    }
    let s = |p: &str| v.pointer(p).and_then(|x| x.as_str()).filter(|x| !x.is_empty()).map(str::to_owned);
    let (email, name) = match service {
        Service::GDrive => (s("/user/emailAddress"), s("/user/displayName")),
        Service::Dropbox => (s("/email"), s("/name/display_name")),
        Service::OneDrive => (s("/mail").or_else(|| s("/userPrincipalName")), s("/displayName")),
    };
    let email = email.ok_or_else(|| CxError::Io(format!("{what}: no e-mail address in the answer")))?;
    Ok((email, name))
}

async fn wait_for_code(listener: &TcpListener, state: &str, service: Service) -> Result<String> {
    loop {
        let (stream, _) = listener.accept().await.map_err(|e| CxError::io("sign-in listener", e))?;
        if let Some(result) = handle_callback(stream, state, service).await {
            return result;
        }
    }
}

/// Answer one loopback request. `None` means "not the callback" (a favicon
/// request, a stray connection): keep waiting.
async fn handle_callback(mut stream: TcpStream, state: &str, service: Service) -> Option<Result<String>> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        match tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk)).await {
            Ok(Ok(0)) | Err(_) | Ok(Err(_)) => break,
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let target = head.lines().next().and_then(|l| {
        let mut parts = l.split_whitespace();
        (parts.next() == Some("GET")).then(|| parts.next()).flatten()
    });
    let Some(target) = target else {
        respond(&mut stream, "400 Bad Request", "Bad request").await;
        return None;
    };
    let (path, q) = target.split_once('?').unwrap_or((target, ""));
    if path != CALLBACK_PATH {
        respond(&mut stream, "404 Not Found", "Not found").await;
        return None;
    }
    let pairs = url::form_urlencoded::parse(q.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned()));
    let result = code_from_query(pairs, state);
    match &result {
        // A stale tab or a forged request: tell it, and keep waiting for ours.
        Err(CxError::PermissionDenied(m)) if m.contains("wrong state") => {
            respond(&mut stream, "400 Bad Request", "This sign-in link is not the current one. Start the sign-in again from Cross Explore.").await;
            return None;
        }
        Ok(_) => {
            let msg = format!("Signed in to {}. You can close this tab and return to Cross Explore.", service.label());
            respond(&mut stream, "200 OK", &msg).await;
        }
        Err(e) => respond(&mut stream, "200 OK", &format!("Sign-in did not complete: {e}. You can close this tab.")).await,
    }
    Some(result)
}

async fn respond(stream: &mut TcpStream, status: &str, message: &str) {
    let escaped = message.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Cross Explore</title></head>\
         <body style=\"font-family:system-ui,sans-serif;margin:3em;text-align:center\"><p>{escaped}</p></body></html>"
    );
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_is_s256_of_the_verifier() {
        let (v, c) = pkce_pair();
        assert_eq!(v.len(), 43);
        assert!(v.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'));
        assert_eq!(c, b64url(&Sha256::digest(v.as_bytes())));
        // RFC 7636 appendix B.
        let rfc = b64url(&Sha256::digest(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"));
        assert_eq!(rfc, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn accounts_map_to_endpoints_and_back() {
        let ep = account_endpoint(Scheme::Dav, "Me.Name@Gmail.com");
        assert_eq!(ep.user.as_deref(), Some("Me.Name"));
        assert_eq!(ep.host, "gmail.com");
        assert_eq!(endpoint_account(&ep), "Me.Name@gmail.com");
        assert_eq!(account_endpoint(Scheme::Dav, "work").host, "work");
    }

    #[test]
    fn callback_parameters() {
        let p = |q: &str| url::form_urlencoded::parse(q.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect::<Vec<_>>();
        assert_eq!(code_from_query(p("code=abc&state=s1").into_iter(), "s1").unwrap(), "abc");
        assert!(matches!(code_from_query(p("code=abc&state=zz").into_iter(), "s1"), Err(CxError::PermissionDenied(_))));
        assert!(matches!(code_from_query(p("error=access_denied&state=s1").into_iter(), "s1"), Err(CxError::Cancelled)));
    }
}
