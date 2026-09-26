//! The directory tree of one archive, built once from its headers.
//!
//! Archives store a flat list of member paths, often without entries for the
//! folders they imply (`a/b/c.txt` alone is a valid zip). Browsing needs a
//! real tree, so folders are synthesised here, and every later `list`/`stat`
//! is a hash lookup instead of a re-read of the archive.

use cx_core::{Entry, EntryKind};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemberKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// One member as the archive describes it, before path cleanup.
#[derive(Debug, Clone)]
pub(crate) struct MemberHeader {
    pub raw_name: String,
    pub kind: MemberKind,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub modified: Option<i64>,
    /// Position in the archive (zip index, tar entry number, 7z file index):
    /// how the member is found again when it is read.
    pub ordinal: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct Node {
    pub entry: Entry,
    /// The member holding this node's data; `None` for synthesised folders.
    pub member: Option<usize>,
    /// Child names (not full paths).
    pub children: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ArchiveIndex {
    /// Keyed by the clean relative path ("" is the root, "a/b" a descendant).
    nodes: HashMap<String, Node>,
    readonly: bool,
}

/// Turn a member name into a safe relative path ("a/b"), or `None` when it
/// could escape the extraction folder ("zip-slip": `..`, absolute paths,
/// drive letters) or is empty. Backslashes count as separators because
/// Windows tools write them.
pub(crate) fn sanitize(raw: &str) -> Option<String> {
    let unified = raw.replace('\\', "/");
    if unified.starts_with('/') {
        return None;
    }
    let mut parts = Vec::new();
    for (i, seg) in unified.split('/').enumerate() {
        match seg {
            "" | "." => {}
            ".." => return None,
            s if s.contains('\0') => return None,
            // "C:" as the first segment is an absolute Windows path.
            s if i == 0 && s.len() == 2 && s.ends_with(':') => return None,
            s => parts.push(s),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The index key for a location's inner path ("/a/b/" → "a/b").
pub(crate) fn key_of(inner: &str) -> String {
    inner.trim_matches('/').to_string()
}

pub(crate) fn split_parent(key: &str) -> (&str, &str) {
    match key.rfind('/') {
        Some(i) => (&key[..i], &key[i + 1..]),
        None => ("", key),
    }
}

impl ArchiveIndex {
    pub fn new(root: Entry, readonly: bool) -> Self {
        let mut nodes = HashMap::new();
        nodes.insert(String::new(), Node { entry: root, member: None, children: Vec::new() });
        ArchiveIndex { nodes, readonly }
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.nodes.get(key)
    }

    pub fn children(&self, key: &str) -> Option<impl Iterator<Item = &Entry> + '_> {
        let node = self.nodes.get(key)?;
        let base = key.to_string();
        Some(node.children.iter().filter_map(move |name| {
            let k = if base.is_empty() { name.clone() } else { format!("{base}/{name}") };
            self.nodes.get(&k).map(|n| &n.entry)
        }))
    }

    fn entry(&self, name: &str, kind: MemberKind, size: u64, modified: Option<i64>) -> Entry {
        let (kind, is_dir) = match kind {
            MemberKind::File => (EntryKind::File, false),
            MemberKind::Dir => (EntryKind::Dir, true),
            MemberKind::Symlink => (EntryKind::Symlink, false),
            MemberKind::Other => (EntryKind::Other, false),
        };
        Entry {
            name: name.to_string(),
            kind,
            is_dir,
            size: if is_dir { 0 } else { size },
            modified,
            created: None,
            hidden: name.starts_with('.'),
            readonly: self.readonly,
        }
    }

    /// Make sure the folder `key` exists, creating implied ancestors.
    fn ensure_dir(&mut self, key: &str) {
        if key.is_empty() || self.nodes.contains_key(key) {
            return;
        }
        let (parent, name) = split_parent(key);
        self.ensure_dir(parent);
        let entry = self.entry(name, MemberKind::Dir, 0, None);
        self.nodes.insert(key.to_string(), Node { entry, member: None, children: Vec::new() });
        if let Some(p) = self.nodes.get_mut(parent) {
            p.children.push(name.to_string());
        }
    }

    /// Add a member. Unsafe names are dropped (they can't be shown or
    /// extracted safely). A later duplicate replaces an earlier one, which
    /// is how tar appends updates.
    pub fn insert(&mut self, h: &MemberHeader) {
        let Some(key) = sanitize(&h.raw_name) else { return };
        let (parent, name) = split_parent(&key);
        let (parent, name) = (parent.to_string(), name.to_string());
        self.ensure_dir(&parent);
        let entry = self.entry(&name, h.kind, h.size, h.modified);
        let member = Some(h.ordinal);
        match self.nodes.get_mut(&key) {
            Some(node) => {
                // An explicit folder entry after its implied creation just
                // brings its timestamp; anything else replaces the node.
                if node.entry.is_dir && entry.is_dir {
                    if entry.modified.is_some() {
                        node.entry.modified = entry.modified;
                    }
                } else {
                    node.entry = entry;
                }
                node.member = member;
            }
            None => {
                self.nodes.insert(key, Node { entry, member, children: Vec::new() });
                if let Some(p) = self.nodes.get_mut(&parent) {
                    p.children.push(name);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(name: &str, kind: MemberKind, ordinal: usize) -> MemberHeader {
        MemberHeader { raw_name: name.into(), kind, size: 3, modified: Some(1000), ordinal }
    }

    #[test]
    fn sanitize_rejects_escapes() {
        assert_eq!(sanitize("a/./b//c.txt").as_deref(), Some("a/b/c.txt"));
        assert_eq!(sanitize("dir/").as_deref(), Some("dir"));
        assert_eq!(sanitize(r"win\path.txt").as_deref(), Some("win/path.txt"));
        assert_eq!(sanitize("../evil"), None);
        assert_eq!(sanitize("a/../../evil"), None);
        assert_eq!(sanitize("/etc/passwd"), None);
        assert_eq!(sanitize(r"C:\x"), None);
        assert_eq!(sanitize("./"), None);
    }

    #[test]
    fn implied_dirs_are_synthesised() {
        let root = Entry { name: "x.zip".into(), kind: EntryKind::Dir, is_dir: true, size: 0, modified: None, created: None, hidden: false, readonly: true };
        let mut idx = ArchiveIndex::new(root, true);
        idx.insert(&h("a/b/c.txt", MemberKind::File, 0));
        idx.insert(&h("a/", MemberKind::Dir, 1));
        idx.insert(&h("../x", MemberKind::File, 2));
        let root: Vec<_> = idx.children("").unwrap().map(|e| e.name.clone()).collect();
        assert_eq!(root, vec!["a"]);
        assert!(idx.get("a/b").unwrap().entry.is_dir);
        assert_eq!(idx.get("a").unwrap().entry.modified, Some(1000));
        assert_eq!(idx.get("a/b").unwrap().entry.modified, None);
        assert_eq!(idx.get("a/b/c.txt").unwrap().member, Some(0));
    }
}
