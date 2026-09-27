//! Colored tags. On macOS local files use Finder's own tags (the
//! `com.apple.metadata:_kMDItemUserTags` attribute, so Finder and Spotlight
//! see them); everything else is kept in a small JSON file keyed by URI.

#[cfg(target_os = "macos")]
use cx_core::Location;
use cx_core::{CxError, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Finder's colored tags, in Finder's menu order. Other names are allowed
/// and shown without a color.
pub const TAG_COLORS: [&str; 7] = ["Red", "Orange", "Yellow", "Green", "Blue", "Purple", "Gray"];

pub struct Tags {
    path: PathBuf,
    db: Mutex<HashMap<String, Vec<String>>>,
    /// Use Finder tags for local files (macOS). Off in tests so they don't
    /// depend on the file system supporting extended attributes.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    finder: bool,
}

/// One file carrying a tag, for the "Tagged …" result lists.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaggedHit {
    pub uri: String,
    pub parent: String,
    pub rel_path: String,
    pub entry: cx_core::Entry,
}

impl Tags {
    pub fn new(path: PathBuf) -> Tags {
        let db = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Tags { path, db: Mutex::new(db), finder: cfg!(target_os = "macos") }
    }

    /// Keep every tag in the JSON file, even for local files on macOS.
    pub fn json_only(path: PathBuf) -> Tags {
        Tags { finder: false, ..Tags::new(path) }
    }

    #[cfg(target_os = "macos")]
    fn finder_path(&self, uri: &str) -> Option<PathBuf> {
        if !self.finder {
            return None;
        }
        match Location::parse(uri) {
            Ok(Location::Local(p)) => Some(p),
            _ => None,
        }
    }

    pub fn get(&self, uri: &str) -> Vec<String> {
        #[cfg(target_os = "macos")]
        if let Some(p) = self.finder_path(uri) {
            return finder::read(&p);
        }
        self.db.lock().unwrap().get(uri).cloned().unwrap_or_default()
    }

    /// Tags of many URIs at once (the list view asks per visible page).
    pub fn get_many(&self, uris: impl IntoIterator<Item = String>) -> HashMap<String, Vec<String>> {
        uris.into_iter().map(|u| (self.get(&u), u)).map(|(t, u)| (u, t)).collect()
    }

    pub fn set(&self, uri: &str, tags: Vec<String>) -> Result<()> {
        #[cfg(target_os = "macos")]
        if let Some(p) = self.finder_path(uri) {
            return finder::write(&p, &tags);
        }
        let mut db = self.db.lock().unwrap();
        if tags.is_empty() {
            db.remove(uri);
        } else {
            db.insert(uri.to_string(), tags);
        }
        let bytes = serde_json::to_vec_pretty(&*db).map_err(|e| CxError::Io(e.to_string()))?;
        std::fs::write(&self.path, bytes).map_err(|e| CxError::from_io(e, self.path.display()))
    }

    /// Everything carrying `tag`: Spotlight for Finder tags, plus our own DB.
    pub fn find(&self, tag: &str) -> Vec<String> {
        let mut out: Vec<String> = self.db.lock().unwrap().iter().filter(|(_, t)| t.iter().any(|x| x == tag)).map(|(u, _)| u.clone()).collect();
        #[cfg(target_os = "macos")]
        if self.finder {
            out.extend(finder::find(tag));
        }
        out.sort();
        out.dedup();
        out
    }

    /// Every tag name in use in our own DB (Finder tags are not enumerated:
    /// that would need a full Spotlight scan), plus the standard colors.
    pub fn known(&self) -> Vec<String> {
        let mut out: Vec<String> = TAG_COLORS.iter().map(|s| s.to_string()).collect();
        let db = self.db.lock().unwrap();
        let mut extra: Vec<String> = db.values().flatten().filter(|t| !TAG_COLORS.contains(&t.as_str())).cloned().collect();
        extra.sort();
        extra.dedup();
        out.extend(extra);
        out
    }
}

#[cfg(target_os = "macos")]
mod finder {
    use super::*;
    use std::path::Path;

    const ATTR: &str = "com.apple.metadata:_kMDItemUserTags";
    const COLORS: [&str; 8] = ["", "Gray", "Green", "Purple", "Blue", "Yellow", "Red", "Orange"];

    pub fn read(path: &Path) -> Vec<String> {
        let Ok(Some(bytes)) = xattr::get(path, ATTR) else { return vec![] };
        let Ok(plist::Value::Array(items)) = plist::from_bytes::<plist::Value>(&bytes) else { return vec![] };
        // Entries look like "Red\n6": the name, then Finder's color index.
        items.into_iter().filter_map(|v| v.into_string()).map(|s| s.split('\n').next().unwrap_or("").to_string()).filter(|s| !s.is_empty()).collect()
    }

    pub fn write(path: &Path, tags: &[String]) -> Result<()> {
        if tags.is_empty() {
            let _ = xattr::remove(path, ATTR);
            return Ok(());
        }
        let values: Vec<plist::Value> = tags
            .iter()
            .map(|t| match COLORS.iter().position(|c| c == t) {
                Some(i) if i > 0 => plist::Value::String(format!("{t}\n{i}")),
                _ => plist::Value::String(t.clone()),
            })
            .collect();
        let mut buf = Vec::new();
        plist::to_writer_binary(&mut buf, &plist::Value::Array(values)).map_err(|e| CxError::Io(e.to_string()))?;
        xattr::set(path, ATTR, &buf).map_err(|e| CxError::from_io(e, path.display()))
    }

    pub fn find(tag: &str) -> Vec<String> {
        let q = format!("kMDItemUserTags == '{}'", tag.replace('\'', "\\'"));
        let Ok(out) = std::process::Command::new("mdfind").arg(q).output() else { return vec![] };
        String::from_utf8_lossy(&out.stdout).lines().take(2000).map(|p| Location::local(p).uri()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_tags_round_trip_and_find() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tags.json");
        let tags = Tags::json_only(path.clone());
        tags.set("sftp://h/a", vec!["Red".into(), "Work".into()]).unwrap();
        tags.set("sftp://h/b", vec!["Red".into()]).unwrap();
        assert_eq!(tags.find("Red"), vec!["sftp://h/a".to_string(), "sftp://h/b".to_string()]);
        let again = Tags::json_only(path);
        assert_eq!(again.get("sftp://h/a"), vec!["Red".to_string(), "Work".to_string()]);
        assert!(again.known().contains(&"Work".to_string()));
        again.set("sftp://h/a", vec![]).unwrap();
        assert!(again.get("sftp://h/a").is_empty());
    }
}
