//! Sidebar sources: the home folder, standard folders and mounted volumes.

use cx_core::Location;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use sysinfo::Disks;

/// Whether the window has a native material behind the web view, so the UI
/// knows to leave its chrome see-through.
static TRANSLUCENT: AtomicBool = AtomicBool::new(false);

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
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

/// A folder kept in sync by a cloud app (Google Drive, Dropbox, OneDrive, iCloud…).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudPlace {
    name: String,
    uri: String,
    /// google | dropbox | onedrive | icloud | box | other
    provider: &'static str,
    /// The signed-in account, when the folder name reveals it.
    account: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Places {
    platform: &'static str,
    translucent: bool,
    home: Place,
    favorites: Vec<Place>,
    volumes: Vec<Volume>,
    cloud: Vec<CloudPlace>,
}

fn place(path: PathBuf, icon: &'static str) -> Option<Place> {
    path.is_dir().then(|| Place {
        name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
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
        mount == Path::new("/")
            || ["/media", "/run/media", "/mnt"]
                .iter()
                .any(|p| mount.starts_with(p))
    }
}

fn volume_name(mount: &Path, label: &str) -> String {
    if cfg!(target_os = "macos") && mount == Path::new("/") {
        return "Macintosh HD".into();
    }
    if cfg!(windows) {
        let drive = mount.to_string_lossy().trim_end_matches('\\').to_string();
        let label = if label.is_empty() {
            "Local Disk"
        } else {
            label
        };
        return format!("{label} ({drive})");
    }
    if mount == Path::new("/") {
        return "File System".into();
    }
    mount
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| label.to_string())
}

/// Off the main thread: listing disks stats every mount, and a stale
/// network mount can take seconds to answer.
#[tauri::command]
pub async fn places() -> std::result::Result<Places, String> {
    tauri::async_runtime::spawn_blocking(list_places)
        .await
        .map_err(|e| e.to_string())
}

fn list_places() -> Places {
    let home_dir = cx_core::location::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let phone = cfg!(any(target_os = "ios", target_os = "android"));
    let favorites = [
        (
            if phone {
                Some(home_dir.join("Documents"))
            } else {
                None
            },
            "documents",
        ),
        (
            if phone {
                Some(home_dir.join("Downloads"))
            } else {
                None
            },
            "downloads",
        ),
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
        if !user_visible_mount(mount)
            || volumes
                .iter()
                .any(|v| v.uri == Location::local(mount).uri())
        {
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
            name: home_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Home".into()),
            uri: Location::local(&home_dir).uri(),
            icon: "home",
        },
        favorites,
        volumes,
        cloud: cloud_places(&home_dir),
    }
}

fn provider_of(name: &str) -> Option<(&'static str, &'static str)> {
    let n = name.to_ascii_lowercase();
    // The service name, alone or followed by an account ("Dropbox (Work)", "OneDrive - Contoso").
    let is = |p: &str| {
        n.strip_prefix(p)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '-', '(']))
    };
    Some(if is("googledrive") || is("google drive") {
        ("google", "Google Drive")
    } else if is("dropbox") {
        ("dropbox", "Dropbox")
    } else if is("onedrive") {
        ("onedrive", "OneDrive")
    } else if is("icloud drive") || is("iclouddrive") {
        ("icloud", "iCloud Drive")
    } else if is("box") {
        ("box", "Box")
    } else if is("pclouddrive") {
        ("other", "pCloud")
    } else if is("nextcloud") {
        ("other", "Nextcloud")
    } else if is("mega") {
        ("other", "MEGA")
    } else {
        return None;
    })
}

/// `GoogleDrive-me@example.com` → `me@example.com`; `OneDrive-Personal` → `Personal`.
fn account_of(dir_name: &str) -> Option<String> {
    // Drop the service's own name first ("Google Drive", "iCloud Drive"), so
    // only what follows it can be an account.
    let lower = dir_name.to_ascii_lowercase();
    let service = [
        "googledrive",
        "google drive",
        "icloud drive",
        "iclouddrive",
        "onedrive",
        "dropbox",
        "pclouddrive",
        "nextcloud",
        "mega",
        "box",
    ]
    .into_iter()
    .find(|p| lower.starts_with(p))?;
    let rest = &dir_name[service.len()..];
    let rest = rest
        .trim_start_matches([' ', '-', '('])
        .trim_end_matches(')');
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Sync folders of the common cloud apps. Only looks at well-known locations,
/// so nothing inside those folders is touched (which could download files or
/// trigger a privacy prompt).
fn cloud_places(home: &Path) -> Vec<CloudPlace> {
    type Found = Vec<(PathBuf, &'static str, String, Option<String>)>;
    let mut found: Found = Vec::new();
    fn add(
        found: &mut Found,
        path: PathBuf,
        dir_name: &str,
        fixed: Option<(&'static str, &'static str)>,
    ) {
        let Some((provider, label)) = fixed.or_else(|| provider_of(dir_name)) else {
            return;
        };
        // `~/Google Drive` is often a link to the CloudStorage folder: list it once.
        let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        if !path.is_dir()
            || found.iter().any(|f| {
                f.0 == path
                    || std::fs::canonicalize(&f.0)
                        .map(|r| r == real)
                        .unwrap_or(false)
            })
        {
            return;
        }
        found.push((path, provider, label.to_string(), account_of(dir_name)));
    }

    if cfg!(target_os = "macos") {
        // File Provider apps (Google Drive, Dropbox, OneDrive, Box…) live here.
        if let Ok(rd) = std::fs::read_dir(home.join("Library/CloudStorage")) {
            let mut names: Vec<String> = rd
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            for n in names {
                let path = home.join("Library/CloudStorage").join(&n);
                if provider_of(&n).is_some() {
                    add(&mut found, path, &n, None);
                } else if path.is_dir() && !found.iter().any(|f| f.0 == path) {
                    // Other File Provider domains (phone bridges and the like) show by name.
                    found.push((path, "other", n.replace('-', " "), None));
                }
            }
        }
        add(
            &mut found,
            home.join("Library/Mobile Documents/com~apple~CloudDocs"),
            "iCloud Drive",
            Some(("icloud", "iCloud Drive")),
        );
    }

    if cfg!(windows) {
        for var in ["OneDriveConsumer", "OneDriveCommercial", "OneDrive"] {
            if let Some(p) = std::env::var_os(var).map(PathBuf::from) {
                let n = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                add(&mut found, p, &n, Some(("onedrive", "OneDrive")));
            }
        }
        // Dropbox records its folder(s) in info.json.
        for base in [
            std::env::var_os("LOCALAPPDATA"),
            std::env::var_os("APPDATA"),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(text) = std::fs::read_to_string(PathBuf::from(base).join("Dropbox/info.json"))
            {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    for (kind, acc) in v.as_object().into_iter().flatten() {
                        if let Some(p) = acc.get("path").and_then(|p| p.as_str()) {
                            let n = format!("Dropbox - {kind}");
                            add(
                                &mut found,
                                PathBuf::from(p),
                                &n,
                                Some(("dropbox", "Dropbox")),
                            );
                        }
                    }
                }
            }
        }
        add(
            &mut found,
            home.join("iCloudDrive"),
            "iCloud Drive",
            Some(("icloud", "iCloud Drive")),
        );
    }

    // Plain folders in the home directory (Linux clients, older app versions).
    if let Ok(rd) = std::fs::read_dir(home) {
        let mut names: Vec<String> = rd
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        for n in names {
            if n.starts_with('.')
                || provider_of(&n).is_none()
                || n.eq_ignore_ascii_case("box") && !cfg!(windows)
            {
                continue;
            }
            add(&mut found, home.join(&n), &n, None);
        }
    }

    // Name them so two accounts of one service can be told apart.
    let mut out: Vec<CloudPlace> = Vec::new();
    for (path, provider, label, account) in &found {
        let same = found.iter().filter(|f| f.2 == *label).count() > 1;
        let name = match account {
            Some(a) if same => format!("{label} ({a})"),
            _ => label.clone(),
        };
        out.push(CloudPlace {
            name,
            uri: Location::local(path).uri(),
            provider,
            account: account.clone(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cloud_folders() {
        assert_eq!(
            provider_of("GoogleDrive-me@example.com").unwrap().0,
            "google"
        );
        assert_eq!(
            account_of("GoogleDrive-me@example.com").as_deref(),
            Some("me@example.com")
        );
        assert_eq!(provider_of("OneDrive - Contoso").unwrap().0, "onedrive");
        assert_eq!(account_of("OneDrive - Contoso").as_deref(), Some("Contoso"));
        assert_eq!(provider_of("Dropbox").unwrap().0, "dropbox");
        assert_eq!(account_of("Dropbox"), None);
        assert_eq!(account_of("Google Drive"), None);
        assert_eq!(account_of("Dropbox (Work)").as_deref(), Some("Work"));
        assert_eq!(account_of("OneDrive-Personal").as_deref(), Some("Personal"));
        assert!(provider_of("Documents").is_none());
        assert!(provider_of("MegaProject").is_none() && provider_of("Boxes").is_none());
    }

    #[test]
    fn finds_sync_folders_in_home() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join("Dropbox")).unwrap();
        std::fs::create_dir(home.path().join("Documents")).unwrap();
        if cfg!(target_os = "macos") {
            std::fs::create_dir_all(
                home.path()
                    .join("Library/CloudStorage/GoogleDrive-me@example.com"),
            )
            .unwrap();
            std::fs::create_dir_all(home.path().join("Library/CloudStorage/OneDrive-Personal"))
                .unwrap();
        }
        #[cfg(unix)]
        if cfg!(target_os = "macos") {
            // A home link to the CloudStorage folder is the same place.
            std::os::unix::fs::symlink(
                home.path()
                    .join("Library/CloudStorage/GoogleDrive-me@example.com"),
                home.path().join("Google Drive"),
            )
            .unwrap();
        }
        let got = cloud_places(home.path());
        if cfg!(target_os = "macos") {
            assert_eq!(
                got.iter().filter(|c| c.provider == "google").count(),
                1,
                "{:?}",
                got.iter().map(|c| &c.name).collect::<Vec<_>>()
            );
        }
        let names: Vec<&str> = got.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"Dropbox"), "{names:?}");
        assert!(!names.contains(&"Documents"));
        if cfg!(target_os = "macos") {
            assert!(
                names.contains(&"Google Drive") && names.contains(&"OneDrive"),
                "{names:?}"
            );
            assert_eq!(
                got.iter()
                    .find(|c| c.provider == "google")
                    .unwrap()
                    .account
                    .as_deref(),
                Some("me@example.com")
            );
        }
    }
}
