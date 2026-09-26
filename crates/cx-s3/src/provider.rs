use crate::client::{describe, Request, S3Client};
use crate::sign::Keys;
use crate::upload;
use crate::util::{encode, encode_key, parse_http_date};
use crate::xml;
use base64::Engine;
use cx_core::{
    validate_name, Capabilities, Connector, Credentials, CxError, Endpoint, Entry, EntryKind, Location, Provider, ReadStream, Result, Scheme,
    Secret, WriteMode, WriteStream,
};
use futures_util::{StreamExt, TryStreamExt};
use md5::Digest;
use reqwest::{Method, StatusCode};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::io::StreamReader;

/// The first listing page is small so the UI can paint right away.
const FIRST_PAGE: usize = 100;
const PAGE: usize = 1000;
/// `DeleteObjects` takes at most this many keys.
const DELETE_BATCH: usize = 1000;
/// Server-side copies running at once when moving a folder.
const COPY_CONCURRENCY: usize = 8;
/// `CopyObject` handles at most 5 GiB; bigger objects are copied in parts.
const SINGLE_COPY_MAX: u64 = 5 * 1024 * 1024 * 1024;
const COPY_PART: u64 = 512 * 1024 * 1024;

/// Opens [`S3Provider`]s: `vfs.register(Arc::new(S3Connector))`.
#[derive(Debug, Clone, Copy, Default)]
pub struct S3Connector;

#[async_trait::async_trait]
impl Connector for S3Connector {
    fn scheme(&self) -> Scheme {
        Scheme::S3
    }

    async fn connect(&self, ep: &Endpoint, creds: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(S3Provider::connect(ep, creds).await?))
    }
}

/// An S3-compatible service. `/` lists buckets, `/bucket/a/b` is the key
/// `a/b` in `bucket`; "folders" are key prefixes up to a `/`.
pub struct S3Provider {
    client: Arc<S3Client>,
    endpoint: Endpoint,
    single_copy_max: AtomicU64,
    copy_part: AtomicU64,
}

/// A location split into bucket and key.
enum Path<'a> {
    Root,
    Bucket(&'a str),
    Key(&'a str, &'a str),
}

fn split(path: &str) -> Path<'_> {
    let p = path.trim_matches('/');
    match p.split_once('/') {
        _ if p.is_empty() => Path::Root,
        None => Path::Bucket(p),
        Some((b, k)) => Path::Key(b, k),
    }
}

fn dir_entry(name: String, modified: Option<i64>) -> Entry {
    Entry { hidden: name.starts_with('.'), kind: EntryKind::Dir, is_dir: true, size: 0, modified, created: None, readonly: false, name }
}

fn file_entry(name: String, size: u64, modified: Option<i64>) -> Entry {
    Entry { hidden: name.starts_with('.'), kind: EntryKind::File, is_dir: false, size, modified, created: None, readonly: false, name }
}

fn last_segment(s: &str) -> String {
    s.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

impl S3Provider {
    /// Connect to `ep`. With credentials (access key id as the user, secret
    /// key as the password) the keys are checked with `ListBuckets`: a wrong
    /// key or secret gives [`CxError::AuthRequired`], while `AccessDenied`
    /// is accepted (keys scoped to one bucket may not list buckets).
    ///
    /// Without credentials requests go out unsigned, which works for public
    /// buckets; anything the service refuses then becomes `AuthRequired`, so
    /// the UI asks for keys. If the URI names an access key but no secret
    /// was given, sign-in is required right away.
    pub async fn connect(ep: &Endpoint, creds: Option<Credentials>) -> Result<S3Provider> {
        if ep.scheme != Scheme::S3 {
            return Err(CxError::InvalidLocation(ep.uri()));
        }
        let keys = creds.and_then(|c| match c.secret {
            Secret::Password { password } if !c.user.is_empty() && c.user != "anonymous" => Some(Keys { access: c.user, secret: password }),
            _ => None,
        });
        let client = Arc::new(S3Client::new(ep, keys)?);
        if client.anonymous() && ep.user.is_some() {
            return Err(client.auth_required("enter the secret key for this access key"));
        }
        let p = S3Provider { client, endpoint: ep.clone(), single_copy_max: AtomicU64::new(SINGLE_COPY_MAX), copy_part: AtomicU64::new(COPY_PART) };
        let resp = p.client.send(&Request::new(Method::GET, None, "")).await?;
        let status = resp.status();
        if !status.is_success() && !p.client.anonymous() {
            match p.client.status_error(resp, &ep.uri()).await {
                // Valid keys that may not list buckets.
                CxError::PermissionDenied(_) => {}
                e => return Err(e),
            }
        }
        Ok(p)
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Lower the size above which objects are copied in parts (and the part
    /// size), so tests can exercise multipart copy without 5 GiB objects.
    #[doc(hidden)]
    pub fn set_copy_limits(&self, single_max: u64, part: u64) {
        self.single_copy_max.store(single_max, Ordering::Relaxed);
        self.copy_part.store(part, Ordering::Relaxed);
    }

    fn path<'a>(&self, loc: &'a Location) -> Result<&'a str> {
        match loc {
            Location::Remote { endpoint, path } if endpoint.scheme == Scheme::S3 => Ok(path),
            _ => Err(CxError::InvalidLocation(loc.uri())),
        }
    }

    /// Bucket and key of an object location.
    fn object<'a>(&self, loc: &'a Location) -> Result<(&'a str, &'a str)> {
        match split(self.path(loc)?) {
            Path::Key(b, k) => Ok((b, k)),
            Path::Root | Path::Bucket(_) => Err(CxError::InvalidLocation(format!("{} is not an object (a bucket or the server)", loc.uri()))),
        }
    }

    fn uri(&self, bucket: &str, key: &str) -> String {
        Location::remote(self.endpoint.clone(), format!("/{bucket}/{key}")).uri()
    }

    /// One page of `ListObjectsV2`.
    async fn list_page(&self, bucket: &str, prefix: &str, delimiter: bool, max: usize, token: Option<&str>) -> Result<xml::Page> {
        let mut req = Request::new(Method::GET, Some(bucket), "")
            .query("list-type", "2")
            .query("prefix", prefix)
            .query("max-keys", max.to_string())
            .query("encoding-type", "url");
        if delimiter {
            req = req.query("delimiter", "/");
        }
        if let Some(t) = token {
            req = req.query("continuation-token", t);
        }
        let what = self.uri(bucket, prefix);
        Ok(xml::page(&self.client.xml_ok(&req, &what).await?))
    }

    /// Every key under `prefix` (no delimiter), page by page.
    async fn all_keys(&self, bucket: &str, prefix: &str) -> Result<Vec<xml::Object>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = self.list_page(bucket, prefix, false, PAGE, token.as_deref()).await?;
            out.extend(page.objects);
            match page.next {
                Some(t) => token = Some(t),
                None => return Ok(out),
            }
        }
    }

    /// HEAD an object: `Some(entry)` if it exists.
    async fn head(&self, bucket: &str, key: &str) -> Result<Option<Entry>> {
        let req = Request::new(Method::HEAD, Some(bucket), key);
        let resp = self.client.send(&req).await?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(self.client.map_error(status, None, &self.uri(bucket, key)));
        }
        let h = resp.headers();
        let size = h.get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
        let modified = h.get("last-modified").and_then(|v| v.to_str().ok()).and_then(parse_http_date);
        Ok(Some(file_entry(last_segment(key), size, modified)))
    }

    /// Is there any key under `key/` (a folder, explicit or implied)?
    async fn is_prefix(&self, bucket: &str, key: &str) -> Result<bool> {
        let page = self.list_page(bucket, &format!("{key}/"), false, 1, None).await?;
        Ok(!page.objects.is_empty() || !page.prefixes.is_empty())
    }

    async fn stat_path(&self, path: &str) -> Result<Entry> {
        match split(path) {
            Path::Root => Ok(dir_entry(self.endpoint.host.clone(), None)),
            Path::Bucket(b) => {
                let resp = self.client.send(&Request::new(Method::HEAD, Some(b), "")).await?;
                match resp.status() {
                    s if s.is_success() => Ok(dir_entry(b.to_string(), None)),
                    s => Err(self.client.map_error(s, None, &self.uri(b, ""))),
                }
            }
            Path::Key(b, k) => {
                if let Some(e) = self.head(b, k).await? {
                    return Ok(e);
                }
                if self.is_prefix(b, k).await? {
                    return Ok(dir_entry(last_segment(k), None));
                }
                Err(CxError::NotFound(self.uri(b, k)))
            }
        }
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        match self.stat_path(path).await {
            Ok(_) => Ok(true),
            Err(CxError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn list_buckets(&self, sink: &mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let what = self.endpoint.uri();
        if self.client.anonymous() {
            return Err(self.client.auth_required("sign in to list buckets, or open a public bucket directly (s3://host/bucket)"));
        }
        let root = match self.client.xml_ok(&Request::new(Method::GET, None, ""), &what).await {
            Err(CxError::PermissionDenied(_)) => {
                return Err(CxError::PermissionDenied(format!(
                    "{what}: this access key may not list buckets; open a bucket directly by adding its name to the address ({what}/bucket)"
                )))
            }
            r => r?,
        };
        let entries: Vec<Entry> = xml::buckets(&root).into_iter().map(|b| dir_entry(b.name, b.created)).collect();
        let n = entries.len();
        if n > 0 {
            let _ = sink.send(entries).await;
        }
        Ok(n)
    }

    /// Server-side copy of one object, in parts when it is too big for a
    /// single `CopyObject`.
    async fn copy_object(&self, (sb, sk): (&str, &str), (db, dk): (&str, &str), size: u64) -> Result<()> {
        let source = format!("/{}/{}", encode(sb), encode_key(sk));
        let what = self.uri(db, dk);
        if size <= self.single_copy_max.load(Ordering::Relaxed) {
            let req = Request::new(Method::PUT, Some(db), dk).header("x-amz-copy-source", &source);
            self.client.xml_ok(&req, &what).await?;
            return Ok(());
        }
        let init = self.client.xml_ok(&Request::new(Method::POST, Some(db), dk).query("uploads", ""), &what).await?;
        let id = init.text("UploadId").unwrap_or_default().to_string();
        let part = self.copy_part.load(Ordering::Relaxed).max(1);
        let result = async {
            let mut body = String::from("<CompleteMultipartUpload xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
            let mut start = 0;
            let mut n = 1;
            while start < size {
                let end = (start + part).min(size) - 1;
                let req = Request::new(Method::PUT, Some(db), dk)
                    .query("partNumber", n.to_string())
                    .query("uploadId", &id)
                    .header("x-amz-copy-source", &source)
                    .header("x-amz-copy-source-range", format!("bytes={start}-{end}"));
                let r = self.client.xml_ok(&req, &what).await?;
                let etag = r.text("ETag").unwrap_or_default();
                body.push_str(&format!("<Part><PartNumber>{n}</PartNumber><ETag>{}</ETag></Part>", xml::escape(etag)));
                start = end + 1;
                n += 1;
            }
            body.push_str("</CompleteMultipartUpload>");
            let req = Request::new(Method::POST, Some(db), dk).query("uploadId", &id).header("content-type", "application/xml").body(body);
            self.client.xml_ok(&req, &what).await.map(|_| ())
        }
        .await;
        if result.is_err() {
            let _ = self.client.send(&Request::new(Method::DELETE, Some(db), dk).query("uploadId", &id)).await;
        }
        result
    }

    /// `DeleteObjects` in batches of 1000.
    async fn delete_keys(&self, bucket: &str, keys: &[String]) -> Result<()> {
        for chunk in keys.chunks(DELETE_BATCH) {
            let mut body = String::from("<Delete><Quiet>true</Quiet>");
            for k in chunk {
                body.push_str(&format!("<Object><Key>{}</Key></Object>", xml::escape(k)));
            }
            body.push_str("</Delete>");
            // S3 insists on an integrity header for this call.
            let md5 = base64::engine::general_purpose::STANDARD.encode(md5::Md5::digest(body.as_bytes()));
            let req = Request::new(Method::POST, Some(bucket), "").query("delete", "").header("content-md5", md5).header("content-type", "application/xml").body(body);
            let root = self.client.xml_ok(&req, &self.uri(bucket, "")).await?;
            if let Some((key, err)) = xml::delete_errors(&root).into_iter().find(|(_, e)| e.code != "NoSuchKey") {
                return Err(self.client.map_error(StatusCode::OK, Some(err), &self.uri(bucket, &key)));
            }
        }
        Ok(())
    }

    async fn delete_object(&self, bucket: &str, key: &str) -> Result<()> {
        let what = self.uri(bucket, key);
        match self.client.check(&Request::new(Method::DELETE, Some(bucket), key), &what).await {
            Ok(_) | Err(CxError::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Create the folder marker `prefix/name/`; `None` if the name is taken.
    async fn make_marker(&self, bucket: &str, prefix: &str, name: &str) -> Result<Option<Entry>> {
        let key = format!("{prefix}{name}");
        if self.head(bucket, &key).await?.is_some() || self.is_prefix(bucket, &key).await? {
            return Ok(None);
        }
        let req = Request::new(Method::PUT, Some(bucket), &format!("{key}/"));
        self.client.check(&req, &self.uri(bucket, &key)).await?;
        Ok(Some(dir_entry(name.to_string(), Some(now_ms()))))
    }

    async fn make_bucket(&self, name: &str) -> Result<Option<Entry>> {
        let mut req = Request::new(Method::PUT, Some(name), "");
        let region = crate::endpoint::region_for_host(&self.endpoint.host);
        if region != crate::endpoint::DEFAULT_REGION && region != "auto" {
            let body = format!(
                "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><LocationConstraint>{}</LocationConstraint></CreateBucketConfiguration>",
                xml::escape(&region)
            );
            req = req.body(body);
        }
        match self.client.check(&req, &self.uri(name, "")).await {
            Ok(_) => Ok(Some(dir_entry(name.to_string(), Some(now_ms())))),
            Err(CxError::AlreadyExists(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[async_trait::async_trait]
impl Provider for S3Provider {
    fn scheme(&self) -> &'static str {
        Scheme::S3.as_str()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: false, polling: true, server_copy: true, trash: false, posix: false, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let (bucket, key) = match split(self.path(dir)?) {
            Path::Root => return self.list_buckets(&sink).await,
            Path::Bucket(b) => (b, ""),
            Path::Key(b, k) => (b, k),
        };
        let prefix = if key.is_empty() { String::new() } else { format!("{key}/") };
        let mut total = 0;
        let mut seen_marker = false;
        let mut token: Option<String> = None;
        let mut max = FIRST_PAGE;
        loop {
            let page = self.list_page(bucket, &prefix, true, max, token.as_deref()).await?;
            let mut batch = Vec::with_capacity(page.prefixes.len() + page.objects.len());
            for p in &page.prefixes {
                let name = p.strip_prefix(&prefix).unwrap_or(p).trim_end_matches('/');
                // A "//" in a key makes an empty segment we can't address.
                if !name.is_empty() {
                    batch.push(dir_entry(name.to_string(), None));
                }
            }
            for o in page.objects {
                let Some(name) = o.key.strip_prefix(&prefix) else { continue };
                if name.is_empty() {
                    seen_marker = true; // the folder's own marker object
                    continue;
                }
                batch.push(file_entry(name.to_string(), o.size, o.modified));
            }
            if !batch.is_empty() {
                total += batch.len();
                if sink.send(batch).await.is_err() {
                    return Ok(total); // cancelled
                }
            }
            match page.next {
                Some(t) => token = Some(t),
                None => break,
            }
            max = PAGE;
        }
        // S3 answers an empty page for any prefix, so tell a missing folder
        // (or a file) apart from an empty one.
        if total == 0 && !seen_marker && !key.is_empty() {
            if self.head(bucket, key).await?.is_some() {
                return Err(CxError::InvalidLocation(format!("{} is not a folder", dir.uri())));
            }
            return Err(CxError::NotFound(dir.uri()));
        }
        Ok(total)
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        self.stat_path(self.path(loc)?).await
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let path = self.path(dir)?;
        let make = |n: String| async move {
            match split(path) {
                Path::Root => self.make_bucket(&n).await,
                Path::Bucket(b) => self.make_marker(b, "", &n).await,
                Path::Key(b, k) => self.make_marker(b, &format!("{k}/"), &n).await,
            }
        };
        if let Some(name) = name {
            validate_name(name)?;
            return make(name.to_string()).await?.ok_or_else(|| CxError::AlreadyExists(dir.join(name).uri()));
        }
        for n in 1..10_000 {
            let name = if n == 1 { "New folder".to_string() } else { format!("New folder ({n})") };
            if let Some(e) = make(name).await? {
                return Ok(e);
            }
        }
        Err(CxError::AlreadyExists("New folder".into()))
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        let ((sb, sk), (db, dk)) = (self.object(src)?, self.object(dst)?);
        if (sb, sk) == (db, dk) {
            return Ok(());
        }
        let entry = self.stat_path(self.path(src)?).await?;
        if self.exists(self.path(dst)?).await? {
            return Err(CxError::AlreadyExists(dst.uri()));
        }
        if !entry.is_dir {
            self.copy_object((sb, sk), (db, dk), entry.size).await?;
            return self.delete_object(sb, sk).await;
        }
        let (sp, dp) = (format!("{sk}/"), format!("{dk}/"));
        if sb == db && dp.starts_with(&sp) {
            return Err(CxError::InvalidLocation(format!("can't move {} into itself", src.uri())));
        }
        // Copy everything first and delete only once all copies landed, so a
        // failure never loses data (at worst both copies exist).
        let objects = self.all_keys(sb, &sp).await?;
        let jobs: Vec<(String, String, u64)> = objects.iter().map(|o| (o.key.clone(), format!("{dp}{}", &o.key[sp.len()..]), o.size)).collect();
        futures_util::stream::iter(jobs)
            .map(|(from, to, size)| async move { self.copy_object((sb, &from), (db, &to), size).await })
        .buffer_unordered(COPY_CONCURRENCY)
        .try_collect::<Vec<()>>()
        .await?;
        let keys: Vec<String> = objects.into_iter().map(|o| o.key).collect();
        self.delete_keys(sb, &keys).await
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let (bucket, key) = match split(self.path(loc)?) {
            Path::Root => return Err(CxError::Unsupported("deleting the server".into())),
            Path::Bucket(b) => {
                // Only an empty bucket: deleting a bucket is not something to
                // do recursively by accident.
                return match self.client.check(&Request::new(Method::DELETE, Some(b), ""), &loc.uri()).await {
                    Ok(_) => Ok(()),
                    Err(CxError::Io(m)) if m.contains("BucketNotEmpty") => {
                        Err(CxError::Unsupported(format!("{}: the bucket is not empty; delete its contents first", loc.uri())))
                    }
                    Err(e) => Err(e),
                };
            }
            Path::Key(b, k) => (b, k),
        };
        let file = self.head(bucket, key).await?.is_some();
        let under: Vec<String> = self.all_keys(bucket, &format!("{key}/")).await?.into_iter().map(|o| o.key).collect();
        if !file && under.is_empty() {
            return Err(CxError::NotFound(loc.uri()));
        }
        if file {
            self.delete_object(bucket, key).await?;
        }
        self.delete_keys(bucket, &under).await
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let (bucket, key) = self.object(loc)?;
        let mut req = Request::new(Method::GET, Some(bucket), key);
        if offset > 0 {
            req = req.header("range", format!("bytes={offset}-"));
        }
        let resp = self.client.send(&req).await?;
        let status = resp.status();
        if status == StatusCode::RANGE_NOT_SATISFIABLE {
            return Ok(Box::pin(tokio::io::empty()));
        }
        if !status.is_success() {
            return Err(self.client.status_error(resp, &loc.uri()).await);
        }
        if offset > 0 && status != StatusCode::PARTIAL_CONTENT {
            return Err(CxError::Io(format!("{}: the server ignored the byte range", loc.uri())));
        }
        let uri = loc.uri();
        let body = resp.bytes_stream().map_err(move |e| std::io::Error::other(format!("{uri}: {}", describe(&e))));
        Ok(Box::pin(StreamReader::new(body)))
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let (bucket, key) = self.object(loc)?;
        match mode {
            WriteMode::Append => return Err(CxError::Unsupported("appending to an S3 object (objects are written whole)".into())),
            WriteMode::CreateNew if self.exists(self.path(loc)?).await? => return Err(CxError::AlreadyExists(loc.uri())),
            _ => {}
        }
        let target = upload::Target { bucket: bucket.to_string(), key: key.to_string(), create_new: mode == WriteMode::CreateNew, uri: loc.uri() };
        Ok(Box::pin(upload::start(self.client.clone(), target)))
    }

    async fn copy_within(&self, src: &Location, dst: &Location) -> Result<bool> {
        let ((sb, sk), (db, dk)) = (self.object(src)?, self.object(dst)?);
        let Some(entry) = self.head(sb, sk).await? else {
            // A folder (or nothing): let the caller walk it.
            return Ok(false);
        };
        if self.exists(self.path(dst)?).await? {
            return Err(CxError::AlreadyExists(dst.uri()));
        }
        self.copy_object((sb, sk), (db, dk), entry.size).await?;
        Ok(true)
    }
}
