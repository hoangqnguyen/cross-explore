//! Keyboard and mouse input for the file lists, the quick filter, Quick
//! Look and the transfers panel. Dialog keys live in `dialogs.rs`.

use super::{App, Focus};
use crate::commands::{resolve, Action};
use crate::dialog::{Dialog, Menu, MenuAction, MenuItem};
use crate::keys::KeyCombo;
use crate::settings::{Keymap, ViewMode};
use crate::ui::layout::{Hit, Layout};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};

/// Re-exported for the terminal loop.
pub type MouseHit = Hit;

impl App {
    pub fn handle_key(&mut self, e: KeyEvent) {
        if e.kind == KeyEventKind::Release {
            return;
        }
        if !self.dialogs.is_empty() {
            self.dialog_key(e);
            return;
        }
        if self.quicklook {
            self.quicklook_key(e);
            return;
        }
        match self.focus {
            Focus::Transfers => self.transfers_key(e),
            Focus::Filter => self.filter_key(e),
            Focus::List => self.list_key(e),
        }
    }

    fn list_key(&mut self, e: KeyEvent) {
        let ctrl = e.modifiers.contains(KeyModifiers::CONTROL);
        let alt = e.modifiers.contains(KeyModifiers::ALT);
        let shift = e.modifiers.contains(KeyModifiers::SHIFT);
        let commander = self.settings.keymap == Keymap::Commander;
        let brief = self.tab().view == ViewMode::Brief;
        let page = self.tab().page.get().max(1) as isize;
        let col = self.tab().column_rows.get().max(1) as isize;
        if !ctrl && !alt {
            let handled = match e.code {
                KeyCode::Up => {
                    self.tab_mut().move_by(-1, shift);
                    true
                }
                KeyCode::Down => {
                    self.tab_mut().move_by(1, shift);
                    true
                }
                KeyCode::PageUp => {
                    self.tab_mut().move_by(-page, shift);
                    true
                }
                KeyCode::PageDown => {
                    self.tab_mut().move_by(page, shift);
                    true
                }
                KeyCode::Home => {
                    self.tab_mut().move_to(0, shift);
                    true
                }
                KeyCode::End => {
                    self.tab_mut().move_to(isize::MAX / 2, shift);
                    true
                }
                KeyCode::Left if brief => {
                    self.tab_mut().move_by(-col, shift);
                    true
                }
                KeyCode::Right if brief => {
                    self.tab_mut().move_by(col, shift);
                    true
                }
                KeyCode::Left if !shift => {
                    self.outline_left();
                    true
                }
                KeyCode::Right if !shift => {
                    self.outline_right();
                    true
                }
                KeyCode::Insert if commander => {
                    let t = self.tab_mut();
                    let c = t.cursor;
                    t.toggle_at(c);
                    t.move_by(1, false);
                    true
                }
                KeyCode::Char(' ') if commander && !shift => {
                    // Total Commander: Space selects and counts folder sizes.
                    let t = self.tab_mut();
                    let c = t.cursor;
                    t.toggle_at(c);
                    if let Some(r) = self.tab().cursor_row() {
                        if self.tab().item(r).entry.is_dir {
                            let uri = self.tab().uri_of(r);
                            self.compute_size(uri);
                        }
                    }
                    true
                }
                KeyCode::Esc => {
                    let t = self.tab_mut();
                    if !t.filter.is_empty() {
                        t.filter.clear();
                    } else if !t.selection.is_empty() {
                        t.clear_selection();
                    } else if self.transfers_open {
                        self.transfers_open = false;
                    }
                    true
                }
                KeyCode::Backspace if !self.tab().filter.is_empty() => {
                    self.tab_mut().filter.pop();
                    true
                }
                _ => false,
            };
            if handled {
                return;
            }
        }
        if let Some(combo) = KeyCombo::from_event(&e) {
            for a in resolve(&combo, self.settings.keymap) {
                if self.run(a) {
                    return;
                }
            }
        }
        if let Some(c) = KeyCombo::typed_char(&e) {
            // Type to filter, Total Commander style.
            self.tab_mut().filter.push(c);
            self.tab_mut().move_to(0, false);
        }
    }

    /// ← in the outline: collapse, or go to the parent row, or up a folder.
    fn outline_left(&mut self) {
        let t = self.tab();
        let Some(r) = t.cursor_row().cloned() else {
            self.go_up();
            return;
        };
        let uri = t.uri_of(&r);
        if t.is_expanded(&uri) {
            self.collapse_uri(&uri);
        } else if let Some(p) = t.parent_row(t.cursor) {
            self.tab_mut().move_to(p as isize, false);
        } else if t.is_folder() {
            self.go_up();
        }
    }

    /// → in the outline: expand a folder, or step into an expanded one.
    fn outline_right(&mut self) {
        let t = self.tab();
        let Some(r) = t.cursor_row().cloned() else { return };
        if !t.item(&r).entry.is_dir {
            return;
        }
        let uri = t.uri_of(&r);
        if t.is_expanded(&uri) {
            let next = t.cursor + 1;
            if t.rows().get(next).is_some_and(|n| n.depth > r.depth) {
                self.tab_mut().move_to(next as isize, false);
            }
        } else {
            let (p, i) = (self.active, self.pane().active);
            self.expand_uri(p, i, uri, false);
        }
    }

    fn filter_key(&mut self, e: KeyEvent) {
        match e.code {
            KeyCode::Esc => {
                self.tab_mut().filter.clear();
                self.focus = Focus::List;
            }
            KeyCode::Enter | KeyCode::Down | KeyCode::Up | KeyCode::Tab => {
                self.focus = Focus::List;
                if e.code == KeyCode::Enter {
                    self.run(Action::Open);
                }
            }
            KeyCode::Backspace => {
                self.tab_mut().filter.pop();
            }
            _ => {
                if let Some(c) = KeyCombo::typed_char(&e) {
                    self.tab_mut().filter.push(c);
                    self.tab_mut().move_to(0, false);
                }
            }
        }
    }

    fn quicklook_key(&mut self, e: KeyEvent) {
        let scroll = |app: &mut App, d: isize| {
            if let Some(p) = app.preview.as_mut() {
                p.scroll = (p.scroll as isize + d).max(0) as usize;
            }
        };
        match e.code {
            KeyCode::Esc | KeyCode::F(3) | KeyCode::Char(' ') | KeyCode::Char('q') => self.quicklook = false,
            KeyCode::Up => self.tab_mut().move_by(-1, false),
            KeyCode::Down => self.tab_mut().move_by(1, false),
            KeyCode::PageDown | KeyCode::Char('j') => scroll(self, 20),
            KeyCode::PageUp | KeyCode::Char('k') => scroll(self, -20),
            KeyCode::Home => scroll(self, -1_000_000),
            KeyCode::Enter => {
                self.quicklook = false;
                self.run(Action::Open);
            }
            _ => {
                // Quit and help work from Quick Look too.
                if let Some(combo) = KeyCombo::from_event(&e) {
                    if let Some(a) = resolve(&combo, self.settings.keymap).into_iter().find(|a| matches!(a, Action::Quit | Action::Help | Action::Palette)) {
                        self.quicklook = a != Action::Quit && self.quicklook;
                        self.run(a);
                    }
                }
            }
        }
    }

    fn transfers_key(&mut self, e: KeyEvent) {
        let n = self.jobs.len();
        let job = self.jobs.get(self.transfers_cursor).cloned();
        match e.code {
            KeyCode::Esc | KeyCode::Tab => self.focus = Focus::List,
            KeyCode::Char('j') if e.modifiers.contains(KeyModifiers::CONTROL) => {
                self.focus = Focus::List;
                self.transfers_open = false;
            }
            KeyCode::Up => self.transfers_cursor = self.transfers_cursor.saturating_sub(1),
            KeyCode::Down => self.transfers_cursor = (self.transfers_cursor + 1).min(n.saturating_sub(1)),
            KeyCode::Char(' ') | KeyCode::Char('p') => {
                if let Some(j) = job {
                    if j.state == "paused" {
                        self.engine.resume(j.id);
                    } else if !j.is_finished() {
                        self.engine.pause(j.id);
                    }
                }
            }
            KeyCode::Delete | KeyCode::Char('c') | KeyCode::Char('x') => {
                if let Some(j) = job.filter(|j| !j.is_finished()) {
                    self.engine.cancel(j.id);
                }
            }
            KeyCode::Char('C') | KeyCode::Char('X') => {
                self.engine.clear_finished();
                self.jobs.retain(|j| !j.is_finished());
                self.transfers_cursor = 0;
            }
            KeyCode::Enter => {
                if let Some(c) = job.as_ref().and_then(|j| j.conflict.clone().map(|c| (j.id, c))) {
                    self.dialogs.push(Dialog::Conflict(crate::dialog::ConflictDlg { job: c.0, conflict: c.1, apply_all: false, cursor: 0 }));
                }
            }
            _ => {
                // App-wide commands (quit, help, palette, dual…) still work.
                if let Some(combo) = KeyCombo::from_event(&e) {
                    if let Some(a) = resolve(&combo, self.settings.keymap).into_iter().find(|a| matches!(a, Action::Quit | Action::Help | Action::Palette | Action::ToggleDual | Action::Settings | Action::Connect | Action::GoTo)) {
                        self.focus = Focus::List;
                        self.run(a);
                    }
                }
            }
        }
    }

    /// Mouse: click selects (Ctrl toggles, Shift extends), double-click
    /// opens, the wheel moves, header clicks sort, right click opens the
    /// item's menu.
    pub fn handle_mouse(&mut self, e: MouseEvent, layout: &Layout) {
        if !self.dialogs.is_empty() {
            if let MouseEventKind::ScrollDown | MouseEventKind::ScrollUp = e.kind {
                let down = matches!(e.kind, MouseEventKind::ScrollDown);
                self.dialog_key(KeyEvent::new(if down { KeyCode::Down } else { KeyCode::Up }, KeyModifiers::NONE));
            }
            return;
        }
        let hit = layout.hit(e.column, e.row, self);
        let ctrl = e.modifiers.contains(KeyModifiers::CONTROL);
        let shift = e.modifiers.contains(KeyModifiers::SHIFT);
        match (e.kind, hit) {
            (MouseEventKind::ScrollDown | MouseEventKind::ScrollUp, Hit::Preview) => {
                let d: isize = if matches!(e.kind, MouseEventKind::ScrollDown) { 3 } else { -3 };
                if let Some(p) = self.preview.as_mut() {
                    p.scroll = (p.scroll as isize + d).max(0) as usize;
                }
            }
            (MouseEventKind::ScrollDown | MouseEventKind::ScrollUp, Hit::Row { pane, .. } | Hit::List { pane } | Hit::Pane { pane }) => {
                let d: isize = if matches!(e.kind, MouseEventKind::ScrollDown) { 3 } else { -3 };
                self.focus_pane(pane);
                self.tab_mut().move_by(d, false);
            }
            (MouseEventKind::ScrollDown | MouseEventKind::ScrollUp, Hit::Transfers { .. }) => {
                let down = matches!(e.kind, MouseEventKind::ScrollDown);
                self.transfers_cursor = if down { (self.transfers_cursor + 1).min(self.jobs.len().saturating_sub(1)) } else { self.transfers_cursor.saturating_sub(1) };
            }
            (MouseEventKind::Down(MouseButton::Left), Hit::Tab { pane, idx }) => {
                self.focus_pane(pane);
                self.activate_tab(pane, idx);
            }
            (MouseEventKind::Down(MouseButton::Left), Hit::Column { pane, key }) => {
                self.focus_pane(pane);
                let s = self.settings.sort.toggled(key);
                self.set_sort(s);
            }
            (MouseEventKind::Down(MouseButton::Left), Hit::Crumb { pane, uri }) => {
                self.focus_pane(pane);
                self.navigate(&uri);
            }
            (MouseEventKind::Down(MouseButton::Left), Hit::Row { pane, row }) => {
                self.focus_pane(pane);
                self.focus = Focus::List;
                let double = self.last_click.is_some_and(|(x, y, t)| x == e.column && y == e.row && t.elapsed() < Duration::from_millis(450));
                self.last_click = Some((e.column, e.row, Instant::now()));
                let t = self.tab_mut();
                if ctrl {
                    t.toggle_at(row);
                } else if shift {
                    t.move_to(row as isize, true);
                } else {
                    let key = t.rows().get(row).map(|r| t.key_of(r).to_string());
                    let single = key.as_ref().is_none_or(|k| !t.selection.contains(k)) || t.selection.len() <= 1;
                    if single {
                        t.select_only(row);
                    } else {
                        t.move_to(row as isize, false);
                    }
                }
                if double {
                    self.last_click = None;
                    self.run(Action::Open);
                }
            }
            (MouseEventKind::Down(MouseButton::Right), Hit::Row { pane, row }) => {
                self.focus_pane(pane);
                let t = self.tab_mut();
                let key = t.rows().get(row).map(|r| t.key_of(r).to_string());
                if key.as_ref().is_none_or(|k| !t.selection.contains(k)) {
                    t.select_only(row);
                }
                self.context_menu();
            }
            (MouseEventKind::Down(MouseButton::Left), Hit::List { pane } | Hit::Pane { pane }) => {
                self.focus_pane(pane);
                self.tab_mut().clear_selection();
            }
            (MouseEventKind::Down(MouseButton::Left), Hit::Transfers { row }) => {
                self.focus = Focus::Transfers;
                self.transfers_cursor = row.min(self.jobs.len().saturating_sub(1));
            }
            _ => {}
        }
    }

    pub(crate) fn focus_pane(&mut self, pane: usize) {
        if self.dual() && pane != self.active {
            self.active = pane;
        }
    }

    /// The item menu (right click, or the palette's "Actions for selection").
    pub(crate) fn context_menu(&mut self) {
        let many = self.tab().targets().len() > 1;
        let is_dir = self.tab().cursor_item().is_some_and(|i| i.entry.is_dir);
        let is_archive = self.tab().cursor_item().is_some_and(|i| cx_archive::is_archive(i.name()));
        let title = self.tab().cursor_item().map(|i| i.name().to_string()).unwrap_or_default();
        let km = self.settings.keymap;
        let located = self.tab().cursor_row().is_some_and(|r| r.depth > 0 || self.tab().cursor_item().is_some_and(|i| i.parent.is_some()));
        let mut actions = vec![Action::Open];
        if located {
            actions.push(Action::ShowInFolder);
        }
        if is_dir || is_archive {
            actions.push(Action::OpenInNewTab);
        }
        actions.extend([Action::QuickLook, Action::OpenExternal, Action::Edit, Action::Cut, Action::Copy]);
        if self.dual() {
            actions.extend([Action::CopyToOther, Action::MoveToOther]);
        }
        actions.extend([Action::CopyTo, Action::MoveTo, Action::Duplicate]);
        actions.push(if many { Action::MultiRename } else { Action::Rename });
        actions.extend([Action::Tags, Action::CopyPath, Action::Reveal]);
        if is_archive {
            actions.push(Action::Extract);
        }
        actions.push(Action::Compress);
        if is_dir {
            actions.push(Action::CalcSize);
        }
        actions.extend([Action::SendTo, Action::Trash, Action::Delete]);
        let items = actions
            .into_iter()
            .filter_map(|a| crate::commands::by_action(a).map(|c| MenuItem::new(c.label, crate::commands::shortcut(a, km).unwrap_or_default(), MenuAction::Run(a))))
            .collect();
        self.dialogs.push(Dialog::Menu(Menu::new(title, items)));
    }

    /// Bracketed paste: into the focused text field, or the quick filter.
    pub fn paste_text(&mut self, text: &str) {
        let text = text.trim_end_matches(['\n', '\r']);
        let Some(d) = self.dialogs.last_mut() else {
            self.tab_mut().filter.push_str(text);
            return;
        };
        let input = match d {
            Dialog::Prompt(p) => Some(&mut p.input),
            Dialog::Palette(p) => Some(&mut p.input),
            Dialog::DestPicker(p) => Some(&mut p.input),
            Dialog::Tags(t) => Some(&mut t.input),
            Dialog::Search(s) => Some(if s.focus == 1 { &mut s.content } else { &mut s.name }),
            Dialog::Pair(p) => Some(if p.focus == 1 { &mut p.code } else { &mut p.address }),
            Dialog::SignIn(s) => match s.focus {
                0 => Some(&mut s.user),
                1 => Some(&mut s.password),
                2 => Some(&mut s.key_file),
                _ => None,
            },
            Dialog::Connect(c) => match c.focus {
                1 => Some(&mut c.host),
                2 => Some(&mut c.port),
                3 => Some(&mut c.path),
                4 => Some(&mut c.user),
                5 => Some(&mut c.password),
                6 => Some(&mut c.key_file),
                _ => None,
            },
            Dialog::MultiRename(m) => match m.focus {
                0 => Some(&mut m.name_mask),
                1 => Some(&mut m.ext_mask),
                2 => Some(&mut m.search),
                3 => Some(&mut m.replace),
                _ => None,
            },
            _ => None,
        };
        if let Some(i) = input {
            i.insert_str(text);
        }
        match d {
            Dialog::Palette(p) => super::dialogs::rank_palette(p),
            Dialog::Connect(c) if c.focus == 1 => c.absorb_uri(),
            _ => {}
        }
    }
}
