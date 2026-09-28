//! Shared folders and the mapping from peer paths ("/<Share>/a/b") to real
//! paths on this machine.
//!
//! This is the security boundary of peer mode, so it is deliberately strict:
//! - `..` segments are rejected outright (never "normalized away");
//! - every segment must be a valid single file name on this OS;
//! - the parent of the target is canonicalized (resolving every symlink on
//!   the way) and must stay inside the canonical share root;
//! - if the target itself is a symlink, its resolved target must also stay
//!   inside the root, and dangling links are refused (writing through one
//!   would create a file wherever it points).
//!
//! Operations then act on `canonical parent + name`, so a remove or rename
//! affects the link itself while reads and writes follow it (inside the root).

use crate::fsutil::write_atomic;
use cx_core::{validate_name, CxError, Entry, EntryKind, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE: &str = "shares.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    /// First path segment peers see ("/<name>/...").
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub read_only: bool,
}

/// A share checked and ready to serve.
#[derive(Debug, Clone)]
pub struct ShareRoot {
    pub share: Share,
    /// Canonical form of `share.path`.
    pub root: PathBuf,
}

/// What a peer path points at.
#[derive(Debug, Clone)]
pub enum Target {
    /// "/": the list of shares.
    Root,
    Path(Resolved),
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub share: ShareRoot,
    /// Real path on this machine (canonical parent + final name).
    pub path: PathBuf,
    /// The peer path as given, normalized ("/Share/a/b").
    pub peer_path: String,
}

impl Resolved {
    pub fn is_share_root(&self) -> bool {
        self.path == self.share.root
    }

    pub fn require_writable(&self) -> Result<()> {
        if self.share.share.read_only {
            return Err(CxError::PermissionDenied(format!("{} is read-only", self.share.share.name)));
        }
        Ok(())
    }

    /// Writes that replace or remove the target also must not hit the share
    /// root itself.
    pub fn require_mutable_entry(&self) -> Result<()> {
        self.require_writable()?;
        if self.is_share_root() {
            return Err(CxError::PermissionDenied(format!("{} is a shared folder", self.peer_path)));
        }
        Ok(())
    }

    /// Replace real paths in error messages with the peer path, so errors
    /// never reveal where a share lives on this machine.
    pub fn scrub(&self, e: CxError) -> CxError {
        scrub(&self.share, e)
    }
}

pub fn scrub(share: &ShareRoot, e: CxError) -> CxError {
    let root = share.root.to_string_lossy().into_owned();
    let orig = share.share.path.to_string_lossy().into_owned();
    let alias = format!("/{}", share.share.name);
    let fix = |s: String| s.replace(&root, &alias).replace(&orig, &alias);
    match e {
        CxError::NotFound(s) => CxError::NotFound(fix(s)),
        CxError::PermissionDenied(s) => CxError::PermissionDenied(fix(s)),
        CxError::AlreadyExists(s) => CxError::AlreadyExists(fix(s)),
        CxError::InvalidLocation(s) => CxError::InvalidLocation(fix(s)),
        CxError::InvalidName(s) => CxError::InvalidName(fix(s)),
        CxError::Unsupported(s) => CxError::Unsupported(fix(s)),
        CxError::Io(s) => CxError::Io(fix(s)),
        other => other,
    }
}

/// Accept what a UI may hand us besides a plain path: a `file://` URI, or
/// the URI's path part on Windows (`/C:/Users/me`).
fn native_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s.starts_with("file://") {
        if let Ok(loc) = cx_core::Location::parse(&s) {
            if let Some(local) = loc.local_path() {
                return local.to_path_buf();
            }
        }
    }
    let b = s.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return PathBuf::from(s[1..].replace('/', std::path::MAIN_SEPARATOR_STR));
    }
    p.to_path_buf()
}

/// Check a share list: unique valid names, existing directories.
pub fn prepare(shares: &[Share]) -> Result<Vec<ShareRoot>> {
    let mut out: Vec<ShareRoot> = Vec::new();
    for s in shares {
        let s = &Share { path: native_path(&s.path), ..s.clone() };
        validate_name(&s.name)?;
        if out.iter().any(|o| o.share.name.eq_ignore_ascii_case(&s.name)) {
            return Err(CxError::AlreadyExists(format!("share {}", s.name)));
        }
        let root = std::fs::canonicalize(&s.path).map_err(|e| CxError::from_io(e, s.path.display()))?;
        if !root.is_dir() {
            return Err(CxError::InvalidLocation(format!("{} is not a folder", s.path.display())));
        }
        out.push(ShareRoot { share: s.clone(), root });
    }
    Ok(out)
}

pub fn load(state_dir: &Path) -> Vec<Share> {
    std::fs::read(state_dir.join(FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(state_dir: &Path, shares: &[Share]) -> Result<()> {
    let json = serde_json::to_vec_pretty(shares).map_err(|e| CxError::io("shares", e))?;
    write_atomic(&state_dir.join(FILE), &json)
}

/// Split a peer path into checked segments.
fn segments(path: &str) -> Result<Vec<&str>> {
    let mut out = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => return Err(CxError::PermissionDenied(format!("{path}: '..' is not allowed"))),
            s => {
                validate_name(s).map_err(|_| CxError::InvalidLocation(format!("{path}: bad path segment")))?;
                out.push(s);
            }
        }
    }
    Ok(out)
}

fn outside(peer: &str) -> CxError {
    CxError::PermissionDenied(format!("{peer} points outside the shared folder"))
}

/// Resolve `path` for a peer allowed to see `visible` shares.
pub fn resolve(visible: &[ShareRoot], path: &str) -> Result<Target> {
    let segs = segments(path)?;
    let Some((first, rest)) = segs.split_first() else { return Ok(Target::Root) };
    let share = visible
        .iter()
        .find(|s| s.share.name == *first)
        .ok_or_else(|| CxError::NotFound(format!("/{first}")))?
        .clone();
    let peer_path = format!("/{}", segs.join("/"));
    let root = &share.root;
    // The root may have been deleted or replaced since it was shared.
    let root_now = std::fs::canonicalize(root).map_err(|_| CxError::NotFound(format!("/{first}")))?;
    if root_now != *root {
        return Err(outside(&peer_path));
    }
    let Some((last, dirs)) = rest.split_last() else {
        return Ok(Target::Path(Resolved { path: root.clone(), share, peer_path }));
    };
    let mut parent = root.clone();
    parent.extend(dirs);
    let parent = std::fs::canonicalize(&parent).map_err(|e| scrub(&share, CxError::from_io(e, &peer_path)))?;
    if !parent.starts_with(root) {
        return Err(outside(&peer_path));
    }
    let full = parent.join(last);
    if let Ok(meta) = std::fs::symlink_metadata(&full) {
        if meta.file_type().is_symlink() {
            match std::fs::canonicalize(&full) {
                Ok(t) if t.starts_with(root) => {}
                _ => return Err(outside(&peer_path)),
            }
        }
    }
    Ok(Target::Path(Resolved { path: full, share, peer_path }))
}

/// Hide details of symlinks that lead out of the share (listings would
/// otherwise reveal whether the outside target is a folder and its size).
pub fn mask_entry(dir: &Path, root: &Path, read_only: bool, mut e: Entry) -> Entry {
    if e.kind == EntryKind::Symlink {
        let inside = std::fs::canonicalize(dir.join(&e.name)).map(|t| t.starts_with(root)).unwrap_or(false);
        if !inside {
            e.is_dir = false;
            e.size = 0;
            e.modified = None;
            e.created = None;
        }
    }
    if read_only {
        e.readonly = true;
    }
    e
}

/// A share as a row in the "/" listing.
pub fn share_entry(s: &ShareRoot) -> Entry {
    let meta = std::fs::metadata(&s.root).ok();
    let modified = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64);
    Entry {
        name: s.share.name.clone(),
        kind: EntryKind::Dir,
        is_dir: true,
        size: 0,
        modified,
        created: None,
        hidden: false,
        readonly: s.share.read_only,
        executable: false,
    }
}

pub fn root_entry() -> Entry {
    Entry { name: String::new(), kind: EntryKind::Dir, is_dir: true, size: 0, modified: None, created: None, hidden: false, readonly: true, executable: false }
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn share_paths_from_uis_are_normalized() {
        let tmp = tempfile::tempdir().unwrap();
        let uri = cx_core::Location::local(tmp.path()).uri();
        let roots = prepare(&[Share { name: "T".into(), path: PathBuf::from(&uri), read_only: false }]).unwrap();
        assert_eq!(std::fs::canonicalize(&roots[0].share.path).unwrap(), std::fs::canonicalize(tmp.path()).unwrap());
        assert_eq!(native_path(Path::new("/C:/Users/PC/Downloads")), PathBuf::from(format!("C:{0}Users{0}PC{0}Downloads", std::path::MAIN_SEPARATOR)));
        assert_eq!(native_path(Path::new("/home/me")), PathBuf::from("/home/me"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Vec<ShareRoot>) {
        let tmp = tempfile::tempdir().unwrap();
        let share = tmp.path().join("share");
        std::fs::create_dir_all(share.join("sub")).unwrap();
        std::fs::write(share.join("sub/a.txt"), b"a").unwrap();
        std::fs::write(tmp.path().join("secret.txt"), b"s").unwrap();
        let roots = prepare(&[Share { name: "S".into(), path: share, read_only: false }]).unwrap();
        (tmp, roots)
    }

    fn resolved(t: Target) -> Resolved {
        match t {
            Target::Path(r) => r,
            Target::Root => panic!("root"),
        }
    }

    #[test]
    fn maps_paths_inside_the_share() {
        let (_tmp, roots) = setup();
        assert!(matches!(resolve(&roots, "/").unwrap(), Target::Root));
        let r = resolved(resolve(&roots, "/S/sub/./a.txt").unwrap());
        assert_eq!(r.path, roots[0].root.join("sub/a.txt"));
        assert_eq!(r.peer_path, "/S/sub/a.txt");
        assert!(resolved(resolve(&roots, "/S").unwrap()).is_share_root());
        // Not-yet-existing names are fine (create), missing parents are not.
        assert!(resolve(&roots, "/S/sub/new.txt").is_ok());
        assert!(matches!(resolve(&roots, "/S/nope/new.txt"), Err(CxError::NotFound(_))));
        assert!(matches!(resolve(&roots, "/Other/x"), Err(CxError::NotFound(_))));
    }

    #[test]
    fn rejects_traversal() {
        let (_tmp, roots) = setup();
        for p in ["/S/../secret.txt", "/S/../../etc", "/S/sub/../../secret.txt", "/../S", "/S/sub/.."] {
            assert!(matches!(resolve(&roots, p), Err(CxError::PermissionDenied(_))), "{p}");
        }
        assert!(resolve(&roots, "/S/a\0b").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_that_escape() {
        let (tmp, roots) = setup();
        let share = &roots[0].root;
        std::os::unix::fs::symlink(tmp.path(), share.join("up")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("secret.txt"), share.join("secret")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("missing"), share.join("dangling")).unwrap();
        std::os::unix::fs::symlink(share.join("sub"), share.join("inner")).unwrap();
        for p in ["/S/up", "/S/up/secret.txt", "/S/secret", "/S/dangling"] {
            assert!(matches!(resolve(&roots, p), Err(CxError::PermissionDenied(_))), "{p}");
        }
        // Links that stay inside (even via a detour outside) are fine.
        assert_eq!(resolved(resolve(&roots, "/S/up/share/sub").unwrap()).path, share.join("sub"));
        let r = resolved(resolve(&roots, "/S/inner/a.txt").unwrap());
        assert_eq!(r.path, share.join("sub/a.txt"));
        // Escaping links show up in listings without details.
        let e = Entry::from_path(&share.join("up")).unwrap();
        let masked = mask_entry(share, share, false, e);
        assert!(!masked.is_dir);
    }

    #[test]
    fn scrubs_real_paths_from_errors() {
        let (_tmp, roots) = setup();
        let e = CxError::from_io(std::io::Error::from(std::io::ErrorKind::NotFound), roots[0].root.join("x").display());
        let CxError::NotFound(msg) = scrub(&roots[0], e) else { panic!() };
        assert_eq!(msg, "/S/x");
    }
}
