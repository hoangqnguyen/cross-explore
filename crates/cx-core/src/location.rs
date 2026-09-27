use crate::{CxError, Result};
use percent_encoding::percent_decode_str;
use serde::Serialize;
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;
use url::Url;

static HOME: OnceLock<PathBuf> = OnceLock::new();

/// Use `path` as the home folder. Phones have no Unix home (Android) or a
/// sandbox (iOS); the app points this at its own data folder there.
pub fn set_home(path: PathBuf) {
    let _ = HOME.set(path);
}

/// The home folder: the override if set, otherwise the user's home.
pub fn home_dir() -> Option<PathBuf> {
    HOME.get().cloned().or_else(dirs::home_dir)
}

/// Remote protocols. Each maps to a URI scheme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Sftp,
    Ftp,
    Ftps,
    Smb,
    /// WebDAV over http.
    Dav,
    /// WebDAV over https.
    Davs,
    /// Another Cross Explore instance (peer mode).
    Peer,
    /// S3-compatible object storage (AWS, MinIO, R2, B2, Wasabi…). The host
    /// is the service endpoint and the first path segment the bucket.
    S3,
    /// Google Drive (`gdrive://me@gmail.com/My Drive/…`); served by cx-cloud.
    GDrive,
    /// Dropbox (`dropbox://me@example.com/…`); served by cx-cloud.
    Dropbox,
    /// OneDrive personal/business via Microsoft Graph; served by cx-cloud.
    OneDrive,
}

impl Scheme {
    pub fn as_str(self) -> &'static str {
        match self {
            Scheme::Sftp => "sftp",
            Scheme::Ftp => "ftp",
            Scheme::Ftps => "ftps",
            Scheme::Smb => "smb",
            Scheme::Dav => "dav",
            Scheme::Davs => "davs",
            Scheme::Peer => "peer",
            Scheme::S3 => "s3",
            Scheme::GDrive => "gdrive",
            Scheme::Dropbox => "dropbox",
            Scheme::OneDrive => "onedrive",
        }
    }

    pub fn parse(s: &str) -> Option<Scheme> {
        Some(match s.to_ascii_lowercase().as_str() {
            "sftp" | "ssh" => Scheme::Sftp,
            "ftp" => Scheme::Ftp,
            "ftps" | "ftpes" => Scheme::Ftps,
            "smb" | "cifs" => Scheme::Smb,
            "dav" | "webdav" | "http" => Scheme::Dav,
            "davs" | "webdavs" | "https" => Scheme::Davs,
            "peer" | "cx" => Scheme::Peer,
            "s3" => Scheme::S3,
            "gdrive" | "googledrive" => Scheme::GDrive,
            "dropbox" => Scheme::Dropbox,
            "onedrive" => Scheme::OneDrive,
            _ => return None,
        })
    }

    pub fn default_port(self) -> u16 {
        match self {
            Scheme::Sftp => 22,
            Scheme::Ftp | Scheme::Ftps => 21,
            Scheme::Smb => 445,
            Scheme::Dav => 80,
            Scheme::Davs => 443,
            Scheme::Peer => 47470,
            Scheme::S3 | Scheme::GDrive | Scheme::Dropbox | Scheme::OneDrive => 443,
        }
    }

    /// Google Drive, Dropbox or OneDrive (signed in through the browser).
    pub fn is_cloud(self) -> bool {
        matches!(self, Scheme::GDrive | Scheme::Dropbox | Scheme::OneDrive)
    }

    pub fn label(self) -> &'static str {
        match self {
            Scheme::Sftp => "SFTP",
            Scheme::Ftp => "FTP",
            Scheme::Ftps => "FTPS",
            Scheme::Smb => "SMB",
            Scheme::Dav | Scheme::Davs => "WebDAV",
            Scheme::Peer => "Cross Explore",
            Scheme::S3 => "S3",
            Scheme::GDrive => "Google Drive",
            Scheme::Dropbox => "Dropbox",
            Scheme::OneDrive => "OneDrive",
        }
    }
}

impl fmt::Display for Scheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A remote server: one connection is kept per endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Endpoint {
    pub scheme: Scheme,
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl Endpoint {
    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or(self.scheme.default_port())
    }

    /// `scheme://[user@]host[:port]` with no path.
    pub fn uri(&self) -> String {
        let mut s = format!("{}://", self.scheme);
        if let Some(u) = &self.user {
            s.push_str(&percent_encoding::utf8_percent_encode(u, USERINFO).to_string());
            s.push('@');
        }
        if self.host.contains(':') && !self.host.starts_with('[') {
            s.push_str(&format!("[{}]", self.host));
        } else {
            s.push_str(&self.host);
        }
        if let Some(p) = self.port {
            s.push_str(&format!(":{p}"));
        }
        s
    }

    /// The same server without a user, for credential lookups by host.
    pub fn without_user(&self) -> Endpoint {
        Endpoint { user: None, ..self.clone() }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.uri())
    }
}

const USERINFO: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');
const PATH_SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'/')
    .add(b'!')
    .add(b'\\');

/// Where something lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Location {
    Local(PathBuf),
    /// `path` is POSIX-style and always absolute ("/" is the server root; for
    /// SMB the first segment is the share name).
    Remote { endpoint: Endpoint, path: String },
    /// A path inside an archive file. `inner` is POSIX-style, "" or "/" for
    /// the archive root.
    Archive { container: Box<Location>, inner: String },
}

/// One breadcrumb segment.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Crumb {
    pub label: String,
    pub uri: String,
    /// Icon hint for the UI: "home", "drive", "server", "cloud", "share", "archive" or "folder".
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
    /// True for local folders (on this machine).
    pub local: bool,
}

impl Location {
    /// Accepts URIs (`file://`, `sftp://`, `smb://`, `ftp://`, `dav://`,
    /// `peer://`, `archive://…!/…`), absolute paths, `~`-prefixed paths and
    /// UNC paths (`\\host\share` → smb).
    pub fn parse(input: &str) -> Result<Location> {
        let input = input.trim();
        if input.is_empty() {
            return Err(CxError::InvalidLocation("empty location".into()));
        }
        if let Some(rest) = input.strip_prefix("archive://") {
            let split = rest.rfind("!/").or_else(|| rest.strip_suffix('!').map(|r| r.len()));
            let Some(i) = split else {
                return Ok(Location::Archive { container: Box::new(Location::parse(rest)?), inner: "/".into() });
            };
            let container = Location::parse(&rest[..i])?;
            let inner = decode(rest.get(i + 1..).unwrap_or(""));
            return Ok(Location::Archive { container: Box::new(container), inner: normalize_posix(&inner) });
        }
        if let Some(scheme_end) = input.find("://") {
            let scheme = &input[..scheme_end];
            if scheme.eq_ignore_ascii_case("file") {
                let url = Url::parse(input).map_err(|e| CxError::InvalidLocation(format!("{input}: {e}")))?;
                let path = url.to_file_path().map_err(|_| CxError::InvalidLocation(input.to_string()))?;
                return Ok(Location::Local(normalize(&path)));
            }
            let Some(scheme) = Scheme::parse(scheme) else {
                return Err(CxError::Unsupported(format!("{scheme}:// locations are not supported")));
            };
            let url = Url::parse(input).map_err(|e| CxError::InvalidLocation(format!("{input}: {e}")))?;
            let host = url.host_str().filter(|h| !h.is_empty()).ok_or_else(|| CxError::InvalidLocation(format!("{input}: missing host")))?;
            let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
            let user = (!url.username().is_empty()).then(|| decode(url.username()));
            let endpoint = Endpoint { scheme, user, host, port: url.port() };
            return Ok(Location::Remote { endpoint, path: normalize_posix(&decode(url.path())) });
        }
        // \\host\share\dir → smb://host/share/dir
        if let Some(unc) = input.strip_prefix("\\\\") {
            let mut parts = unc.split(['\\', '/']).filter(|s| !s.is_empty());
            let host = parts.next().ok_or_else(|| CxError::InvalidLocation(input.into()))?.to_string();
            let path = format!("/{}", parts.collect::<Vec<_>>().join("/"));
            return Ok(Location::Remote { endpoint: Endpoint { scheme: Scheme::Smb, user: None, host, port: None }, path: normalize_posix(&path) });
        }
        let path = if input == "~" || input.starts_with("~/") || input.starts_with("~\\") {
            let home = home_dir().ok_or_else(|| CxError::InvalidLocation("no home directory".into()))?;
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

    pub fn remote(endpoint: Endpoint, path: impl Into<String>) -> Location {
        Location::Remote { endpoint, path: normalize_posix(&path.into()) }
    }

    pub fn local_path(&self) -> Option<&Path> {
        match self {
            Location::Local(p) => Some(p),
            _ => None,
        }
    }

    pub fn endpoint(&self) -> Option<&Endpoint> {
        match self {
            Location::Remote { endpoint, .. } => Some(endpoint),
            _ => None,
        }
    }

    /// The POSIX path for remote and archive locations.
    pub fn posix_path(&self) -> Option<&str> {
        match self {
            Location::Remote { path, .. } => Some(path),
            Location::Archive { inner, .. } => Some(inner),
            Location::Local(_) => None,
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Location::Local(_))
    }

    /// Same provider instance serves both (same machine, server or archive).
    pub fn same_provider(&self, other: &Location) -> bool {
        match (self, other) {
            (Location::Local(_), Location::Local(_)) => true,
            (Location::Remote { endpoint: a, .. }, Location::Remote { endpoint: b, .. }) => a == b,
            (Location::Archive { container: a, .. }, Location::Archive { container: b, .. }) => a == b,
            _ => false,
        }
    }

    pub fn uri(&self) -> String {
        match self {
            Location::Local(p) => path_uri(p),
            Location::Remote { endpoint, path } => format!("{}{}", endpoint.uri(), encode_path(path)),
            Location::Archive { container, inner } => {
                let inner = if inner.is_empty() { "/".to_string() } else { encode_path(inner) };
                format!("archive://{}!{}", container.uri(), inner)
            }
        }
    }

    pub fn join(&self, name: &str) -> Location {
        match self {
            Location::Local(p) => Location::Local(p.join(name)),
            Location::Remote { endpoint, path } => Location::Remote { endpoint: endpoint.clone(), path: join_posix(path, name) },
            Location::Archive { container, inner } => Location::Archive { container: container.clone(), inner: join_posix(inner, name) },
        }
    }

    pub fn parent(&self) -> Option<Location> {
        match self {
            Location::Local(p) => p.parent().map(|p| Location::Local(p.to_path_buf())),
            Location::Remote { endpoint, path } => parent_posix(path).map(|p| Location::Remote { endpoint: endpoint.clone(), path: p }),
            Location::Archive { container, inner } => match parent_posix(inner) {
                Some(p) => Some(Location::Archive { container: container.clone(), inner: p }),
                // The archive root's parent is the folder holding the archive.
                None => container.parent(),
            },
        }
    }

    /// Last path component ("" for a root).
    pub fn name(&self) -> String {
        match self {
            Location::Local(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            Location::Remote { path, .. } => path.rsplit('/').next().unwrap_or("").to_string(),
            Location::Archive { container, inner } => {
                let n = inner.trim_end_matches('/').rsplit('/').next().unwrap_or("");
                if n.is_empty() { container.name() } else { n.to_string() }
            }
        }
    }

    pub fn info(&self) -> LocationInfo {
        match self {
            Location::Local(p) => local_info(p),
            Location::Remote { endpoint, path } => remote_info(endpoint, path),
            Location::Archive { container, inner } => archive_info(container, inner),
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.uri())
    }
}

fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn encode_path(path: &str) -> String {
    let mut out = String::new();
    for seg in path.split('/').skip(1) {
        out.push('/');
        out.push_str(&percent_encoding::utf8_percent_encode(seg, PATH_SEGMENT).to_string());
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// Absolute, no trailing slash (except the root), `.`/`..` resolved.
pub fn normalize_posix(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

pub fn join_posix(base: &str, name: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}/{name}")
}

fn parent_posix(path: &str) -> Option<String> {
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return None;
    }
    let i = path.rfind('/')?;
    Some(if i == 0 { "/".into() } else { path[..i].to_string() })
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
    let home = home_dir();
    let mut crumbs = Vec::new();

    // Paths inside the home folder start at the home crumb, like Finder and
    // Explorer do, instead of spelling out /Users/<name>.
    let rel_start = match &home {
        Some(h) if path.starts_with(h) => {
            // Phone apps live in a sandbox whose folder name is a UUID.
            let label = if cfg!(any(target_os = "ios", target_os = "android")) { "On this device".to_string() } else { file_label(h) };
            crumbs.push(Crumb { label, uri: path_uri(h), icon: "home" });
            Some(h.clone())
        }
        _ => None,
    };

    let mut acc = rel_start.clone().unwrap_or_default();
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
        local: true,
    }
}

fn remote_info(endpoint: &Endpoint, path: &str) -> LocationInfo {
    let host_label = match &endpoint.user {
        Some(u) if endpoint.scheme != Scheme::Peer => format!("{u}@{}", endpoint.host),
        _ => endpoint.host.clone(),
    };
    let cloud = endpoint.scheme.is_cloud();
    let mut crumbs = vec![Crumb { label: host_label.clone(), uri: Location::remote(endpoint.clone(), "/").uri(), icon: if cloud { "cloud" } else { "server" } }];
    let mut acc = String::new();
    for (i, seg) in path.split('/').filter(|s| !s.is_empty()).enumerate() {
        acc = join_posix(&acc, seg);
        let icon = if i == 0 && matches!(endpoint.scheme, Scheme::Smb | Scheme::S3 | Scheme::GDrive) { "share" } else { "folder" };
        crumbs.push(Crumb { label: seg.to_string(), uri: Location::remote(endpoint.clone(), acc.clone()).uri(), icon });
    }
    LocationInfo {
        uri: Location::remote(endpoint.clone(), path).uri(),
        scheme: endpoint.scheme.as_str(),
        display: Location::remote(endpoint.clone(), path).uri(),
        name: crumbs.last().map(|c| c.label.clone()).unwrap_or(host_label),
        parent: parent_posix(path).map(|p| Location::remote(endpoint.clone(), p).uri()),
        crumbs,
        local: false,
    }
}

fn archive_info(container: &Location, inner: &str) -> LocationInfo {
    let outer = container.info();
    let mut crumbs = outer.crumbs.clone();
    if let Some(last) = crumbs.last_mut() {
        last.icon = "archive";
        last.uri = Location::Archive { container: Box::new(container.clone()), inner: "/".into() }.uri();
    }
    let mut acc = String::new();
    for seg in inner.split('/').filter(|s| !s.is_empty()) {
        acc = join_posix(&acc, seg);
        crumbs.push(Crumb {
            label: seg.to_string(),
            uri: Location::Archive { container: Box::new(container.clone()), inner: acc.clone() }.uri(),
            icon: "folder",
        });
    }
    let me = Location::Archive { container: Box::new(container.clone()), inner: inner.to_string() };
    LocationInfo {
        uri: me.uri(),
        scheme: "archive",
        display: format!("{}{}", outer.display, if inner.is_empty() || inner == "/" { String::new() } else { inner.to_string() }),
        name: crumbs.last().map(|c| c.label.clone()).unwrap_or_default(),
        parent: me.parent().map(|p| p.uri()),
        crumbs,
        local: container.is_local(),
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
        let info = Location::local(home.join("Some Folder")).info();
        assert_eq!(info.crumbs.first().unwrap().icon, "home");
        assert_eq!(info.crumbs.last().unwrap().label, "Some Folder");
        assert_eq!(info.name, "Some Folder");
    }

    #[test]
    fn tilde_expands_to_home() {
        let loc = Location::parse("~/Downloads").unwrap();
        assert_eq!(loc.local_path().unwrap(), dirs::home_dir().unwrap().join("Downloads"));
    }

    #[test]
    fn remote_round_trip_with_odd_names() {
        let loc = Location::parse("sftp://pi@nas.local:2222/home/pi/My Files/a#b?.txt").ok();
        // '#' and '?' start fragment/query in a raw URI, so build it instead.
        assert!(loc.is_some());
        let ep = Endpoint { scheme: Scheme::Sftp, user: Some("pi".into()), host: "nas.local".into(), port: Some(2222) };
        let loc = Location::remote(ep.clone(), "/home/pi/My Files/a#b?.txt");
        let uri = loc.uri();
        assert_eq!(Location::parse(&uri).unwrap(), loc, "{uri}");
        assert_eq!(loc.name(), "a#b?.txt");
        assert_eq!(loc.parent().unwrap().posix_path(), Some("/home/pi/My Files"));
    }

    #[test]
    fn remote_info_and_parents() {
        let loc = Location::parse("smb://nas/Media/Movies").unwrap();
        let info = loc.info();
        assert_eq!(info.crumbs.len(), 3);
        assert_eq!(info.crumbs[0].icon, "server");
        assert_eq!(info.crumbs[1].icon, "share");
        assert_eq!(info.name, "Movies");
        let root = Location::parse("smb://nas/").unwrap();
        assert_eq!(root.parent(), None);
        assert_eq!(root.posix_path(), Some("/"));
    }

    #[test]
    fn unc_paths_become_smb() {
        let loc = Location::parse(r"\\nas\Media\Movies").unwrap();
        assert_eq!(loc.uri(), "smb://nas/Media/Movies");
    }

    #[test]
    fn archive_locations() {
        let tmp = std::env::temp_dir().join("x y.zip");
        let container = Location::local(&tmp);
        let loc = Location::Archive { container: Box::new(container.clone()), inner: "/docs/a b".into() };
        let uri = loc.uri();
        assert_eq!(Location::parse(&uri).unwrap(), loc, "{uri}");
        assert_eq!(loc.parent().unwrap(), Location::Archive { container: Box::new(container.clone()), inner: "/docs".into() });
        let root = Location::Archive { container: Box::new(container.clone()), inner: "/".into() };
        assert_eq!(root.parent().unwrap(), container.parent().unwrap());
        assert_eq!(root.info().crumbs.last().unwrap().icon, "archive");
        let parsed_root = Location::parse(&format!("archive://{}", container.uri())).unwrap();
        assert!(matches!(parsed_root, Location::Archive { .. }));
    }
}
