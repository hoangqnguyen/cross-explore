//! Google Drive v3, the subset cx-cloud uses: files.list/get/create/update/
//! delete/copy/export, drives.list, about, and resumable uploads. JSON shapes
//! follow https://developers.google.com/drive/api/reference/rest/v3.

use super::{ts, Handler, Req, Resp};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;

pub const FOLDER: &str = "application/vnd.google-apps.folder";
pub const DOC: &str = "application/vnd.google-apps.document";
pub const SHEET: &str = "application/vnd.google-apps.spreadsheet";
pub const FORM: &str = "application/vnd.google-apps.form";

#[derive(Clone, Debug)]
pub struct F {
    pub id: String,
    pub name: String,
    pub mime: String,
    pub parents: Vec<String>,
    pub content: Vec<u8>,
    pub created: u64,
    pub modified: String,
    pub trashed: bool,
    pub shared: bool,
    pub drive_id: Option<String>,
}

enum Target {
    Create { name: String, parents: Vec<String> },
    Replace(String),
}

struct Session {
    target: Target,
    data: Vec<u8>,
}

#[derive(Default)]
pub struct State {
    pub files: HashMap<String, F>,
    pub drives: Vec<(String, String)>,
    sessions: HashMap<String, Session>,
    pub seq: u64,
    /// Chunk PUTs received per session, in order.
    pub chunk_puts: Vec<String>,
    /// The next N API calls answer 403 `userRateLimitExceeded`.
    pub rate_limit_403: usize,
}

#[derive(Default)]
pub struct DriveMock {
    pub st: Mutex<State>,
}

fn not_found(id: &str) -> Resp {
    Resp::json(
        404,
        json!({"error": {"code": 404, "message": format!("File not found: {id}."), "errors": [{"domain": "global", "reason": "notFound", "message": format!("File not found: {id}.")}]}}),
    )
}

fn file_json(f: &F) -> Value {
    let mut v = json!({
        "kind": "drive#file",
        "id": f.id,
        "name": f.name,
        "mimeType": f.mime,
        "modifiedTime": f.modified,
        "createdTime": ts(f.created),
        "capabilities": {"canEdit": true},
    });
    if !f.mime.starts_with("application/vnd.google-apps.") {
        v["size"] = json!(f.content.len().to_string());
    }
    if let Some(d) = &f.drive_id {
        v["driveId"] = json!(d);
    }
    v
}

impl DriveMock {
    /// Add a file (or folder / native doc) and return its id.
    pub fn add(&self, parent: &str, name: &str, mime: &str, content: &[u8]) -> String {
        let mut st = self.st.lock().unwrap();
        st.seq += 1;
        let id = format!("id{:04}", st.seq);
        let drive_id = st.files.get(parent).and_then(|p| p.drive_id.clone()).or_else(|| st.drives.iter().find(|(d, _)| d == parent).map(|(d, _)| d.clone()));
        let f = F {
            id: id.clone(),
            name: name.into(),
            mime: mime.into(),
            parents: vec![parent.into()],
            content: content.to_vec(),
            created: st.seq,
            modified: ts(st.seq),
            trashed: false,
            shared: false,
            drive_id,
        };
        st.files.insert(id.clone(), f);
        id
    }

    pub fn add_shared(&self, name: &str, mime: &str) -> String {
        let id = self.add("someone-elses-folder", name, mime, b"shared");
        self.st.lock().unwrap().files.get_mut(&id).unwrap().shared = true;
        id
    }

    pub fn file(&self, id: &str) -> F {
        self.st.lock().unwrap().files[id].clone()
    }

    /// Live (untrashed) children of `parent` by name.
    pub fn find(&self, parent: &str, name: &str) -> Vec<F> {
        let st = self.st.lock().unwrap();
        let mut v: Vec<F> = st.files.values().filter(|f| !f.trashed && f.name == name && f.parents.iter().any(|p| p == parent)).cloned().collect();
        v.sort_by_key(|f| f.created);
        v
    }

    fn list(&self, req: &Req) -> Resp {
        let st = self.st.lock().unwrap();
        let q = req.q("q").unwrap_or_default();
        let mut files: Vec<&F> = if q.contains("sharedWithMe = true") {
            st.files.values().filter(|f| f.shared && !f.trashed).collect()
        } else {
            let parent = q.split('\'').nth(1).unwrap_or_default();
            assert!(q.contains("trashed = false"), "listing must exclude trash: {q}");
            st.files.values().filter(|f| !f.trashed && f.parents.iter().any(|p| p == parent)).collect()
        };
        assert_eq!(req.q("orderBy"), Some("name,createdTime"));
        files.sort_by(|a, b| a.name.cmp(&b.name).then(a.created.cmp(&b.created)));
        let size: usize = req.q("pageSize").and_then(|s| s.parse().ok()).unwrap_or(100);
        let start: usize = req.q("pageToken").and_then(|s| s.parse().ok()).unwrap_or(0);
        let end = (start + size).min(files.len());
        let mut v = json!({"kind": "drive#fileList", "incompleteSearch": false, "files": files[start..end].iter().map(|f| file_json(f)).collect::<Vec<_>>()});
        if end < files.len() {
            v["nextPageToken"] = json!(end.to_string());
        }
        Resp::json(200, v)
    }

    fn upload_chunk(&self, req: &Req, sid: &str, origin: &str) -> Resp {
        let _ = origin;
        let mut st = self.st.lock().unwrap();
        st.chunk_puts.push(req.h("content-range").unwrap_or_default().to_string());
        let range = req.h("content-range").unwrap_or_default().strip_prefix("bytes ").unwrap_or_default().to_string();
        let (span, total) = range.split_once('/').unwrap();
        let total: Option<usize> = total.parse().ok();
        let Some(session) = st.sessions.get_mut(sid) else { return Resp::json(404, json!({"error": {"code": 404, "message": "session"}})) };
        if span != "*" {
            let (a, b) = span.split_once('-').unwrap();
            let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
            assert_eq!(a, session.data.len(), "chunks arrive in order");
            assert_eq!(b + 1 - a, req.body.len(), "range matches the body");
            if total.is_none() {
                assert_eq!(req.body.len() % (256 * 1024), 0, "intermediate chunks are multiples of 256 KiB");
            }
            session.data.extend_from_slice(&req.body);
        }
        match total {
            Some(t) if t == session.data.len() => {
                let session = st.sessions.remove(sid).unwrap();
                st.seq += 1;
                let seq = st.seq;
                let id = match session.target {
                    Target::Replace(id) => {
                        let f = st.files.get_mut(&id).unwrap();
                        f.content = session.data;
                        f.modified = ts(seq);
                        id
                    }
                    Target::Create { name, parents } => {
                        let id = format!("id{seq:04}");
                        let f = F { id: id.clone(), name, mime: "application/octet-stream".into(), parents, content: session.data, created: seq, modified: ts(seq), trashed: false, shared: false, drive_id: None };
                        st.files.insert(id.clone(), f);
                        id
                    }
                };
                Resp::json(200, json!({"id": id}))
            }
            _ => {
                let len = session.data.len();
                Resp::empty(308).header("range", format!("bytes=0-{}", len.saturating_sub(1)))
            }
        }
    }
}

impl Handler for DriveMock {
    fn handle(&self, req: &Req, origin: &str) -> Resp {
        let m = req.method.as_str();
        {
            let mut st = self.st.lock().unwrap();
            if st.rate_limit_403 > 0 {
                st.rate_limit_403 -= 1;
                return Resp::json(403, json!({"error": {"code": 403, "message": "User Rate Limit Exceeded", "errors": [{"domain": "usageLimits", "reason": "userRateLimitExceeded"}]}}));
            }
        }
        let parts: Vec<&str> = req.path.trim_start_matches('/').split('/').collect();
        match (m, parts.as_slice()) {
            ("GET", ["api", "files"]) => self.list(req),
            ("GET", ["api", "drives"]) => {
                let st = self.st.lock().unwrap();
                Resp::json(200, json!({"kind": "drive#driveList", "drives": st.drives.iter().map(|(id, n)| json!({"kind": "drive#drive", "id": id, "name": n})).collect::<Vec<_>>()}))
            }
            ("GET", ["api", "about"]) => {
                if req.q("fields").unwrap_or_default().contains("storageQuota") {
                    Resp::json(200, json!({"storageQuota": {"limit": "16106127360", "usage": "6106127360", "usageInDrive": "5000000000", "usageInDriveTrash": "0"}}))
                } else {
                    Resp::json(200, json!({"user": {"kind": "drive#user", "displayName": "Test User", "emailAddress": "me@example.com"}}))
                }
            }
            ("POST", ["api", "files"]) => {
                let b = req.json();
                let parent = b["parents"][0].as_str().unwrap().to_string();
                let id = self.add(&parent, b["name"].as_str().unwrap(), b["mimeType"].as_str().unwrap_or("application/octet-stream"), b"");
                Resp::json(200, file_json(&self.file(&id)))
            }
            ("GET", ["api", "files", id]) => {
                let st = self.st.lock().unwrap();
                let Some(f) = st.files.get(*id) else { return not_found(id) };
                if req.q("alt") == Some("media") {
                    if f.mime.starts_with("application/vnd.google-apps.") {
                        return Resp::json(403, json!({"error": {"code": 403, "message": "Only files with binary content can be downloaded. Use Export with Docs Editors files.", "errors": [{"reason": "fileNotDownloadable"}]}}));
                    }
                    return Resp::ranged(req, &f.content);
                }
                if req.q("fields") == Some("parents") {
                    return Resp::json(200, json!({"parents": f.parents}));
                }
                Resp::json(200, file_json(f))
            }
            ("GET", ["api", "files", id, "export"]) => {
                let st = self.st.lock().unwrap();
                let Some(f) = st.files.get(*id) else { return not_found(id) };
                let mime = req.q("mimeType").unwrap_or_default();
                Resp::bytes(200, format!("EXPORT({mime}):{}", f.name).into_bytes())
            }
            ("PATCH", ["api", "files", id]) => {
                let mut st = self.st.lock().unwrap();
                st.seq += 1;
                let seq = st.seq;
                let Some(f) = st.files.get_mut(*id) else { return not_found(id) };
                let b = req.json();
                if let Some(n) = b["name"].as_str() {
                    f.name = n.into();
                }
                if let Some(t) = b["trashed"].as_bool() {
                    f.trashed = t;
                }
                if let Some(t) = b["modifiedTime"].as_str() {
                    f.modified = t.into();
                } else {
                    f.modified = ts(seq);
                }
                if let Some(rm) = req.q("removeParents") {
                    let rm: Vec<&str> = rm.split(',').collect();
                    f.parents.retain(|p| !rm.contains(&p.as_str()));
                }
                if let Some(add) = req.q("addParents") {
                    f.parents.push(add.into());
                }
                Resp::json(200, file_json(f))
            }
            ("DELETE", ["api", "files", id]) => {
                let mut st = self.st.lock().unwrap();
                match st.files.remove(*id) {
                    Some(_) => Resp::empty(204),
                    None => not_found(id),
                }
            }
            ("POST", ["api", "files", id, "copy"]) => {
                let Some(src) = self.st.lock().unwrap().files.get(*id).cloned() else { return not_found(id) };
                let b = req.json();
                let parent = b["parents"][0].as_str().unwrap();
                let nid = self.add(parent, b["name"].as_str().unwrap_or(&src.name), &src.mime, &src.content);
                Resp::json(200, json!({"id": nid}))
            }
            ("POST", ["content", "files"]) | ("PATCH", ["content", "files", _]) => {
                assert_eq!(req.q("uploadType"), Some("resumable"));
                let b = req.json();
                let target = match parts.get(2) {
                    Some(id) => {
                        if !self.st.lock().unwrap().files.contains_key(*id) {
                            return not_found(id);
                        }
                        Target::Replace(id.to_string())
                    }
                    None => Target::Create {
                        name: b["name"].as_str().unwrap().into(),
                        parents: b["parents"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect(),
                    },
                };
                let mut st = self.st.lock().unwrap();
                st.seq += 1;
                let sid = format!("s{}", st.seq);
                st.sessions.insert(sid.clone(), Session { target, data: Vec::new() });
                Resp::empty(200).header("location", format!("{origin}/content/session/{sid}"))
            }
            ("PUT", ["content", "session", sid]) => self.upload_chunk(req, sid, origin),
            ("DELETE", ["content", "session", sid]) => {
                self.st.lock().unwrap().sessions.remove(*sid);
                Resp::empty(499)
            }
            _ => Resp::json(404, json!({"error": {"code": 404, "message": format!("no route {m} {}", req.path)}})),
        }
    }
}
