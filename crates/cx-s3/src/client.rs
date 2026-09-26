//! A thin S3 REST client: path-style URLs, SigV4 signing, error mapping and
//! per-bucket region correction.
//!
//! Path-style addressing (`https://endpoint/bucket/key`) is used everywhere.
//! Every S3-compatible service supports it, it needs no wildcard DNS or
//! certificates (MinIO on an IP address, a NAS), and it lets one connection
//! serve all buckets of an endpoint, which is what an `s3://host/` location
//! means. AWS still serves path-style requests for existing and new buckets.
//!
//! All request bodies are in memory ([`Bytes`]): listing and delete XML is
//! small, and uploads are sent part by part (see the `upload` module). That
//! makes every request replayable, so a request that hits a bucket in
//! another region can simply be re-signed and sent again.

use crate::endpoint::{self, Target};
use crate::sign::{self, Keys, Signable};
use crate::util::{amz_date, encode, encode_key, sha256_hex};
use crate::xml::{self, Node, S3Error};
use bytes::Bytes;
use cx_core::{CxError, Endpoint, Result};
use reqwest::{Method, Response, StatusCode};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// One S3 request, before signing.
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub method: Method,
    pub bucket: Option<String>,
    pub key: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
}

impl Request {
    pub fn new(method: Method, bucket: Option<&str>, key: &str) -> Request {
        Request { method, bucket: bucket.map(str::to_owned), key: key.to_owned(), query: Vec::new(), headers: Vec::new(), body: None }
    }

    pub fn query(mut self, k: &str, v: impl Into<String>) -> Request {
        self.query.push((k.to_owned(), v.into()));
        self
    }

    pub fn header(mut self, k: &str, v: impl Into<String>) -> Request {
        self.headers.push((k.to_ascii_lowercase(), v.into()));
        self
    }

    pub fn body(mut self, b: impl Into<Bytes>) -> Request {
        self.body = Some(b.into());
        self
    }
}

pub(crate) struct S3Client {
    http: reqwest::Client,
    target: Target,
    aws: bool,
    keys: Option<Keys>,
    /// The endpoint URI and user, for `AuthRequired`.
    uri: String,
    user: Option<String>,
    /// Buckets that answered from another region (AWS only needs this when
    /// the endpoint's region is not the bucket's).
    moved: Mutex<HashMap<String, Target>>,
}

impl S3Client {
    pub fn new(ep: &Endpoint, keys: Option<Keys>) -> Result<S3Client> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            // S3 does not redirect for anything we do except wrong-region
            // buckets, which are handled below (and need re-signing anyway).
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("CrossExplore/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| CxError::Connection(format!("http client: {e}")))?;
        let user = keys.as_ref().map(|k| k.access.clone()).or_else(|| ep.user.clone());
        Ok(S3Client { http, target: endpoint::target(ep), aws: endpoint::is_aws(&ep.host), keys, uri: ep.uri(), user, moved: Mutex::new(HashMap::new()) })
    }

    pub fn anonymous(&self) -> bool {
        self.keys.is_none()
    }

    pub fn auth_required(&self, reason: impl Into<String>) -> CxError {
        CxError::AuthRequired { uri: self.uri.clone(), user: self.user.clone(), reason: reason.into() }
    }

    fn target_for(&self, bucket: Option<&str>) -> Target {
        bucket.and_then(|b| self.moved.lock().unwrap().get(b).cloned()).unwrap_or_else(|| self.target.clone())
    }

    /// Send `req`, retrying once in the right region if the bucket lives
    /// elsewhere. Only transport failures are errors here; use [`check`] or
    /// [`xml_ok`] for HTTP statuses.
    pub async fn send(&self, req: &Request) -> Result<Response> {
        for attempt in 0..2 {
            let t = self.target_for(req.bucket.as_deref());
            let resp = self.send_to(&t, req).await?;
            let status = resp.status();
            let hint = resp.headers().get("x-amz-bucket-region").and_then(|v| v.to_str().ok()).map(str::to_owned);
            let wrong_region = matches!(status, StatusCode::MOVED_PERMANENTLY | StatusCode::BAD_REQUEST | StatusCode::TEMPORARY_REDIRECT | StatusCode::FORBIDDEN);
            if let (0, true, Some(bucket), Some(region)) = (attempt, wrong_region, req.bucket.as_deref(), hint) {
                if region != t.region && !region.is_empty() {
                    let origin = if self.aws { format!("https://s3.{region}.amazonaws.com") } else { t.origin.clone() };
                    self.moved.lock().unwrap().insert(bucket.to_owned(), Target { origin, region });
                    continue;
                }
            }
            return Ok(resp);
        }
        unreachable!("the loop returns on its second pass")
    }

    async fn send_to(&self, t: &Target, req: &Request) -> Result<Response> {
        let path = match (&req.bucket, req.key.is_empty()) {
            (None, _) => "/".to_string(),
            (Some(b), true) => format!("/{}", encode(b)),
            (Some(b), false) => format!("/{}/{}", encode(b), encode_key(&req.key)),
        };
        let query = sign::canonical_query(&req.query);
        let url = if query.is_empty() { format!("{}{path}", t.origin) } else { format!("{}{path}?{query}", t.origin) };
        let mut rb = self.http.request(req.method.clone(), &url);
        for (k, v) in &req.headers {
            rb = rb.header(k, v);
        }
        if let Some(keys) = &self.keys {
            let host = t.origin.split_once("://").map(|(_, h)| h).unwrap_or(&t.origin).to_string();
            let hash = sha256_hex(req.body.as_deref().unwrap_or(&[]));
            let date = amz_date(SystemTime::now());
            let mut signed: Vec<(String, String)> = req.headers.iter().filter(|(k, _)| k.starts_with("x-amz-") || k == "content-md5" || k == "range").cloned().collect();
            signed.push(("host".into(), host));
            signed.push(("x-amz-date".into(), date.clone()));
            signed.push(("x-amz-content-sha256".into(), hash.clone()));
            let auth = sign::authorization(
                keys,
                &Signable { method: req.method.as_str(), path: &path, query: &req.query, headers: &signed, payload_hash: &hash, amz_date: &date, region: &t.region },
            );
            rb = rb.header("x-amz-date", date).header("x-amz-content-sha256", hash).header("authorization", auth);
        }
        if let Some(body) = &req.body {
            rb = rb.body(body.clone());
        } else if matches!(req.method, Method::PUT | Method::POST) {
            rb = rb.body(Bytes::new());
        }
        rb.send().await.map_err(|e| CxError::Connection(format!("{}: {}", self.target.origin, describe(&e))))
    }

    /// Send and require a 2xx answer.
    pub async fn check(&self, req: &Request, what: &str) -> Result<Response> {
        let resp = self.send(req).await?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        Err(self.status_error(resp, what).await)
    }

    /// Send, require success and parse the XML answer. S3 can report a
    /// failure inside a 200 (CopyObject, CompleteMultipartUpload), so an
    /// `<Error>` root is an error too.
    pub async fn xml_ok(&self, req: &Request, what: &str) -> Result<Node> {
        let resp = self.check(req, what).await?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| CxError::Connection(format!("{what}: {}", describe(&e))))?;
        let root = xml::parse(&body).map_err(|e| CxError::Io(format!("{what}: {e}")))?;
        if let Some(err) = xml::error(&root) {
            return Err(self.map_error(status, Some(err), what));
        }
        Ok(root)
    }

    /// Turn a failed response into an error, reading its `<Error>` body.
    pub async fn status_error(&self, resp: Response, what: &str) -> CxError {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let err = xml::parse(&body).ok().and_then(|n| xml::error(&n));
        self.map_error(status, err, what)
    }

    pub fn map_error(&self, status: StatusCode, err: Option<S3Error>, what: &str) -> CxError {
        let err = err.unwrap_or_default();
        let detail = if err.message.is_empty() { err.code.clone() } else { err.message.clone() };
        match err.code.as_str() {
            "NoSuchKey" | "NoSuchBucket" | "NoSuchUpload" => return CxError::NotFound(what.to_string()),
            "InvalidAccessKeyId" => return self.auth_required("the access key is not known to this server"),
            "SignatureDoesNotMatch" => return self.auth_required("the secret key is wrong"),
            "ExpiredToken" | "InvalidToken" | "TokenRefreshRequired" => return self.auth_required("the credentials have expired"),
            "BucketAlreadyExists" | "BucketAlreadyOwnedByYou" | "PreconditionFailed" => return CxError::AlreadyExists(what.to_string()),
            "RequestTimeTooSkewed" => return CxError::Connection(format!("{what}: this computer's clock is too far off the server's; fix the time and retry")),
            "AuthorizationHeaderMalformed" if err.region.is_some() => {
                return CxError::Connection(format!("{what}: the bucket is in region {}", err.region.unwrap_or_default()))
            }
            _ => {}
        }
        match status {
            StatusCode::NOT_FOUND => CxError::NotFound(what.to_string()),
            StatusCode::UNAUTHORIZED => self.auth_required(if detail.is_empty() { "sign-in required".into() } else { detail }),
            StatusCode::FORBIDDEN if self.anonymous() => self.auth_required("this bucket is not public; sign in with an access key"),
            StatusCode::FORBIDDEN => CxError::PermissionDenied(if detail.is_empty() { what.to_string() } else { format!("{what}: {detail}") }),
            StatusCode::PRECONDITION_FAILED | StatusCode::CONFLICT if err.code.is_empty() => CxError::AlreadyExists(what.to_string()),
            _ if detail.is_empty() => CxError::Io(format!("{what}: HTTP {status}")),
            _ => CxError::Io(format!("{what}: {detail} ({})", if err.code.is_empty() { status.to_string() } else { err.code })),
        }
    }
}

/// reqwest's top-level message ("error sending request") hides the cause.
pub(crate) fn describe(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        s.push_str(": ");
        s.push_str(&c.to_string());
        src = c.source();
    }
    s
}
