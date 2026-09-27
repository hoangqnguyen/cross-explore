//! Dropbox (API v2).
//!
//! Dropbox is path-based, so locations map straight onto API paths (`/` is
//! `""` to the API). Calls are JSON RPCs on the API host, except content
//! (download, upload) which goes to the content host with its arguments in
//! a `Dropbox-API-Arg` header. That header must be ASCII, so non-ASCII
//! characters in paths are sent as `\uXXXX` escapes.
//!
//! list: `list_folder` + `list_folder/continue` (the first page is split so
//! the UI gets a small first batch). stat: `get_metadata`. create_dir:
//! `create_folder_v2`. move_to: `move_v2` with `autorename: false`, so an
//! existing target is a conflict (`AlreadyExists`), never a renamed copy.
//! remove: `delete_v2`. open_read: `download` with `Range`. open_write: one
//! `upload` when the file fits in one chunk, an upload session
//! (`start`/`append_v2`/`finish`) otherwise, so memory stays bounded.
//! copy_within: `copy_v2` (folders too). The modification time is the
//! server's (read-only), so set_modified does nothing. free_space:
//! `get_space_usage`. No push watch; callers poll.

use crate::api::Api;
use crate::common::{ranged_body, remote_path, split_parent};
use crate::upload::{self, Uploader};
use crate::util::{dir_entry, file_entry, new_folder_names, parse_rfc3339};
use bytes::Bytes;
use cx_core::{validate_name, Capabilities, CxError, Endpoint, Entry, Location, Provider, ReadStream, Result, Space, WriteMode, WriteStream};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

const FIRST_BATCH: usize = 100;
/// Upload chunk. Files up to this size go up in one request; bigger ones in
/// an upload session, chunk by chunk.
pub(crate) const CHUNK: usize = 8 * 1024 * 1024;

/// A Dropbox account.
pub struct DropboxProvider {
    api: Arc<Api>,
    endpoint: Endpoint,
    chunk: AtomicUsize,
}

/// API path for a location path: `/` is `""`.
fn api_path(path: &str) -> String {
    let p = path.trim_end_matches('/');
    p.to_string()
}

/// JSON for `Dropbox-API-Arg`: HTTP headers must be ASCII, so DEL and
/// everything beyond ASCII is `\u`-escaped (as Dropbox documents).
fn arg_header(v: &Value) -> String {
    let s = v.to_string();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii() && c != '\u{7f}' {
            out.push(c);
        } else {
            let mut buf = [0u16; 2];
            for unit in c.encode_utf16(&mut buf) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

fn entry(v: &Value) -> Option<Entry> {
    let name = v.get("name")?.as_str()?.to_string();
    let time = |k: &str| v.get(k).and_then(Value::as_str).and_then(parse_rfc3339);
    match v.get(".tag")?.as_str()? {
        "folder" => Some(dir_entry(name, None, false)),
        "file" => Some(file_entry(name, v.get("size").and_then(Value::as_u64).unwrap_or(0), time("server_modified"), None)),
        _ => None, // "deleted"
    }
}

impl DropboxProvider {
    pub(crate) fn new(api: Arc<Api>, endpoint: Endpoint) -> DropboxProvider {
        DropboxProvider { api, endpoint, chunk: AtomicUsize::new(CHUNK) }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Use a smaller upload chunk (tests).
    #[doc(hidden)]
    pub fn set_upload_chunk(&self, bytes: usize) {
        self.chunk.store(bytes.max(1), Ordering::Relaxed);
    }

    async fn rpc(&self, route: &str, body: Value, what: &str) -> Result<Value> {
        let url = format!("{}/{route}", self.api.cfg.urls.api);
        let body = body.to_string();
        self.api.json(what, |_| self.api.http.post(&url).header("content-type", "application/json").body(body.clone())).await
    }

    async fn metadata(&self, loc: &Location) -> Result<Entry> {
        let path = remote_path(loc)?;
        if path == "/" {
            return Ok(dir_entry(crate::oauth::endpoint_account(&self.endpoint), None, false));
        }
        let v = self.rpc("files/get_metadata", json!({ "path": api_path(path) }), &loc.uri()).await?;
        entry(&v).ok_or_else(|| CxError::NotFound(loc.uri()))
    }

    async fn exists(&self, loc: &Location) -> Result<bool> {
        match self.metadata(loc).await {
            Ok(_) => Ok(true),
            Err(CxError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[async_trait::async_trait]
impl Provider for DropboxProvider {
    fn scheme(&self) -> &'static str {
        crate::DROPBOX_SCHEME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: false, polling: true, server_copy: true, trash: false, posix: false, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let path = api_path(remote_path(dir)?);
        let mut v = self.rpc("files/list_folder", json!({ "path": path, "include_deleted": false }), &dir.uri()).await?;
        let mut sent = 0;
        let mut first = true;
        loop {
            let mut entries: Vec<Entry> = v.get("entries").and_then(Value::as_array).map(|a| a.iter().filter_map(entry).collect()).unwrap_or_default();
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
            if !v.get("has_more").and_then(Value::as_bool).unwrap_or(false) {
                return Ok(sent);
            }
            let cursor = v.get("cursor").and_then(Value::as_str).unwrap_or_default().to_string();
            v = self.rpc("files/list_folder/continue", json!({ "cursor": cursor }), &dir.uri()).await?;
        }
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        self.metadata(loc).await
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let make = |n: String| async move {
            let loc = dir.join(&n);
            let path = api_path(remote_path(&loc)?);
            match self.rpc("files/create_folder_v2", json!({ "path": path, "autorename": false }), &loc.uri()).await {
                Ok(v) => Ok(Some(v.get("metadata").and_then(entry).unwrap_or_else(|| dir_entry(n, None, false)))),
                Err(CxError::AlreadyExists(_)) => Ok(None),
                Err(e) => Err(e),
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
        let (from, to) = (api_path(remote_path(src)?), api_path(remote_path(dst)?));
        if from == to {
            return Ok(());
        }
        if let Some((_, name)) = split_parent(&to) {
            validate_name(name)?;
        }
        let body = json!({ "from_path": from, "to_path": to, "autorename": false, "allow_ownership_transfer": false });
        match self.rpc("files/move_v2", body, &src.uri()).await {
            // "to/conflict/…": never overwrite.
            Err(CxError::AlreadyExists(_)) => Err(CxError::AlreadyExists(dst.uri())),
            r => r.map(|_| ()),
        }
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let path = api_path(remote_path(loc)?);
        if path.is_empty() {
            return Err(CxError::Unsupported("deleting the whole Dropbox".into()));
        }
        self.rpc("files/delete_v2", json!({ "path": path }), &loc.uri()).await.map(|_| ())
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let arg = arg_header(&json!({ "path": api_path(remote_path(loc)?) }));
        let url = format!("{}/files/download", self.api.cfg.urls.content);
        let resp = self
            .api
            .send(&loc.uri(), |_| {
                let rb = self.api.http.post(&url).header("dropbox-api-arg", arg.clone());
                if offset > 0 { rb.header("range", format!("bytes={offset}-")) } else { rb }
            })
            .await?;
        ranged_body(&self.api, resp, offset, loc.uri()).await
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let path = api_path(remote_path(loc)?);
        let (_, name) = split_parent(&path).ok_or_else(|| CxError::InvalidLocation(loc.uri()))?;
        validate_name(name)?;
        let mode = match mode {
            WriteMode::Append => return Err(CxError::Unsupported("appending to a Dropbox file (files are uploaded whole)".into())),
            // Fail now rather than after uploading the whole file.
            WriteMode::CreateNew if self.exists(loc).await? => return Err(CxError::AlreadyExists(loc.uri())),
            WriteMode::CreateNew => "add",
            WriteMode::Truncate => "overwrite",
        };
        let up = DropboxUpload { api: self.api.clone(), path, mode, session: None, uri: loc.uri() };
        Ok(Box::pin(upload::start(self.chunk.load(Ordering::Relaxed), up)))
    }

    async fn copy_within(&self, src: &Location, dst: &Location) -> Result<bool> {
        let (from, to) = (api_path(remote_path(src)?), api_path(remote_path(dst)?));
        let body = json!({ "from_path": from, "to_path": to, "autorename": false });
        match self.rpc("files/copy_v2", body, &src.uri()).await {
            Ok(_) => Ok(true),
            Err(CxError::AlreadyExists(_)) => Err(CxError::AlreadyExists(dst.uri())),
            Err(e) => Err(e),
        }
    }

    async fn free_space(&self, _loc: &Location) -> Result<Option<Space>> {
        let url = format!("{}/users/get_space_usage", self.api.cfg.urls.api);
        // Routes without arguments take a literal `null` body.
        let v = self.api.json("Dropbox space usage", |_| self.api.http.post(&url).header("content-type", "application/json").body("null")).await?;
        let used = v.get("used").and_then(Value::as_u64).unwrap_or(0);
        let Some(total) = v.pointer("/allocation/allocated").and_then(Value::as_u64) else { return Ok(None) };
        Ok(Some(Space { free: total.saturating_sub(used), total }))
    }
}

/// A single `upload`, or an upload session for files bigger than a chunk.
struct DropboxUpload {
    api: Arc<Api>,
    path: String,
    mode: &'static str,
    session: Option<String>,
    uri: String,
}

impl DropboxUpload {
    async fn content(&self, route: &str, arg: Value, data: Bytes) -> Result<Value> {
        let url = format!("{}/{route}", self.api.cfg.urls.content);
        let arg = arg_header(&arg);
        let r = self
            .api
            .json(&self.uri, |_| {
                self.api.http.post(&url).header("dropbox-api-arg", arg.clone()).header("content-type", "application/octet-stream").body(data.clone())
            })
            .await;
        match r {
            // A conflict at commit time: the file appeared meanwhile.
            Err(CxError::AlreadyExists(_)) => Err(CxError::AlreadyExists(self.uri.clone())),
            r => r,
        }
    }

    fn commit(&self) -> Value {
        json!({ "path": self.path, "mode": self.mode, "autorename": false, "mute": true })
    }
}

#[async_trait::async_trait]
impl Uploader for DropboxUpload {
    async fn whole(&mut self, data: Bytes) -> Result<()> {
        self.content("files/upload", self.commit(), data).await.map(|_| ())
    }

    async fn part(&mut self, data: Bytes, offset: u64) -> Result<()> {
        match self.session.clone() {
            None => {
                let v = self.content("files/upload_session/start", json!({ "close": false }), data).await?;
                let id = v.get("session_id").and_then(Value::as_str).ok_or_else(|| CxError::Io(format!("{}: no upload session id", self.uri)))?;
                self.session = Some(id.to_string());
            }
            Some(id) => {
                let arg = json!({ "cursor": { "session_id": id, "offset": offset }, "close": false });
                self.content("files/upload_session/append_v2", arg, data).await?;
            }
        }
        Ok(())
    }

    async fn last(&mut self, data: Bytes, offset: u64, _total: u64) -> Result<()> {
        let id = self.session.clone().ok_or_else(|| CxError::Io(format!("{}: upload session missing", self.uri)))?;
        let arg = json!({ "cursor": { "session_id": id, "offset": offset }, "commit": self.commit() });
        self.content("files/upload_session/finish", arg, data).await.map(|_| ())
    }

    // Abandoned sessions expire on their own (after a week); there is no
    // call to cancel one.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_arg_header_is_ascii() {
        let h = arg_header(&json!({ "path": "/Ünïcödé/😀.txt" }));
        assert!(h.is_ascii());
        assert!(h.contains("\\u00dc") && h.contains("\\ud83d\\ude00"), "{h}");
        let back: Value = serde_json::from_str(&h).unwrap();
        assert_eq!(back["path"], "/Ünïcödé/😀.txt");
        assert_eq!(api_path("/"), "");
        assert_eq!(api_path("/a/b"), "/a/b");
    }
}
