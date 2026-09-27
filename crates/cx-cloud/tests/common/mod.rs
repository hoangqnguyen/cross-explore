//! An in-process HTTP server (hyper) that the service mocks plug into, plus
//! the OAuth token endpoint they share.
//!
//! Every mock lives under one origin laid out like `ServiceUrls::local`:
//! `/token`, `/api/…`, `/content/…`, plus whatever pre-authorized URLs the
//! mock hands out (upload sessions, download links, copy monitors).

#![allow(dead_code)]

pub mod dropbox_mock;
pub mod gdrive_mock;
pub mod onedrive_mock;

use base64::Engine;
use bytes::Bytes;
use cx_cloud::{CloudProvider, ClientConfig, RetryPolicy, Service, ServiceUrls, Tokens};
use cx_core::provider::list_all;
use cx_core::{Endpoint, Entry, Location, Provider, Scheme, WriteMode};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct Req {
    pub method: String,
    /// Percent-decoded path.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: HashMap<String, String>,
    pub body: Bytes,
}

impl Req {
    pub fn q(&self, k: &str) -> Option<&str> {
        self.query.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str())
    }
    pub fn h(&self, k: &str) -> Option<&str> {
        self.headers.get(k).map(String::as_str)
    }
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
    pub fn bearer(&self) -> Option<&str> {
        self.h("authorization").and_then(|a| a.strip_prefix("Bearer "))
    }
    /// `Range: bytes=N-` start.
    pub fn range_start(&self) -> Option<usize> {
        self.h("range")?.strip_prefix("bytes=")?.trim_end_matches('-').parse().ok()
    }
}

pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(status: u16, v: Value) -> Resp {
        Resp { status, headers: vec![("content-type".into(), "application/json".into())], body: v.to_string().into_bytes() }
    }
    pub fn empty(status: u16) -> Resp {
        Resp { status, headers: vec![], body: vec![] }
    }
    pub fn bytes(status: u16, data: Vec<u8>) -> Resp {
        Resp { status, headers: vec![("content-type".into(), "application/octet-stream".into())], body: data }
    }
    pub fn header(mut self, k: &str, v: impl Into<String>) -> Resp {
        self.headers.push((k.into(), v.into()));
        self
    }
    /// A download honoring `Range: bytes=N-`.
    pub fn ranged(req: &Req, data: &[u8]) -> Resp {
        match req.range_start() {
            Some(start) if start >= data.len() && !data.is_empty() => Resp::empty(416),
            Some(start) => Resp::bytes(206, data[start.min(data.len())..].to_vec())
                .header("content-range", format!("bytes {start}-{}/{}", data.len().saturating_sub(1), data.len())),
            None => Resp::bytes(200, data.to_vec()),
        }
    }
}

/// What a service mock implements.
pub trait Handler: Send + Sync + 'static {
    fn handle(&self, req: &Req, origin: &str) -> Resp;
}

/// Shared OAuth state: which access tokens are valid and how refreshes go.
#[derive(Default)]
pub struct Auth {
    pub valid: Mutex<HashSet<String>>,
    /// Refresh tokens the token endpoint accepts.
    pub refresh_ok: Mutex<HashSet<String>>,
    pub refreshes: AtomicUsize,
    /// code → (PKCE challenge, redirect uri) for authorization-code grants.
    pub codes: Mutex<HashMap<String, (String, String)>>,
    pub issued: AtomicUsize,
}

impl Auth {
    pub fn new(access: &str, refresh: &str) -> Auth {
        let a = Auth::default();
        a.valid.lock().unwrap().insert(access.into());
        a.refresh_ok.lock().unwrap().insert(refresh.into());
        a
    }

    fn issue(&self) -> String {
        let t = format!("access-{}", self.issued.fetch_add(1, Ordering::SeqCst) + 1);
        self.valid.lock().unwrap().insert(t.clone());
        t
    }

    pub fn revoke_all_access(&self) {
        self.valid.lock().unwrap().clear();
    }

    fn token_endpoint(&self, req: &Req) -> Resp {
        let form: HashMap<String, String> = url::form_urlencoded::parse(&req.body).into_owned().collect();
        match form.get("grant_type").map(String::as_str) {
            Some("refresh_token") => {
                self.refreshes.fetch_add(1, Ordering::SeqCst);
                let rt = form.get("refresh_token").cloned().unwrap_or_default();
                if form.get("client_id").map(String::as_str) != Some("test-client") || !self.refresh_ok.lock().unwrap().contains(&rt) {
                    return Resp::json(400, json!({"error": "invalid_grant", "error_description": "Token has been expired or revoked."}));
                }
                Resp::json(200, json!({"access_token": self.issue(), "expires_in": 3599, "token_type": "Bearer"}))
            }
            Some("authorization_code") => {
                let code = form.get("code").cloned().unwrap_or_default();
                let Some((challenge, redirect)) = self.codes.lock().unwrap().remove(&code) else {
                    return Resp::json(400, json!({"error": "invalid_grant"}));
                };
                let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
                if computed != challenge || form.get("redirect_uri") != Some(&redirect) {
                    return Resp::json(400, json!({"error": "invalid_grant", "error_description": "PKCE or redirect mismatch"}));
                }
                self.refresh_ok.lock().unwrap().insert("fresh-refresh".into());
                Resp::json(200, json!({"access_token": self.issue(), "refresh_token": "fresh-refresh", "expires_in": 3599, "token_type": "Bearer"}))
            }
            _ => Resp::json(400, json!({"error": "unsupported_grant_type"})),
        }
    }
}

pub struct Logged {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: HashMap<String, String>,
    pub body_len: usize,
}

pub struct Mock {
    pub origin: String,
    pub auth: Arc<Auth>,
    /// The next N `/api` or `/content` requests get 429 with `Retry-After`.
    pub throttle: Arc<AtomicUsize>,
    pub retry_after: Arc<Mutex<String>>,
    pub log: Arc<Mutex<Vec<Logged>>>,
}

impl Mock {
    pub fn requests(&self, method: &str, path_prefix: &str) -> usize {
        self.log.lock().unwrap().iter().filter(|l| l.method == method && l.path.starts_with(path_prefix)).count()
    }
}

pub async fn serve(handler: Arc<dyn Handler>, auth: Arc<Auth>) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let throttle = Arc::new(AtomicUsize::new(0));
    let retry_after = Arc::new(Mutex::new("1".to_string()));
    let log = Arc::new(Mutex::new(Vec::new()));
    let mock = Mock { origin: origin.clone(), auth: auth.clone(), throttle: throttle.clone(), retry_after: retry_after.clone(), log: log.clone() };
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let (handler, auth, origin, throttle, retry_after, log) = (handler.clone(), auth.clone(), origin.clone(), throttle.clone(), retry_after.clone(), log.clone());
            tokio::spawn(async move {
                let svc = service_fn(move |req: hyper::Request<Incoming>| {
                    let (handler, auth, origin, throttle, retry_after, log) = (handler.clone(), auth.clone(), origin.clone(), throttle.clone(), retry_after.clone(), log.clone());
                    async move {
                        let (parts, body) = req.into_parts();
                        let body = body.collect().await.map(|b| b.to_bytes()).unwrap_or_default();
                        let raw_path = parts.uri.path().to_string();
                        let path = percent_encoding::percent_decode_str(&raw_path).decode_utf8_lossy().into_owned();
                        let query: Vec<(String, String)> = url::form_urlencoded::parse(parts.uri.query().unwrap_or("").as_bytes()).into_owned().collect();
                        let headers = parts.headers.iter().map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or_default().to_string())).collect();
                        let req = Req { method: parts.method.to_string(), path, query, headers, body };
                        log.lock().unwrap().push(Logged {
                            method: req.method.clone(),
                            path: req.path.clone(),
                            query: req.query.clone(),
                            headers: req.headers.clone(),
                            body_len: req.body.len(),
                        });
                        let resp = if req.path == "/token" {
                            auth.token_endpoint(&req)
                        } else if (req.path.starts_with("/api") || req.path.starts_with("/content"))
                            && throttle.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)).is_ok()
                        {
                            Resp::json(429, json!({"error": {"code": 429, "message": "Rate Limit Exceeded"}})).header("retry-after", retry_after.lock().unwrap().clone())
                        } else if (req.path.starts_with("/api") || req.path.starts_with("/content"))
                            && !req.bearer().is_some_and(|t| auth.valid.lock().unwrap().contains(t))
                        {
                            Resp::json(401, json!({"error": {"code": 401, "message": "Invalid Credentials", "errors": [{"reason": "authError"}]}}))
                        } else {
                            handler.handle(&req, &origin)
                        };
                        let mut b = hyper::Response::builder().status(resp.status);
                        for (k, v) in resp.headers {
                            b = b.header(k, v);
                        }
                        Ok::<_, Infallible>(b.body(Full::new(Bytes::from(resp.body))).unwrap())
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(stream), svc).await;
            });
        }
    });
    mock
}

pub const ACCESS: &str = "initial-access";
pub const REFRESH: &str = "good-refresh";

pub fn endpoint() -> Endpoint {
    // cx-core has no cloud schemes yet; providers only use the path.
    Endpoint { scheme: Scheme::Davs, user: Some("me".into()), host: "example.com".into(), port: None }
}

pub fn loc(path: &str) -> Location {
    Location::remote(endpoint(), path)
}

pub fn config(service: Service, mock: &Mock) -> ClientConfig {
    ClientConfig::new(service, "test-client")
        .with_urls(ServiceUrls::local(&mock.origin))
        .with_retry(RetryPolicy { max_retries: 3, base_delay: Duration::from_millis(10), max_delay: Duration::from_secs(5) })
}

pub fn tokens(service: Service, access: &str, expires_in_ms: i64) -> Tokens {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
    Tokens { service, access_token: access.into(), refresh_token: Some(REFRESH.into()), expires_at: Some(now + expires_in_ms), client_id: Some("test-client".into()) }
}

pub async fn connect(service: Service, mock: &Mock) -> CloudProvider {
    let creds = tokens(service, ACCESS, 3_600_000).to_credentials("me@example.com");
    cx_cloud::open(service, &endpoint(), Some(creds), Some(&config(service, mock)), None).await.unwrap()
}

pub async fn names(p: &dyn Provider, path: &str) -> Vec<String> {
    let mut v: Vec<String> = list_all(p, &loc(path)).await.unwrap().into_iter().map(|e| e.name).collect();
    v.sort();
    v
}

pub async fn entries(p: &dyn Provider, path: &str) -> HashMap<String, Entry> {
    list_all(p, &loc(path)).await.unwrap().into_iter().map(|e| (e.name.clone(), e)).collect()
}

pub async fn read(p: &dyn Provider, path: &str, offset: u64) -> Vec<u8> {
    let mut r = p.open_read(&loc(path), offset).await.unwrap();
    let mut out = Vec::new();
    r.read_to_end(&mut out).await.unwrap();
    out
}

pub async fn write(p: &dyn Provider, path: &str, mode: WriteMode, data: &[u8]) -> cx_core::Result<()> {
    let mut w = p.open_write(&loc(path), mode).await?;
    // Odd-sized writes, like a real copy loop.
    for chunk in data.chunks(10_000) {
        w.write_all(chunk).await.map_err(|e| cx_core::CxError::from_io(e, path))?;
    }
    w.shutdown().await.map_err(|e| cx_core::CxError::from_io(e, path))
}

/// Deterministic test bytes.
pub fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 7 % 251) as u8).collect()
}

/// RFC 3339 for mock timestamps: `2024-01-01T00:00:00Z` plus `secs`.
pub fn ts(secs: u64) -> String {
    format!("2024-01-01T{:02}:{:02}:{:02}Z", secs / 3600 % 24, secs / 60 % 60, secs % 60)
}
