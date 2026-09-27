//! Sidebar sources: the home folder, standard folders and mounted volumes.

use cx_core::Location;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use sysinfo::Disks;

/// Whether the window has a native material behind the web view, so the UI
/// knows to leave its chrome see-through. Only the desktop app sets it.
static TRANSLUCENT: AtomicBool = AtomicBool::new(false);

pub fn set_translucent(on: bool) {
    TRANSLUCENT.store(on, Ordering::Relaxed);
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub name: String,
    pub uri: String,
    /// "home", "desktop", "documents", "downloads", "pictures", "music" or "videos".
    pub icon: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub name: String,
    pub uri: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Places {
    pub platform: &'static str,
    pub translucent: bool,
    pub home: Place,
    pub favorites: Vec<Place>,
    pub volumes: Vec<Volume>,
}

fn place(path: PathBuf, icon: &'static str) -> Option<Place> {
    path.is_dir().then(|| Place {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        uri: Location::local(&path).uri(),
        icon,
    })
}

fn user_visible_mount(mount: &Path) -> bool {
    // Phone mounts are system internals the app can't browse anyway.
    if cfg!(any(target_os = "ios", target_os = "android")) {
        return false;
    }
    if cfg!(target_os = "macos") {
        mount == Path::new("/") || mount.starts_with("/Volumes")
    } else if cfg!(windows) {
        true
    } else {
        mount == Path::new("/") || ["/media", "/run/media", "/mnt"].iter().any(|p| mount.starts_with(p))
    }
}

fn volume_name(mount: &Path, label: &str) -> String {
    if cfg!(target_os = "macos") && mount == Path::new("/") {
        return "Macintosh HD".into();
    }
    if cfg!(windows) {
        let drive = mount.to_string_lossy().trim_end_matches('\\').to_string();
        let label = if label.is_empty() { "Local Disk" } else { label };
        return format!("{label} ({drive})");
    }
    if mount == Path::new("/") {
        return "File System".into();
    }
    mount.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| label.to_string())
}

/// Home, the standard user folders that exist, and mounted volumes. Reads
/// the disk list, so call it off the UI thread when that matters.
pub fn places() -> Places {
    let home_dir = cx_core::location::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let phone = cfg!(any(target_os = "ios", target_os = "android"));
    let favorites = [
        (if phone { Some(home_dir.join("Documents")) } else { None }, "documents"),
        (if phone { Some(home_dir.join("Downloads")) } else { None }, "downloads"),
        (dirs::desktop_dir(), "desktop"),
        (dirs::document_dir(), "documents"),
        (dirs::download_dir(), "downloads"),
        (dirs::picture_dir(), "pictures"),
        (dirs::audio_dir(), "music"),
        (dirs::video_dir(), "videos"),
    ]
    .into_iter()
    .filter_map(|(p, icon)| p.and_then(|p| place(p, icon)))
    .fold(Vec::<Place>::new(), |mut v, p| {
        if !v.iter().any(|x| x.uri == p.uri) {
            v.push(p);
        }
        v
    });

    let disks = Disks::new_with_refreshed_list();
    let mut volumes: Vec<Volume> = Vec::new();
    for d in disks.list() {
        let mount = d.mount_point();
        if !user_visible_mount(mount) || volumes.iter().any(|v| v.uri == Location::local(mount).uri()) {
            continue;
        }
        volumes.push(Volume {
            name: volume_name(mount, &d.name().to_string_lossy()),
            uri: Location::local(mount).uri(),
            total: d.total_space(),
            free: d.available_space(),
            removable: d.is_removable() || (cfg!(target_os = "macos") && mount != Path::new("/")),
        });
    }

    Places {
        platform: std::env::consts::OS,
        translucent: TRANSLUCENT.load(Ordering::Relaxed),
        home: Place {
            name: home_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Home".into()),
            uri: Location::local(&home_dir).uri(),
            icon: "home",
        },
        favorites,
        volumes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_is_a_file_uri() {
        let p = places();
        assert!(p.home.uri.starts_with("file://"));
        assert_eq!(p.platform, std::env::consts::OS);
        assert!(p.favorites.iter().all(|f| f.uri.starts_with("file://")));
    }
}
