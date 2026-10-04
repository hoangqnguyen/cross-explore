//! Move to the system trash in a way that can be undone.
//!
//! On macOS `NSFileManager.trashItemAtURL:resultingItemURL:` tells us where
//! each item landed, so undo is a plain move back. Elsewhere the `trash`
//! crate's platform listing finds the item by its original path.

use cx_core::{CxError, Location, Result, TrashedItem};
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
fn trash_one(path: &Path) -> Result<Option<PathBuf>> {
    use objc2::rc::Retained;
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let fm = NSFileManager::defaultManager();
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let mut result: Option<Retained<NSURL>> = None;
    match fm.trashItemAtURL_resultingItemURL_error(&url, Some(&mut result)) {
        Ok(()) => Ok(result.and_then(|u| u.path()).map(|p| PathBuf::from(p.to_string()))),
        // Items under an iCloud-synced Desktop or Documents go to iCloud
        // Drive's own trash, and the system can refuse an app that may
        // otherwise change the file (seen with Full Disk Access granted).
        // Moving it into the user's Trash ourselves is what `mv` would do.
        Err(e) => move_to_user_trash(path).map(Some).map_err(|_| {
            CxError::Io(format!("move to trash failed: {} ({} {})", e.localizedDescription(), e.domain(), e.code()))
        }),
    }
}

/// Move `path` into ~/.Trash under a free name ("a.txt", "a 2.txt"…).
/// Only within the home volume: elsewhere the item belongs in that volume's trash.
#[cfg(target_os = "macos")]
fn move_to_user_trash(path: &Path) -> std::io::Result<PathBuf> {
    let trash = dirs::home_dir().ok_or(std::io::ErrorKind::NotFound)?.join(".Trash");
    let name = path.file_name().ok_or(std::io::ErrorKind::InvalidInput)?;
    let (stem, ext) = match (path.file_stem(), path.extension()) {
        (Some(stem), Some(ext)) if !path.is_dir() => (stem.to_string_lossy(), format!(".{}", ext.to_string_lossy())),
        _ => (name.to_string_lossy(), String::new()),
    };
    let mut dest = trash.join(name);
    let mut n = 2;
    while dest.symlink_metadata().is_ok() {
        dest = trash.join(format!("{stem} {n}{ext}"));
        n += 1;
    }
    std::fs::rename(path, &dest)?;
    Ok(dest)
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
fn trash_one(path: &Path) -> Result<Option<PathBuf>> {
    trash::delete(path).map_err(|e| CxError::Io(format!("move to trash failed: {e}")))?;
    Ok(None)
}

/// Phones have no system trash for app files.
#[cfg(any(target_os = "ios", target_os = "android"))]
fn trash_one(path: &Path) -> Result<Option<PathBuf>> {
    Err(CxError::Unsupported(format!("moving {} to a trash", path.display())))
}

/// Whether this platform has a restorable trash.
pub const AVAILABLE: bool = !cfg!(any(target_os = "ios", target_os = "android"));

pub fn trash_paths(paths: &[PathBuf]) -> Result<Vec<TrashedItem>> {
    paths
        .iter()
        .map(|p| {
            let trashed = trash_one(p)?;
            Ok(TrashedItem { original: Location::local(p).uri(), trashed: trashed.map(|t| Location::local(t).uri()) })
        })
        .collect()
}

/// Put trashed items back where they were.
pub fn restore(items: &[TrashedItem]) -> Result<()> {
    let mut by_listing = Vec::new();
    for item in items {
        let original = Location::parse(&item.original)?;
        let original = original.local_path().ok_or_else(|| CxError::Unsupported("restoring remote items".into()))?.to_path_buf();
        if original.exists() {
            return Err(CxError::AlreadyExists(original.display().to_string()));
        }
        match &item.trashed {
            Some(t) => {
                let from = Location::parse(t)?;
                let from = from.local_path().ok_or_else(|| CxError::InvalidLocation(t.clone()))?;
                std::fs::rename(from, &original).map_err(|e| CxError::from_io(e, from.display()))?;
            }
            None => by_listing.push(original),
        }
    }
    if !by_listing.is_empty() {
        restore_by_listing(&by_listing)?;
    }
    Ok(())
}

#[cfg(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))))]
fn restore_by_listing(paths: &[PathBuf]) -> Result<()> {
    use trash::os_limited;
    let all = os_limited::list().map_err(|e| CxError::Io(format!("reading the trash failed: {e}")))?;
    let mut pick = Vec::new();
    for p in paths {
        let found = all
            .iter()
            .filter(|i| i.original_path() == *p)
            .max_by_key(|i| i.time_deleted)
            .ok_or_else(|| CxError::NotFound(format!("{} in the trash", p.display())))?;
        pick.push(found.clone());
    }
    os_limited::restore_all(pick).map_err(|e| CxError::Io(format!("restore failed: {e}")))
}

#[cfg(not(any(windows, all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))))]
fn restore_by_listing(paths: &[PathBuf]) -> Result<()> {
    Err(CxError::Unsupported(format!("restoring {} from the trash", paths[0].display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Touches the real trash, so it only runs when asked: cargo test -- --ignored
    #[test]
    #[ignore]
    fn trash_and_restore_round_trip() {
        let dir = tempfile::tempdir_in(dirs::home_dir().unwrap()).unwrap();
        let f = dir.path().join("cx-trash-test.txt");
        std::fs::write(&f, b"hello").unwrap();
        let items = trash_paths(std::slice::from_ref(&f)).unwrap();
        assert!(!f.exists());
        restore(&items).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"hello");
    }

    // The fallback when the system refuses: ~/.Trash under a free name, undoable.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn fallback_trash_picks_a_free_name_and_restores() {
        let dir = tempfile::tempdir_in(dirs::home_dir().unwrap()).unwrap();
        let a = dir.path().join("cx fallback.txt");
        let b = dir.path().join("b").join("cx fallback.txt");
        std::fs::create_dir(b.parent().unwrap()).unwrap();
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&b, b"two").unwrap();
        let ta = move_to_user_trash(&a).unwrap();
        let tb = move_to_user_trash(&b).unwrap();
        assert_ne!(ta, tb);
        assert!(tb.file_name().unwrap().to_string_lossy().starts_with("cx fallback "), "{tb:?}");
        let items = [(&a, ta), (&b, tb)].map(|(o, t)| TrashedItem { original: Location::local(o).uri(), trashed: Some(Location::local(t).uri()) });
        restore(&items).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        assert_eq!(std::fs::read(&b).unwrap(), b"two");
    }
}
