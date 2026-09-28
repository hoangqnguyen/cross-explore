use serde::{Deserialize, Serialize};
use std::fs::Metadata;
use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// One row in a directory listing. Kept deliberately small: it is sent to the
/// UI for every file in a folder, so large folders mean many of these.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    /// True for directories and for symlinks that point at a directory, so
    /// the UI can treat both as navigable folders.
    pub is_dir: bool,
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub modified: Option<i64>,
    pub created: Option<i64>,
    pub hidden: bool,
    pub readonly: bool,
    /// A program you can run: a file with an execute bit (Unix permissions),
    /// where the provider knows them. Windows programs are told by extension.
    #[serde(default)]
    pub executable: bool,
}

impl Entry {
    /// Stat `path` (without following a final symlink) and build an entry.
    pub fn from_path(path: &Path) -> io::Result<Entry> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let meta = std::fs::symlink_metadata(path)?;
        Ok(Self::from_metadata(name, path, &meta))
    }

    pub fn from_metadata(name: String, path: &Path, meta: &Metadata) -> Entry {
        let ft = meta.file_type();
        let (kind, is_dir, size) = if ft.is_symlink() {
            // Follow the link for dir-ness and size; broken links stay files.
            match std::fs::metadata(path) {
                Ok(target) => (EntryKind::Symlink, target.is_dir(), if target.is_dir() { 0 } else { target.len() }),
                Err(_) => (EntryKind::Symlink, false, 0),
            }
        } else if ft.is_dir() {
            (EntryKind::Dir, true, 0)
        } else if ft.is_file() {
            (EntryKind::File, false, meta.len())
        } else {
            (EntryKind::Other, false, 0)
        };
        Entry {
            hidden: is_hidden(&name, meta),
            readonly: meta.permissions().readonly(),
            executable: is_executable(ft.is_file(), meta),
            modified: meta.modified().ok().and_then(to_millis),
            created: meta.created().ok().and_then(to_millis),
            name,
            kind,
            is_dir,
            size,
        }
    }
}

#[cfg(unix)]
fn is_executable(is_file: bool, meta: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    is_file && meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_is_file: bool, _meta: &Metadata) -> bool {
    false
}

fn to_millis(t: SystemTime) -> Option<i64> {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => Some(d.as_millis() as i64),
        Err(e) => Some(-(e.duration().as_millis() as i64)),
    }
}

#[cfg(target_os = "macos")]
fn is_hidden(name: &str, meta: &Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const UF_HIDDEN: u32 = 0x8000;
    name.starts_with('.') || meta.st_flags() & UF_HIDDEN != 0
}

#[cfg(windows)]
fn is_hidden(name: &str, meta: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    name.starts_with('.') || meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0
}

#[cfg(not(any(target_os = "macos", windows)))]
fn is_hidden(name: &str, _meta: &Metadata) -> bool {
    name.starts_with('.')
}
