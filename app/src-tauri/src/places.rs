//! Sidebar sources: the home folder, standard folders and mounted volumes.

use cx_core::{Location, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use sysinfo::Disks;

/// Whether the window has a native material behind the web view, so the UI
/// knows to leave its chrome see-through.
static TRANSLUCENT: AtomicBool = AtomicBool::new(false);

pub fn set_translucent(on: bool) {
    TRANSLUCENT.store(on, Ordering::Relaxed);
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    name: String,
    uri: String,
    icon: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    name: String,
    uri: String,
    total: u64,
    free: u64,
    removable: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Places {
    platform: &'static str,
    translucent: bool,
    home: Place,
    favorites: Vec<Place>,
    volumes: Vec<Volume>,
}

#[derive(Serialize)]
pub struct Space {
    free: u64,
    total: u64,
}

fn place(path: PathBuf, icon: &'static str) -> Option<Place> {
    path.is_dir().then(|| Place {
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        uri: Location::local(&path).uri(),
        icon,
    })
}

fn user_visible_mount(mount: &Path) -> bool {
    if cfg!(target_os = "macos") {
        mount == Path::new("/") || mount.starts_with("/Volumes")
    } else if cfg!(windows) {
        true
    } else {
        mount == Path::new("/")
            || ["/media", "/run/media", "/mnt"].iter().any(|p| mount.starts_with(p))
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

#[tauri::command]
pub fn places() -> Places {
    let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let favorites = [
        (dirs::desktop_dir(), "desktop"),
        (dirs::document_dir(), "documents"),
        (dirs::download_dir(), "downloads"),
        (dirs::picture_dir(), "pictures"),
        (dirs::audio_dir(), "music"),
        (dirs::video_dir(), "videos"),
    ]
    .into_iter()
    .filter_map(|(p, icon)| p.and_then(|p| place(p, icon)))
    .collect();

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

/// Free space on the volume holding `uri` (the longest matching mount point).
#[tauri::command]
pub fn free_space(uri: String) -> Result<Option<Space>> {
    let loc = Location::parse(&uri)?;
    let Some(path) = loc.local_path() else { return Ok(None) };
    let disks = Disks::new_with_refreshed_list();
    Ok(disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| Space { free: d.available_space(), total: d.total_space() }))
}
