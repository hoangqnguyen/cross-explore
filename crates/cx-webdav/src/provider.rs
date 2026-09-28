use crate::client::{href_path, method, DavClient};
use crate::io;
use crate::propfind::{self, Multistatus, Resource};
use async_trait::async_trait;
use bytes::Bytes;
use cx_core::{
    validate_name, Capabilities, Connector, Credentials, CxError, Endpoint, Entry, EntryKind, Location, Provider, ReadStream, Result, Scheme,
    Secret, Space, WriteMode, WriteStream,
};
use futures_util::TryStreamExt;
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::StatusCode;
use std::sync::Arc;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio_util::io::StreamReader;

type BodyStream = std::pin::Pin<Box<dyn futures_util::Stream<Item = std::io::Result<Bytes>> + Send>>;
type BodyReader = tokio::io::BufReader<StreamReader<BodyStream, Bytes>>;

const FIRST_BATCH: usize = 128;
const NEXT_BATCH: usize = 2048;

/// Opens [`DavProvider`]s. WebDAV has two schemes (`dav://` over http and
/// `davs://` over https), so register one connector for each:
/// `vfs.register(Arc::new(DavConnector::http()))` and `…::https()`.
#[derive(Debug, Clone, Copy)]
pub struct DavConnector {
    scheme: Scheme,
}

impl DavConnector {
    /// `scheme` must be [`Scheme::Dav`] or [`Scheme::Davs`].
    pub fn new(scheme: Scheme) -> Self {
        assert!(matches!(scheme, Scheme::Dav | Scheme::Davs), "not a WebDAV scheme: {scheme}");
        DavConnector { scheme }
    }

    pub fn http() -> Self {
        Self::new(Scheme::Dav)
    }

    pub fn https() -> Self {
        Self::new(Scheme::Davs)
    }
}

#[async_trait]
impl Connector for DavConnector {
    fn scheme(&self) -> Scheme {
        self.scheme
    }

    async fn connect(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(DavProvider::connect(ep, creds).await?))
    }
}

/// A WebDAV server. Paths are the URL paths on the server.
pub struct DavProvider {
    client: Arc<DavClient>,
    endpoint: Endpoint,
}

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in pairs {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(*k, v);
        }
    }
    h
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn to_entry(name: String, r: &Resource) -> Entry {
    Entry {
        hidden: is_hidden(&name),
        kind: if r.is_dir { EntryKind::Dir } else { EntryKind::File },
        is_dir: r.is_dir,
        size: if r.is_dir { 0 } else { r.size.unwrap_or(0) },
        modified: r.modified,
        created: r.created,
        readonly: false,
        executable: false,
        name,
    }
}

impl DavProvider {
    /// Connect and check access with a `PROPFIND` of the server root. With
    /// `creds` a password sign-in is used (Basic or Digest, whichever the
    /// server asks for); without, requests are anonymous. A 401 gives
    /// [`CxError::AuthRequired`].
    pub async fn connect(ep: &Endpoint, creds: Option<Credentials>) -> Result<DavProvider> {
        if !matches!(ep.scheme, Scheme::Dav | Scheme::Davs) {
            return Err(CxError::InvalidLocation(ep.uri()));
        }
        let creds = creds.and_then(|c| match &c.secret {
            Secret::Password { password } => Some((c.user.clone(), password.clone())),
            _ => None,
        });
        let client = Arc::new(DavClient::new(ep, creds)?);
        let p = DavProvider { client, endpoint: ep.clone() };
        // Any answer but 401 means we can talk to the server: the root may
        // well be 403/404/405 on servers that serve DAV under a sub-path.
        let url = p.client.url("/", true);
        p.client.send(method("PROPFIND"), &url, headers(&[("Depth", "0")]), Some(Bytes::from_static(propfind::PROPS.as_bytes()))).await?;
        Ok(p)
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    fn path<'a>(&self, loc: &'a Location) -> Result<&'a str> {
        match loc {
            Location::Remote { endpoint, path } if matches!(endpoint.scheme, Scheme::Dav | Scheme::Davs) => Ok(path),
            _ => Err(CxError::InvalidLocation(loc.uri())),
        }
    }

    /// PROPFIND with `depth`; the responses are read as they stream in.
    async fn propfind(&self, path: &str, dir: bool, depth: &str, body: &'static str) -> Result<(String, Multistatus<BodyReader>)> {
        let url = self.client.url(path, dir);
        let h = headers(&[("Depth", depth), ("Content-Type", "application/xml; charset=utf-8")]);
        let resp = self.client.send(method("PROPFIND"), &url, h, Some(Bytes::from_static(body.as_bytes()))).await?;
        let status = resp.status();
        if status != StatusCode::MULTI_STATUS {
            if status.is_success() {
                return Err(CxError::Unsupported(format!("{url}: not a WebDAV server (PROPFIND answered {status})")));
            }
            return Err(self.client.status_error(status, &url));
        }
        let body: BodyStream = Box::pin(resp.bytes_stream().map_err(std::io::Error::other));
        Ok((url, Multistatus::new(tokio::io::BufReader::new(StreamReader::new(body)))))
    }

    async fn first(&self, path: &str, dir: bool, body: &'static str) -> Result<Resource> {
        let (url, mut ms) = self.propfind(path, dir, "0", body).await?;
        ms.next().await.map_err(|e| CxError::Io(format!("{url}: {e}")))?.ok_or(CxError::NotFound(url))
    }

    async fn stat_path(&self, path: &str) -> Result<Entry> {
        let r = self.first(path, false, propfind::PROPS).await?;
        let name = path.trim_end_matches('/').rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or(&self.endpoint.host).to_string();
        Ok(to_entry(name, &r))
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        match self.stat_path(path).await {
            Ok(_) => Ok(true),
            Err(CxError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// `None` when the name is taken.
    async fn mkcol(&self, base: &str, name: &str) -> Result<Option<Entry>> {
        let path = cx_core::location::join_posix(base, name);
        if self.exists(&path).await? {
            return Ok(None);
        }
        let url = self.client.url(&path, true);
        let resp = self.client.send(method("MKCOL"), &url, HeaderMap::new(), None).await?;
        match resp.status() {
            s if s.is_success() => self.stat_path(&path).await.map(Some),
            // RFC 4918 § 9.3.1: MKCOL on an existing resource is 405. (Some
            // servers, rclone among them, answer 201 instead, hence the
            // existence check above.)
            StatusCode::METHOD_NOT_ALLOWED => Ok(None),
            s => Err(self.client.status_error(s, &url)),
        }
    }

    fn destination(&self, path: &str, dir: bool) -> String {
        self.client.url(path, dir)
    }
}

#[async_trait]
impl Provider for DavProvider {
    fn scheme(&self) -> &'static str {
        self.endpoint.scheme.as_str()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: false, polling: true, server_copy: true, trash: false, posix: false, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let path = self.path(dir)?;
        let want = cx_core::location::normalize_posix(path);
        let mut total = 0;
        let mut limit = FIRST_BATCH;
        let mut batch = Vec::with_capacity(FIRST_BATCH);
        let mut first = true;
        let mut self_is_file = false;
        let (url, mut ms) = self.propfind(path, true, "1", propfind::PROPS).await?;
        while let Some(r) = ms.next().await.map_err(|e| CxError::Io(format!("{url}: {e}")))? {
            let href = href_path(&r.href);
            let was_first = std::mem::replace(&mut first, false);
            // Skip the folder itself. Servers behind a path-rewriting proxy may
            // report it under another prefix, so the first entry also counts
            // when its path ends with ours.
            if href == want || (was_first && want != "/" && href.ends_with(&want)) {
                self_is_file = !r.is_dir;
                continue;
            }
            let name = href.rsplit('/').next().unwrap_or("").to_string();
            if name.is_empty() {
                continue;
            }
            batch.push(to_entry(name, &r));
            if batch.len() >= limit {
                total += batch.len();
                if sink.send(std::mem::take(&mut batch)).await.is_err() {
                    return Ok(total); // cancelled
                }
                limit = NEXT_BATCH;
            }
        }
        if self_is_file && total == 0 && batch.is_empty() {
            return Err(CxError::InvalidLocation(format!("{} is not a folder", dir.uri())));
        }
        total += batch.len();
        if !batch.is_empty() {
            let _ = sink.send(batch).await;
        }
        Ok(total)
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        self.stat_path(self.path(loc)?).await
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let base = self.path(dir)?;
        if let Some(name) = name {
            validate_name(name)?;
            return self.mkcol(base, name).await?.ok_or_else(|| CxError::AlreadyExists(dir.join(name).uri()));
        }
        for n in 1..10_000 {
            let name = if n == 1 { "New folder".to_string() } else { format!("New folder ({n})") };
            if let Some(e) = self.mkcol(base, &name).await? {
                return Ok(e);
            }
        }
        Err(CxError::AlreadyExists("New folder".into()))
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        let (s, d) = (self.path(src)?, self.path(dst)?);
        let is_dir = self.stat_path(s).await?.is_dir;
        let url = self.client.url(s, is_dir);
        let h = headers(&[("Destination", &self.destination(d, is_dir)), ("Overwrite", "F")]);
        let resp = self.client.send(method("MOVE"), &url, h, None).await?;
        match resp.status() {
            s if s.is_success() => Ok(()),
            StatusCode::PRECONDITION_FAILED => Err(CxError::AlreadyExists(dst.uri())),
            s => Err(self.client.status_error(s, &url)),
        }
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let path = self.path(loc)?;
        if path == "/" {
            return Err(CxError::Unsupported("deleting the server root".into()));
        }
        // DELETE on a collection is recursive (RFC 4918 § 9.6.1). The slash
        // matters on servers that redirect folder URLs.
        let is_dir = self.stat_path(path).await?.is_dir;
        let url = self.client.url(path, is_dir);
        let resp = self.client.send(method("DELETE"), &url, HeaderMap::new(), None).await?;
        match resp.status() {
            s if s.is_success() => Ok(()),
            s => Err(self.client.status_error(s, &url)),
        }
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let url = self.client.url(self.path(loc)?, false);
        let range = format!("bytes={offset}-");
        let h = if offset > 0 { headers(&[("Range", &range)]) } else { HeaderMap::new() };
        let resp = self.client.send(reqwest::Method::GET, &url, h, None).await?;
        let status = resp.status();
        if status == StatusCode::RANGE_NOT_SATISFIABLE {
            return Ok(Box::pin(tokio::io::empty()));
        }
        if !status.is_success() {
            return Err(self.client.status_error(status, &url));
        }
        let mut reader = StreamReader::new(resp.bytes_stream().map_err(std::io::Error::other));
        if offset > 0 && status != StatusCode::PARTIAL_CONTENT {
            // The server ignored the Range header: skip ahead ourselves.
            let skipped = tokio::io::copy(&mut (&mut reader).take(offset), &mut tokio::io::sink()).await.map_err(|e| CxError::io(&url, e))?;
            if skipped < offset {
                return Ok(Box::pin(tokio::io::empty()));
            }
        }
        Ok(Box::pin(reader))
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let path = self.path(loc)?;
        if mode == WriteMode::Append {
            return Err(CxError::Unsupported("appending to a WebDAV file (PUT replaces the whole file)".into()));
        }
        let url = self.client.url(path, false);
        let parent = loc.parent().and_then(|p| p.posix_path().map(str::to_owned)).unwrap_or_else(|| "/".into());
        // Negotiate authentication first: the streamed body can't be replayed.
        self.client.prepare_auth(&self.client.url(&parent, true)).await?;
        let mut h = HeaderMap::new();
        if mode == WriteMode::CreateNew {
            // If-None-Match: * makes the server refuse to replace, but not
            // every server honors it; the explicit check covers those.
            if self.exists(path).await? {
                return Err(CxError::AlreadyExists(loc.uri()));
            }
            h.insert("If-None-Match", HeaderValue::from_static("*"));
        }
        let (rx, done, writer) = io::channel();
        let client = self.client.clone();
        let uri = loc.uri();
        let parent_url = self.client.url(&parent, true);
        tokio::spawn(async move {
            let r = match client.send_once(reqwest::Method::PUT, &url, h, io::body(rx)).await {
                Ok(resp) if resp.status().is_success() => Ok(()),
                Ok(resp) => {
                    let mut status = resp.status();
                    // A missing parent folder is 409 on most servers but 403
                    // on Apache: tell the two apart.
                    if status == StatusCode::FORBIDDEN {
                        let depth = headers(&[("Depth", "0")]);
                        if let Ok(r) = client.send(method("PROPFIND"), &parent_url, depth, None).await {
                            if r.status() == StatusCode::NOT_FOUND {
                                status = StatusCode::CONFLICT;
                            }
                        }
                    }
                    Err(put_error(status, &uri))
                }
                Err(e) => Err(e.into()),
            };
            let _ = done.send(r);
        });
        Ok(Box::pin(writer))
    }

    async fn copy_within(&self, src: &Location, dst: &Location) -> Result<bool> {
        let (s, d) = (self.path(src)?, self.path(dst)?);
        let is_dir = self.stat_path(s).await?.is_dir;
        let url = self.client.url(s, is_dir);
        let h = headers(&[("Destination", &self.destination(d, is_dir)), ("Overwrite", "F"), ("Depth", "infinity")]);
        let resp = self.client.send(method("COPY"), &url, h, None).await?;
        match resp.status() {
            s if s.is_success() => Ok(true),
            StatusCode::PRECONDITION_FAILED => Err(CxError::AlreadyExists(dst.uri())),
            // Not implemented, or the server can't copy there itself.
            StatusCode::NOT_IMPLEMENTED | StatusCode::METHOD_NOT_ALLOWED | StatusCode::BAD_GATEWAY | StatusCode::FORBIDDEN => Ok(false),
            s => Err(self.client.status_error(s, &url)),
        }
    }

    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        // Best effort: most servers treat getlastmodified as protected and
        // answer 207 with a 403 inside, or refuse PROPPATCH entirely. Neither
        // is worth failing a copy over.
        let path = self.path(loc)?;
        let Some(t) = std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_millis(ms.max(0) as u64)) else {
            return Ok(());
        };
        let date = httpdate::fmt_http_date(t);
        let body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><d:propertyupdate xmlns:d="DAV:"><d:set><d:prop><d:getlastmodified>{date}</d:getlastmodified></d:prop></d:set></d:propertyupdate>"#
        );
        let url = self.client.url(path, false);
        let _ = self.client.send(method("PROPPATCH"), &url, headers(&[("Content-Type", "application/xml; charset=utf-8")]), Some(Bytes::from(body))).await;
        Ok(())
    }

    async fn free_space(&self, loc: &Location) -> Result<Option<Space>> {
        let path = self.path(loc)?;
        Ok(match self.first(path, true, propfind::QUOTA).await {
            Ok(Resource { quota_available: Some(free), quota_used, .. }) => Some(Space { free, total: free.saturating_add(quota_used.unwrap_or(0)) }),
            _ => None,
        })
    }
}

fn put_error(status: StatusCode, uri: &str) -> std::io::Error {
    match status {
        StatusCode::PRECONDITION_FAILED => CxError::AlreadyExists(uri.to_string()).into(),
        StatusCode::FORBIDDEN => CxError::PermissionDenied(uri.to_string()).into(),
        StatusCode::NOT_FOUND | StatusCode::CONFLICT => CxError::NotFound(format!("{uri} (parent folder missing)")).into(),
        StatusCode::LENGTH_REQUIRED => CxError::Unsupported(format!("{uri}: the server refuses streamed (chunked) uploads")).into(),
        StatusCode::INSUFFICIENT_STORAGE => CxError::Io(format!("{uri}: server storage is full")).into(),
        s => std::io::Error::other(format!("{uri}: HTTP {s}")),
    }
}
