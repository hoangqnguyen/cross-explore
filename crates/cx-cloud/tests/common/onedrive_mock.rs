//! Microsoft Graph `me/drive`, the subset cx-cloud uses. Shapes follow
//! https://learn.microsoft.com/graph/api/resources/driveitem: path
//! addressing (`root:/a/b:`), `@odata.nextLink` paging, redirects to a
//! pre-authenticated download URL, upload sessions and async copy monitors.
//! Pre-authenticated URLs (download, upload, monitor) reject a bearer token,
//! as the real service's CDN would not want one.

use super::{ts, Handler, Req, Resp};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct I {
    pub id: String,
    pub path: String,
    pub dir: bool,
    pub content: Vec<u8>,
    pub modified: String,
}

struct Session {
    path: String,
    fail_on_conflict: bool,
    data: Vec<u8>,
}

#[derive(Default)]
pub struct State {
    pub items: BTreeMap<String, I>,
    sessions: HashMap<String, Session>,
    monitors: HashMap<String, usize>,
    pub seq: u64,
    pub page: usize,
    pub chunk_ranges: Vec<String>,
}

pub struct GraphMock {
    pub st: Mutex<State>,
}

fn err(status: u16, code: &str, msg: &str) -> Resp {
    Resp::json(status, json!({"error": {"code": code, "message": msg, "innerError": {"request-id": "0000"}}}))
}

fn parent_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some(("", _)) | None => "/".into(),
        Some((p, _)) => p.into(),
    }
}

fn name_of(path: &str) -> String {
    if path == "/" { "root".into() } else { path.rsplit('/').next().unwrap().into() }
}

impl GraphMock {
    pub fn new(page: usize) -> GraphMock {
        let mut st = State { page, ..Default::default() };
        st.items.insert("/".into(), I { id: "root".into(), path: "/".into(), dir: true, content: vec![], modified: ts(0) });
        GraphMock { st: Mutex::new(st) }
    }

    pub fn add(&self, path: &str, dir: bool, content: &[u8]) -> String {
        let mut st = self.st.lock().unwrap();
        st.seq += 1;
        let id = format!("ITEM{}", st.seq);
        let modified = ts(st.seq);
        st.items.insert(path.into(), I { id: id.clone(), path: path.into(), dir, content: content.to_vec(), modified });
        id
    }

    pub fn get(&self, path: &str) -> Option<I> {
        self.st.lock().unwrap().items.get(path).cloned()
    }

    fn item_json(st: &State, i: &I) -> Value {
        let parent = st.items.get(&parent_of(&i.path));
        let mut v = json!({
            "id": i.id, "name": name_of(&i.path),
            "createdDateTime": ts(0), "lastModifiedDateTime": i.modified,
            "fileSystemInfo": {"createdDateTime": ts(0), "lastModifiedDateTime": i.modified},
            "parentReference": {"driveId": "b!drive1", "driveType": "personal", "id": parent.map(|p| p.id.clone()).unwrap_or_default()},
            "size": i.content.len(),
        });
        if i.dir {
            let n = st.items.keys().filter(|k| *k != "/" && parent_of(k) == i.path).count();
            v["folder"] = json!({"childCount": n});
            if i.path == "/" {
                v["root"] = json!({});
            }
        } else {
            v["file"] = json!({"mimeType": "application/octet-stream", "hashes": {"quickXorHash": "AAAA"}});
        }
        v
    }

    fn by_id(st: &State, id: &str) -> Option<I> {
        st.items.values().find(|i| i.id == id).cloned()
    }

    /// Move or copy the subtree at `from` to `to`.
    fn relocate(st: &mut State, from: &str, to: &str, keep: bool) {
        let prefix = format!("{from}/");
        let moving: Vec<I> = st.items.values().filter(|i| i.path == from || i.path.starts_with(&prefix)).cloned().collect();
        for mut i in moving {
            if !keep {
                st.items.remove(&i.path);
            } else {
                st.seq += 1;
                i.id = format!("ITEM{}", st.seq);
            }
            i.path = format!("{to}{}", &i.path[from.len()..]);
            st.items.insert(i.path.clone(), i);
        }
    }

    fn children(&self, req: &Req, path: &str, origin: &str) -> Resp {
        let st = self.st.lock().unwrap();
        match st.items.get(path) {
            None => return err(404, "itemNotFound", "Item does not exist"),
            Some(i) if !i.dir => return err(404, "itemNotFound", "Item does not exist"),
            _ => {}
        }
        let kids: Vec<&I> = st.items.values().filter(|i| i.path != "/" && parent_of(&i.path) == path).collect();
        let skip: usize = req.q("$skiptoken").and_then(|s| s.parse().ok()).unwrap_or(0);
        let end = (skip + st.page).min(kids.len());
        let mut v = json!({"@odata.context": "https://graph.microsoft.com/v1.0/$metadata#Collection(driveItem)", "value": kids[skip..end].iter().map(|i| Self::item_json(&st, i)).collect::<Vec<_>>()});
        if end < kids.len() {
            let base = if path == "/" { "/api/me/drive/root/children".to_string() } else { format!("/api/me/drive/root:{path}:/children") };
            v["@odata.nextLink"] = json!(format!("{origin}{base}?$skiptoken={end}"));
        }
        Resp::json(200, v)
    }

    fn create_or_replace(st: &mut State, path: &str, data: Vec<u8>, fail: bool) -> Resp {
        if let Some(existing) = st.items.get_mut(path) {
            if fail || existing.dir {
                return err(409, "nameAlreadyExists", "The specified item name already exists.");
            }
            existing.content = data;
            let i = existing.clone();
            return Resp::json(200, Self::item_json(st, &i));
        }
        st.seq += 1;
        let i = I { id: format!("ITEM{}", st.seq), path: path.into(), dir: false, content: data, modified: ts(st.seq) };
        st.items.insert(path.into(), i.clone());
        Resp::json(201, Self::item_json(st, &i))
    }

    fn item_route(&self, req: &Req, path: &str, suffix: &str, origin: &str) -> Resp {
        let m = req.method.as_str();
        match (m, suffix) {
            ("GET", "") => {
                let st = self.st.lock().unwrap();
                match st.items.get(path) {
                    Some(i) => Resp::json(200, Self::item_json(&st, i)),
                    None => err(404, "itemNotFound", "The resource could not be found."),
                }
            }
            ("GET", "/children") => self.children(req, path, origin),
            ("POST", "/children") => {
                let b = req.json();
                assert_eq!(b["@microsoft.graph.conflictBehavior"], "fail");
                assert!(b.get("folder").is_some());
                let child = format!("{}/{}", path.trim_end_matches('/'), b["name"].as_str().unwrap());
                if self.get(&child).is_some() {
                    return err(409, "nameAlreadyExists", "The specified item name already exists.");
                }
                self.add(&child, true, b"");
                let st = self.st.lock().unwrap();
                Resp::json(201, Self::item_json(&st, &st.items[&child]))
            }
            ("GET", "/content") => match self.get(path) {
                Some(i) if !i.dir => Resp::empty(302).header("location", format!("{origin}/download/{}", i.id)),
                _ => err(404, "itemNotFound", "The resource could not be found."),
            },
            ("PUT", "/content") => {
                let fail = req.q("@microsoft.graph.conflictBehavior") == Some("fail");
                let mut st = self.st.lock().unwrap();
                Self::create_or_replace(&mut st, path, req.body.to_vec(), fail)
            }
            ("POST", "/createUploadSession") => {
                let fail = req.json()["item"]["@microsoft.graph.conflictBehavior"] == "fail";
                let mut st = self.st.lock().unwrap();
                st.seq += 1;
                let sid = format!("up{}", st.seq);
                st.sessions.insert(sid.clone(), Session { path: path.into(), fail_on_conflict: fail, data: Vec::new() });
                Resp::json(200, json!({"uploadUrl": format!("{origin}/upload/{sid}"), "expirationDateTime": "2030-01-01T00:00:00Z"}))
            }
            ("PATCH", "") => {
                let mut st = self.st.lock().unwrap();
                let Some(i) = st.items.get_mut(path) else { return err(404, "itemNotFound", "") };
                if let Some(t) = req.json()["fileSystemInfo"]["lastModifiedDateTime"].as_str() {
                    i.modified = t.into();
                }
                let i = i.clone();
                Resp::json(200, Self::item_json(&st, &i))
            }
            ("DELETE", "") => {
                let mut st = self.st.lock().unwrap();
                if !st.items.contains_key(path) {
                    return err(404, "itemNotFound", "");
                }
                let prefix = format!("{path}/");
                st.items.retain(|k, _| k != path && !k.starts_with(&prefix));
                Resp::empty(204)
            }
            _ => err(400, "invalidRequest", &format!("no route {m} {path} {suffix}")),
        }
    }
}

impl Handler for GraphMock {
    fn handle(&self, req: &Req, origin: &str) -> Resp {
        let p = req.path.as_str();
        let pre_authorized = p.starts_with("/download/") || p.starts_with("/upload/") || p.starts_with("/monitor/");
        if pre_authorized && req.h("authorization").is_some() {
            return err(401, "unauthenticated", "pre-authenticated URLs must not carry a bearer token");
        }
        if p == "/api/me" {
            return Resp::json(200, json!({"displayName": "Test User", "mail": null, "userPrincipalName": "me@example.com", "id": "u1"}));
        }
        if p == "/api/me/drive" {
            return Resp::json(200, json!({"id": "b!drive1", "driveType": "personal", "quota": {"total": 5368709120u64, "used": 1073741824u64, "remaining": 4294967296u64, "deleted": 0, "state": "normal"}}));
        }
        if p == "/api/me/drive/root" {
            return self.item_route(req, "/", "", origin);
        }
        if p == "/api/me/drive/root/children" {
            return self.item_route(req, "/", "/children", origin);
        }
        if let Some(rest) = p.strip_prefix("/api/me/drive/root:") {
            let (path, suffix) = rest.rsplit_once(':').expect("path addressing ends with ':'");
            return self.item_route(req, path, suffix, origin);
        }
        if let Some(rest) = p.strip_prefix("/api/me/drive/items/") {
            let (id, action) = rest.split_once('/').unwrap_or((rest, ""));
            let mut st = self.st.lock().unwrap();
            let Some(item) = Self::by_id(&st, id) else { return err(404, "itemNotFound", "") };
            let b = req.json();
            let parent_id = b["parentReference"]["id"].as_str().map(str::to_owned);
            let parent = parent_id.as_deref().and_then(|pid| Self::by_id(&st, pid)).map(|p| p.path).unwrap_or_else(|| parent_of(&item.path));
            let name = b["name"].as_str().map(str::to_owned).unwrap_or_else(|| name_of(&item.path));
            let to = format!("{}/{name}", parent.trim_end_matches('/'));
            match (req.method.as_str(), action) {
                ("PATCH", "") => {
                    if to.to_lowercase() != item.path.to_lowercase() && st.items.contains_key(&to) {
                        return err(409, "nameAlreadyExists", "");
                    }
                    Self::relocate(&mut st, &item.path, &to, false);
                    let i = st.items[&to].clone();
                    return Resp::json(200, Self::item_json(&st, &i));
                }
                ("POST", "copy") => {
                    assert_eq!(b["parentReference"]["driveId"], "b!drive1");
                    Self::relocate(&mut st, &item.path, &to, true);
                    st.seq += 1;
                    let mid = format!("m{}", st.seq);
                    st.monitors.insert(mid.clone(), 0);
                    return Resp::empty(202).header("location", format!("{origin}/monitor/{mid}"));
                }
                _ => return err(400, "invalidRequest", "unknown item action"),
            }
        }
        if let Some(id) = p.strip_prefix("/download/") {
            let st = self.st.lock().unwrap();
            return match Self::by_id(&st, id) {
                Some(i) => Resp::ranged(req, &i.content),
                None => err(404, "itemNotFound", ""),
            };
        }
        if let Some(mid) = p.strip_prefix("/monitor/") {
            let mut st = self.st.lock().unwrap();
            let polls = st.monitors.get_mut(mid).unwrap();
            *polls += 1;
            return if *polls < 2 {
                Resp::json(202, json!({"operation": "itemCopy", "percentageComplete": 50.0, "status": "inProgress"}))
            } else {
                Resp::json(200, json!({"operation": "itemCopy", "percentageComplete": 100.0, "status": "completed", "resourceId": "x"}))
            };
        }
        if let Some(sid) = p.strip_prefix("/upload/") {
            let mut st = self.st.lock().unwrap();
            if req.method == "DELETE" {
                st.sessions.remove(sid);
                return Resp::empty(204);
            }
            let range = req.h("content-range").unwrap_or_default().to_string();
            st.chunk_ranges.push(range.clone());
            let (span, total) = range.strip_prefix("bytes ").unwrap().split_once('/').unwrap();
            let total: usize = total.parse().expect("every chunk states the total size");
            let (a, b) = span.split_once('-').unwrap();
            let (a, b): (usize, usize) = (a.parse().unwrap(), b.parse().unwrap());
            let Some(s) = st.sessions.get_mut(sid) else { return err(404, "itemNotFound", "session") };
            assert_eq!(a, s.data.len(), "chunks in order");
            assert_eq!(b + 1 - a, req.body.len());
            assert_eq!(req.h("content-length").and_then(|l| l.parse().ok()), Some(req.body.len()));
            s.data.extend_from_slice(&req.body);
            if s.data.len() < total {
                assert_eq!(req.body.len() % (320 * 1024), 0, "chunks are multiples of 320 KiB");
                let next = s.data.len();
                return Resp::json(202, json!({"expirationDateTime": "2030-01-01T00:00:00Z", "nextExpectedRanges": [format!("{next}-")]}));
            }
            let s = st.sessions.remove(sid).unwrap();
            return Self::create_or_replace(&mut st, &s.path, s.data, s.fail_on_conflict);
        }
        err(400, "invalidRequest", &format!("no route {} {p}", req.method))
    }
}
