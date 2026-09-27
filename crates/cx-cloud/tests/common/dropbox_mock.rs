//! Dropbox API v2, the subset cx-cloud uses. Shapes follow
//! https://www.dropbox.com/developers/documentation/http/documentation:
//! RPC routes take JSON bodies, content routes a `Dropbox-API-Arg` header,
//! and route errors are `409 {"error_summary": "...", "error": {...}}`.

use super::{ts, Handler, Req, Resp};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct E {
    pub path: String,
    pub dir: bool,
    pub content: Vec<u8>,
    pub modified: u64,
}

#[derive(Default)]
pub struct State {
    /// Keyed by lower-cased path (Dropbox is case-insensitive).
    pub entries: BTreeMap<String, E>,
    pub sessions: HashMap<String, Vec<u8>>,
    cursors: HashMap<String, (Vec<Value>, usize)>,
    pub seq: u64,
    /// Entries per list_folder page.
    pub page: usize,
    pub uploads: Vec<String>,
}

#[derive(Default)]
pub struct DropboxMock {
    pub st: Mutex<State>,
}

fn conflict(summary: &str) -> Resp {
    let tag = summary.split('/').next().unwrap_or_default();
    Resp::json(409, json!({"error_summary": summary, "error": {".tag": tag}}))
}

fn name_of(path: &str) -> String {
    path.rsplit('/').next().unwrap_or_default().to_string()
}

fn parent_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((p, _)) => p.to_string(),
        None => String::new(),
    }
}

fn meta(e: &E) -> Value {
    if e.dir {
        json!({".tag": "folder", "name": name_of(&e.path), "path_lower": e.path.to_lowercase(), "path_display": e.path, "id": format!("id:{}", e.path.len())})
    } else {
        json!({
            ".tag": "file", "name": name_of(&e.path), "path_lower": e.path.to_lowercase(), "path_display": e.path,
            "id": format!("id:{}", e.path.len()), "client_modified": ts(e.modified), "server_modified": ts(e.modified),
            "rev": "a1c10ce0dd78", "size": e.content.len(), "is_downloadable": true, "content_hash": "0".repeat(64)
        })
    }
}

impl DropboxMock {
    pub fn new(page: usize) -> DropboxMock {
        let m = DropboxMock::default();
        m.st.lock().unwrap().page = page;
        m
    }

    pub fn add(&self, path: &str, dir: bool, content: &[u8]) {
        let mut st = self.st.lock().unwrap();
        st.seq += 1;
        let modified = st.seq;
        st.entries.insert(path.to_lowercase(), E { path: path.into(), dir, content: content.to_vec(), modified });
    }

    pub fn get(&self, path: &str) -> Option<E> {
        self.st.lock().unwrap().entries.get(&path.to_lowercase()).cloned()
    }

    fn arg(req: &Req) -> Value {
        let h = req.h("dropbox-api-arg").expect("content routes carry Dropbox-API-Arg");
        assert!(h.is_ascii(), "Dropbox-API-Arg must be ASCII: {h}");
        serde_json::from_str(h).unwrap()
    }

    fn commit(&self, commit: &Value, data: Vec<u8>) -> Resp {
        let path = commit["path"].as_str().unwrap().to_string();
        let mode = commit["mode"].as_str().unwrap_or("add");
        assert_eq!(commit["autorename"], json!(false));
        let mut st = self.st.lock().unwrap();
        if let Some(e) = st.entries.get(&path.to_lowercase()) {
            if e.dir || mode == "add" {
                return conflict("path/conflict/file/..");
            }
        }
        st.seq += 1;
        let modified = st.seq;
        st.uploads.push(path.clone());
        let e = E { path: path.clone(), dir: false, content: data, modified };
        st.entries.insert(path.to_lowercase(), e.clone());
        Resp::json(200, meta(&e))
    }
}

impl Handler for DropboxMock {
    fn handle(&self, req: &Req, _origin: &str) -> Resp {
        let route = req.path.as_str();
        let body = req.json();
        let path = |k: &str| body[k].as_str().unwrap_or_default().to_string();
        match route {
            "/api/users/get_current_account" => {
                assert_eq!(&req.body[..], b"null");
                Resp::json(200, json!({"account_id": "dbid:AAH4f99", "name": {"display_name": "Test User"}, "email": "me@example.com", "email_verified": true}))
            }
            "/api/users/get_space_usage" => Resp::json(200, json!({"used": 314159265, "allocation": {".tag": "individual", "allocated": 10000000000u64}})),
            "/api/files/list_folder" => {
                let dir = path("path");
                let mut st = self.st.lock().unwrap();
                if !dir.is_empty() {
                    match st.entries.get(&dir.to_lowercase()) {
                        None => return conflict("path/not_found/.."),
                        Some(e) if !e.dir => return conflict("path/not_folder/.."),
                        _ => {}
                    }
                }
                let items: Vec<Value> = st.entries.values().filter(|e| parent_of(&e.path).to_lowercase() == dir.to_lowercase()).map(meta).collect();
                st.seq += 1;
                let cursor = format!("cursor{}", st.seq);
                st.cursors.insert(cursor.clone(), (items, 0));
                drop(st);
                self.page_from(&cursor)
            }
            "/api/files/list_folder/continue" => self.page_from(&path("cursor")),
            "/api/files/get_metadata" => match self.get(&path("path")) {
                Some(e) => Resp::json(200, meta(&e)),
                None => conflict("path/not_found/.."),
            },
            "/api/files/create_folder_v2" => {
                let p = path("path");
                if self.get(&p).is_some() {
                    return conflict("path/conflict/folder/..");
                }
                self.add(&p, true, b"");
                Resp::json(200, json!({"metadata": meta(&self.get(&p).unwrap())}))
            }
            "/api/files/move_v2" | "/api/files/copy_v2" => {
                let (from, to) = (path("from_path"), path("to_path"));
                assert_eq!(body["autorename"], json!(false));
                let mut st = self.st.lock().unwrap();
                if !st.entries.contains_key(&from.to_lowercase()) {
                    return conflict("from_lookup/not_found/..");
                }
                if from.to_lowercase() != to.to_lowercase() && st.entries.contains_key(&to.to_lowercase()) {
                    return conflict("to/conflict/file/..");
                }
                let prefix = format!("{}/", from.to_lowercase());
                let moving: Vec<(String, E)> = st.entries.iter().filter(|(k, _)| **k == from.to_lowercase() || k.starts_with(&prefix)).map(|(k, v)| (k.clone(), v.clone())).collect();
                for (k, mut e) in moving {
                    if route.ends_with("move_v2") {
                        st.entries.remove(&k);
                    }
                    e.path = format!("{to}{}", &e.path[from.len()..]);
                    st.entries.insert(e.path.to_lowercase(), e);
                }
                let e = st.entries[&to.to_lowercase()].clone();
                Resp::json(200, json!({"metadata": meta(&e)}))
            }
            "/api/files/delete_v2" => {
                let p = path("path").to_lowercase();
                let mut st = self.st.lock().unwrap();
                let Some(e) = st.entries.get(&p).cloned() else { return conflict("path_lookup/not_found/..") };
                let prefix = format!("{p}/");
                st.entries.retain(|k, _| *k != p && !k.starts_with(&prefix));
                Resp::json(200, json!({"metadata": meta(&e)}))
            }
            "/content/files/download" => {
                let arg = Self::arg(req);
                match self.get(arg["path"].as_str().unwrap()) {
                    Some(e) if !e.dir => {
                        let m = meta(&e).to_string();
                        Resp::ranged(req, &e.content).header("dropbox-api-result", m)
                    }
                    Some(_) => conflict("path/not_file/.."),
                    None => conflict("path/not_found/.."),
                }
            }
            "/content/files/upload" => {
                let arg = Self::arg(req);
                self.commit(&arg, req.body.to_vec())
            }
            "/content/files/upload_session/start" => {
                let mut st = self.st.lock().unwrap();
                st.seq += 1;
                let id = format!("session{}", st.seq);
                st.sessions.insert(id.clone(), req.body.to_vec());
                Resp::json(200, json!({"session_id": id}))
            }
            "/content/files/upload_session/append_v2" | "/content/files/upload_session/finish" => {
                let arg = Self::arg(req);
                let id = arg["cursor"]["session_id"].as_str().unwrap().to_string();
                let offset = arg["cursor"]["offset"].as_u64().unwrap() as usize;
                let data = {
                    let mut st = self.st.lock().unwrap();
                    let Some(buf) = st.sessions.get_mut(&id) else { return conflict("lookup_failed/not_found/") };
                    if buf.len() != offset {
                        return Resp::json(409, json!({"error_summary": "lookup_failed/incorrect_offset/..", "error": {".tag": "lookup_failed", "lookup_failed": {".tag": "incorrect_offset", "correct_offset": buf.len()}}}));
                    }
                    buf.extend_from_slice(&req.body);
                    if route.ends_with("append_v2") {
                        return Resp::json(200, Value::Null);
                    }
                    st.sessions.remove(&id).unwrap()
                };
                self.commit(&arg["commit"], data)
            }
            _ => Resp::bytes(400, format!("Error in call to API function \"{route}\": unknown route").into_bytes()),
        }
    }
}

impl DropboxMock {
    fn page_from(&self, cursor: &str) -> Resp {
        let mut st = self.st.lock().unwrap();
        let page = st.page.max(1);
        let Some((items, pos)) = st.cursors.get_mut(cursor) else { return conflict("reset/..") };
        let end = (*pos + page).min(items.len());
        let chunk: Vec<Value> = items[*pos..end].to_vec();
        *pos = end;
        let has_more = end < items.len();
        Resp::json(200, json!({"entries": chunk, "cursor": cursor, "has_more": has_more}))
    }
}
