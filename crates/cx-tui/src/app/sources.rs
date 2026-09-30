//! Opening locations: plain folders (listed, then watched while visible),
//! the home page, search results, folder comparisons and tag lists; tab
//! history; the in-place outline.

use super::{App, Focus};
use crate::dialog::{AfterAuth, Dialog, HostKey, SignIn};
use crate::folder::{Folder, Item};
use crate::input::TextInput;
use crate::msg::{Msg, Update};
use crate::tab::{child_uri, Source, Tab, HOME_URI};
use crate::util::{cx_uri, param};
use cx_core::{CxError, Entry, EntryKind, Location};
use cx_engine::{ListEvent, SearchEvent, SearchHitView, SearchRequest, WatchInfo};
use cx_transfer::DiffKind;
use std::collections::HashSet;

fn nav_entry(name: &str) -> Entry {
    Entry {
        name: name.into(),
        kind: EntryKind::Dir,
        is_dir: true,
        size: 0,
        modified: None,
        created: None,
        hidden: false,
        readonly: false,
        executable: false,
    }
}

pub fn search_uri(root: &str, text: &str, content: bool, hidden: bool) -> String {
    cx_uri(
        "search",
        &[
            ("root", root),
            ("q", text),
            ("content", if content { "1" } else { "0" }),
            ("hidden", if hidden { "1" } else { "0" }),
        ],
    )
}

pub fn compare_uri(left: &str, right: &str) -> String {
    cx_uri("compare", &[("left", left), ("right", right)])
}

pub fn tag_uri(name: &str) -> String {
    cx_uri("tag", &[("name", name)])
}

fn is_local(uri: &str) -> bool {
    uri.starts_with("file:") || uri.starts_with('/') || uri.starts_with('~')
}

impl App {
    /// Create the folder and source for `uri` and start whatever fills it.
    fn make(&mut self, uri: &str) -> (Folder, Source) {
        let token = self.token();
        let sort = self.settings.sort;
        if uri == HOME_URI {
            let items = self.home_items();
            return (
                Folder::with_items(token, HOME_URI, sort, items, true),
                Source::Home,
            );
        }
        if uri.starts_with("cx:search") {
            let root = param(uri, "root").unwrap_or_default();
            let text = param(uri, "q").unwrap_or_default();
            let content = param(uri, "content").as_deref() == Some("1");
            let hidden = param(uri, "hidden").as_deref() == Some("1");
            let mut f = Folder::new(token, uri, sort);
            f.info = Location::parse(&root).ok().map(|l| l.info());
            let req = if content {
                SearchRequest {
                    content: Some(text.clone()),
                    include_hidden: hidden,
                    ..Default::default()
                }
            } else {
                SearchRequest {
                    text: text.clone(),
                    include_hidden: hidden,
                    ..Default::default()
                }
            };
            let tx = self.tx.clone();
            let task = match self.engine.search_start(&root, req, move |event| {
                let _ = tx.send(Msg::Search { token, event });
            }) {
                Ok(id) => Some(id),
                Err(e) => {
                    f.fail(e);
                    None
                }
            };
            return (
                f,
                Source::Search {
                    root,
                    text,
                    content,
                    task,
                    scanned: 0,
                    truncated: false,
                },
            );
        }
        if uri.starts_with("cx:compare") {
            let left = param(uri, "left").unwrap_or_default();
            let right = param(uri, "right").unwrap_or_default();
            let f = Folder::new(token, uri, sort);
            self.run_compare(token, left.clone(), right.clone(), false);
            return (
                f,
                Source::Compare {
                    left,
                    right,
                    by_content: false,
                    diff: Vec::new(),
                },
            );
        }
        if uri.starts_with("cx:tag") {
            let name = param(uri, "name").unwrap_or_default();
            let f = Folder::new(token, uri, sort);
            let tag = name.clone();
            self.spawn(move |engine| async move {
                let hits = engine.tags_find(&tag).await;
                Box::new(move |app: &mut App| {
                    if let Some(f) = app.folder_mut(token) {
                        let items = hits
                            .into_iter()
                            .map(|h| {
                                Item::located(h.entry, h.uri, Some(h.parent), Some(h.rel_path))
                            })
                            .collect();
                        f.set_items(items);
                        f.finish_load(0.0);
                    }
                }) as Update
            });
            return (f, Source::Tag { name });
        }
        // A plain location: canonicalise what was typed (~, paths, UNC).
        let canonical = Location::parse(uri).map(|l| l.uri());
        let mut f = Folder::new(
            token,
            canonical.clone().unwrap_or_else(|_| uri.to_string()),
            sort,
        );
        match canonical {
            Ok(u) => {
                self.load(token, 0, &u);
                if is_local(&u) {
                    // Local watches are live and need no baseline: start now.
                    f.watch_pending = true;
                    self.start_watch(token, &u);
                }
            }
            Err(e) => f.fail(e),
        }
        (f, Source::Folder)
    }

    pub(crate) fn run_compare(
        &mut self,
        token: u64,
        left: String,
        right: String,
        by_content: bool,
    ) {
        self.spawn(move |engine| async move {
            let result = engine.compare_dirs(&left, &right, by_content).await;
            Box::new(move |app: &mut App| app.on_compared(token, result)) as Update
        });
    }

    fn on_compared(&mut self, token: u64, result: cx_core::Result<Vec<cx_transfer::DiffItem>>) {
        let Some((p, i, _)) = self.locate(token) else {
            return;
        };
        let tab = &mut self.panes[p].tabs[i];
        match result {
            Ok(diff) => {
                let items = diff
                    .iter()
                    .filter(|d| d.kind != DiffKind::Same)
                    .map(|d| {
                        let e = d
                            .left
                            .clone()
                            .or_else(|| d.right.clone())
                            .unwrap_or_else(|| nav_entry(&d.rel_path));
                        let label = match d.kind {
                            DiffKind::LeftOnly => "left only",
                            DiffKind::RightOnly => "right only",
                            DiffKind::NewerLeft => "newer left",
                            DiffKind::NewerRight => "newer right",
                            DiffKind::Different => "different",
                            DiffKind::Same => "same",
                        };
                        let mut item = Item::located(
                            Entry {
                                name: d.rel_path.clone(),
                                ..e
                            },
                            d.rel_path.clone(),
                            None,
                            Some(label.into()),
                        );
                        item.entry.hidden = false;
                        item
                    })
                    .collect();
                tab.folder.keep_order = true;
                tab.folder.set_items(items);
                tab.folder.finish_load(0.0);
                if let Source::Compare { diff: d, .. } = &mut tab.source {
                    *d = diff;
                }
            }
            Err(e) => tab.folder.fail(e),
        }
    }

    /// The home page: favorites, recent folders, drives, servers, nearby
    /// devices and tags, in that order, each once.
    pub(crate) fn home_items(&self) -> Vec<Item> {
        let mut out: Vec<Item> = Vec::new();
        let mut seen = HashSet::new();
        let mut add = |label: String, uri: String, section: &str| {
            if seen.insert(uri.clone()) {
                out.push(Item::located(
                    nav_entry(&label),
                    uri,
                    None,
                    Some(section.to_string()),
                ));
            }
        };
        for b in &self.settings.bookmarks {
            add(b.name.clone(), b.uri.clone(), "Favorites");
        }
        if let Some(p) = &self.places {
            add(p.home.name.clone(), p.home.uri.clone(), "Places");
            for f in &p.favorites {
                add(f.name.clone(), f.uri.clone(), "Places");
            }
            for v in &p.volumes {
                add(v.name.clone(), v.uri.clone(), "Drives");
            }
        }
        for r in self.settings.recent.iter().take(10) {
            add(crate::util::name_of(r), r.clone(), "Recent");
        }
        for s in &self.settings.servers {
            add(s.name.clone(), s.uri.clone(), "Servers");
        }
        for c in self.engine.connections() {
            add(crate::util::display(&c), c, "Connected");
        }
        for d in self.devices.iter().filter(|d| !d.is_self()) {
            for s in &d.shares {
                add(format!("{} — {}", d.name, s.name), s.uri.clone(), "Nearby");
            }
            for s in &d.services {
                add(format!("{} ({})", d.name, s.label), s.uri.clone(), "Nearby");
            }
        }
        for t in cx_engine::tags::TAG_COLORS {
            add(format!("{t} tag"), tag_uri(t), "Tags");
        }
        out
    }

    /// Rebuild home pages on screen (devices or places changed).
    pub(crate) fn refresh_home(&mut self) {
        let items = self.home_items();
        for p in 0..2 {
            for t in self.panes[p]
                .tabs
                .iter_mut()
                .filter(|t| t.source == Source::Home)
            {
                t.folder.set_items(items.clone());
            }
        }
    }

    pub(crate) fn load_places(&mut self) {
        self.spawn(|_| async move {
            let places = tokio::task::spawn_blocking(cx_engine::places::places)
                .await
                .ok();
            Box::new(move |app: &mut App| {
                app.places = places;
                app.refresh_home();
            }) as Update
        });
    }

    // ---- tabs ----

    /// Open `uri` in a new tab of `pane`; returns the tab id.
    pub fn open_tab(&mut self, pane: usize, uri: &str, activate: bool) -> u64 {
        let (folder, source) = self.make(uri);
        self.next_tab += 1;
        let id = self.next_tab;
        let tab = Tab::new(id, folder, source, self.settings.default_view);
        let p = &mut self.panes[pane];
        let at = if p.tabs.is_empty() { 0 } else { p.active + 1 };
        p.tabs.insert(at, tab);
        if activate || p.tabs.len() == 1 {
            p.active = at;
        } else if at <= p.active && p.tabs.len() > 1 {
            // keep the same tab active
        }
        self.sync_watches();
        id
    }

    pub fn activate_tab(&mut self, pane: usize, idx: usize) {
        if idx < self.panes[pane].tabs.len() {
            self.panes[pane].active = idx;
            if self.dual() || pane == 0 {
                self.active = if self.dual() { pane } else { 0 };
            }
            self.sync_watches();
        }
    }

    pub fn close_tab(&mut self) {
        let p = self.active;
        let pane = &mut self.panes[p];
        let t = pane.tab();
        pane.closed.push((t.uri().to_string(), t.view));
        if pane.tabs.len() == 1 {
            // Closing the last tab goes home instead of leaving an empty pane.
            self.navigate(HOME_URI);
            return;
        }
        let tab = pane.tabs.remove(pane.active);
        pane.active = pane.active.min(pane.tabs.len() - 1);
        self.dispose_tab(tab);
        self.sync_watches();
    }

    pub(crate) fn dispose_tab_public(&mut self, tab: Tab) {
        self.dispose_tab(tab);
    }

    fn dispose_tab(&mut self, tab: Tab) {
        for f in tab.folders() {
            if let Some((id, _)) = f.watch {
                self.engine.unwatch_dir(id);
            }
        }
        if let Source::Search { task: Some(id), .. } = tab.source {
            self.engine.cancel_task(id);
        }
    }

    // ---- navigation ----

    /// Go to `uri` in the active tab.
    pub fn navigate(&mut self, uri: &str) {
        self.navigate_select(uri, None);
    }

    pub fn navigate_select(&mut self, uri: &str, select: Option<String>) {
        let p = self.active;
        self.navigate_in(p, uri, select);
    }

    pub fn navigate_in(&mut self, pane: usize, uri: &str, select: Option<String>) {
        let same = {
            let t = self.panes[pane].tab();
            let canon = Location::parse(uri)
                .map(|l| l.uri())
                .unwrap_or_else(|_| uri.to_string());
            t.uri() == uri || t.dir_uri() == canon
        };
        if same {
            if let Some(s) = select {
                self.panes[pane].tab_mut().select_key(&s);
            }
            return;
        }
        let (folder, source) = self.make(uri);
        let tab = self.panes[pane].tab_mut();
        let old_watch = tab.folder.watch;
        let old_expanded = std::mem::take(&mut tab.expanded);
        let old_source = tab.source.clone();
        tab.navigate(folder, source, select);
        if let Some((id, _)) = old_watch {
            self.engine.unwatch_dir(id);
        }
        self.drop_folders(old_expanded, old_source);
        self.focus = Focus::List;
        self.sync_watches();
    }

    /// Unwatch folders that are going away.
    fn drop_folders(&mut self, expanded: Vec<crate::tab::Expanded>, source: Source) {
        for e in expanded {
            if let Some((id, _)) = e.folder.watch {
                self.engine.unwatch_dir(id);
            }
        }
        if let Source::Search { task: Some(id), .. } = source {
            self.engine.cancel_task(id);
        }
    }

    pub fn go_history(&mut self, delta: isize) {
        let Some(target) = self.tab().history_target(delta).map(str::to_owned) else {
            return;
        };
        let (folder, source) = self.make(&target);
        let tab = self.tab_mut();
        let expanded = std::mem::take(&mut tab.expanded);
        let old_source = tab.source.clone();
        let old_watch = tab.folder.watch;
        tab.go_history(delta, folder, source);
        if let Some((id, _)) = old_watch {
            self.engine.unwatch_dir(id);
        }
        self.drop_folders(expanded, old_source);
        self.sync_watches();
    }

    pub fn go_up(&mut self) {
        let t = self.tab();
        let parent = match &t.source {
            Source::Folder => t.folder.info.as_ref().and_then(|i| i.parent.clone()),
            Source::Search { root, .. } => Some(root.clone()),
            _ => None,
        };
        let select = t
            .folder
            .info
            .as_ref()
            .and_then(|i| i.crumbs.last().map(|c| c.label.clone()))
            .filter(|_| t.is_folder());
        if let Some(p) = parent {
            self.navigate_select(&p, select);
        }
    }

    pub fn reload(&mut self) {
        let token = self.tab().folder.token;
        self.reload_token(token);
    }

    /// Re-list one folder (keeping its rows on screen until done).
    pub(crate) fn reload_token(&mut self, token: u64) {
        let Some((p, i, _)) = self.locate(token) else {
            return;
        };
        let tab = &self.panes[p].tabs[i];
        match tab.source.clone() {
            Source::Home => {
                let items = self.home_items();
                self.panes[p].tabs[i].folder.set_items(items);
            }
            Source::Compare {
                left,
                right,
                by_content,
                ..
            } => {
                let tab = &mut self.panes[p].tabs[i];
                let _ = tab.folder.begin_load(true);
                let tok = tab.folder.token;
                self.run_compare(tok, left, right, by_content);
            }
            Source::Search { .. } | Source::Tag { .. } => {
                let uri = tab.uri().to_string();
                let (folder, source) = self.make(&uri);
                let tab = &mut self.panes[p].tabs[i];
                if let Source::Search { task: Some(id), .. } = tab.source {
                    self.engine.cancel_task(id);
                }
                tab.folder = folder;
                tab.source = source;
            }
            Source::Folder => {
                let Some(f) = self.folder_mut(token) else {
                    return;
                };
                let seq = f.begin_load(true);
                let uri = f.uri.clone();
                let watching = f.watch.is_some() || f.watch_pending;
                self.load(token, seq, &uri);
                // A folder that failed before (not signed in) never watched.
                if !watching && is_local(&uri) && self.is_visible(p, i) {
                    self.start_watch(token, &uri);
                }
            }
        }
    }

    /// List `uri` into the folder `token`.
    pub(crate) fn load(&self, token: u64, seq: u64, uri: &str) {
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        let uri = uri.to_string();
        tokio::spawn(async move {
            let tx2 = tx.clone();
            let r = engine
                .list_dir(&uri, move |event| {
                    tx2.send(Msg::List { token, seq, event }).is_ok()
                })
                .await;
            if let Err(error) = r {
                let _ = tx.send(Msg::ListFailed { token, seq, error });
            }
        });
    }

    fn start_watch(&self, token: u64, uri: &str) {
        let engine = self.engine.clone();
        let tx = self.tx.clone();
        let uri = uri.to_string();
        tokio::spawn(async move {
            let tx2 = tx.clone();
            let result = engine
                .watch_dir_with(&uri, move |changes| {
                    let _ = tx2.send(Msg::Changes { token, changes });
                })
                .await;
            let _ = tx.send(Msg::Watching { token, result });
        });
    }

    pub(crate) fn on_watching(&mut self, token: u64, result: Result<WatchInfo, CxError>) {
        let visible = self
            .locate(token)
            .map(|(p, i, _)| self.is_visible(p, i))
            .unwrap_or(false);
        let engine = self.engine.clone();
        let mut relist = false;
        match (self.folder_mut(token), result) {
            (Some(f), Ok(info)) if visible => {
                f.watch = Some((info.id, info.mode));
                f.watch_pending = false;
                // A live watch only reports what happens from now on. If the
                // listing finished first, anything that changed in between
                // would be missed: list once more (rows stay on screen, and
                // patches arriving meanwhile are queued, not lost).
                relist = info.mode == cx_engine::WatchMode::Live && f.is_ready() && !f.is_listing();
            }
            (f, Ok(info)) => {
                // Went away or hidden meanwhile.
                engine.unwatch_dir(info.id);
                if let Some(f) = f {
                    f.watch_pending = false;
                }
            }
            (Some(f), Err(_)) => f.watch_pending = false,
            (None, Err(_)) => {}
        }
        if relist {
            self.reload_token(token);
        }
    }

    /// Watch exactly the folders on screen: the active tab of each visible
    /// pane with its expanded folders. Hidden ones stop watching and are
    /// re-listed when they come back.
    pub fn sync_watches(&mut self) {
        let mut start = Vec::new();
        let mut reload = Vec::new();
        let dual = self.dual();
        for (p, pane) in self.panes.iter_mut().enumerate() {
            let active = pane.active;
            for (i, tab) in pane.tabs.iter_mut().enumerate() {
                let visible = (p == 0 || dual) && i == active;
                let plain = tab.is_folder();
                for (k, f) in tab.folders_mut().enumerate() {
                    // Home, search, compare and tag views aren't folders
                    // to watch (their expanded subfolders are).
                    if k == 0 && !plain {
                        continue;
                    }
                    if visible {
                        if f.watch.is_none() && !f.watch_pending {
                            if self.stale.remove(&f.token) {
                                let seq = f.begin_load(true);
                                reload.push((f.token, seq, f.uri.clone()));
                            }
                            if f.is_ready() || is_local(&f.uri) {
                                f.watch_pending = true;
                                start.push((f.token, f.uri.clone()));
                            }
                        }
                    } else if let Some((id, _)) = f.watch.take() {
                        self.engine.unwatch_dir(id);
                        self.stale.insert(f.token);
                    }
                }
            }
        }
        for (t, seq, u) in reload {
            self.load(t, seq, &u);
        }
        for (t, u) in start {
            self.start_watch(t, &u);
        }
    }

    pub(crate) fn on_list(&mut self, token: u64, event: ListEvent) {
        let Some((p, i, main)) = self.locate(token) else {
            return;
        };
        let visible = self.is_visible(p, i);
        let recursive =
            matches!(event, ListEvent::Done { .. }) && self.expand_recursive.remove(&token);
        let Some(f) = self.folder_mut(token) else {
            return;
        };
        match event {
            ListEvent::Meta { info, capabilities } => f.set_meta(info, capabilities),
            ListEvent::Batch { entries } => f.add_batch(entries),
            ListEvent::Done { elapsed_ms, .. } => {
                f.finish_load(elapsed_ms);
                let uri = f.dir_uri().to_string();
                let needs_watch = visible && f.watch.is_none() && !f.watch_pending;
                if needs_watch {
                    // Polled folders diff against this listing.
                    f.watch_pending = true;
                }
                let children: Vec<String> = if recursive {
                    f.items
                        .iter()
                        .filter(|c| c.entry.is_dir && !c.entry.hidden)
                        .map(|c| child_uri(&uri, &c.entry.name))
                        .collect()
                } else {
                    Vec::new()
                };
                if needs_watch {
                    self.start_watch(token, &uri);
                }
                if main {
                    if self.panes[p].tabs[i].is_folder() {
                        self.settings.add_recent(&uri);
                    }
                    self.request_free_space(&uri);
                }
                for c in children {
                    self.expand_uri(p, i, c, true);
                }
            }
        }
    }

    pub(crate) fn on_list_failed(&mut self, token: u64, error: CxError) {
        let Some((p, i, main)) = self.locate(token) else {
            return;
        };
        let visible = self.is_visible(p, i) && p == self.active;
        let uri = self
            .folder_mut(token)
            .map(|f| f.uri.clone())
            .unwrap_or_default();
        if let Some(f) = self.folder_mut(token) {
            f.fail(error.clone());
        }
        if !(visible && main) {
            return;
        }
        match error {
            CxError::AuthRequired {
                uri: _,
                user,
                reason,
            } => {
                if !self.dialogs.iter().any(|d| {
                    matches!(
                        d,
                        Dialog::SignIn(_) | Dialog::HostKey(_) | Dialog::Connect(_)
                    )
                }) {
                    let loc = Location::parse(&uri).ok();
                    let user = user
                        .or_else(|| {
                            loc.as_ref()
                                .and_then(|l| l.endpoint())
                                .and_then(|e| e.user.clone())
                        })
                        .unwrap_or_default();
                    self.dialogs.push(Dialog::SignIn(SignIn {
                        uri,
                        user: TextInput::new(user.clone()),
                        password: TextInput::secret(),
                        key_file: TextInput::default(),
                        remember: true,
                        focus: if user.is_empty() { 0 } else { 1 },
                        reason,
                        error: None,
                        busy: false,
                        then: AfterAuth::Reload,
                    }));
                }
            }
            CxError::HostKeyUnknown {
                uri: key_uri,
                host,
                key_type,
                fingerprint,
                changed,
            } => {
                if !self.dialogs.iter().any(|d| matches!(d, Dialog::HostKey(_))) {
                    self.dialogs.push(Dialog::HostKey(HostKey {
                        uri: if key_uri.is_empty() { uri } else { key_uri },
                        host,
                        key_type,
                        fingerprint,
                        changed,
                        then: AfterAuth::Reload,
                        on_trust: !changed,
                    }));
                }
            }
            _ => {}
        }
    }

    pub(crate) fn on_search(&mut self, token: u64, event: SearchEvent) {
        let Some((p, i, _)) = self.locate(token) else {
            return;
        };
        let tab = &mut self.panes[p].tabs[i];
        match event {
            SearchEvent::Hits { hits } => {
                let content = matches!(tab.source, Source::Search { content: true, .. });
                let items: Vec<Item> = hits
                    .into_iter()
                    .map(|h: SearchHitView| {
                        let detail = match (content, h.line, &h.snippet) {
                            (true, Some(l), Some(s)) => format!("{}:{l}: {}", h.rel_path, s.trim()),
                            _ => h.rel_path.clone(),
                        };
                        Item::located(h.entry, h.uri, Some(h.parent), Some(detail))
                    })
                    .collect();
                tab.folder.add_batch(Vec::new());
                let mut all = std::mem::take(&mut tab.folder.items);
                all.extend(items);
                tab.folder.loaded = all.len();
                tab.folder.set_items(all);
            }
            SearchEvent::Done {
                scanned: n,
                truncated: t,
                elapsed_ms,
            } => {
                if let Source::Search {
                    scanned,
                    truncated,
                    task,
                    ..
                } = &mut tab.source
                {
                    *scanned = n;
                    *truncated = t;
                    *task = None;
                }
                tab.folder.finish_load(elapsed_ms);
            }
        }
    }

    pub(crate) fn request_free_space(&self, uri: &str) {
        if self.free.contains_key(uri) && !is_local(uri) {
            return;
        }
        let u = uri.to_string();
        self.spawn(move |engine| async move {
            let space = engine.free_space(&u).await.ok().flatten();
            Box::new(move |app: &mut App| {
                if let Some(s) = space {
                    app.free.insert(u, s);
                }
            }) as Update
        });
    }

    pub(crate) fn refresh_peer(&self) {
        self.spawn(|engine| async move {
            let s = engine.peer_status().await.ok();
            Box::new(move |app: &mut App| {
                if s.is_some() {
                    app.peer = s;
                }
            }) as Update
        });
    }

    // ---- outline ----

    /// Expand `uri` (a folder row of tab `i` in pane `p`) in place.
    pub(crate) fn expand_uri(&mut self, p: usize, i: usize, uri: String, recursive: bool) {
        let token = self.token();
        let folder = Folder::new(token, uri.clone(), self.settings.sort);
        if !self.panes[p].tabs[i].add_expanded(uri.clone(), folder) {
            return;
        }
        if recursive {
            self.expand_recursive.insert(token);
        }
        self.load(token, 0, &uri);
        if is_local(&uri) && self.is_visible(p, i) {
            if let Some(f) = self.folder_mut(token) {
                f.watch_pending = true;
            }
            self.start_watch(token, &uri);
        }
    }

    pub(crate) fn collapse_uri(&mut self, uri: &str) {
        let gone = self.tab_mut().collapse(uri);
        for e in gone {
            if let Some((id, _)) = e.folder.watch {
                self.engine.unwatch_dir(id);
            }
            self.expand_recursive.remove(&e.folder.token);
        }
    }

    pub(crate) fn collapse_all(&mut self) {
        let gone = self.tab_mut().collapse_all();
        for e in gone {
            if let Some((id, _)) = e.folder.watch {
                self.engine.unwatch_dir(id);
            }
        }
    }
}
