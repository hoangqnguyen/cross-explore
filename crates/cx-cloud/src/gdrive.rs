//! Google Drive (API v3).
//!
//! ## Paths over an id-based store
//!
//! Drive has no paths: every file is an id with a name and parent ids, and
//! names are not unique (a folder can hold three `report.pdf`). A file
//! explorer needs paths, so:
//!
//! - The root is virtual: `/My Drive`, `/Shared drives/<drive>` and
//!   `/Shared with me`.
//! - A folder's children are named in a fixed order (`orderBy=name,createdTime`)
//!   and a repeated name gets ` (2)`, ` (3)`… before its extension, so the
//!   same listing always gives the same names. A `/` in a name (Drive allows
//!   it) is shown as `∕` (division slash).
//! - Every listing fills a per-connection cache from path to file, which is
//!   how a path (including a suffixed duplicate) maps back to an id. A path
//!   not in the cache lists its parent. Entries expire after [`TTL`] and
//!   every change made through this provider invalidates what it touched.
//!
//! ## Google Docs, Sheets, Slides and Drawings
//!
//! Native Google files have no bytes to download. They are listed as
//! `name.docx` / `.xlsx` / `.pptx` / `.png` with an unknown (0) size and
//! read through `files.export`. Other native types (Forms, Sites, …) can't
//! be exported and are listed read-only.
//!
//! ## Operations
//!
//! list: `files.list` (`'<id>' in parents`), first page small. create_dir:
//! `files.create` with the folder mime type. move_to: `files.update` with
//! `addParents`/`removeParents` and the new name. remove: `files.delete`
//! (permanent). trash: `files.update trashed=true`, restorable through
//! [`crate::CloudConnector::restore_trashed`]. open_read: `alt=media` with
//! `Range`. open_write: a resumable upload session (CreateNew / Truncate).
//! copy_within: `files.copy` (files only; Drive can't copy folders).
//! set_modified: `modifiedTime`. free_space: `about.storageQuota`. There is
//! no push watch; callers poll.

use crate::api::Api;
use crate::common::{body_stream, ranged_body, remote_path, skip, split_parent};
use crate::upload::{self, Uploader};
use crate::util::{dir_entry, encode, file_entry, format_rfc3339, new_folder_names, parse_rfc3339, query, unique_name};
use bytes::Bytes;
use cx_core::{
    validate_name, Capabilities, CxError, Endpoint, Entry, EntryKind, Location, Provider, ReadStream, Result, Space, TrashedItem, WriteMode,
    WriteStream,
};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub const MY_DRIVE: &str = "My Drive";
pub const SHARED_DRIVES: &str = "Shared drives";
pub const SHARED_WITH_ME: &str = "Shared with me";

const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const NATIVE_PREFIX: &str = "application/vnd.google-apps.";
const FILE_FIELDS: &str = "id,name,mimeType,size,modifiedTime,createdTime,driveId,capabilities(canEdit),shortcutDetails(targetId,targetMimeType)";

/// How long cached path → id mappings are trusted.
const TTL: Duration = Duration::from_secs(60);
const FIRST_PAGE: usize = 100;
const PAGE: usize = 1000;
/// Resumable upload chunk; Drive wants multiples of 256 KiB.
pub(crate) const CHUNK: usize = 8 * 1024 * 1024;
const CHUNK_UNIT: usize = 256 * 1024;

/// Export formats for native Google files: (mime, extension, export mime).
const EXPORTS: &[(&str, &str, &str)] = &[
    ("application/vnd.google-apps.document", ".docx", "application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
    ("application/vnd.google-apps.spreadsheet", ".xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
    ("application/vnd.google-apps.presentation", ".pptx", "application/vnd.openxmlformats-officedocument.presentationml.presentation"),
    ("application/vnd.google-apps.drawing", ".png", "image/png"),
];

/// A Drive file as the cache keeps it.
#[derive(Debug, Clone)]
struct Node {
    id: String,
    /// The name shown (extension added, duplicates suffixed).
    name: String,
    /// The name on Drive.
    raw_name: String,
    mime: String,
    size: Option<u64>,
    modified: Option<i64>,
    created: Option<i64>,
    can_edit: bool,
    drive_id: Option<String>,
    /// For shortcuts: the file they point at and its type.
    target: Option<(String, String)>,
}

impl Node {
    fn parse(v: &Value) -> Option<Node> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let raw_name = s("name")?;
        let target = v.get("shortcutDetails").and_then(|d| Some((d.get("targetId")?.as_str()?.to_string(), d.get("targetMimeType")?.as_str()?.to_string())));
        Some(Node {
            id: s("id")?,
            name: raw_name.clone(),
            raw_name,
            mime: s("mimeType").unwrap_or_default(),
            size: v.get("size").and_then(|x| x.as_u64().or_else(|| x.as_str()?.parse().ok())),
            modified: s("modifiedTime").as_deref().and_then(parse_rfc3339),
            created: s("createdTime").as_deref().and_then(parse_rfc3339),
            can_edit: v.pointer("/capabilities/canEdit").and_then(Value::as_bool).unwrap_or(true),
            drive_id: s("driveId"),
            target,
        })
    }

    fn virtual_folder(id: &str, name: &str, drive_id: Option<String>) -> Node {
        Node {
            id: id.into(),
            name: name.into(),
            raw_name: name.into(),
            mime: FOLDER_MIME.into(),
            size: None,
            modified: None,
            created: None,
            can_edit: true,
            drive_id,
            target: None,
        }
    }

    /// The type that matters: a shortcut's target's.
    fn effective_mime(&self) -> &str {
        self.target.as_ref().map(|(_, m)| m.as_str()).unwrap_or(&self.mime)
    }

    /// The id whose content or children to use (a shortcut's target).
    fn content_id(&self) -> &str {
        self.target.as_ref().map(|(id, _)| id.as_str()).unwrap_or(&self.id)
    }

    fn is_folder(&self) -> bool {
        self.effective_mime() == FOLDER_MIME
    }

    fn is_native(&self) -> bool {
        self.effective_mime().starts_with(NATIVE_PREFIX) && !self.is_folder()
    }

    fn export(&self) -> Option<(&'static str, &'static str)> {
        EXPORTS.iter().find(|(m, _, _)| *m == self.effective_mime()).map(|(_, ext, mime)| (*ext, *mime))
    }

    /// The name before duplicate suffixing: Drive's name with `/` made
    /// displayable and the export extension added.
    fn display_base(&self) -> String {
        let mut n = self.raw_name.replace('/', "\u{2215}");
        if let Some((ext, _)) = self.export() {
            n.push_str(ext);
        }
        n
    }

    /// The Drive name for a shown `name` (drops an export extension).
    fn raw_for(&self, name: &str) -> String {
        match self.export() {
            Some((ext, _)) => name.strip_suffix(ext).unwrap_or(name).to_string(),
            None => name.to_string(),
        }
    }

    fn container(&self) -> Option<Container> {
        self.is_folder().then(|| Container::Folder { id: self.content_id().to_string(), drive_id: self.drive_id.clone() })
    }

    fn entry(&self) -> Entry {
        let readonly = !self.can_edit || (self.is_native() && self.export().is_none());
        if self.target.is_some() {
            let dir = self.is_folder();
            return Entry {
                hidden: self.name.starts_with('.'),
                kind: EntryKind::Symlink,
                is_dir: dir,
                size: if dir || self.is_native() { 0 } else { self.size.unwrap_or(0) },
                modified: self.modified,
                created: self.created,
                readonly,
                executable: false,
                name: self.name.clone(),
            };
        }
        if self.is_folder() {
            let mut e = dir_entry(self.name.clone(), self.modified, readonly);
            e.created = self.created;
            return e;
        }
        // Native files have no size until exported.
        let size = if self.is_native() { 0 } else { self.size.unwrap_or(0) };
        let mut e = file_entry(self.name.clone(), size, self.modified, self.created);
        e.readonly = readonly;
        e
    }
}

/// Something whose children can be listed.
#[derive(Debug, Clone, PartialEq)]
enum Container {
    Folder { id: String, drive_id: Option<String> },
    SharedDrives,
    SharedWithMe,
}

/// What a path points at.
enum Target {
    Root,
    SharedDrives,
    SharedWithMe,
    Node(Box<Node>),
}

#[derive(Default)]
struct Cache {
    nodes: HashMap<String, (Node, Instant)>,
    /// Folders whose complete listing is in `nodes`.
    listed: HashMap<String, Instant>,
}

fn parent_of(path: &str) -> &str {
    split_parent(path).map(|(p, _)| p).unwrap_or("/")
}

fn join(dir: &str, name: &str) -> String {
    format!("{}/{name}", dir.trim_end_matches('/'))
}

impl Cache {
    fn fresh(&self, path: &str) -> Option<Node> {
        self.nodes.get(path).filter(|(_, t)| t.elapsed() < TTL).map(|(n, _)| n.clone())
    }

    fn listed(&self, dir: &str) -> bool {
        self.listed.get(dir).is_some_and(|t| t.elapsed() < TTL)
    }

    fn put(&mut self, path: String, node: Node) {
        self.nodes.insert(path, (node, Instant::now()));
    }

    /// Forget `dir`'s listing and its direct children.
    fn forget_listing(&mut self, dir: &str) {
        self.listed.remove(dir);
        self.nodes.retain(|p, _| parent_of(p) != dir);
    }

    /// Forget `path` and everything under it.
    fn forget_tree(&mut self, path: &str) {
        let prefix = format!("{}/", path.trim_end_matches('/'));
        self.nodes.retain(|p, _| p != path && !p.starts_with(&prefix));
        self.listed.retain(|p, _| p != path && !p.starts_with(&prefix));
    }

    /// Children names currently known in `dir`.
    fn names_in(&self, dir: &str) -> HashSet<String> {
        self.nodes.keys().filter(|p| parent_of(p) == dir).filter_map(|p| split_parent(p).map(|(_, n)| n.to_string())).collect()
    }
}

/// A Google Drive account.
pub struct GDriveProvider {
    api: Arc<Api>,
    endpoint: Endpoint,
    cache: Arc<Mutex<Cache>>,
    chunk: AtomicUsize,
}

impl GDriveProvider {
    pub(crate) fn new(api: Arc<Api>, endpoint: Endpoint) -> GDriveProvider {
        GDriveProvider { api, endpoint, cache: Arc::default(), chunk: AtomicUsize::new(CHUNK) }
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Use smaller upload chunks (rounded to Drive's 256 KiB unit) so tests
    /// can exercise multi-chunk uploads with small files.
    #[doc(hidden)]
    pub fn set_upload_chunk(&self, bytes: usize) {
        let n = bytes.div_ceil(CHUNK_UNIT).max(1) * CHUNK_UNIT;
        self.chunk.store(n, Ordering::Relaxed);
    }

    /// Forget every cached path (after changes made elsewhere).
    pub fn clear_cache(&self) {
        *self.cache.lock().unwrap() = Cache::default();
    }

    fn uri(&self, path: &str) -> String {
        Location::remote(self.endpoint.clone(), path).uri()
    }

    fn url(&self, path_and_query: &str) -> String {
        format!("{}/{}", self.api.cfg.urls.api, path_and_query)
    }

    /// One page of a container's children.
    async fn page(&self, c: &Container, token: Option<&str>, size: usize) -> Result<(Vec<Node>, Option<String>)> {
        let size = size.to_string();
        let mut q: Vec<(&str, &str)> = vec![("pageSize", &size)];
        if let Some(t) = token {
            q.push(("pageToken", t));
        }
        let filter;
        let files_fields = format!("nextPageToken,files({FILE_FIELDS})");
        let url = match c {
            Container::SharedDrives => {
                q.push(("fields", "nextPageToken,drives(id,name)"));
                self.url(&format!("drives?{}", query(&q)))
            }
            Container::SharedWithMe | Container::Folder { .. } => {
                filter = match c {
                    Container::Folder { id, .. } => format!("'{}' in parents and trashed = false", id.replace('\'', "\\'")),
                    _ => "sharedWithMe = true and trashed = false".to_string(),
                };
                q.extend([
                    ("q", filter.as_str()),
                    ("fields", files_fields.as_str()),
                    // A stable order is what keeps duplicate suffixes stable.
                    ("orderBy", "name,createdTime"),
                    ("supportsAllDrives", "true"),
                    ("includeItemsFromAllDrives", "true"),
                ]);
                if let Container::Folder { drive_id: Some(d), .. } = c {
                    q.extend([("corpora", "drive"), ("driveId", d.as_str())]);
                }
                self.url(&format!("files?{}", query(&q)))
            }
        };
        let v = self.api.json("Google Drive listing", |_| self.api.http.get(&url)).await?;
        let next = v.get("nextPageToken").and_then(Value::as_str).map(str::to_owned);
        let nodes = if *c == Container::SharedDrives {
            let drives = v.get("drives").and_then(Value::as_array).cloned().unwrap_or_default();
            drives
                .iter()
                .filter_map(|d| {
                    let id = d.get("id")?.as_str()?;
                    Some(Node::virtual_folder(id, d.get("name")?.as_str()?, Some(id.to_string())))
                })
                .collect()
        } else {
            v.get("files").and_then(Value::as_array).map(|a| a.iter().filter_map(Node::parse).collect()).unwrap_or_default()
        };
        Ok((nodes, next))
    }

    /// List `dir` completely into the cache, naming children, and stream
    /// the entries to `sink` if given. Returns how many were sent.
    async fn load(&self, dir: &str, c: &Container, sink: Option<&mpsc::Sender<Vec<Entry>>>) -> Result<usize> {
        let mut used = HashSet::new();
        let mut fresh: Vec<(String, Node)> = Vec::new();
        let mut token: Option<String> = None;
        let mut size = FIRST_PAGE;
        let mut sent = 0;
        loop {
            let (nodes, next) = self.page(c, token.as_deref(), size).await?;
            let mut batch = Vec::with_capacity(nodes.len());
            for mut n in nodes {
                n.name = unique_name(&mut used, &n.display_base());
                batch.push(n.entry());
                fresh.push((join(dir, &n.name), n));
            }
            if let Some(sink) = sink {
                if !batch.is_empty() {
                    sent += batch.len();
                    if sink.send(batch).await.is_err() {
                        return Ok(sent); // cancelled: the cache keeps its old state
                    }
                }
            }
            match next {
                Some(t) => token = Some(t),
                None => break,
            }
            size = PAGE;
        }
        let mut cache = self.cache.lock().unwrap();
        cache.forget_listing(dir);
        for (p, n) in fresh {
            cache.put(p, n);
        }
        cache.listed.insert(dir.to_string(), Instant::now());
        Ok(sent)
    }

    /// The child `name` of `dir` (a container), from the cache or by listing.
    async fn child(&self, dir: &str, c: &Container, name: &str) -> Result<Node> {
        let path = join(dir, name);
        {
            let cache = self.cache.lock().unwrap();
            if let Some(n) = cache.fresh(&path) {
                return Ok(n);
            }
            if cache.listed(dir) {
                return Err(CxError::NotFound(self.uri(&path)));
            }
        }
        self.load(dir, c, None).await?;
        self.cache.lock().unwrap().fresh(&path).ok_or_else(|| CxError::NotFound(self.uri(&path)))
    }

    async fn resolve(&self, path: &str) -> Result<Target> {
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let Some((&first, rest)) = segs.split_first() else {
            return Ok(Target::Root);
        };
        let (mut node, rest, mut cur) = match first {
            MY_DRIVE => (Node::virtual_folder("root", MY_DRIVE, None), rest, format!("/{MY_DRIVE}")),
            SHARED_DRIVES => {
                let Some((&drive, rest)) = rest.split_first() else { return Ok(Target::SharedDrives) };
                let dir = format!("/{SHARED_DRIVES}");
                let n = self.child(&dir, &Container::SharedDrives, drive).await?;
                (n, rest, join(&dir, drive))
            }
            SHARED_WITH_ME => {
                let Some((&item, rest)) = rest.split_first() else { return Ok(Target::SharedWithMe) };
                let dir = format!("/{SHARED_WITH_ME}");
                let n = self.child(&dir, &Container::SharedWithMe, item).await?;
                (n, rest, join(&dir, item))
            }
            _ => return Err(CxError::NotFound(self.uri(path))),
        };
        for seg in rest {
            let Some(c) = node.container() else {
                return Err(CxError::NotFound(self.uri(path)));
            };
            node = self.child(&cur, &c, seg).await?;
            cur = join(&cur, seg);
        }
        Ok(Target::Node(Box::new(node)))
    }

    async fn node(&self, loc: &Location) -> Result<Node> {
        match self.resolve(remote_path(loc)?).await? {
            Target::Node(n) => Ok(*n),
            _ => Err(CxError::Unsupported(format!("{} is a fixed Google Drive folder", loc.uri()))),
        }
    }

    /// A node that is a real file or folder (not My Drive or a shared drive).
    async fn item(&self, loc: &Location) -> Result<Node> {
        let path = remote_path(loc)?;
        let n = self.node(loc).await?;
        if split_parent(path).is_none_or(|(p, _)| p == "/" || p == format!("/{SHARED_DRIVES}")) {
            return Err(CxError::Unsupported(format!("{} is a fixed Google Drive folder", loc.uri())));
        }
        Ok(n)
    }

    /// The folder a path points at, as a container that can take new files.
    async fn folder(&self, dir: &str) -> Result<(String, Option<String>)> {
        match self.resolve(dir).await? {
            Target::Node(n) => match n.container() {
                Some(Container::Folder { id, drive_id }) => Ok((id, drive_id)),
                _ => Err(CxError::InvalidLocation(format!("{} is not a folder", self.uri(dir)))),
            },
            _ => Err(CxError::PermissionDenied(format!("{}: files can only be added inside My Drive, a shared drive or a shared folder", self.uri(dir)))),
        }
    }

    /// `Some(node)` if `dir/name` exists.
    async fn existing(&self, dir: &str, name: &str) -> Result<Option<Node>> {
        match self.resolve(&join(dir, name)).await {
            Ok(Target::Node(n)) => Ok(Some(*n)),
            Ok(_) => Ok(None),
            Err(CxError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn fetch(&self, id: &str) -> Result<Node> {
        let url = self.url(&format!("files/{}?{}", encode(id), query(&[("fields", FILE_FIELDS), ("supportsAllDrives", "true")])));
        let v = self.api.json(&format!("Google Drive file {id}"), |_| self.api.http.get(&url)).await?;
        Node::parse(&v).ok_or_else(|| CxError::Io(format!("Google Drive file {id}: unexpected answer")))
    }

    async fn update(&self, id: &str, extra_query: &[(&str, &str)], body: Value, what: &str) -> Result<Value> {
        let mut q = vec![("supportsAllDrives", "true"), ("fields", FILE_FIELDS)];
        q.extend_from_slice(extra_query);
        let url = self.url(&format!("files/{}?{}", encode(id), query(&q)));
        let body = body.to_string();
        self.api.json(what, |_| self.api.http.patch(&url).header("content-type", "application/json").body(body.clone())).await
    }

    fn forget(&self, f: impl FnOnce(&mut Cache)) {
        f(&mut self.cache.lock().unwrap());
    }

    /// Put trashed files back (the undo of [`Provider::trash`]).
    pub async fn restore(&self, items: &[TrashedItem]) -> Result<()> {
        for item in items {
            let Some(id) = item.trashed.as_deref().and_then(trash_marker_id) else { continue };
            self.update(id, &[], json!({ "trashed": false }), &item.original).await?;
        }
        self.clear_cache();
        Ok(())
    }
}

/// The marker put in [`TrashedItem::trashed`]: `<endpoint>/#gdrive-trash=<id>`.
fn trash_marker(ep: &Endpoint, id: &str) -> String {
    format!("{}/#gdrive-trash={id}", ep.uri())
}

/// The Drive file id in a trash marker.
pub fn trash_marker_id(marker: &str) -> Option<&str> {
    marker.split_once("#gdrive-trash=").map(|(_, id)| id).filter(|id| !id.is_empty())
}

#[async_trait::async_trait]
impl Provider for GDriveProvider {
    fn scheme(&self) -> &'static str {
        crate::GDRIVE_SCHEME
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: false, polling: true, server_copy: true, trash: true, posix: false, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let path = remote_path(dir)?;
        let c = match self.resolve(path).await? {
            Target::Root => {
                let entries = vec![dir_entry(MY_DRIVE.into(), None, false), dir_entry(SHARED_DRIVES.into(), None, true), dir_entry(SHARED_WITH_ME.into(), None, true)];
                let n = entries.len();
                let _ = sink.send(entries).await;
                return Ok(n);
            }
            Target::SharedDrives => Container::SharedDrives,
            Target::SharedWithMe => Container::SharedWithMe,
            Target::Node(n) => n.container().ok_or_else(|| CxError::InvalidLocation(format!("{} is not a folder", dir.uri())))?,
        };
        self.load(path.trim_end_matches('/'), &c, Some(&sink)).await
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        let path = remote_path(loc)?;
        match self.resolve(path).await? {
            Target::Root => Ok(dir_entry(crate::oauth::endpoint_account(&self.endpoint), None, true)),
            Target::SharedDrives => Ok(dir_entry(SHARED_DRIVES.into(), None, true)),
            Target::SharedWithMe => Ok(dir_entry(SHARED_WITH_ME.into(), None, true)),
            Target::Node(n) if n.id == "root" || n.drive_id.as_deref() == Some(n.id.as_str()) => Ok(n.entry()),
            Target::Node(n) => {
                // Fresh metadata (sizes and times change), cached name.
                let mut fresh = self.fetch(&n.id).await?;
                fresh.name = n.name;
                Ok(fresh.entry())
            }
        }
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let dir_path = remote_path(dir)?.trim_end_matches('/').to_string();
        let dir_path = if dir_path.is_empty() { "/".to_string() } else { dir_path };
        let (parent_id, drive_id) = self.folder(&dir_path).await?;
        let name = match name {
            Some(n) => {
                validate_name(n)?;
                if self.existing(&dir_path, n).await?.is_some() {
                    return Err(CxError::AlreadyExists(dir.join(n).uri()));
                }
                n.to_string()
            }
            None => {
                let c = Container::Folder { id: parent_id.clone(), drive_id };
                if !self.cache.lock().unwrap().listed(&dir_path) {
                    self.load(&dir_path, &c, None).await?;
                }
                let taken = self.cache.lock().unwrap().names_in(&dir_path);
                new_folder_names().find(|n| !taken.contains(n)).ok_or_else(|| CxError::AlreadyExists("New folder".into()))?
            }
        };
        let url = self.url(&format!("files?{}", query(&[("supportsAllDrives", "true"), ("fields", FILE_FIELDS)])));
        let body = json!({ "name": name, "mimeType": FOLDER_MIME, "parents": [parent_id] }).to_string();
        let v = self.api.json(&dir.join(&name).uri(), |_| self.api.http.post(&url).header("content-type", "application/json").body(body.clone())).await?;
        let mut node = Node::parse(&v).ok_or_else(|| CxError::Io(format!("{}: unexpected answer", dir.join(&name).uri())))?;
        node.name = name.clone();
        let entry = node.entry();
        self.cache.lock().unwrap().put(join(&dir_path, &name), node);
        Ok(entry)
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        let (sp, dp) = (remote_path(src)?, remote_path(dst)?);
        if sp == dp {
            return Ok(());
        }
        let (Some((src_dir, _)), Some((dst_dir, dst_name))) = (split_parent(sp), split_parent(dp)) else {
            return Err(CxError::InvalidLocation(dst.uri()));
        };
        validate_name(dst_name)?;
        if dp.starts_with(&format!("{sp}/")) {
            return Err(CxError::InvalidLocation(format!("can't move {} into itself", src.uri())));
        }
        let node = self.item(src).await?;
        let (dst_parent, _) = self.folder(dst_dir).await?;
        if let Some(existing) = self.existing(dst_dir, dst_name).await? {
            if existing.id != node.id {
                return Err(CxError::AlreadyExists(dst.uri()));
            }
        }
        let raw = node.raw_for(dst_name);
        let mut extra: Vec<(&str, &str)> = Vec::new();
        let old_parents;
        if src_dir != dst_dir {
            let url = self.url(&format!("files/{}?{}", encode(&node.id), query(&[("fields", "parents"), ("supportsAllDrives", "true")])));
            let v = self.api.json(&src.uri(), |_| self.api.http.get(&url)).await?;
            old_parents = v.get("parents").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")).unwrap_or_default();
            extra.push(("addParents", &dst_parent));
            if !old_parents.is_empty() {
                extra.push(("removeParents", &old_parents));
            }
        }
        self.update(&node.id, &extra, json!({ "name": raw }), &src.uri()).await?;
        self.forget(|c| {
            c.forget_tree(sp);
            c.forget_listing(src_dir);
            c.forget_listing(dst_dir);
        });
        Ok(())
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let path = remote_path(loc)?;
        let node = self.item(loc).await?;
        let url = self.url(&format!("files/{}?supportsAllDrives=true", encode(&node.id)));
        self.api.check(&loc.uri(), |_| self.api.http.delete(&url)).await?;
        self.forget(|c| {
            c.forget_tree(path);
            c.forget_listing(parent_of(path));
        });
        Ok(())
    }

    async fn trash(&self, dir: &Location, names: &[String]) -> Result<Vec<TrashedItem>> {
        let mut out = Vec::with_capacity(names.len());
        for name in names {
            let loc = dir.join(name);
            let node = self.item(&loc).await?;
            self.update(&node.id, &[], json!({ "trashed": true }), &loc.uri()).await?;
            let path = remote_path(&loc)?.to_string();
            self.forget(|c| c.forget_tree(&path));
            out.push(TrashedItem { original: loc.uri(), trashed: Some(trash_marker(&self.endpoint, &node.id)) });
        }
        let dir_path = remote_path(dir)?.trim_end_matches('/').to_string();
        self.forget(|c| c.forget_listing(&dir_path));
        Ok(out)
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let node = self.node(loc).await?;
        if node.is_folder() {
            return Err(CxError::InvalidLocation(format!("{} is a folder", loc.uri())));
        }
        let id = node.content_id().to_string();
        if let Some((_, export_mime)) = node.export() {
            // Exports can't be ranged: skip to the offset by reading.
            let url = self.url(&format!("files/{}/export?{}", encode(&id), query(&[("mimeType", export_mime)])));
            let resp = self.api.check(&loc.uri(), |_| self.api.http.get(&url)).await?;
            let mut stream = body_stream(resp, loc.uri());
            skip(&mut stream, offset, &loc.uri()).await?;
            return Ok(stream);
        }
        if node.is_native() {
            return Err(CxError::Unsupported(format!("{}: this kind of Google file can't be downloaded", loc.uri())));
        }
        let url = self.url(&format!("files/{}?alt=media&supportsAllDrives=true", encode(&id)));
        let resp = self
            .api
            .send(&loc.uri(), |_| {
                let rb = self.api.http.get(&url);
                if offset > 0 { rb.header("range", format!("bytes={offset}-")) } else { rb }
            })
            .await?;
        ranged_body(&self.api, resp, offset, loc.uri()).await
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let path = remote_path(loc)?;
        let (dir, name) = split_parent(path).ok_or_else(|| CxError::InvalidLocation(loc.uri()))?;
        validate_name(name)?;
        if mode == WriteMode::Append {
            return Err(CxError::Unsupported("appending to a Google Drive file (files are uploaded whole)".into()));
        }
        let (parent_id, _) = self.folder(dir).await?;
        let target = match self.existing(dir, name).await? {
            Some(_) if mode == WriteMode::CreateNew => return Err(CxError::AlreadyExists(loc.uri())),
            Some(n) if n.is_folder() => return Err(CxError::AlreadyExists(format!("{} (a folder)", loc.uri()))),
            Some(n) if n.is_native() => return Err(CxError::Unsupported(format!("{}: can't overwrite a Google Docs file", loc.uri()))),
            Some(n) => UploadTarget::Replace { id: n.content_id().to_string() },
            None => UploadTarget::Create { parent: parent_id, name: name.to_string() },
        };
        let up = DriveUpload { api: self.api.clone(), target, session: None, uri: loc.uri(), cache: self.cache.clone(), dir: dir.to_string() };
        Ok(Box::pin(upload::start(self.chunk.load(Ordering::Relaxed), up)))
    }

    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        let Ok(node) = self.item(loc).await else { return Ok(()) };
        self.update(&node.id, &[], json!({ "modifiedTime": format_rfc3339(ms) }), &loc.uri()).await?;
        let path = remote_path(loc)?.to_string();
        self.forget(|c| c.forget_listing(parent_of(&path)));
        Ok(())
    }

    async fn copy_within(&self, src: &Location, dst: &Location) -> Result<bool> {
        let node = self.item(src).await?;
        if node.is_folder() {
            return Ok(false); // Drive can't copy folders; the caller walks them
        }
        let dp = remote_path(dst)?;
        let (dst_dir, dst_name) = split_parent(dp).ok_or_else(|| CxError::InvalidLocation(dst.uri()))?;
        validate_name(dst_name)?;
        let (parent, _) = self.folder(dst_dir).await?;
        if self.existing(dst_dir, dst_name).await?.is_some() {
            return Err(CxError::AlreadyExists(dst.uri()));
        }
        let url = self.url(&format!("files/{}/copy?{}", encode(node.content_id()), query(&[("supportsAllDrives", "true"), ("fields", "id")])));
        let body = json!({ "name": node.raw_for(dst_name), "parents": [parent] }).to_string();
        self.api.json(&dst.uri(), |_| self.api.http.post(&url).header("content-type", "application/json").body(body.clone())).await?;
        let dst_dir = dst_dir.to_string();
        self.forget(|c| c.forget_listing(&dst_dir));
        Ok(true)
    }

    async fn free_space(&self, loc: &Location) -> Result<Option<Space>> {
        let path = remote_path(loc)?;
        // The quota is the user's; shared drives count against the organization.
        if path.starts_with(&format!("/{SHARED_DRIVES}")) || path.starts_with(&format!("/{SHARED_WITH_ME}")) {
            return Ok(None);
        }
        let url = self.url(&format!("about?{}", query(&[("fields", "storageQuota")])));
        let v = self.api.json("Google Drive quota", |_| self.api.http.get(&url)).await?;
        let num = |k: &str| v.pointer(&format!("/storageQuota/{k}")).and_then(|x| x.as_u64().or_else(|| x.as_str()?.parse().ok()));
        // No limit means unlimited storage (some Workspace plans).
        let (Some(limit), Some(usage)) = (num("limit"), num("usage")) else { return Ok(None) };
        Ok(Some(Space { free: limit.saturating_sub(usage), total: limit }))
    }
}

enum UploadTarget {
    Create { parent: String, name: String },
    Replace { id: String },
}

/// A resumable upload session: POST (or PATCH) the metadata, get a session
/// URI, PUT the bytes chunk by chunk with `Content-Range`. Intermediate
/// chunks are answered `308 Resume Incomplete`, the last with the file.
struct DriveUpload {
    api: Arc<Api>,
    target: UploadTarget,
    session: Option<String>,
    uri: String,
    cache: Arc<Mutex<Cache>>,
    dir: String,
}

impl DriveUpload {
    async fn session(&mut self) -> Result<String> {
        if let Some(s) = &self.session {
            return Ok(s.clone());
        }
        let base = &self.api.cfg.urls.content;
        let q = query(&[("uploadType", "resumable"), ("supportsAllDrives", "true"), ("fields", "id")]);
        let (method, url, body) = match &self.target {
            UploadTarget::Create { parent, name } => (Method::POST, format!("{base}/files?{q}"), json!({ "name": name, "parents": [parent] })),
            UploadTarget::Replace { id } => (Method::PATCH, format!("{base}/files/{}?{q}", encode(id)), json!({})),
        };
        let body = body.to_string();
        let resp = self
            .api
            .check(&self.uri, |_| {
                self.api
                    .http
                    .request(method.clone(), &url)
                    .header("content-type", "application/json; charset=UTF-8")
                    .header("x-upload-content-type", "application/octet-stream")
                    .body(body.clone())
            })
            .await?;
        let loc = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| CxError::Io(format!("{}: no upload session in the answer", self.uri)))?;
        self.session = Some(loc.clone());
        Ok(loc)
    }

    async fn put(&mut self, data: Bytes, range: String, last: bool) -> Result<()> {
        let session = self.session().await?;
        let resp = self.api.send(&self.uri, |_| self.api.http.put(&session).header("content-range", range.clone()).body(data.clone())).await?;
        let status = resp.status();
        let ok = if last { status.is_success() } else { status == StatusCode::PERMANENT_REDIRECT || status.is_success() };
        if !ok {
            return Err(self.api.error(resp, &self.uri).await);
        }
        if last {
            self.cache.lock().unwrap().forget_listing(&self.dir);
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Uploader for DriveUpload {
    async fn whole(&mut self, data: Bytes) -> Result<()> {
        let n = data.len();
        let range = if n == 0 { "bytes */0".to_string() } else { format!("bytes 0-{}/{n}", n - 1) };
        self.put(data, range, true).await
    }

    async fn part(&mut self, data: Bytes, offset: u64) -> Result<()> {
        let range = format!("bytes {offset}-{}/*", offset + data.len() as u64 - 1);
        self.put(data, range, false).await
    }

    async fn last(&mut self, data: Bytes, offset: u64, total: u64) -> Result<()> {
        let range = if data.is_empty() { format!("bytes */{total}") } else { format!("bytes {offset}-{}/{total}", total - 1) };
        self.put(data, range, true).await
    }

    async fn abort(&mut self) {
        // Cancelling a session is a DELETE on its URI (answered 499).
        if let Some(s) = self.session.take() {
            let _ = self.api.send(&self.uri, |_| self.api.http.delete(&s)).await;
        }
    }
}
