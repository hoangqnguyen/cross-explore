//! Running commands. [`App::run`] returns false when a command doesn't
//! apply right now, so the key falls through to the next binding.

use super::{App, Focus};
use crate::commands::Action;
use crate::dialog::{ConnectForm, Dialog, Menu, MenuAction, MenuItem, PairForm, Prompt, PromptKind, SearchForm};
use crate::input::TextInput;
use crate::settings::{Keymap, ViewMode};
use crate::sort::SortKey;
use crate::tab::{Source, HOME_URI};
use std::collections::HashSet;

impl App {
    pub fn run(&mut self, a: Action) -> bool {
        let has_targets = !self.tab().targets().is_empty();
        match a {
            // ---- navigate ----
            Action::Back => {
                if !self.tab().can_back() {
                    return false;
                }
                self.go_history(-1);
            }
            Action::Forward => {
                if !self.tab().can_forward() {
                    return false;
                }
                self.go_history(1);
            }
            Action::Up => self.go_up(),
            Action::HomePage => self.navigate(HOME_URI),
            Action::UserHome => {
                if let Some(h) = cx_core::location::home_dir() {
                    self.navigate(&cx_core::Location::local(h).uri());
                }
            }
            Action::GoTo => {
                let current = match &self.tab().source {
                    Source::Folder => crate::util::display(self.tab().dir_uri()),
                    _ => String::new(),
                };
                self.dialogs.push(Dialog::Prompt(Prompt {
                    title: "Go to folder".into(),
                    label: "Path or URI (sftp://, smb://, ftp://, dav://, s3://, peer://, archive://…). Tab completes.".into(),
                    input: TextInput::new(current),
                    ok: "Go".into(),
                    kind: PromptKind::GoTo,
                    completions: Vec::new(),
                }));
            }
            Action::Reload => self.reload(),
            Action::PlacesLeft => self.places_menu(0),
            Action::PlacesRight => self.places_menu(if self.dual() { 1 } else { 0 }),
            Action::Hotlist => self.hotlist(),
            Action::RecentFolders => {
                let items: Vec<MenuItem> = self.settings.recent.iter().map(|u| MenuItem::new(crate::util::name_of(u), crate::util::display(u), MenuAction::Navigate { uri: u.clone(), pane: None })).collect();
                if items.is_empty() {
                    self.toast("No recent folders yet");
                } else {
                    self.dialogs.push(Dialog::Menu(Menu::new("Recent folders", items)));
                }
            }
            Action::ToggleBookmark => {
                if !self.tab().is_folder() {
                    return false;
                }
                let (title, uri) = (self.tab().title(), self.tab().dir_uri().to_string());
                let added = self.settings.toggle_bookmark(&title, &uri);
                self.toast(if added { format!("Added “{title}” to Favorites") } else { format!("Removed “{title}” from Favorites") });
                self.refresh_home();
                self.save_settings();
            }
            // ---- tabs & panes ----
            Action::NewTab => {
                let uri = self.tab().uri().to_string();
                let p = self.active;
                self.open_tab(p, &uri, true);
            }
            Action::CloseTab => self.close_tab(),
            Action::ReopenTab => {
                let p = self.active;
                if let Some((uri, view)) = self.panes[p].closed.pop() {
                    let id = self.open_tab(p, &uri, true);
                    if let Some(t) = self.tab_by_id_mut(id) {
                        t.view = view;
                    }
                }
            }
            Action::NextTab | Action::PrevTab => {
                let p = self.active;
                let n = self.panes[p].tabs.len();
                let step = if a == Action::NextTab { 1 } else { n - 1 };
                let next = (self.panes[p].active + step) % n;
                self.activate_tab(p, next);
            }
            Action::TabN(k) => {
                let p = self.active;
                let n = self.panes[p].tabs.len();
                let i = if k == 9 { n - 1 } else { (k as usize).saturating_sub(1) };
                if i >= n {
                    return false;
                }
                self.activate_tab(p, i);
            }
            Action::ToggleDual => {
                self.settings.dual = !self.settings.dual;
                if !self.settings.dual {
                    self.active = 0;
                }
                self.sync_watches();
            }
            Action::SwitchPane => {
                if !self.dual() {
                    return false;
                }
                self.active = 1 - self.active;
                self.focus = Focus::List;
            }
            Action::SwapPanes => {
                if !self.dual() {
                    return false;
                }
                self.panes.swap(0, 1);
                self.active = 1 - self.active;
            }
            Action::Mirror => {
                if !self.dual() {
                    return false;
                }
                let uri = self.tab().uri().to_string();
                let other = 1 - self.active;
                self.navigate_in(other, &uri, None);
            }
            // ---- file ----
            Action::Open => {
                if !has_targets {
                    return false;
                }
                self.open_targets(false);
            }
            Action::OpenInNewTab => self.open_targets(true),
            Action::OpenExternal => self.open_external(),
            Action::Edit => self.edit(),
            Action::QuickLook => {
                if !has_targets {
                    return false;
                }
                self.quicklook = !self.quicklook;
            }
            Action::Rename => self.rename_prompt(),
            Action::MultiRename => self.multi_rename(),
            Action::NewFolder => self.new_folder_prompt(),
            Action::Trash => self.trash(),
            Action::Delete => self.delete(),
            Action::Copy | Action::Cut => self.copy(a == Action::Cut),
            Action::Paste => self.paste(),
            Action::Duplicate => self.duplicate(),
            Action::Undo => self.undo_last(),
            Action::CopyToOther | Action::MoveToOther => self.to_other_pane(a == Action::MoveToOther),
            Action::CopyTo | Action::MoveTo => self.destination_picker(a == Action::MoveTo),
            Action::CopyPath => self.copy_path(),
            Action::Reveal => self.reveal(),
            Action::Shell => self.shell(),
            Action::TerminalWindow => {
                let uri = self.tab().dir_uri().to_string();
                let r = cx_engine::system::open_terminal(&uri);
                self.report(r);
            }
            Action::CalcSize => self.calc_sizes(),
            Action::Compress => self.compress_prompt(),
            Action::Extract => self.extract(),
            Action::Tags => self.tags_dialog(),
            Action::SendTo => self.send_to_menu(),
            Action::DiffFiles => self.diff_files(),
            Action::CompareDirs => {
                if !self.dual() {
                    self.toast("Turn on dual pane (F9) to compare folders");
                    return true;
                }
                let left = self.panes[0].tab().dir_uri().to_string();
                let right = self.panes[1].tab().dir_uri().to_string();
                self.navigate(&super::sources::compare_uri(&left, &right));
            }
            // ---- selection ----
            Action::SelectAll => self.tab_mut().select_all(),
            Action::SelectNone => self.tab_mut().clear_selection(),
            Action::InvertSelection => self.tab_mut().invert_selection(),
            Action::SelectPattern | Action::DeselectPattern => {
                let select = a == Action::SelectPattern;
                self.dialogs.push(Dialog::Prompt(Prompt {
                    title: if select { "Select by pattern".into() } else { "Deselect by pattern".into() },
                    label: "Wildcards like *.jpg; several separated by ;".into(),
                    input: TextInput::new("*"),
                    ok: if select { "Select".into() } else { "Deselect".into() },
                    kind: PromptKind::SelectPattern { select },
                    completions: Vec::new(),
                }));
            }
            // ---- view ----
            Action::ViewDetails | Action::ViewBrief | Action::ToggleView => {
                let v = match a {
                    Action::ViewDetails => ViewMode::Details,
                    Action::ViewBrief => ViewMode::Brief,
                    _ if self.tab().view == ViewMode::Details => ViewMode::Brief,
                    _ => ViewMode::Details,
                };
                if v == ViewMode::Brief {
                    self.collapse_all();
                }
                self.tab_mut().view = v;
                self.settings.default_view = v;
            }
            Action::TogglePreview => self.settings.preview_pane = !self.settings.preview_pane,
            Action::ToggleHidden => {
                self.settings.show_hidden = !self.settings.show_hidden;
                self.toast(if self.settings.show_hidden { "Showing hidden items" } else { "Hiding hidden items" });
            }
            Action::ToggleStripes => self.settings.stripes = !self.settings.stripes,
            Action::CollapseAll => self.collapse_all(),
            Action::ExpandAll => {
                let t = self.tab();
                let Some(r) = t.cursor_row().cloned() else { return false };
                if !t.item(&r).entry.is_dir || t.view != ViewMode::Details {
                    return false;
                }
                let uri = t.uri_of(&r);
                if t.is_expanded(&uri) {
                    self.collapse_uri(&uri);
                }
                let (p, i) = (self.active, self.pane().active);
                self.expand_uri(p, i, uri, true);
            }
            Action::Filter => self.focus = Focus::Filter,
            Action::Search => {
                let root = match &self.tab().source {
                    Source::Search { root, .. } => root.clone(),
                    Source::Folder => self.tab().dir_uri().to_string(),
                    _ => return false,
                };
                let name = TextInput::new(self.tab().filter.clone());
                self.dialogs.push(Dialog::Search(SearchForm { root, name, content: TextInput::default(), include_hidden: self.settings.show_hidden, focus: 0 }));
            }
            Action::SortBy(k) => {
                let s = self.settings.sort.toggled(k);
                self.set_sort(s);
            }
            Action::SortMenu => {
                let cur = self.settings.sort;
                let items = [SortKey::Name, SortKey::Modified, SortKey::Type, SortKey::Size]
                    .into_iter()
                    .map(|k| {
                        let mark = if cur.key == k { if cur.desc { "▼" } else { "▲" } } else { "" };
                        MenuItem::new(k.label(), mark, MenuAction::Sort(k))
                    })
                    .collect();
                self.dialogs.push(Dialog::Menu(Menu::new("Sort by", items)));
            }
            // ---- network ----
            Action::Connect => self.dialogs.push(Dialog::Connect(ConnectForm::new("smb", ""))),
            Action::Disconnect => {
                let uri = self.tab().dir_uri().to_string();
                if !uri.contains("://") || uri.starts_with("file:") || uri.starts_with("archive:") {
                    self.toast("This folder isn't on a server");
                    return true;
                }
                self.disconnect(uri);
            }
            Action::Pair => self.dialogs.push(Dialog::Pair(PairForm { address: TextInput::default(), code: TextInput::default(), focus: 0, error: None, busy: false })),
            Action::PeerSettings => {
                self.refresh_peer();
                self.dialogs.push(Dialog::Peer { cursor: 0 });
            }
            Action::Scan => {
                self.engine.refresh_discovery();
                self.toast("Scanning for nearby devices…");
            }
            // ---- app ----
            Action::Transfers => {
                if self.focus == Focus::Transfers {
                    self.focus = Focus::List;
                    self.transfers_open = false;
                } else {
                    self.transfers_open = true;
                    self.focus = Focus::Transfers;
                    self.transfers_cursor = 0;
                }
            }
            Action::Palette => self.open_palette(),
            Action::Help => self.dialogs.push(Dialog::Help { scroll: 0 }),
            Action::Settings => self.dialogs.push(Dialog::Settings { cursor: 0 }),
            Action::SwitchKeymap => {
                self.settings.keymap = match self.settings.keymap {
                    Keymap::Explorer => Keymap::Commander,
                    Keymap::Commander => Keymap::Explorer,
                };
                self.toast(format!("{} keys", self.settings.keymap.label()));
                self.save_settings();
            }
            Action::CycleTheme => {
                self.settings.theme = self.settings.theme.next();
                self.toast(format!("Theme: {}", self.settings.theme.label()));
            }
            Action::ToggleIcons => self.settings.nerd_icons = !self.settings.nerd_icons,
            Action::Quit => {
                self.save_settings();
                self.quit = true;
            }
        }
        true
    }

    pub fn set_sort(&mut self, sort: crate::sort::SortSpec) {
        self.settings.sort = sort;
        for p in self.panes.iter_mut() {
            for t in p.tabs.iter_mut() {
                for f in t.folders_mut() {
                    f.set_sort(sort);
                }
            }
        }
    }

    /// Alt+F1 / Alt+F2: home, standard folders, drives, favorites, servers
    /// and devices, opened in that pane.
    fn places_menu(&mut self, pane: usize) {
        let mut items = vec![MenuItem::new("Home page", "", MenuAction::Navigate { uri: HOME_URI.into(), pane: Some(pane) })];
        let mut seen = HashSet::new();
        let mut section = |items: &mut Vec<MenuItem>, title: &str, list: Vec<(String, String)>| {
            let list: Vec<_> = list.into_iter().filter(|(_, u)| seen.insert(u.clone())).collect();
            if !list.is_empty() {
                items.push(MenuItem::header(title));
                items.extend(list.into_iter().map(|(n, u)| MenuItem::new(n, crate::util::display(&u), MenuAction::Navigate { uri: u, pane: Some(pane) })));
            }
        };
        if let Some(p) = self.places.clone() {
            let mut places = vec![(p.home.name.clone(), p.home.uri.clone())];
            places.extend(p.favorites.iter().map(|f| (f.name.clone(), f.uri.clone())));
            section(&mut items, "Places", places);
            section(&mut items, "Drives", p.volumes.iter().map(|v| (format!("{}  {} free", v.name, crate::format::size(v.free)), v.uri.clone())).collect());
        }
        section(&mut items, "Favorites", self.settings.bookmarks.iter().map(|b| (b.name.clone(), b.uri.clone())).collect());
        let mut servers: Vec<(String, String)> = self.settings.servers.iter().map(|s| (s.name.clone(), s.uri.clone())).collect();
        servers.extend(self.engine.connections().into_iter().map(|c| (crate::util::display(&c), c)));
        section(&mut items, "Servers", servers);
        let devices: Vec<(String, String)> = self.devices.iter().filter(|d| !d.is_self()).flat_map(|d| d.shares.iter().map(|s| (format!("{} — {}", d.name, s.name), s.uri.clone())).chain(d.services.iter().map(|s| (format!("{} ({})", d.name, s.label), s.uri.clone()))).collect::<Vec<_>>()).collect();
        section(&mut items, "Nearby", devices);
        items.push(MenuItem::header("Network"));
        items.push(MenuItem::new("Connect to server…", crate::commands::shortcut(Action::Connect, self.settings.keymap).unwrap_or_default(), MenuAction::Run(Action::Connect)));
        let title = if self.dual() { if pane == 0 { "Left pane" } else { "Right pane" } } else { "Go to" };
        self.dialogs.push(Dialog::Menu(Menu::new(title, items)));
    }

    /// Ctrl+D: favorites, plus adding/removing the current folder.
    fn hotlist(&mut self) {
        let mut items: Vec<MenuItem> = self.settings.bookmarks.iter().map(|b| MenuItem::new(b.name.clone(), crate::util::display(&b.uri), MenuAction::Navigate { uri: b.uri.clone(), pane: None })).collect();
        if self.tab().is_folder() {
            let marked = self.settings.is_bookmarked(self.tab().dir_uri());
            items.push(MenuItem::header(""));
            items.push(MenuItem::new(if marked { "Remove current folder" } else { "Add current folder" }, crate::commands::shortcut(Action::ToggleBookmark, self.settings.keymap).unwrap_or_default(), MenuAction::Run(Action::ToggleBookmark)));
        }
        self.dialogs.push(Dialog::Menu(Menu::new("Favorites", items)));
    }

    /// Run a menu or palette choice.
    pub(crate) fn menu_action(&mut self, a: MenuAction) {
        match a {
            MenuAction::Navigate { uri, pane } => {
                let p = pane.unwrap_or(self.active);
                self.focus_pane(p);
                self.navigate_in(p, &uri, None);
            }
            MenuAction::Run(a) => {
                self.run(a);
            }
            MenuAction::Sort(k) => {
                let s = self.settings.sort.toggled(k);
                self.set_sort(s);
            }
            MenuAction::Sync(dir) => self.sync(dir),
            MenuAction::ActivateTab { pane, id } => {
                if let Some(i) = self.panes[pane].tabs.iter().position(|t| t.id == id) {
                    self.focus_pane(pane);
                    self.activate_tab(pane, i);
                }
            }
            MenuAction::ConnectTo { scheme, host } => self.dialogs.push(Dialog::Connect(ConnectForm::new(&scheme, &host))),
            MenuAction::SendTo(device) => self.send_to(device),
            MenuAction::None => {}
        }
    }
}
