//! Dialog state. Each dialog is plain data; `app::dialogs` handles their
//! keys and `ui::dialogs` draws them. Dialogs stack (a host-key prompt can
//! open over the connect dialog; a conflict can arrive over anything).

use crate::commands::Action;
use crate::input::TextInput;
use crate::rename::CaseMode;
use crate::sort::SortKey;
use cx_engine::events::IncomingOffer;
use cx_engine::JobConflict;
use cx_transfer::SyncDirection;
use std::collections::BTreeSet;

/// What a menu or palette entry does.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    /// Open a location, in `pane` (default: the active one).
    Navigate { uri: String, pane: Option<usize> },
    Run(Action),
    Sort(SortKey),
    Sync(SyncDirection),
    ActivateTab { pane: usize, id: u64 },
    /// Reopen a saved workspace (its tabs in both panes).
    Workspace(String),
    /// Offer the files picked for "Send to device" to this device.
    SendTo(String),
    /// Connect dialog prefilled with a host.
    ConnectTo { scheme: String, host: String },
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub label: String,
    pub detail: String,
    pub action: MenuAction,
    /// A section header (not selectable).
    pub header: bool,
}

impl MenuItem {
    pub fn new(label: impl Into<String>, detail: impl Into<String>, action: MenuAction) -> MenuItem {
        MenuItem { label: label.into(), detail: detail.into(), action, header: false }
    }

    pub fn header(label: impl Into<String>) -> MenuItem {
        MenuItem { label: label.into(), detail: String::new(), action: MenuAction::None, header: true }
    }
}

#[derive(Debug, Clone)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
    pub cursor: usize,
    /// Type to narrow the list.
    pub filter: String,
}

impl Menu {
    pub fn new(title: impl Into<String>, items: Vec<MenuItem>) -> Menu {
        let mut m = Menu { title: title.into(), items, cursor: 0, filter: String::new() };
        m.cursor = m.visible().into_iter().find(|&i| !m.items[i].header).unwrap_or(0);
        m
    }

    /// Indices of items passing the filter (headers kept when unfiltered).
    pub fn visible(&self) -> Vec<usize> {
        let q = self.filter.to_lowercase();
        (0..self.items.len()).filter(|&i| if q.is_empty() { true } else { !self.items[i].header && (self.items[i].label.to_lowercase().contains(&q) || self.items[i].detail.to_lowercase().contains(&q)) }).collect()
    }

    pub fn step(&mut self, delta: isize) {
        let vis = self.visible();
        let selectable: Vec<usize> = vis.into_iter().filter(|&i| !self.items[i].header).collect();
        if selectable.is_empty() {
            return;
        }
        let pos = selectable.iter().position(|&i| i == self.cursor).unwrap_or(0) as isize;
        let next = (pos + delta).clamp(0, selectable.len() as isize - 1) as usize;
        self.cursor = selectable[next];
    }

    pub fn fix_cursor(&mut self) {
        let vis = self.visible();
        if !vis.contains(&self.cursor) || self.items.get(self.cursor).is_some_and(|i| i.header) {
            self.cursor = vis.into_iter().find(|&i| !self.items[i].header).unwrap_or(0);
        }
    }

    pub fn selected(&self) -> Option<&MenuItem> {
        self.items.get(self.cursor).filter(|i| !i.header && self.visible().contains(&self.cursor))
    }
}

#[derive(Debug, Clone)]
pub struct Palette {
    pub input: TextInput,
    pub items: Vec<MenuItem>,
    /// Ranked indices into `items`, with matched character positions.
    pub results: Vec<(usize, Vec<u32>)>,
    pub cursor: usize,
}

/// What a confirmation runs when accepted.
#[derive(Debug, Clone, PartialEq)]
pub enum Pending {
    Delete(Vec<String>),
    Transfer { kind: &'static str, sources: Vec<String>, dests: Vec<String> },
    Trash(Vec<(String, Vec<String>)>),
    Upload { local: String, dest_dir: String },
    Disconnect(String),
    Quit,
}

#[derive(Debug, Clone)]
pub struct Confirm {
    pub title: String,
    pub body: String,
    pub ok: String,
    pub danger: bool,
    pub then: Pending,
    /// Focus on "Cancel" (the safe choice for dangerous confirmations).
    pub on_cancel: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PromptKind {
    Rename { dir: String, from: String },
    NewFolder { dir: String },
    Compress { sources: Vec<String>, dir: String },
    SelectPattern { select: bool },
    GoTo,
    Filter,
    SaveWorkspace,
}

#[derive(Debug, Clone)]
pub struct Prompt {
    pub title: String,
    pub label: String,
    pub input: TextInput,
    pub ok: String,
    pub kind: PromptKind,
    /// Tab completions offered for path prompts.
    pub completions: Vec<String>,
}

/// What to do once a server accepts us.
#[derive(Debug, Clone, PartialEq)]
pub enum AfterAuth {
    /// Re-list whatever shows this server.
    Reload,
    /// Retry the connect dialog's connection.
    Connect,
    Navigate(String),
}

pub const PROTOCOLS: [(&str, &str, u16); 8] = [
    ("smb", "SMB (Windows / NAS share)", 445),
    ("sftp", "SFTP (SSH)", 22),
    ("ftp", "FTP", 21),
    ("ftps", "FTPS (FTP over TLS)", 21),
    ("davs", "WebDAV (HTTPS)", 443),
    ("dav", "WebDAV (HTTP)", 80),
    ("s3", "S3 / object storage", 443),
    ("peer", "Cross Explore device", 47470),
];

#[derive(Debug, Clone)]
pub struct ConnectForm {
    pub scheme: usize,
    pub host: TextInput,
    pub port: TextInput,
    pub path: TextInput,
    pub user: TextInput,
    pub password: TextInput,
    pub key_file: TextInput,
    pub anonymous: bool,
    pub remember: bool,
    pub save: bool,
    pub focus: usize,
    pub error: Option<String>,
    pub busy: bool,
}

impl ConnectForm {
    pub const FIELDS: usize = 11;

    pub fn new(scheme: &str, host: &str) -> ConnectForm {
        ConnectForm {
            scheme: PROTOCOLS.iter().position(|p| p.0 == scheme).unwrap_or(0),
            host: TextInput::new(host),
            port: TextInput::default(),
            path: TextInput::default(),
            user: TextInput::default(),
            password: TextInput::secret(),
            key_file: TextInput::default(),
            anonymous: false,
            remember: true,
            save: true,
            focus: if host.is_empty() { 1 } else { 4 },
            error: None,
            busy: false,
        }
    }

    pub fn scheme(&self) -> &'static str {
        PROTOCOLS[self.scheme].0
    }

    pub fn is_s3(&self) -> bool {
        self.scheme() == "s3"
    }

    /// Pasting a full URI into the host field fills in everything.
    pub fn absorb_uri(&mut self) {
        let h = self.host.text.trim().to_string();
        let Some((scheme, rest)) = h.split_once("://") else { return };
        let s = match scheme.to_ascii_lowercase().as_str() {
            "webdav" | "http" => "dav".to_string(),
            "webdavs" | "https" => "davs".to_string(),
            "ssh" => "sftp".to_string(),
            other => other.to_string(),
        };
        if let Some(i) = PROTOCOLS.iter().position(|p| p.0 == s) {
            self.scheme = i;
        }
        let (auth, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        let (user, hostport) = match auth.rsplit_once('@') {
            Some((u, hp)) => (Some(u), hp),
            None => (None, auth),
        };
        if let Some(u) = user {
            self.user.set(decode(u));
        }
        let (host, port) = if hostport.starts_with('[') {
            match hostport.find(']') {
                Some(i) => (&hostport[..=i], hostport[i + 1..].strip_prefix(':')),
                None => (hostport, None),
            }
        } else {
            match hostport.rsplit_once(':') {
                Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
                _ => (hostport, None),
            }
        };
        if let Some(p) = port {
            self.port.set(p);
        }
        if !path.is_empty() && path != "/" {
            self.path.set(decode(path));
        }
        self.host.set(host);
    }

    /// The URI the form describes (desktop app's `buildUri`).
    pub fn uri(&self) -> Option<String> {
        let host = self.host.text.trim();
        if host.is_empty() {
            return None;
        }
        Some(build_uri(self.scheme(), host, self.port.text.trim(), if self.anonymous { "" } else { self.user.text.trim() }, self.path.text.trim()))
    }
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() && s.is_char_boundary(i + 3) {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn build_uri(scheme: &str, host: &str, port: &str, user: &str, path: &str) -> String {
    let h = host.trim();
    let h = h.split_once("://").map(|(_, r)| r).unwrap_or(h);
    let h = h.split('/').next().unwrap_or(h);
    let u = if user.is_empty() { String::new() } else { format!("{}@", encode(user)) };
    let def = PROTOCOLS.iter().find(|p| p.0 == scheme).map(|p| p.2);
    let pt = match port.parse::<u16>() {
        Ok(p) if Some(p) != def => format!(":{p}"),
        _ => String::new(),
    };
    let p = path.trim().trim_start_matches('/');
    let p = if p.is_empty() { "/".to_string() } else { format!("/{}", p.split('/').map(encode).collect::<Vec<_>>().join("/")) };
    let h = if h.contains(':') && !h.starts_with('[') { format!("[{h}]") } else { h.to_string() };
    format!("{scheme}://{u}{h}{pt}{p}")
}

#[derive(Debug, Clone)]
pub struct SignIn {
    pub uri: String,
    pub user: TextInput,
    pub password: TextInput,
    pub key_file: TextInput,
    pub remember: bool,
    pub focus: usize,
    pub reason: String,
    pub error: Option<String>,
    pub busy: bool,
    pub then: AfterAuth,
}

#[derive(Debug, Clone)]
pub struct HostKey {
    pub uri: String,
    pub host: String,
    pub key_type: String,
    pub fingerprint: String,
    pub changed: bool,
    pub then: AfterAuth,
    pub on_trust: bool,
}

#[derive(Debug, Clone)]
pub struct ConflictDlg {
    pub job: u64,
    pub conflict: JobConflict,
    pub apply_all: bool,
    pub cursor: usize,
}

pub const CONFLICT_CHOICES: [&str; 4] = ["Replace", "Skip", "Keep both", "Keep newer"];

#[derive(Debug, Clone, PartialEq)]
pub struct DestItem {
    pub label: String,
    pub detail: String,
    pub uri: String,
    pub section: &'static str,
}

#[derive(Debug, Clone)]
pub struct DestPicker {
    pub moving: bool,
    pub sources: Vec<String>,
    pub items: Vec<DestItem>,
    pub checked: BTreeSet<usize>,
    pub cursor: usize,
    /// A typed path or URI (also filters the list).
    pub input: TextInput,
}

impl DestPicker {
    pub fn visible(&self) -> Vec<usize> {
        let q = self.input.text.trim().to_lowercase();
        // A typed location is a destination of its own, not a filter.
        let is_path = q.starts_with('/') || q.starts_with('~') || q.contains("://") || q.starts_with("\\\\") || (q.len() > 1 && q.as_bytes()[1] == b':');
        (0..self.items.len()).filter(|&i| q.is_empty() || is_path || self.items[i].label.to_lowercase().contains(&q) || self.items[i].detail.to_lowercase().contains(&q)).collect()
    }

    /// The chosen destinations: ticked items, else the typed location, else
    /// the cursor item.
    pub fn chosen(&self) -> Vec<String> {
        let typed = self.input.text.trim();
        let mut out: Vec<String> = self.checked.iter().filter_map(|&i| self.items.get(i)).map(|d| d.uri.clone()).collect();
        let typed_is_location = typed.starts_with('/') || typed.starts_with('~') || typed.contains("://") || typed.starts_with("\\\\") || (typed.len() > 1 && typed.as_bytes()[1] == b':');
        if typed_is_location {
            out.push(typed.to_string());
        }
        if out.is_empty() {
            if let Some(d) = self.visible().get(self.cursor).and_then(|&i| self.items.get(i)) {
                out.push(d.uri.clone());
            }
        }
        if self.moving {
            out.truncate(1);
        }
        out.dedup();
        out
    }
}

#[derive(Debug, Clone)]
pub struct RenameItem {
    pub dir: String,
    pub name: String,
    pub is_dir: bool,
    pub modified: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct MultiRename {
    pub items: Vec<RenameItem>,
    /// Names in the folder that are not being renamed (clash check).
    pub others: Vec<String>,
    pub parent: String,
    pub name_mask: TextInput,
    pub ext_mask: TextInput,
    pub search: TextInput,
    pub replace: TextInput,
    pub regex: bool,
    pub case: CaseMode,
    pub start: TextInput,
    pub step: TextInput,
    pub digits: TextInput,
    pub focus: usize,
    pub scroll: usize,
    pub busy: bool,
}

impl MultiRename {
    pub const FIELDS: usize = 9;

    pub fn spec(&self) -> crate::rename::RenameSpec {
        crate::rename::RenameSpec {
            name_mask: self.name_mask.text.clone(),
            ext_mask: self.ext_mask.text.clone(),
            search: self.search.text.clone(),
            replace: self.replace.text.clone(),
            regex: self.regex,
            case: self.case,
            start: self.start.text.trim().parse().unwrap_or(1),
            step: self.step.text.trim().parse().unwrap_or(1),
            digits: self.digits.text.trim().parse().unwrap_or(2).min(12),
            parent: self.parent.clone(),
        }
    }

    pub fn plan(&self) -> Vec<crate::rename::Planned> {
        let sources: Vec<crate::rename::Source> = self.items.iter().map(|i| crate::rename::Source { name: &i.name, is_dir: i.is_dir, modified: i.modified }).collect();
        let others: Vec<&str> = self.others.iter().map(String::as_str).collect();
        crate::rename::plan(&self.spec(), &sources, &others)
    }
}

#[derive(Debug, Clone)]
pub struct SearchForm {
    pub root: String,
    pub name: TextInput,
    pub content: TextInput,
    pub include_hidden: bool,
    pub focus: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagState {
    All,
    Some,
    None,
}

#[derive(Debug, Clone)]
pub struct TagsDlg {
    pub uris: Vec<String>,
    pub tags: Vec<(String, TagState)>,
    pub cursor: usize,
    pub input: TextInput,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub struct PairForm {
    pub address: TextInput,
    pub code: TextInput,
    pub focus: usize,
    pub error: Option<String>,
    pub busy: bool,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub tag: char,
    pub left: Option<usize>,
    pub right: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct DiffView {
    pub left: String,
    pub right: String,
    pub lines: Vec<DiffLine>,
    pub added: usize,
    pub removed: usize,
    pub scroll: usize,
}

#[derive(Debug, Clone)]
pub enum Dialog {
    Confirm(Confirm),
    Prompt(Prompt),
    Menu(Menu),
    Palette(Palette),
    Help { scroll: usize },
    Connect(ConnectForm),
    SignIn(SignIn),
    HostKey(HostKey),
    Conflict(ConflictDlg),
    DestPicker(DestPicker),
    MultiRename(MultiRename),
    Search(SearchForm),
    Tags(TagsDlg),
    Pair(PairForm),
    Peer { cursor: usize },
    Offer { offer: IncomingOffer, cursor: usize },
    Settings { cursor: usize },
    Diff(DiffView),
    Info { title: String, lines: Vec<String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_like_the_desktop_connect_dialog() {
        assert_eq!(build_uri("sftp", "nas", "22", "pi", "home/pi"), "sftp://pi@nas/home/pi");
        assert_eq!(build_uri("sftp", "nas", "2222", "", ""), "sftp://nas:2222/");
        assert_eq!(build_uri("smb", "::1", "", "a b", "My Share"), "smb://a%20b@[::1]/My%20Share");
        let mut f = ConnectForm::new("smb", "");
        f.host.set("ssh://me@box.local:2200/srv/data");
        f.absorb_uri();
        assert_eq!(f.scheme(), "sftp");
        assert_eq!(f.host.text, "box.local");
        assert_eq!(f.port.text, "2200");
        assert_eq!(f.user.text, "me");
        assert_eq!(f.path.text, "/srv/data");
        assert_eq!(f.uri().unwrap(), "sftp://me@box.local:2200/srv/data");
    }

    #[test]
    fn dest_picker_choices() {
        let items = vec![
            DestItem { label: "Other pane".into(), detail: String::new(), uri: "file:///a".into(), section: "Panes" },
            DestItem { label: "Docs".into(), detail: String::new(), uri: "file:///docs".into(), section: "Favorites" },
        ];
        let mut p = DestPicker { moving: false, sources: vec![], items, checked: BTreeSet::new(), cursor: 1, input: TextInput::default() };
        assert_eq!(p.chosen(), vec!["file:///docs"]);
        p.checked.insert(0);
        p.checked.insert(1);
        p.input.set("/tmp/x");
        assert_eq!(p.chosen(), vec!["file:///a", "file:///docs", "/tmp/x"]);
        p.moving = true;
        assert_eq!(p.chosen().len(), 1, "a move has one destination");
        p.input.set("doc");
        assert_eq!(p.visible(), vec![1]);
    }

    #[test]
    fn menus_skip_headers() {
        let mut m = Menu::new("t", vec![MenuItem::header("H"), MenuItem::new("a", "", MenuAction::None), MenuItem::new("b", "", MenuAction::None)]);
        assert_eq!(m.cursor, 1);
        m.step(-1);
        assert_eq!(m.cursor, 1);
        m.step(5);
        assert_eq!(m.cursor, 2);
        m.filter = "a".into();
        m.fix_cursor();
        assert_eq!(m.selected().unwrap().label, "a");
    }
}
