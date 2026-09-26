//! Colored tags. On macOS local files use Finder's own tags (the
//! `com.apple.metadata:_kMDItemUserTags` attribute, so Finder and Spotlight
//! see them); everything else is kept in a small JSON file keyed by URI.

#[cfg(target_os = "macos")]
use cx_core::Location;
use cx_core::{CxError, Result};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct Tags {
    path: PathBuf,
    db: Mutex<HashMap<String, Vec<String>>>,
}

impl Tags {
    pub fn new(path: PathBuf) -> Tags {
        let db = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Tags { path, db: Mutex::new(db) }
    }

    pub fn get(&self, uri: &str) -> Vec<String> {
        #[cfg(target_os = "macos")]
        if let Ok(Location::Local(p)) = Location::parse(uri) {
            return finder::read(&p);
        }
        self.db.lock().unwrap().get(uri).cloned().unwrap_or_default()
    }

    pub fn set(&self, uri: &str, tags: Vec<String>) -> Result<()> {
        #[cfg(target_os = "macos")]
        if let Ok(Location::Local(p)) = Location::parse(uri) {
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
        out.extend(finder::find(tag));
        out.sort();
        out.dedup();
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
