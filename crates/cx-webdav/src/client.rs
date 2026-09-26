//! A thin HTTP layer for WebDAV: URL building, authentication and redirects.
//!
//! Redirects are followed here rather than by reqwest, because reqwest (like
//! browsers) turns a redirected non-GET request into a GET, which silently
//! breaks PROPFIND, MOVE or DELETE on servers that redirect `/dir` to
//! `/dir/`.

use crate::auth::{self, Scheme as AuthScheme};
use bytes::Bytes;
use cx_core::{CxError, Endpoint, Result, Scheme};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, LOCATION, WWW_AUTHENTICATE};
use reqwest::{Method, Response, StatusCode};
use std::sync::Mutex;
use std::time::Duration;

/// Everything but RFC 3986 unreserved characters is escaped in a segment.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');
const MAX_REDIRECTS: usize = 5;

pub(crate) struct DavClient {
    http: reqwest::Client,
    /// `http(s)://host[:port]`, no trailing slash.
    origin: String,
    creds: Option<(String, String)>,
    auth: Mutex<AuthScheme>,
    uri: String,
    user: Option<String>,
}

pub(crate) fn method(name: &str) -> Method {
    Method::from_bytes(name.as_bytes()).expect("valid method")
}

impl DavClient {
    pub(crate) fn new(ep: &Endpoint, creds: Option<(String, String)>) -> Result<DavClient> {
        let scheme = if ep.scheme == Scheme::Davs { "https" } else { "http" };
        let host = if ep.host.contains(':') { format!("[{}]", ep.host) } else { ep.host.clone() };
        let origin = match ep.port {
            Some(p) if p != ep.scheme.default_port() => format!("{scheme}://{host}:{p}"),
            _ => format!("{scheme}://{host}"),
        };
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            // Keep connections warm between listings.
            .pool_idle_timeout(Duration::from_secs(90))
            .user_agent(concat!("CrossExplore/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| CxError::Connection(format!("http client: {e}")))?;
        let user = creds.as_ref().map(|(u, _)| u.clone()).or_else(|| ep.user.clone());
        Ok(DavClient { http, origin, creds, auth: Mutex::new(AuthScheme::Unknown), uri: ep.uri(), user })
    }

    /// The URL of a POSIX `path`; folders get a trailing slash, which many
    /// servers otherwise answer with a redirect.
    pub(crate) fn url(&self, path: &str, dir: bool) -> String {
        let mut url = self.origin.clone();
        for seg in path.split('/').filter(|s| !s.is_empty()) {
            url.push('/');
            url.push_str(&utf8_percent_encode(seg, SEGMENT).to_string());
        }
        if dir || url.len() == self.origin.len() {
            url.push('/');
        }
        url
    }

    pub(crate) fn auth_required(&self, reason: impl Into<String>) -> CxError {
        CxError::AuthRequired { uri: self.uri.clone(), user: self.user.clone(), reason: reason.into() }
    }

    pub(crate) fn has_credentials(&self) -> bool {
        self.creds.is_some()
    }

    /// True once a 401 has told us how to sign requests.
    pub(crate) fn auth_known(&self) -> bool {
        !matches!(*self.auth.lock().unwrap(), AuthScheme::Unknown)
    }

    fn authorize(&self, method: &Method, url: &str, headers: &mut HeaderMap) {
        let Some((user, password)) = &self.creds else { return };
        let target = request_target(url);
        let value = auth::authorization(&mut self.auth.lock().unwrap(), user, password, method.as_str(), &target);
        if let Some(v) = value.and_then(|v| HeaderValue::from_str(&v).ok()) {
            headers.insert(AUTHORIZATION, v);
        }
    }

    /// Learn the scheme from a 401. Returns false when retrying is pointless
    /// (no credentials, or they were already rejected).
    fn learn(&self, resp: &Response) -> bool {
        if self.creds.is_none() {
            return false;
        }
        let offer = auth::parse_offer(resp.headers().get_all(WWW_AUTHENTICATE).iter().filter_map(|v| v.to_str().ok()));
        let stale = offer.stale;
        let Some(next) = offer.pick() else { return false };
        let mut cur = self.auth.lock().unwrap();
        let retry = match (&*cur, &next) {
            (AuthScheme::Unknown, _) => true,
            // Only a new nonce is worth another try with the same password.
            (AuthScheme::Digest(_), AuthScheme::Digest(_)) => stale,
            _ => false,
        };
        if retry {
            *cur = next;
        }
        retry
    }

    /// Send a request whose body can be replayed (for auth and redirects).
    pub(crate) async fn send(&self, method: Method, url: &str, headers: HeaderMap, body: Option<Bytes>) -> Result<Response> {
        let mut url = url.to_string();
        let mut redirects = 0;
        let mut auth_tries = 0;
        loop {
            let mut h = headers.clone();
            self.authorize(&method, &url, &mut h);
            let mut req = self.http.request(method.clone(), &url).headers(h);
            if let Some(b) = &body {
                req = req.body(b.clone());
            }
            let resp = req.send().await.map_err(|e| self.transport_error(e))?;
            match resp.status() {
                StatusCode::UNAUTHORIZED => {
                    auth_tries += 1;
                    if auth_tries > 2 || !self.learn(&resp) {
                        return Err(self.rejected());
                    }
                }
                s if s.is_redirection() && s != StatusCode::NOT_MODIFIED => {
                    let Some(next) = resp.headers().get(LOCATION).and_then(|l| l.to_str().ok()).and_then(|l| self.resolve(&url, l)) else {
                        return Ok(resp);
                    };
                    redirects += 1;
                    if redirects > MAX_REDIRECTS {
                        return Err(CxError::Io(format!("{url}: too many redirects")));
                    }
                    url = next;
                }
                _ => return Ok(resp),
            }
        }
    }

    /// Send a request with a streamed (one-shot) body. Authentication must
    /// already be negotiated (see [`DavClient::prepare_auth`]).
    pub(crate) async fn send_once(&self, method: Method, url: &str, mut headers: HeaderMap, body: reqwest::Body) -> Result<Response> {
        self.authorize(&method, url, &mut headers);
        let resp = self.http.request(method, url).headers(headers).body(body).send().await.map_err(|e| self.transport_error(e))?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(self.rejected());
        }
        Ok(resp)
    }

    /// Make sure requests to `url` will be signed correctly before sending a
    /// body that can't be replayed: a cheap PROPFIND triggers the 401 dance.
    pub(crate) async fn prepare_auth(&self, url: &str) -> Result<()> {
        if !self.has_credentials() || self.auth_known() {
            return Ok(());
        }
        let mut h = HeaderMap::new();
        h.insert("Depth", HeaderValue::from_static("0"));
        self.send(method("PROPFIND"), url, h, None).await.map(|_| ())
    }

    fn rejected(&self) -> CxError {
        if self.creds.is_some() {
            self.auth_required("the server rejected the user name or password")
        } else {
            self.auth_required("this server requires signing in")
        }
    }

    fn transport_error(&self, e: reqwest::Error) -> CxError {
        CxError::Connection(format!("{}: {e}", self.uri))
    }

    /// Resolve a `Location` header against the current URL, staying on the
    /// same origin (credentials must not leak to another host).
    fn resolve(&self, current: &str, location: &str) -> Option<String> {
        if location.starts_with('/') {
            return Some(format!("{}{location}", self.origin));
        }
        if location.starts_with(&format!("{}/", self.origin)) {
            return Some(location.to_string());
        }
        if !location.contains("://") {
            let base = &current[..current.rfind('/')? + 1];
            return Some(format!("{base}{location}"));
        }
        None
    }

    /// Map a non-success status to an error.
    pub(crate) fn status_error(&self, status: StatusCode, what: &str) -> CxError {
        match status.as_u16() {
            401 => self.rejected(),
            403 => CxError::PermissionDenied(what.to_string()),
            404 | 410 => CxError::NotFound(what.to_string()),
            // Conflict: usually "the parent folder does not exist".
            409 => CxError::NotFound(format!("{what} (parent folder missing)")),
            412 => CxError::AlreadyExists(what.to_string()),
            423 => CxError::PermissionDenied(format!("{what} is locked")),
            501 => CxError::Unsupported(format!("{what}: not implemented by the server")),
            507 => CxError::Io(format!("{what}: server storage is full")),
            _ => CxError::Io(format!("{what}: HTTP {status}")),
        }
    }
}

/// The request target (path and query) of a URL, as Digest signs it.
fn request_target(url: &str) -> String {
    let after_scheme = url.find("://").map(|i| i + 3).unwrap_or(0);
    match url[after_scheme..].find('/') {
        Some(i) => url[after_scheme + i..].to_string(),
        None => "/".to_string(),
    }
}

/// The decoded POSIX path of an href (absolute URL or path), without a
/// trailing slash except for the root.
pub(crate) fn href_path(href: &str) -> String {
    let path = if href.contains("://") { request_target(href) } else { href.to_string() };
    let path = path.split(['?', '#']).next().unwrap_or("");
    let decoded = percent_decode_str(path).decode_utf8_lossy();
    cx_core::location::normalize_posix(&decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> DavClient {
        let ep = Endpoint { scheme: Scheme::Dav, user: None, host: "h".into(), port: Some(8088) };
        DavClient::new(&ep, None).unwrap()
    }

    #[test]
    fn builds_urls() {
        let c = client();
        assert_eq!(c.url("/", true), "http://h:8088/");
        assert_eq!(c.url("/", false), "http://h:8088/");
        assert_eq!(c.url("/a b/ü#?.txt", false), "http://h:8088/a%20b/%C3%BC%23%3F.txt");
        assert_eq!(c.url("/dir", true), "http://h:8088/dir/");
    }

    #[test]
    fn decodes_hrefs() {
        assert_eq!(href_path("/a%20b/c/"), "/a b/c");
        assert_eq!(href_path("http://h:8088/x/%C3%BC.txt"), "/x/ü.txt");
        assert_eq!(href_path("https://h/"), "/");
        assert_eq!(href_path("/dav/a%2Bb"), "/dav/a+b");
    }

    #[test]
    fn resolves_redirects_on_same_origin_only() {
        let c = client();
        assert_eq!(c.resolve("http://h:8088/a/b", "/a/b/").as_deref(), Some("http://h:8088/a/b/"));
        assert_eq!(c.resolve("http://h:8088/a/b", "c/").as_deref(), Some("http://h:8088/a/c/"));
        assert_eq!(c.resolve("http://h:8088/a/b", "http://evil/x"), None);
    }
}
