use crate::{CxError, Result};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};
use url::Url;

/// Where something lives. Only local paths exist in Phase 0; remote schemes
/// (sftp, smb, ftp, webdav, peer) become further variants.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    Local(PathBuf),
}

/// One breadcrumb segment.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Crumb {
    pub label: String,
    pub uri: String,
    /// Icon hint for the UI: "home", "drive" or "folder".
    pub icon: &'static str,
}

/// Everything the UI needs to render a location's chrome.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocationInfo {
    pub uri: String,
    pub scheme: &'static str,
    /// Human-readable path shown when the address bar is in edit mode.
    pub display: String,
    /// Title for the tab.
    pub name: String,
    pub parent: Option<String>,
    pub crumbs: Vec<Crumb>,
}

impl Location {
    /// Accepts `file://` URIs, absolute paths and `~`-prefixed paths.
    pub fn parse(input: &str) -> Result<Location> {
        let input = input.trim();
        if input.is_empty() {
            return Err(CxError::InvalidLocation("empty location".into()));
        }
        if let Some(scheme_end) = input.find("://") {
            let scheme = &input[..scheme_end];
            if scheme.eq_ignore_ascii_case("file") {
                let url = Url::parse(input).map_err(|e| CxError::InvalidLocation(format!("{input}: {e}")))?;
                let path = url
                    .to_file_path()
                    .map_err(|_| CxError::InvalidLocation(input.to_string()))?;
                return Ok(Location::Local(normalize(&path)));
            }
            return Err(CxError::Unsupported(format!("{scheme}:// locations are not supported yet")));
        }
        let path = if input == "~" || input.starts_with("~/") || input.starts_with("~\\") {
            let home = dirs::home_dir().ok_or_else(|| CxError::InvalidLocation("no home directory".into()))?;
            home.join(input[1..].trim_start_matches(['/', '\\']))
        } else {
            PathBuf::from(input)
        };
        if !path.is_absolute() {
            return Err(CxError::InvalidLocation(format!("{input} is not an absolute path")));
        }
        Ok(Location::Local(normalize(&path)))
    }

    pub fn local(path: impl Into<PathBuf>) -> Location {
        Location::Local(path.into())
    }

    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Location::Local(p) => Some(p),
        }
    }

    pub fn uri(&self) -> String {
        match self {
            Location::Local(p) => path_uri(p),
        }
    }

    pub fn join(&self, name: &str) -> Location {
        match self {
            Location::Local(p) => Location::Local(p.join(name)),
        }
    }

    pub fn parent(&self) -> Option<Location> {
        match self {
            Location::Local(p) => p.parent().map(|p| Location::Local(p.to_path_buf())),
        }
    }

    pub fn info(&self) -> LocationInfo {
        match self {
            Location::Local(p) => local_info(p),
        }
    }
}

fn path_uri(p: &Path) -> String {
    Url::from_file_path(p)
        .map(String::from)
        .unwrap_or_else(|_| format!("file://{}", p.to_string_lossy()))
}

/// Resolve `.` and `..` lexically so crumbs and parents stay tidy.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn local_info(path: &Path) -> LocationInfo {
    let home = dirs::home_dir();
    let mut crumbs = Vec::new();

    // Paths inside the home folder start at the home crumb, like Finder and
    // Explorer do, instead of spelling out /Users/<name>.
    let rel_start = match &home {
        Some(h) if path.starts_with(h) => {
            crumbs.push(Crumb { label: file_label(h), uri: path_uri(h), icon: "home" });
            Some(h.clone())
        }
        _ => None,
    };

    let mut acc = match &rel_start {
        Some(h) => h.clone(),
        None => PathBuf::new(),
    };
    let rest = match &rel_start {
        Some(h) => path.strip_prefix(h).unwrap_or(path).to_path_buf(),
        None => path.to_path_buf(),
    };
    for c in rest.components() {
        acc.push(c.as_os_str());
        match c {
            Component::Prefix(_) => {}
            Component::RootDir => {
                crumbs.push(Crumb { label: root_label(&acc), uri: path_uri(&acc), icon: "drive" });
            }
            Component::Normal(name) => {
                crumbs.push(Crumb { label: name.to_string_lossy().into_owned(), uri: path_uri(&acc), icon: "folder" });
            }
            _ => {}
        }
    }

    LocationInfo {
        uri: path_uri(path),
        scheme: "file",
        display: path.to_string_lossy().into_owned(),
        name: crumbs.last().map(|c| c.label.clone()).unwrap_or_else(|| path.to_string_lossy().into_owned()),
        parent: path.parent().map(path_uri),
        crumbs,
    }
}

fn file_label(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root_label(p))
}

#[cfg(windows)]
fn root_label(root: &Path) -> String {
    let s = root.to_string_lossy();
    let drive = s.trim_end_matches(['\\', '/']);
    format!("Local Disk ({drive})")
}

#[cfg(target_os = "macos")]
fn root_label(_root: &Path) -> String {
    "Macintosh HD".into()
}

#[cfg(not(any(windows, target_os = "macos")))]
fn root_label(_root: &Path) -> String {
    "File System".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uri_and_path_to_same_location() {
        let tmp = std::env::temp_dir();
        let from_path = Location::parse(tmp.to_str().unwrap()).unwrap();
        let from_uri = Location::parse(&from_path.uri()).unwrap();
        assert_eq!(from_path, from_uri);
    }

    #[test]
    fn rejects_relative_and_unknown_schemes() {
        assert!(Location::parse("relative/dir").is_err());
        assert!(matches!(Location::parse("gopher://x/y"), Err(CxError::Unsupported(_))));
    }

    #[test]
    fn crumbs_end_with_current_folder() {
        let home = dirs::home_dir().unwrap();
        let loc = Location::local(home.join("Some Folder"));
        let info = loc.info();
        assert_eq!(info.crumbs.first().unwrap().icon, "home");
        assert_eq!(info.crumbs.last().unwrap().label, "Some Folder");
        assert_eq!(info.name, "Some Folder");
    }

    #[test]
    fn tilde_expands_to_home() {
        let loc = Location::parse("~/Downloads").unwrap();
        assert_eq!(loc.local_path().unwrap(), dirs::home_dir().unwrap().join("Downloads"));
    }
}
