//! OneDrive (personal and for Business) through Microsoft Graph, `me/drive`.
//!
//! Graph addresses items by path (`/me/drive/root:/a/b.txt:`), so locations
//! map straight onto it. Listing pages follow `@odata.nextLink`; the first
//! page is split so the UI gets a small first batch.
//!
//! create_dir: POST to `children` with `conflictBehavior: fail`. move_to:
//! PATCH `parentReference` and `name`, after checking the target is free
//! (never overwrite). remove: DELETE (to the OneDrive recycle bin, which
//! Graph can't restore from, so it is not offered as `trash`). open_read:
//! `/content` answers with a redirect to a pre-authenticated download URL,
//! which is followed by hand without the bearer token, with `Range`.
//! copy_within: `copy` is asynchronous: it answers with a monitor URL that
//! is polled until the copy is done. set_modified: `fileSystemInfo`.
//! free_space: the drive's `quota`.
//!
//! ## Uploads
//!
//! Files up to 4 MiB go up in one PUT. Bigger ones need an upload session,
//! and every chunk PUT must state the file's *total* size, which a stream of
//! unknown length only knows at the end. So the bytes are spooled to an
//! anonymous temporary file (never to memory) and sent in chunks of a
//! multiple of 320 KiB once the size is known. The temp file vanishes when
//! the upload ends or is dropped.

use crate::api::Api;
use crate::common::{ranged_body, remote_path, split_parent};
use crate::upload::{self, Uploader};
use crate::util::{dir_entry, encode_path, file_entry, format_rfc3339, new_folder_names, parse_rfc3339, query};
use bytes::Bytes;
use cx_core::{validate_name, Capabilities, CxError, Endpoint, Entry, Location, Provider, ReadStream, Result, Space, WriteMode, WriteStream};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;

const SELECT: &str = "id,name,size,folder,file,package,remoteItem,fileSystemInfo,lastModifiedDateTime,createdDateTime,parentReference";
const FIRST_BATCH: usize = 100;
/// Largest file sent with a single PUT.
pub(crate) const SIMPLE_MAX: usize = 4 * 1024 * 1024;
/// Upload session chunk: Graph wants multiples of 320 KiB.
const SESSION_UNIT: usize = 320 * 1024;
pub(crate) const SESSION_CHUNK: usize = 32 * SESSION_UNIT; // 10 MiB
/// How long a server-side copy may take before we give up waiting.
const COPY_DEADLINE: Duration = Duration::from_secs(60 * 60);

/// A OneDrive account.
pub struct OneDriveProvider {
    api: Arc<Api>,
    endpoint: Endpoint,
    simple_max: AtomicUsize,
    session_chunk: AtomicUsize,
}

fn entry(v: &Value) -> Option<Entry> {
    let name = v.get("name")?.as_str()?.to_string();
    let time = |a: &str, b: &str| v.pointer(a).or_else(|| v.pointer(b)).and_then(Value::as_str).and_then(parse_rfc3339);
    let modified = time("/fileSystemInfo/lastModifiedDateTime", "/lastModifiedDateTime");
    let created = time("/fileSystemInfo/createdDateTime", "/createdDateTime");
    if v.get("folder").is_some() || v.pointer("/remoteItem/folder").is_some() {
        let mut e = dir_entry(name, modified, false);
        e.created = created;
        return Some(e);
    }
    Some(file_entry(name, v.get("size").and_then(Value::as_u64).unwrap_or(0), modified, created))
}

impl OneDriveProvider {
    pub(crate) fn new(api: Arc<Api>, endpoint: Endpoint) -> OneDriveProvider {
        OneDriveProvider { api, endpoint, simple_max: AtomicUsize::new(SIMPLE_MAX), session_chunk: AtomicUsize::new(SESSION_CHUNK) }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Lower the single-PUT limit and the session chunk (rounded to 320
    /// KiB) so tests can exercise upload sessions with small files.
    #[doc(hidden)]
    pub fn set_upload_limits(&self, simple_max: usize, session_chunk: usize) {
        self.simple_max.store(simple_max.max(1), Ordering::Relaxed);
        self.session_chunk.store(session_chunk.div_ceil(SESSION_UNIT).max(1) * SESSION_UNIT, Ordering::Relaxed);
    }

    /// `…/me/drive/root` or `…/me/drive/root:/a/b:`.
    fn item_url(&self, path: &str) -> String {
        item_url(&self.api, path)
    }

    fn children_url(&self, path: &str) -> String {
        let p = path.trim_end_matches('/');
        if p.is_empty() {
            format!("{}/me/drive/root/children", self.api.cfg.urls.api)
        } else {
            format!("{}/children", self.item_url(p))
        }
    }

    /// The item's JSON (`None` if it doesn't exist).
    async fn get(&self, path: &str, uri: &str) -> Result<Option<Value>> {
        let url = format!("{}?{}", self.item_url(path), query(&[("$select", SELECT)]));
        match self.api.json(uri, |_| self.api.http.get(&url)).await {
            Ok(v) => Ok(Some(v)),
            Err(CxError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn item(&self, loc: &Location) -> Result<Value> {
        self.get(remote_path(loc)?, &loc.uri()).await?.ok_or_else(|| CxError::NotFound(loc.uri()))
    }

    /// The id (and drive id) of the folder at `path`.
    async fn folder_ref(&self, path: &str) -> Result<(String, Option<String>)> {
        let uri = Location::remote(self.endpoint.clone(), path).uri();
        let v = self.get(path, &uri).await?.ok_or_else(|| CxError::NotFound(uri.clone()))?;
        if v.get("folder").is_none() && v.get("root").is_none() && path != "/" {
            return Err(CxError::InvalidLocation(format!("{uri} is not a folder")));
        }
        let id = v.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        Ok((id, v.pointer("/parentReference/driveId").and_then(Value::as_str).map(str::to_owned)))
    }

    /// Wait for an asynchronous copy to finish.
    async fn wait_copy(&self, monitor: &str, uri: &str) -> Result<()> {
        let deadline = Instant::now() + COPY_DEADLINE;
        let mut pause = Duration::from_millis(100);
        loop {
            let resp = self.api.send_plain(uri, || self.api.http.get(monitor)).await?;
            let status = resp.status();
            // Done: the monitor redirects to the new item.
            if status.is_redirection() {
                return Ok(());
            }
            if !status.is_success() {
                return Err(self.api.error(resp, uri).await);
            }
            let v = crate::api::read_json(resp).await.map_err(|e| CxError::Io(format!("{uri}: {e}")))?;
            match v.get("status").and_then(Value::as_str).unwrap_or_default() {
                "completed" => return Ok(()),
                "failed" => {
                    let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("the copy failed");
                    let code = v.pointer("/error/code").and_then(Value::as_str).unwrap_or_default();
                    if code == "nameAlreadyExists" {
                        return Err(CxError::AlreadyExists(uri.to_string()));
                    }
                    return Err(CxError::Io(format!("{uri}: {msg}")));
                }
                _ if Instant::now() > deadline => return Err(CxError::Io(format!("{uri}: the copy is taking too long"))),
                _ => {}
            }
            tokio::time::sleep(pause).await;
            pause = (pause * 2).min(Duration::from_secs(2));
        }
    }
}

fn item_url(api: &Api, path: &str) -> String {
    let p = path.trim_end_matches('/');
    if p.is_empty() {
        format!("{}/me/drive/root", api.cfg.urls.api)
    } else {
        format!("{}/me/drive/root:{}:", api.cfg.urls.api, encode_path(p))
    }
}

#[async_trait::async_trait]
impl Provider for OneDriveProvider {
    fn scheme(&self) -> &'static str {
        crate::ONEDRIVE_SCHEME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: false, polling: true, server_copy: true, trash: false, posix: false, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let path = remote_path(dir)?;
        let mut url = format!("{}?{}", self.children_url(path), query(&[("$select", SELECT)]));
        let mut sent = 0;
        let mut first = true;
        loop {
            let v = match self.api.json(&dir.uri(), |_| self.api.http.get(&url)).await {
                // Listing a file's children: Graph says "not found" or "bad request".
                Err(CxError::NotFound(_)) if path != "/" => match self.get(path, &dir.uri()).await? {
                    Some(_) => return Err(CxError::InvalidLocation(format!("{} is not a folder", dir.uri()))),
                    None => return Err(CxError::NotFound(dir.uri())),
                },
                r => r?,
            };
            let mut entries: Vec<Entry> = v.get("value").and_then(Value::as_array).map(|a| a.iter().filter_map(entry).collect()).unwrap_or_default();
            if first && entries.len() > FIRST_BATCH {
                let rest = entries.split_off(FIRST_BATCH);
                sent += entries.len();
                if sink.send(entries).await.is_err() {
                    return Ok(sent);
                }
                entries = rest;
            }
            first = false;
            if !entries.is_empty() {
                sent += entries.len();
                if sink.send(entries).await.is_err() {
                    return Ok(sent); // cancelled
                }
            }
            match v.get("@odata.nextLink").and_then(Value::as_str) {
                Some(next) => url = next.to_string(),
                None => return Ok(sent),
            }
        }
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        let path = remote_path(loc)?;
        if path == "/" {
            return Ok(dir_entry(crate::oauth::endpoint_account(&self.endpoint), None, false));
        }
        let v = self.item(loc).await?;
        entry(&v).ok_or_else(|| CxError::Io(format!("{}: unexpected answer", loc.uri())))
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let url = self.children_url(remote_path(dir)?);
        let make = |n: String| {
            let url = url.clone();
            async move {
                let body = json!({ "name": n, "folder": {}, "@microsoft.graph.conflictBehavior": "fail" }).to_string();
                let what = dir.join(&n).uri();
                match self.api.json(&what, |_| self.api.http.post(&url).header("content-type", "application/json").body(body.clone())).await {
                    Ok(v) => Ok(Some(entry(&v).unwrap_or_else(|| dir_entry(n, None, false)))),
                    Err(CxError::AlreadyExists(_)) => Ok(None),
                    Err(e) => Err(e),
                }
            }
        };
        if let Some(n) = name {
            validate_name(n)?;
            return make(n.to_string()).await?.ok_or_else(|| CxError::AlreadyExists(dir.join(n).uri()));
        }
        for n in new_folder_names() {
            if let Some(e) = make(n).await? {
                return Ok(e);
            }
        }
        Err(CxError::AlreadyExists("New folder".into()))
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        let (sp, dp) = (remote_path(src)?, remote_path(dst)?);
        if sp == dp {
            return Ok(());
        }
        let (dst_dir, dst_name) = split_parent(dp).ok_or_else(|| CxError::InvalidLocation(dst.uri()))?;
        validate_name(dst_name)?;
        let item = self.item(src).await?;
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let (parent, _) = self.folder_ref(dst_dir).await?;
        if let Some(existing) = self.get(dp, &dst.uri()).await? {
            // A case-only rename finds the item itself.
            if existing.get("id").and_then(Value::as_str) != Some(id.as_str()) {
                return Err(CxError::AlreadyExists(dst.uri()));
            }
        }
        let url = format!("{}/me/drive/items/{}", self.api.cfg.urls.api, crate::util::encode(&id));
        let body = json!({ "parentReference": { "id": parent }, "name": dst_name }).to_string();
        match self.api.json(&src.uri(), |_| self.api.http.patch(&url).header("content-type", "application/json").body(body.clone())).await {
            Err(CxError::AlreadyExists(_)) => Err(CxError::AlreadyExists(dst.uri())),
            r => r.map(|_| ()),
        }
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let path = remote_path(loc)?;
        if path == "/" {
            return Err(CxError::Unsupported("deleting the whole OneDrive".into()));
        }
        let url = self.item_url(path);
        self.api.check(&loc.uri(), |_| self.api.http.delete(&url)).await.map(|_| ())
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let url = format!("{}/content", self.item_url(remote_path(loc)?));
        let range = (offset > 0).then(|| format!("bytes={offset}-"));
        let with_range = |rb: reqwest::RequestBuilder| match &range {
            Some(r) => rb.header("range", r.clone()),
            None => rb,
        };
        let resp = self.api.send(&loc.uri(), |_| with_range(self.api.http.get(&url))).await?;
        if resp.status().is_redirection() {
            // The pre-authenticated download URL must not get our token.
            let target = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .ok_or_else(|| CxError::Io(format!("{}: redirect without a location", loc.uri())))?;
            let resp = self.api.send_plain(&loc.uri(), || with_range(self.api.http.get(&target))).await?;
            return ranged_body(&self.api, resp, offset, loc.uri()).await;
        }
        ranged_body(&self.api, resp, offset, loc.uri()).await
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let path = remote_path(loc)?;
        let (_, name) = split_parent(path).ok_or_else(|| CxError::InvalidLocation(loc.uri()))?;
        validate_name(name)?;
        let conflict = match mode {
            WriteMode::Append => return Err(CxError::Unsupported("appending to a OneDrive file (files are uploaded whole)".into())),
            WriteMode::CreateNew if self.get(path, &loc.uri()).await?.is_some() => return Err(CxError::AlreadyExists(loc.uri())),
            WriteMode::CreateNew => "fail",
            WriteMode::Truncate => "replace",
        };
        let up = OneDriveUpload {
            api: self.api.clone(),
            path: path.to_string(),
            conflict,
            spool: None,
            session: None,
            chunk: self.session_chunk.load(Ordering::Relaxed),
            uri: loc.uri(),
        };
        Ok(Box::pin(upload::start(self.simple_max.load(Ordering::Relaxed), up)))
    }

    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        let path = remote_path(loc)?;
        if path == "/" {
            return Ok(());
        }
        let url = self.item_url(path);
        let body = json!({ "fileSystemInfo": { "lastModifiedDateTime": format_rfc3339(ms) } }).to_string();
        self.api.json(&loc.uri(), |_| self.api.http.patch(&url).header("content-type", "application/json").body(body.clone())).await.map(|_| ())
    }

    async fn copy_within(&self, src: &Location, dst: &Location) -> Result<bool> {
        let dp = remote_path(dst)?;
        let (dst_dir, dst_name) = split_parent(dp).ok_or_else(|| CxError::InvalidLocation(dst.uri()))?;
        validate_name(dst_name)?;
        let item = self.item(src).await?;
        let id = item.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let (parent, drive) = self.folder_ref(dst_dir).await?;
        if self.get(dp, &dst.uri()).await?.is_some() {
            return Err(CxError::AlreadyExists(dst.uri()));
        }
        let mut parent_ref = json!({ "id": parent });
        if let Some(d) = drive {
            parent_ref["driveId"] = json!(d);
        }
        let url = format!("{}/me/drive/items/{}/copy", self.api.cfg.urls.api, crate::util::encode(&id));
        let body = json!({ "parentReference": parent_ref, "name": dst_name }).to_string();
        let resp = self.api.check(&dst.uri(), |_| self.api.http.post(&url).header("content-type", "application/json").body(body.clone())).await?;
        let monitor = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_owned);
        if let Some(m) = monitor {
            self.wait_copy(&m, &dst.uri()).await?;
        }
        Ok(true)
    }

    async fn free_space(&self, _loc: &Location) -> Result<Option<Space>> {
        let url = format!("{}/me/drive?{}", self.api.cfg.urls.api, query(&[("$select", "quota")]));
        let v = self.api.json("OneDrive quota", |_| self.api.http.get(&url)).await?;
        let num = |k: &str| v.pointer(&format!("/quota/{k}")).and_then(Value::as_u64);
        match (num("total"), num("remaining")) {
            (Some(total), Some(free)) => Ok(Some(Space { free, total })),
            _ => Ok(None),
        }
    }
}

/// One PUT for small files; a spooled upload session for big ones.
struct OneDriveUpload {
    api: Arc<Api>,
    path: String,
    /// `fail` (CreateNew) or `replace` (Truncate).
    conflict: &'static str,
    spool: Option<tokio::fs::File>,
    session: Option<String>,
    chunk: usize,
    uri: String,
}

impl OneDriveUpload {
    fn conflict_error(&self, e: CxError) -> CxError {
        match e {
            CxError::AlreadyExists(_) => CxError::AlreadyExists(self.uri.clone()),
            e => e,
        }
    }

    async fn spool(&mut self, data: &[u8]) -> Result<()> {
        if self.spool.is_none() {
            let f = tokio::task::spawn_blocking(tempfile::tempfile).await.map_err(|e| CxError::io("upload spool", e))?.map_err(|e| CxError::io("upload spool", e))?;
            self.spool = Some(tokio::fs::File::from_std(f));
        }
        let f = self.spool.as_mut().expect("just created");
        f.write_all(data).await.map_err(|e| CxError::io("upload spool", e))
    }

    async fn send_session(&mut self, total: u64) -> Result<()> {
        let url = format!("{}/createUploadSession", item_url(&self.api, &self.path));
        let body = json!({ "item": { "@microsoft.graph.conflictBehavior": self.conflict } }).to_string();
        let v = self
            .api
            .json(&self.uri, |_| self.api.http.post(&url).header("content-type", "application/json").body(body.clone()))
            .await
            .map_err(|e| self.conflict_error(e))?;
        let upload_url = v.get("uploadUrl").and_then(Value::as_str).ok_or_else(|| CxError::Io(format!("{}: no upload URL", self.uri)))?.to_string();
        self.session = Some(upload_url.clone());
        let mut file = self.spool.take().ok_or_else(|| CxError::Io(format!("{}: nothing to upload", self.uri)))?;
        file.seek(std::io::SeekFrom::Start(0)).await.map_err(|e| CxError::io("upload spool", e))?;
        let mut offset = 0u64;
        while offset < total {
            let n = (self.chunk as u64).min(total - offset) as usize;
            let mut buf = vec![0u8; n];
            file.read_exact(&mut buf).await.map_err(|e| CxError::io("upload spool", e))?;
            let data = Bytes::from(buf);
            let range = format!("bytes {offset}-{}/{total}", offset + n as u64 - 1);
            // The upload URL is pre-authorized: no bearer token.
            let resp = self
                .api
                .send_plain(&self.uri, || self.api.http.put(&upload_url).header("content-range", range.clone()).header("content-length", n).body(data.clone()))
                .await?;
            let status = resp.status();
            if !status.is_success() {
                let e = self.api.error(resp, &self.uri).await;
                return Err(self.conflict_error(e));
            }
            offset += n as u64;
            if offset < total && status != StatusCode::ACCEPTED {
                return Err(CxError::Io(format!("{}: the upload ended early (HTTP {status})", self.uri)));
            }
        }
        self.session = None;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Uploader for OneDriveUpload {
    async fn whole(&mut self, data: Bytes) -> Result<()> {
        let url = format!("{}/content?{}", item_url(&self.api, &self.path), query(&[("@microsoft.graph.conflictBehavior", self.conflict)]));
        self.api
            .check(&self.uri, |_| self.api.http.put(&url).header("content-type", "application/octet-stream").body(data.clone()))
            .await
            .map(|_| ())
            .map_err(|e| self.conflict_error(e))
    }

    async fn part(&mut self, data: Bytes, _offset: u64) -> Result<()> {
        self.spool(&data).await
    }

    async fn last(&mut self, data: Bytes, _offset: u64, total: u64) -> Result<()> {
        self.spool(&data).await?;
        self.send_session(total).await
    }

    async fn abort(&mut self) {
        if let Some(url) = self.session.take() {
            let _ = self.api.send_plain(&self.uri, || self.api.http.delete(&url)).await;
        }
    }
}
