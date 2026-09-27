//! Application state and everything that changes it.
//!
//! The terminal loop feeds [`App::handle_key`], [`App::handle_mouse`] and
//! [`App::handle_msg`]; rendering (`crate::ui`) only reads the state. Engine
//! calls run as tokio tasks and report back through [`Msg`]s, so neither
//! rendering nor input ever waits on disk or network. Tests drive the same
//! API against real folders (see `tests/`).

mod actions;
mod dialogs;
mod input;
mod net;
mod ops;
mod sources;

use crate::dialog::{ConflictDlg, Dialog};
use crate::folder::Folder;
use crate::msg::{Msg, Update};
use crate::preview::{Content, Preview};
use crate::settings::{SavedPane, SavedTab, Session, Settings, ViewMode};
use crate::tab::{Source, Tab, HOME_URI};
use cx_core::{CxError, Space};
use cx_discovery::Device;
use cx_engine::places::Places;
use cx_engine::{Engine, EngineEvent, JobView, PeerStatus};
use cx_transfer::UndoOp;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

pub use input::MouseHit;
pub use sources::{compare_uri, search_uri, tag_uri};

pub struct Pane {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// Closed tabs, for "Reopen closed tab".
    pub closed: Vec<(String, ViewMode)>,
}

impl Pane {
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub at: Instant,
}

/// One undoable user action (a multi-rename is several renames).
#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub label: String,
    pub ops: Vec<UndoOp>,
}

/// Something the terminal loop runs with the TUI suspended.
#[derive(Debug, Clone, PartialEq)]
pub enum External {
    /// Program and arguments (empty = login shell) in `cwd`.
    Shell { argv: Vec<String>, cwd: Option<PathBuf> },
    /// Edit a local file; for a downloaded copy of a remote file, `upload`
    /// is (remote folder URI, mtime before editing).
    Edit { path: PathBuf, upload: Option<(String, Option<std::time::SystemTime>)> },
}

/// Which list has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    List,
    /// Typing into the quick filter (Ctrl+F).
    Filter,
    Transfers,
}

pub struct App {
    pub engine: Arc<Engine>,
    tx: UnboundedSender<Msg>,
    pub settings: Settings,
    settings_path: Option<PathBuf>,
    pub panes: [Pane; 2],
    /// The pane with the keyboard (always 0 in single-pane mode).
    pub active: usize,
    pub dialogs: Vec<Dialog>,
    pub toasts: Vec<Toast>,
    /// Newest first.
    pub jobs: Vec<JobView>,
    pub undo: Vec<UndoEntry>,
    /// Copied or cut URIs (`true` = cut).
    pub clipboard: Option<(Vec<String>, bool)>,
    pub devices: Vec<Device>,
    pub peer: Option<PeerStatus>,
    pub places: Option<Places>,
    /// Folder sizes: bytes so far, and whether counting finished.
    pub sizes: HashMap<String, (u64, bool)>,
    pub free: HashMap<String, Space>,
    pub tags: HashMap<String, Vec<String>>,
    tags_pending: HashSet<String>,
    pub preview: Option<Preview>,
    pub quicklook: bool,
    pub transfers_open: bool,
    pub transfers_cursor: usize,
    pub focus: Focus,
    pub external: Option<External>,
    /// Text for the terminal's clipboard (OSC 52), written by the loop.
    pub osc52: Option<String>,
    pub quit: bool,
    next_token: u64,
    next_tab: u64,
    /// Folders to expand recursively once listed.
    expand_recursive: HashSet<u64>,
    /// Folders unwatched while hidden, re-listed when shown again.
    stale: HashSet<u64>,
    /// Conflicts already shown.
    conflicts_seen: HashSet<(u64, u64)>,
    finished_jobs: HashSet<u64>,
    size_limit: Arc<tokio::sync::Semaphore>,
    pub(crate) last_click: Option<(u16, u16, Instant)>,
    /// Files waiting for "Send to device" to pick a device.
    pub(crate) pending_send: Option<Vec<String>>,
}

impl App {
    /// Build the app on an engine. Returns the message receiver the loop
    /// (or a test) must pump into [`App::handle_msg`].
    pub fn new(engine: Arc<Engine>, settings: Settings, settings_path: Option<PathBuf>) -> (App, UnboundedReceiver<Msg>) {
        let (tx, rx) = unbounded_channel();
        {
            let tx = tx.clone();
            engine.events.set(Arc::new(move |e| {
                let _ = tx.send(Msg::Engine(e));
            }));
        }
        let mut app = App {
            engine,
            tx,
            settings,
            settings_path,
            panes: [Pane { tabs: Vec::new(), active: 0, closed: Vec::new() }, Pane { tabs: Vec::new(), active: 0, closed: Vec::new() }],
            active: 0,
            dialogs: Vec::new(),
            toasts: Vec::new(),
            jobs: Vec::new(),
            undo: Vec::new(),
            clipboard: None,
            devices: Vec::new(),
            peer: None,
            places: None,
            sizes: HashMap::new(),
            free: HashMap::new(),
            tags: HashMap::new(),
            tags_pending: HashSet::new(),
            preview: None,
            quicklook: false,
            transfers_open: false,
            transfers_cursor: 0,
            focus: Focus::List,
            external: None,
            osc52: None,
            quit: false,
            next_token: 1,
            next_tab: 1,
            expand_recursive: HashSet::new(),
            stale: HashSet::new(),
            conflicts_seen: HashSet::new(),
            finished_jobs: HashSet::new(),
            size_limit: Arc::new(tokio::sync::Semaphore::new(4)),
            last_click: None,
            pending_send: None,
        };
        // Jobs restored from a previous run (paused), and current devices.
        app.jobs = app.engine.job_list();
        app.devices = app.engine.devices();
        app.load_places();
        app.refresh_peer();
        (app, rx)
    }

    /// Open the start locations: `starts` if given, else the saved session,
    /// else the home folder.
    pub fn start(&mut self, starts: &[String]) {
        let home = cx_core::location::home_dir().map(|h| cx_core::Location::local(h).uri()).unwrap_or_else(|| HOME_URI.into());
        let session = self.settings.session.clone().filter(|s| self.settings.restore_session && s.panes.iter().any(|p| !p.tabs.is_empty()));
        if !starts.is_empty() {
            self.open_tab(0, &starts[0], true);
            if let Some(second) = starts.get(1) {
                self.open_tab(1, second, true);
                self.settings.dual = true;
            }
        } else if let Some(s) = session {
            self.open_session(&s);
        }
        if self.panes[0].tabs.is_empty() {
            self.open_tab(0, &home, true);
        }
        if self.panes[1].tabs.is_empty() {
            let uri = self.panes[0].tab().uri().to_string();
            self.open_tab(1, &uri, true);
        }
        self.sync_watches();
        if !self.settings.fda_tip_shown && !cx_engine::system::full_disk_access() {
            self.settings.fda_tip_shown = true;
            self.toast("Tip: give your terminal Full Disk Access (System Settings → Privacy & Security) to browse every folder");
        }
    }

    fn open_session(&mut self, s: &Session) {
        for (i, p) in s.panes.iter().enumerate().take(2) {
            for (j, t) in p.tabs.iter().enumerate() {
                let id = self.open_tab(i, &t.uri, j == p.active);
                if let Some(tab) = self.tab_by_id_mut(id) {
                    tab.view = t.view;
                }
            }
        }
        self.settings.dual = s.dual;
        self.active = if s.dual { s.active_pane.min(1) } else { 0 };
    }

    /// Replace every tab with a saved workspace's.
    pub fn restore_session(&mut self, s: &Session) {
        for p in 0..2 {
            let tabs = std::mem::take(&mut self.panes[p].tabs);
            for t in tabs {
                self.dispose_tab_public(t);
            }
            self.panes[p].active = 0;
        }
        self.open_session(s);
        for p in 0..2 {
            if self.panes[p].tabs.is_empty() {
                let uri = cx_core::location::home_dir().map(|h| cx_core::Location::local(h).uri()).unwrap_or_else(|| HOME_URI.into());
                self.open_tab(p, &uri, true);
            }
        }
        self.sync_watches();
    }

    pub fn sender(&self) -> UnboundedSender<Msg> {
        self.tx.clone()
    }

    /// Run `f` on the runtime and apply its resulting update on the UI thread.
    pub(crate) fn spawn<F, Fut>(&self, f: F)
    where
        F: FnOnce(Arc<Engine>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Update> + Send + 'static,
    {
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let update = f(engine).await;
            let _ = tx.send(Msg::Apply(update));
        });
    }

    fn token(&mut self) -> u64 {
        self.next_token += 1;
        self.next_token
    }

    // ---- accessors ----

    pub fn dual(&self) -> bool {
        self.settings.dual
    }

    pub fn pane(&self) -> &Pane {
        &self.panes[self.active]
    }

    pub fn pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.active]
    }

    pub fn tab(&self) -> &Tab {
        self.pane().tab()
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        self.pane_mut().tab_mut()
    }

    pub fn other_tab(&self) -> Option<&Tab> {
        self.dual().then(|| self.panes[1 - self.active].tab())
    }

    pub fn tab_by_id_mut(&mut self, id: u64) -> Option<&mut Tab> {
        self.panes.iter_mut().flat_map(|p| p.tabs.iter_mut()).find(|t| t.id == id)
    }

    /// Panes on screen.
    pub fn visible_panes(&self) -> Vec<usize> {
        if self.dual() {
            vec![0, 1]
        } else {
            vec![0]
        }
    }

    pub fn folder_mut(&mut self, token: u64) -> Option<&mut Folder> {
        self.panes.iter_mut().flat_map(|p| p.tabs.iter_mut()).find_map(|t| t.folder_by_token_mut(token))
    }

    /// (pane, tab index) holding the folder `token`, and whether it's the
    /// tab's own listing (not an expanded subfolder).
    fn locate(&self, token: u64) -> Option<(usize, usize, bool)> {
        for (p, pane) in self.panes.iter().enumerate() {
            for (i, t) in pane.tabs.iter().enumerate() {
                if t.folder.token == token {
                    return Some((p, i, true));
                }
                if t.expanded.iter().any(|e| e.folder.token == token) {
                    return Some((p, i, false));
                }
            }
        }
        None
    }

    fn is_visible(&self, pane: usize, tab: usize) -> bool {
        (pane == 0 || self.dual()) && self.panes[pane].active == tab
    }

    // ---- toasts & settings ----

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast { text: text.into(), error: false, at: Instant::now() });
    }

    pub fn error(&mut self, e: impl std::fmt::Display) {
        self.toasts.push(Toast { text: e.to_string(), error: true, at: Instant::now() });
    }

    /// The toast to show now, if any.
    pub fn current_toast(&self) -> Option<&Toast> {
        self.toasts.last().filter(|t| t.at.elapsed() < Duration::from_secs(if t.error { 8 } else { 4 }))
    }

    pub fn save_settings(&mut self) {
        self.settings.session = Some(self.session());
        if let Some(p) = &self.settings_path {
            if let Err(e) = self.settings.save(p) {
                self.error(format!("couldn't save settings: {e}"));
            }
        }
    }

    pub fn session(&self) -> Session {
        Session {
            dual: self.dual(),
            active_pane: self.active,
            panes: self
                .panes
                .iter()
                .map(|p| {
                    let tabs: Vec<(usize, SavedTab)> = p.tabs.iter().enumerate().filter(|(_, t)| !matches!(t.source, Source::Search { .. })).map(|(i, t)| (i, SavedTab { uri: t.uri().to_string(), view: t.view })).collect();
                    let active = tabs.iter().position(|(i, _)| *i == p.active).unwrap_or(0);
                    SavedPane { tabs: tabs.into_iter().map(|(_, t)| t).collect(), active }
                })
                .collect(),
        }
    }

    // ---- messages ----

    pub fn handle_msg(&mut self, msg: Msg) {
        match msg {
            Msg::List { token, seq, event } => {
                if self.folder_mut(token).is_some_and(|f| f.load_seq == seq) {
                    self.on_list(token, event);
                }
            }
            Msg::ListFailed { token, seq, error } => {
                if self.folder_mut(token).is_some_and(|f| f.load_seq == seq) {
                    self.on_list_failed(token, error);
                }
            }
            Msg::Changes { token, changes } => {
                let mut reset = false;
                if let Some(f) = self.folder_mut(token) {
                    reset = f.apply(changes);
                }
                if reset {
                    self.reload_token(token);
                }
            }
            Msg::Watching { token, result } => self.on_watching(token, result),
            Msg::Search { token, event } => self.on_search(token, event),
            Msg::Engine(e) => self.on_engine(e),
            Msg::Apply(f) => f(self),
        }
    }

    fn on_engine(&mut self, e: EngineEvent) {
        match e {
            EngineEvent::Job { job } => self.on_job(job),
            EngineEvent::Devices { devices } => {
                self.devices = devices;
                self.refresh_home();
            }
            EngineEvent::Offer { offer } => self.dialogs.push(Dialog::Offer { offer, cursor: 0 }),
            EngineEvent::Peer { status } => self.peer = Some(status),
        }
    }

    fn on_job(&mut self, job: JobView) {
        if let Some(c) = &job.conflict {
            if self.conflicts_seen.insert((job.id, c.id)) {
                self.dialogs.push(Dialog::Conflict(ConflictDlg { job: job.id, conflict: c.clone(), apply_all: false, cursor: 0 }));
            }
        } else {
            // Answered elsewhere (apply-to-all, cancel): drop a stale prompt.
            self.dialogs.retain(|d| !matches!(d, Dialog::Conflict(c) if c.job == job.id));
        }
        if job.is_finished() && self.finished_jobs.insert(job.id) {
            self.on_job_finished(&job);
        }
        match self.jobs.iter_mut().find(|j| j.id == job.id) {
            Some(j) => *j = job,
            None => self.jobs.insert(0, job),
        }
    }

    fn on_job_finished(&mut self, job: &JobView) {
        let n = job.sources.len();
        let what = if n == 1 { format!("“{}”", crate::util::name_of(&job.sources[0])) } else { format!("{n} items") };
        let verb = match job.kind.as_str() {
            "copy" => "Copy",
            "move" => "Move",
            "trash" => "Move to Trash",
            "delete" => "Delete",
            "compress" => "Compress",
            "extract" => "Extract",
            "send" => "Send",
            "receive" => "Receive",
            k => k,
        };
        match job.state.as_str() {
            "done" if job.errors.is_empty() => {
                if !matches!(job.kind.as_str(), "delete") {
                    self.toast(format!("{verb} {what}: done"));
                }
            }
            "cancelled" => self.toast(format!("{verb} {what}: cancelled")),
            _ => {
                let first = job.errors.first().map(|e| e.message.clone()).unwrap_or_else(|| "failed".into());
                let more = if job.errors.len() > 1 { format!(" (+{} more)", job.errors.len() - 1) } else { String::new() };
                self.error(format!("{verb} {what}: {first}{more}"));
            }
        }
        if let Some(op) = job.undo.clone() {
            self.undo.push(UndoEntry { label: format!("{verb} {what}"), ops: vec![op] });
        }
        // Polled folders the job touched refresh right away.
        let mut dirs: HashSet<String> = job.sources.iter().filter_map(|s| cx_core::Location::parse(s).ok()?.parent().map(|p| p.uri())).collect();
        if let Some(d) = &job.dest {
            dirs.insert(d.trim_end_matches('/').to_string());
        }
        self.refresh_polled(&dirs);
    }

    /// Re-list shown folders among `dirs` that aren't watched live.
    pub(crate) fn refresh_polled(&mut self, dirs: &HashSet<String>) {
        let mut tokens = Vec::new();
        for p in self.visible_panes() {
            let t = self.panes[p].tab();
            for f in t.folders() {
                let live = f.watch.map(|(_, m)| m == cx_engine::WatchMode::Live).unwrap_or(false);
                if !live && dirs.contains(f.dir_uri().trim_end_matches('/')) {
                    tokens.push(f.token);
                }
            }
        }
        for t in tokens {
            self.reload_token(t);
        }
    }

    // ---- periodic work ----

    /// Housekeeping before drawing: rows, highlights, preview, tags.
    /// Returns true while something animates (the loop keeps ticking fast).
    pub fn prepare(&mut self) -> bool {
        let show_hidden = self.settings.show_hidden;
        let mut animating = false;
        for p in self.visible_panes() {
            let pane = &mut self.panes[p];
            let tab = pane.tab_mut();
            for f in tab.folders_mut() {
                animating |= f.expire_fresh();
            }
            tab.refresh_rows(show_hidden);
        }
        self.update_preview();
        self.request_tags();
        animating || self.jobs.iter().any(|j| !j.is_finished()) || self.current_toast().is_some() || self.preview.as_ref().is_some_and(|p| matches!(p.content, Content::Loading))
    }

    fn preview_visible(&self) -> bool {
        self.quicklook || self.settings.preview_pane
    }

    fn update_preview(&mut self) {
        if !self.preview_visible() {
            return;
        }
        let tab = self.tab();
        let Some(row) = tab.cursor_row() else {
            self.preview = None;
            return;
        };
        if !tab.is_folder() && !matches!(tab.source, Source::Search { .. } | Source::Tag { .. }) {
            self.preview = None;
            return;
        }
        let uri = tab.uri_of(row);
        let entry = tab.item(row).entry.clone();
        if self.preview.as_ref().is_some_and(|p| p.uri == uri && p.entry.modified == entry.modified && p.entry.size == entry.size) {
            return;
        }
        self.preview = Some(Preview { uri: uri.clone(), entry: entry.clone(), content: Content::Loading, scroll: 0 });
        self.spawn(move |engine| async move {
            let content = crate::preview::load(engine, uri.clone(), entry).await;
            Box::new(move |app: &mut App| {
                if let Some(p) = app.preview.as_mut().filter(|p| p.uri == uri) {
                    p.content = content;
                }
            }) as Update
        });
    }

    /// Fetch tags for rows on screen (Finder tags need a syscall per file).
    fn request_tags(&mut self) {
        let mut want = Vec::new();
        for p in self.visible_panes() {
            let t = self.panes[p].tab();
            if !(t.is_folder() || matches!(t.source, Source::Search { .. } | Source::Tag { .. })) {
                continue;
            }
            let start = t.scroll.get();
            for r in t.rows().iter().skip(start).take(t.page.get().max(1) * 3) {
                let uri = t.uri_of(r);
                if !self.tags.contains_key(&uri) && !self.tags_pending.contains(&uri) {
                    want.push(uri);
                }
            }
        }
        if want.is_empty() {
            return;
        }
        self.tags_pending.extend(want.iter().cloned());
        self.spawn(move |engine| async move {
            let got = tokio::task::spawn_blocking(move || engine.tags.get_many(want)).await.unwrap_or_default();
            Box::new(move |app: &mut App| {
                for (u, t) in got {
                    app.tags_pending.remove(&u);
                    app.tags.insert(u, t);
                }
            }) as Update
        });
    }

    /// The job shown in the status bar: the running one, else the newest.
    pub fn active_jobs(&self) -> Vec<&JobView> {
        self.jobs.iter().filter(|j| !j.is_finished()).collect()
    }

    pub(crate) fn report<T>(&mut self, r: Result<T, CxError>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.error(e);
                None
            }
        }
    }
}
