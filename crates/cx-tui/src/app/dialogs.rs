//! Keys for dialogs (the top of the stack gets them), and the command
//! palette's candidates.

use super::App;
use crate::commands::{shortcut, Action, COMMANDS};
use crate::dialog::*;
use crate::input::TextInput;
use crate::keys::KeyCombo;
use crate::settings::Keymap;
use crate::tab::Source;
use cx_transfer::Resolution;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Local path completions for a partially typed path.
pub fn complete_path(text: &str) -> Vec<String> {
    let expanded = if let Some(rest) = text.strip_prefix('~') {
        match cx_core::location::home_dir() {
            Some(h) => format!("{}{}", h.display(), rest),
            None => return vec![],
        }
    } else {
        text.to_string()
    };
    let path = std::path::Path::new(&expanded);
    let (dir, prefix) = if expanded.ends_with('/') || expanded.ends_with(std::path::MAIN_SEPARATOR)
    {
        (path.to_path_buf(), String::new())
    } else {
        (
            path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let lp = prefix.to_lowercase();
    let mut out: Vec<String> = rd
        .flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false) || e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| {
            n.to_lowercase().starts_with(&lp) && (!n.starts_with('.') || lp.starts_with('.'))
        })
        .map(|n| {
            format!(
                "{}{}{}",
                dir.display().to_string().trim_end_matches('/'),
                if dir.as_os_str().is_empty() { "" } else { "/" },
                n
            ) + "/"
        })
        .collect();
    out.sort();
    out
}

/// Longest common prefix of the completions.
fn common_prefix(v: &[String]) -> String {
    let Some(first) = v.first() else {
        return String::new();
    };
    let mut end = first.len();
    for s in &v[1..] {
        end = end.min(
            first
                .bytes()
                .zip(s.bytes())
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    while !first.is_char_boundary(end) {
        end -= 1;
    }
    first[..end].to_string()
}

fn cycle(focus: &mut usize, n: usize, e: &KeyEvent) -> bool {
    let back = e.modifiers.contains(KeyModifiers::SHIFT)
        || e.code == KeyCode::BackTab
        || e.code == KeyCode::Up;
    match e.code {
        KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up => {
            *focus = if back {
                (*focus + n - 1) % n
            } else {
                (*focus + 1) % n
            };
            true
        }
        _ => false,
    }
}

impl App {
    pub(crate) fn dialog_key(&mut self, e: KeyEvent) {
        let Some(mut dlg) = self.dialogs.pop() else {
            return;
        };
        let base = self.dialogs.len();
        let keep = self.dialog_key_inner(&mut dlg, e);
        if keep {
            // Handlers may have pushed dialogs above this one (a host-key
            // prompt over the connect form): keep ours underneath them.
            let pushed = self.dialogs.split_off(base.min(self.dialogs.len()));
            self.dialogs.push(dlg);
            self.dialogs.extend(pushed);
        }
    }

    /// Returns whether the dialog stays open. Handlers that push new
    /// dialogs do so after this one was popped, so those land on top.
    fn dialog_key_inner(&mut self, dlg: &mut Dialog, e: KeyEvent) -> bool {
        let esc = e.code == KeyCode::Esc;
        let enter = e.code == KeyCode::Enter;
        match dlg {
            Dialog::Info { .. } => false,
            Dialog::Help { scroll } => match e.code {
                KeyCode::Esc | KeyCode::F(1) | KeyCode::Char('q') | KeyCode::Enter => false,
                KeyCode::Down | KeyCode::Char('j') => {
                    *scroll += 1;
                    true
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    *scroll = scroll.saturating_sub(1);
                    true
                }
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    *scroll += 20;
                    true
                }
                KeyCode::PageUp => {
                    *scroll = scroll.saturating_sub(20);
                    true
                }
                KeyCode::Home => {
                    *scroll = 0;
                    true
                }
                _ => true,
            },
            Dialog::Diff(d) => {
                let changes: Vec<usize> = d
                    .lines
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.tag != ' ')
                    .map(|(i, _)| i)
                    .collect();
                match e.code {
                    KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => return false,
                    KeyCode::Down | KeyCode::Char('j') => d.scroll += 1,
                    KeyCode::Up | KeyCode::Char('k') => d.scroll = d.scroll.saturating_sub(1),
                    KeyCode::PageDown | KeyCode::Char(' ') => d.scroll += 20,
                    KeyCode::PageUp => d.scroll = d.scroll.saturating_sub(20),
                    KeyCode::Home => d.scroll = 0,
                    KeyCode::End => d.scroll = d.lines.len().saturating_sub(1),
                    KeyCode::Char('n') => {
                        if let Some(&i) = changes.iter().find(|&&i| i > d.scroll + 2) {
                            d.scroll = i.saturating_sub(2);
                        }
                    }
                    KeyCode::Char('p') => {
                        if let Some(&i) = changes.iter().rev().find(|&&i| i + 2 < d.scroll) {
                            d.scroll = i.saturating_sub(2);
                        }
                    }
                    _ => {}
                }
                d.scroll = d.scroll.min(d.lines.len().saturating_sub(1));
                true
            }
            Dialog::Menu(m) => {
                if esc {
                    return false;
                }
                match e.code {
                    KeyCode::Up => m.step(-1),
                    KeyCode::Down => m.step(1),
                    KeyCode::PageUp => m.step(-10),
                    KeyCode::PageDown => m.step(10),
                    KeyCode::Home => m.step(-100_000),
                    KeyCode::End => m.step(100_000),
                    KeyCode::Enter => {
                        if let Some(item) = m.selected().cloned() {
                            self.menu_action(item.action);
                        }
                        return false;
                    }
                    KeyCode::Backspace => {
                        m.filter.pop();
                        m.fix_cursor();
                    }
                    _ => {
                        if let Some(c) = KeyCombo::typed_char(&e) {
                            m.filter.push(c);
                            m.fix_cursor();
                        }
                    }
                }
                true
            }
            Dialog::Palette(p) => {
                match e.code {
                    KeyCode::Esc => return false,
                    KeyCode::Up => p.cursor = p.cursor.saturating_sub(1),
                    KeyCode::Down => {
                        p.cursor = (p.cursor + 1).min(p.results.len().saturating_sub(1))
                    }
                    KeyCode::PageUp => p.cursor = p.cursor.saturating_sub(10),
                    KeyCode::PageDown => {
                        p.cursor = (p.cursor + 10).min(p.results.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some(action) = p
                            .results
                            .get(p.cursor)
                            .and_then(|(i, _)| p.items.get(*i))
                            .map(|it| it.action.clone())
                        {
                            self.menu_action(action);
                        }
                        return false;
                    }
                    _ => {
                        if p.input.handle(&e) {
                            rank_palette(p);
                        }
                    }
                }
                true
            }
            Dialog::Confirm(c) => match e.code {
                KeyCode::Esc | KeyCode::Char('n') => false,
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                    c.on_cancel = !c.on_cancel;
                    true
                }
                KeyCode::Char('y') => {
                    self.confirmed(c.then.clone());
                    false
                }
                KeyCode::Enter => {
                    if !c.on_cancel {
                        self.confirmed(c.then.clone());
                    }
                    false
                }
                _ => true,
            },
            Dialog::Prompt(p) => {
                if esc {
                    return false;
                }
                if enter {
                    let value = p.input.text.clone();
                    self.prompt_done(p.kind.clone(), value);
                    return false;
                }
                if e.code == KeyCode::Tab && p.kind == PromptKind::GoTo {
                    let c = complete_path(&p.input.text);
                    match c.len() {
                        0 => {}
                        1 => p.input.set(c[0].clone()),
                        _ => {
                            let prefix = common_prefix(&c);
                            if prefix.len() > p.input.text.len() {
                                p.input.set(prefix);
                            }
                        }
                    }
                    p.completions = c;
                    return true;
                }
                if p.input.handle(&e) {
                    p.completions.clear();
                }
                true
            }
            Dialog::Connect(f) => {
                if f.busy {
                    return !esc;
                }
                if esc {
                    return false;
                }
                if enter {
                    // The connect call reads the form from the stack.
                    self.dialogs.push(dlg.clone());
                    self.connect_submit();
                    // Drop the copy we pushed: the caller re-pushes `dlg`.
                    if let Some(pos) = self
                        .dialogs
                        .iter()
                        .rposition(|d| matches!(d, Dialog::Connect(_)))
                    {
                        let updated = self.dialogs.remove(pos);
                        *dlg = updated;
                    }
                    return true;
                }
                if cycle(&mut f.focus, ConnectForm::FIELDS, &e) {
                    if f.focus == 6 && f.scheme() != "sftp" {
                        let back = e.modifiers.contains(KeyModifiers::SHIFT)
                            || matches!(e.code, KeyCode::Up | KeyCode::BackTab);
                        f.focus = if back { 5 } else { 7 };
                    }
                    if f.focus != 1 {
                        f.absorb_uri();
                    }
                    return true;
                }
                match f.focus {
                    0 => match e.code {
                        KeyCode::Left => {
                            f.scheme = (f.scheme + PROTOCOLS.len() - 1) % PROTOCOLS.len()
                        }
                        KeyCode::Right | KeyCode::Char(' ') => {
                            f.scheme = (f.scheme + 1) % PROTOCOLS.len()
                        }
                        _ => {}
                    },
                    1 => {
                        f.host.handle(&e);
                    }
                    2 => {
                        f.port.handle(&e);
                    }
                    3 => {
                        f.path.handle(&e);
                    }
                    4 => {
                        f.user.handle(&e);
                    }
                    5 => {
                        f.password.handle(&e);
                    }
                    6 => {
                        f.key_file.handle(&e);
                    }
                    7 if e.code == KeyCode::Char(' ') => f.anonymous = !f.anonymous,
                    8 if e.code == KeyCode::Char(' ') => f.remember = !f.remember,
                    9 if e.code == KeyCode::Char(' ') => f.save = !f.save,
                    _ => {}
                }
                true
            }
            Dialog::SignIn(s) => {
                if s.busy {
                    return !esc;
                }
                if esc {
                    return false;
                }
                if enter {
                    self.dialogs.push(dlg.clone());
                    self.sign_in_submit();
                    if let Some(pos) = self
                        .dialogs
                        .iter()
                        .rposition(|d| matches!(d, Dialog::SignIn(_)))
                    {
                        *dlg = self.dialogs.remove(pos);
                    }
                    return true;
                }
                if cycle(&mut s.focus, 4, &e) {
                    return true;
                }
                match s.focus {
                    0 => {
                        s.user.handle(&e);
                    }
                    1 => {
                        s.password.handle(&e);
                    }
                    2 => {
                        s.key_file.handle(&e);
                    }
                    3 if e.code == KeyCode::Char(' ') => s.remember = !s.remember,
                    _ => {}
                }
                true
            }
            Dialog::HostKey(h) => match e.code {
                KeyCode::Esc => {
                    if h.then == AfterAuth::Connect {
                        if let Some(Dialog::Connect(f)) = self.dialogs.last_mut() {
                            f.error = Some("Connection cancelled".into());
                        }
                    }
                    false
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                    h.on_trust = !h.on_trust;
                    true
                }
                KeyCode::Char('t') => {
                    self.trust_key(h.clone());
                    false
                }
                KeyCode::Enter => {
                    if h.on_trust {
                        self.trust_key(h.clone());
                    }
                    false
                }
                _ => true,
            },
            Dialog::Conflict(c) => {
                let resolve = |app: &mut App, c: &ConflictDlg, r: Resolution| {
                    app.engine.resolve(c.job, c.conflict.id, r, c.apply_all)
                };
                match e.code {
                    KeyCode::Esc => return false,
                    KeyCode::Left | KeyCode::BackTab => {
                        c.cursor = (c.cursor + CONFLICT_CHOICES.len() - 1) % CONFLICT_CHOICES.len()
                    }
                    KeyCode::Right | KeyCode::Tab => {
                        c.cursor = (c.cursor + 1) % CONFLICT_CHOICES.len()
                    }
                    KeyCode::Char(' ') | KeyCode::Char('a') => c.apply_all = !c.apply_all,
                    KeyCode::Char('c') => {
                        self.engine.cancel(c.job);
                        return false;
                    }
                    KeyCode::Enter
                    | KeyCode::Char('r')
                    | KeyCode::Char('s')
                    | KeyCode::Char('k')
                    | KeyCode::Char('n') => {
                        let choice = match e.code {
                            KeyCode::Char('r') => 0,
                            KeyCode::Char('s') => 1,
                            KeyCode::Char('k') => 2,
                            KeyCode::Char('n') => 3,
                            _ => c.cursor,
                        };
                        let r = [
                            Resolution::Replace,
                            Resolution::Skip,
                            Resolution::KeepBoth,
                            Resolution::ReplaceIfNewer,
                        ][choice];
                        resolve(self, c, r);
                        return false;
                    }
                    _ => {}
                }
                true
            }
            Dialog::DestPicker(p) => {
                if esc {
                    return false;
                }
                let vis = p.visible();
                match e.code {
                    KeyCode::Up => p.cursor = p.cursor.saturating_sub(1),
                    KeyCode::Down => p.cursor = (p.cursor + 1).min(vis.len().saturating_sub(1)),
                    KeyCode::PageUp => p.cursor = p.cursor.saturating_sub(10),
                    KeyCode::PageDown => {
                        p.cursor = (p.cursor + 10).min(vis.len().saturating_sub(1))
                    }
                    KeyCode::Char(' ') if p.input.text.is_empty() => {
                        if let Some(&i) = vis.get(p.cursor) {
                            if p.moving {
                                p.checked.clear();
                            }
                            if !p.checked.remove(&i) {
                                p.checked.insert(i);
                            }
                            p.cursor = (p.cursor + 1).min(vis.len().saturating_sub(1));
                        }
                    }
                    KeyCode::Tab => {
                        let c = complete_path(&p.input.text);
                        if c.len() == 1 {
                            p.input.set(c[0].clone());
                        } else if c.len() > 1 {
                            let prefix = common_prefix(&c);
                            if prefix.len() > p.input.text.len() {
                                p.input.set(prefix);
                            }
                        }
                    }
                    KeyCode::Enter => {
                        let dests = p.chosen();
                        if dests.is_empty() {
                            return true;
                        }
                        let (moving, sources) = (p.moving, p.sources.clone());
                        self.transfer_to(moving, sources, dests);
                        return false;
                    }
                    _ => {
                        if p.input.handle(&e) {
                            p.cursor = 0;
                        }
                    }
                }
                true
            }
            Dialog::MultiRename(m) => {
                if esc {
                    return false;
                }
                if enter {
                    let snapshot = m.clone();
                    self.apply_multi_rename(&snapshot);
                    return snapshot.plan().iter().any(|p| !p.problem.is_empty());
                }
                match e.code {
                    KeyCode::PageDown => {
                        m.scroll = (m.scroll + 10).min(m.items.len().saturating_sub(1));
                        return true;
                    }
                    KeyCode::PageUp => {
                        m.scroll = m.scroll.saturating_sub(10);
                        return true;
                    }
                    _ => {}
                }
                if cycle(&mut m.focus, MultiRename::FIELDS, &e) {
                    return true;
                }
                match m.focus {
                    0 => {
                        m.name_mask.handle(&e);
                    }
                    1 => {
                        m.ext_mask.handle(&e);
                    }
                    2 => {
                        m.search.handle(&e);
                    }
                    3 => {
                        m.replace.handle(&e);
                    }
                    4 if matches!(e.code, KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right) => {
                        m.regex = !m.regex
                    }
                    5 if matches!(e.code, KeyCode::Char(' ') | KeyCode::Right) => {
                        m.case = m.case.next()
                    }
                    5 if e.code == KeyCode::Left => m.case = m.case.next().next().next(),
                    6 => {
                        m.start.handle(&e);
                    }
                    7 => {
                        m.step.handle(&e);
                    }
                    8 => {
                        m.digits.handle(&e);
                    }
                    _ => {}
                }
                true
            }
            Dialog::Search(s) => {
                if esc {
                    return false;
                }
                if enter {
                    let content = !s.content.text.trim().is_empty();
                    let text = if content {
                        s.content.text.clone()
                    } else {
                        s.name.text.clone()
                    };
                    if text.trim().is_empty() {
                        return true;
                    }
                    let uri = super::sources::search_uri(&s.root, &text, content, s.include_hidden);
                    self.tab_mut().filter.clear();
                    self.navigate(&uri);
                    return false;
                }
                if cycle(&mut s.focus, 3, &e) {
                    return true;
                }
                match s.focus {
                    0 => {
                        s.name.handle(&e);
                    }
                    1 => {
                        s.content.handle(&e);
                    }
                    _ if e.code == KeyCode::Char(' ') => s.include_hidden = !s.include_hidden,
                    _ => {}
                }
                true
            }
            Dialog::Tags(t) => {
                if esc {
                    return false;
                }
                match e.code {
                    KeyCode::Up => t.cursor = t.cursor.saturating_sub(1),
                    KeyCode::Down => t.cursor = (t.cursor + 1).min(t.tags.len().saturating_sub(1)),
                    KeyCode::Char(' ') if t.input.text.is_empty() => {
                        if let Some((_, s)) = t.tags.get_mut(t.cursor) {
                            *s = if *s == TagState::All {
                                TagState::None
                            } else {
                                TagState::All
                            };
                        }
                    }
                    KeyCode::Enter => {
                        let name = t.input.text.trim().to_string();
                        if !name.is_empty() {
                            match t.tags.iter_mut().find(|(n, _)| *n == name) {
                                Some((_, s)) => *s = TagState::All,
                                None => t.tags.push((name, TagState::All)),
                            }
                            t.input = TextInput::default();
                            return true;
                        }
                        self.apply_tags(t.clone());
                        return false;
                    }
                    _ => {
                        t.input.handle(&e);
                    }
                }
                true
            }
            Dialog::Pair(f) => {
                if f.busy {
                    return !esc;
                }
                if esc {
                    return false;
                }
                if enter {
                    self.dialogs.push(dlg.clone());
                    self.pair_submit();
                    if let Some(pos) = self
                        .dialogs
                        .iter()
                        .rposition(|d| matches!(d, Dialog::Pair(_)))
                    {
                        *dlg = self.dialogs.remove(pos);
                    }
                    return true;
                }
                if cycle(&mut f.focus, 2, &e) {
                    return true;
                }
                if f.focus == 0 {
                    f.address.handle(&e);
                } else {
                    f.code.handle(&e);
                }
                true
            }
            Dialog::Peer { cursor } => {
                if esc {
                    return false;
                }
                let items = self.peer_items();
                let selectable: Vec<usize> =
                    (0..items.len()).filter(|&i| !items[i].header).collect();
                let pos = selectable.iter().position(|&i| i == *cursor).unwrap_or(0);
                match e.code {
                    KeyCode::Up => *cursor = selectable[pos.saturating_sub(1)],
                    KeyCode::Down => *cursor = selectable[(pos + 1).min(selectable.len() - 1)],
                    _ => {}
                }
                let Some(p) = self.peer.clone() else {
                    return true;
                };
                let share_at = |i: usize| i.checked_sub(6).filter(|&k| k < p.shares.len());
                let device_at = |i: usize| {
                    i.checked_sub(7 + p.shares.len())
                        .filter(|&k| k < p.trusted.len())
                };
                match e.code {
                    KeyCode::Enter | KeyCode::Char(' ') => match *cursor {
                        0 => self.peer_set_enabled(!p.enabled),
                        1 => self.peer_set_auto_trust(!p.tailnet_auto_trust),
                        2 => self.peer_show_code(),
                        3 => self.dialogs.push(Dialog::Pair(PairForm {
                            address: TextInput::default(),
                            code: TextInput::default(),
                            focus: 0,
                            error: None,
                            busy: false,
                        })),
                        4 => self.peer_share_current(),
                        _ => {}
                    },
                    KeyCode::Char('r') => {
                        if let Some(k) = share_at(*cursor) {
                            let mut shares = p.shares.clone();
                            shares[k].read_only = !shares[k].read_only;
                            self.peer_set_shares(shares);
                        }
                    }
                    KeyCode::Delete | KeyCode::Backspace | KeyCode::Char('d') => {
                        if let Some(k) = share_at(*cursor) {
                            let mut shares = p.shares.clone();
                            shares.remove(k);
                            self.peer_set_shares(shares);
                        } else if let Some(k) = device_at(*cursor) {
                            self.peer_forget(p.trusted[k].id.clone());
                        }
                    }
                    _ => {}
                }
                true
            }
            Dialog::Offer { offer, cursor } => match e.code {
                KeyCode::Up | KeyCode::Left | KeyCode::BackTab => {
                    *cursor = (*cursor + 2) % 3;
                    true
                }
                KeyCode::Down | KeyCode::Right | KeyCode::Tab => {
                    *cursor = (*cursor + 1) % 3;
                    true
                }
                KeyCode::Enter => {
                    let here = self
                        .tab()
                        .is_folder()
                        .then(|| self.tab().dir_uri().to_string())
                        .filter(|u| u.starts_with("file:"));
                    match *cursor {
                        0 => self.respond_offer(offer.id.clone(), true, None),
                        1 => self.respond_offer(offer.id.clone(), true, here),
                        _ => self.respond_offer(offer.id.clone(), false, None),
                    }
                    false
                }
                KeyCode::Esc => {
                    self.respond_offer(offer.id.clone(), false, None);
                    false
                }
                _ => true,
            },
            Dialog::Settings { cursor } => {
                const N: usize = 12;
                match e.code {
                    KeyCode::Esc | KeyCode::Char('q') => {
                        self.save_settings();
                        return false;
                    }
                    KeyCode::Up => *cursor = cursor.saturating_sub(1),
                    KeyCode::Down => *cursor = (*cursor + 1).min(N - 1),
                    KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                        let s = &mut self.settings;
                        match *cursor {
                            0 => {
                                s.keymap = if s.keymap == Keymap::Explorer {
                                    Keymap::Commander
                                } else {
                                    Keymap::Explorer
                                }
                            }
                            1 => s.theme = s.theme.next(),
                            2 => s.nerd_icons = !s.nerd_icons,
                            3 => s.show_hidden = !s.show_hidden,
                            4 => s.stripes = !s.stripes,
                            5 => s.preview_pane = !s.preview_pane,
                            6 => s.confirm_trash = !s.confirm_trash,
                            7 => s.confirm_permanent_delete = !s.confirm_permanent_delete,
                            8 => s.confirm_transfer = !s.confirm_transfer,
                            9 => s.restore_session = !s.restore_session,
                            10 => s.mouse = !s.mouse,
                            _ => s.dual = !s.dual,
                        }
                        if *cursor == 11 {
                            self.sync_watches();
                        }
                    }
                    _ => {}
                }
                true
            }
        }
    }

    /// A confirmation was accepted.
    fn confirmed(&mut self, then: Pending) {
        match then {
            Pending::Delete(uris) => {
                self.submit("delete", uris, None, cx_transfer::ConflictPolicy::Ask);
            }
            Pending::Transfer {
                kind,
                sources,
                dests,
            } => {
                for d in dests {
                    self.submit(
                        kind,
                        sources.clone(),
                        Some(d),
                        cx_transfer::ConflictPolicy::Ask,
                    );
                }
            }
            Pending::Trash(groups) => self.trash_now(groups),
            Pending::Upload { local, dest_dir } => {
                self.submit(
                    "copy",
                    vec![local],
                    Some(dest_dir),
                    cx_transfer::ConflictPolicy::Replace,
                );
            }
            Pending::Disconnect(uri) => self.disconnect(uri),
            Pending::Quit => self.quit = true,
            Pending::SshCopyId {
                user,
                host,
                port,
                control,
            } => {
                if control.is_none() {
                    // No multiplexed session (Windows, or a socket path that
                    // was too long). The password is used once and not saved.
                    self.dialogs.push(Dialog::Prompt(Prompt {
                        title: "Server password".into(),
                        label: format!(
                            "Password for {}. It is used once to install the key and is not saved.",
                            cx_term::user_at_host(&user, &host)
                        ),
                        input: TextInput::secret(),
                        ok: "Install key".into(),
                        kind: PromptKind::SshPassword { user, host, port },
                        completions: Vec::new(),
                    }));
                    return;
                }
                match cx_term::install_public_key(&user, &host, port, control.as_deref(), None) {
                    Ok(msg) => self.toast(msg),
                    Err(e) => self.error(e),
                }
            }
        }
    }

    fn prompt_done(&mut self, kind: PromptKind, value: String) {
        match kind {
            PromptKind::Rename { dir, from } => self.rename(dir, from, value),
            PromptKind::NewFolder { dir } => self.create_folder(dir, value),
            PromptKind::Compress { sources, dir } => self.compress(sources, dir, value),
            PromptKind::SelectPattern { select } => {
                let n = self.tab_mut().select_pattern(&value, select);
                self.toast(format!(
                    "{} {n} {}",
                    if select { "Selected" } else { "Deselected" },
                    if n == 1 { "item" } else { "items" }
                ));
            }
            PromptKind::GoTo => {
                let v = value.trim();
                if !v.is_empty() {
                    self.navigate(v);
                }
            }
            PromptKind::Filter => self.tab_mut().filter = value,
            PromptKind::SshPassword { user, host, port } => {
                let password = value.trim().to_string();
                if password.is_empty() {
                    return;
                }
                match cx_term::install_public_key(&user, &host, port, None, Some(&password)) {
                    Ok(msg) => self.toast(msg),
                    Err(e) => self.error(e),
                }
            }
            PromptKind::SshUser {
                host,
                port,
                path,
                after,
            } => {
                let user = value.trim().to_string();
                if user.is_empty() {
                    return;
                }
                let endpoint = cx_core::Endpoint {
                    scheme: cx_core::Scheme::Sftp,
                    user: Some(user.clone()),
                    host,
                    port: Some(port),
                };
                match after {
                    crate::dialog::SshAfter::Shell => self.start_ssh(&endpoint, &path, &user),
                    crate::dialog::SshAfter::Window => self.open_ssh_window(&endpoint, &path),
                }
            }
            PromptKind::SaveWorkspace => {
                let name = value.trim().to_string();
                if name.is_empty() {
                    return;
                }
                let session = self.session();
                self.settings.workspaces.retain(|w| w.name != name);
                self.settings.workspaces.push(crate::settings::Workspace {
                    name: name.clone(),
                    session,
                });
                self.save_settings();
                self.toast(format!("Saved workspace “{name}”"));
            }
        }
    }

    // ---- palette ----

    pub(crate) fn open_palette(&mut self) {
        let km = self.settings.keymap;
        let mut items: Vec<MenuItem> = COMMANDS
            .iter()
            .filter(|c| !matches!(c.action, Action::TabN(_) | Action::Palette))
            .map(|c| {
                MenuItem::new(
                    c.label,
                    shortcut(c.action, km).unwrap_or_default(),
                    MenuAction::Run(c.action),
                )
            })
            .collect();
        for (p, pane) in self.panes.iter().enumerate() {
            for t in &pane.tabs {
                items.push(MenuItem::new(
                    format!("Tab: {}", t.title()),
                    if self.dual() {
                        if p == 0 {
                            "left pane"
                        } else {
                            "right pane"
                        }
                    } else {
                        ""
                    },
                    MenuAction::ActivateTab { pane: p, id: t.id },
                ));
            }
        }
        for b in &self.settings.bookmarks {
            items.push(MenuItem::new(
                format!("Favorite: {}", b.name),
                crate::util::display(&b.uri),
                MenuAction::Navigate {
                    uri: b.uri.clone(),
                    pane: None,
                },
            ));
        }
        for s in &self.settings.servers {
            items.push(MenuItem::new(
                format!("Server: {}", s.name),
                s.uri.clone(),
                MenuAction::Navigate {
                    uri: s.uri.clone(),
                    pane: None,
                },
            ));
        }
        for d in self.devices.iter().filter(|d| !d.is_self()) {
            for s in &d.shares {
                items.push(MenuItem::new(
                    format!("Device: {} — {}", d.name, s.name),
                    s.uri.clone(),
                    MenuAction::Navigate {
                        uri: s.uri.clone(),
                        pane: None,
                    },
                ));
            }
            for s in &d.services {
                items.push(MenuItem::new(
                    format!("Device: {} ({})", d.name, s.label),
                    s.uri.clone(),
                    MenuAction::Navigate {
                        uri: s.uri.clone(),
                        pane: None,
                    },
                ));
            }
        }
        if let Some(pl) = &self.places {
            for v in &pl.volumes {
                items.push(MenuItem::new(
                    format!("Drive: {}", v.name),
                    crate::util::display(&v.uri),
                    MenuAction::Navigate {
                        uri: v.uri.clone(),
                        pane: None,
                    },
                ));
            }
        }
        for t in cx_engine::tags::TAG_COLORS {
            items.push(MenuItem::new(
                format!("Tag: {t}"),
                "",
                MenuAction::Navigate {
                    uri: super::sources::tag_uri(t),
                    pane: None,
                },
            ));
        }
        for w in &self.settings.workspaces {
            let n: usize = w.session.panes.iter().map(|p| p.tabs.len()).sum();
            items.push(MenuItem::new(
                format!("Workspace: {}", w.name),
                format!("{n} tabs"),
                MenuAction::Workspace(w.name.clone()),
            ));
        }
        for r in &self.settings.recent {
            items.push(MenuItem::new(
                crate::util::display(r),
                "recent folder",
                MenuAction::Navigate {
                    uri: r.clone(),
                    pane: None,
                },
            ));
        }
        if matches!(self.tab().source, Source::Compare { .. }) {
            items.insert(
                0,
                MenuItem::new(
                    "Sync: copy left → right",
                    "",
                    MenuAction::Sync(cx_transfer::SyncDirection::LeftToRight),
                ),
            );
            items.insert(
                1,
                MenuItem::new(
                    "Sync: copy right → left",
                    "",
                    MenuAction::Sync(cx_transfer::SyncDirection::RightToLeft),
                ),
            );
            items.insert(
                2,
                MenuItem::new(
                    "Sync: both ways",
                    "",
                    MenuAction::Sync(cx_transfer::SyncDirection::Both),
                ),
            );
        }
        let mut p = Palette {
            input: TextInput::default(),
            items,
            results: Vec::new(),
            cursor: 0,
        };
        rank_palette(&mut p);
        self.dialogs.push(Dialog::Palette(p));
    }
}

/// Re-rank the palette for its current input. A typed path or URI becomes
/// a "Go to" entry at the top.
pub fn rank_palette(p: &mut Palette) {
    let q = p.input.text.trim().to_string();
    p.items.retain(|i| !i.label.starts_with("Go to “"));
    let looks_like_path =
        q.starts_with('/') || q.starts_with('~') || q.contains("://") || q.starts_with("\\\\");
    let labels: Vec<String> = p.items.iter().map(|i| i.label.clone()).collect();
    let ranked = cx_search::fuzzy_rank(&q, &labels);
    p.results = ranked.into_iter().map(|m| (m.index, m.positions)).collect();
    if looks_like_path {
        p.items.push(MenuItem::new(
            format!("Go to “{q}”"),
            "",
            MenuAction::Navigate {
                uri: q.clone(),
                pane: None,
            },
        ));
        p.results.insert(0, (p.items.len() - 1, Vec::new()));
    }
    p.cursor = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_ranks_fuzzily_and_offers_typed_paths() {
        let items = [
            "Copy",
            "Copy to…",
            "Move to…",
            "Connect to server…",
            "Toggle dual pane",
        ]
        .iter()
        .map(|l| MenuItem::new(*l, "", MenuAction::None))
        .collect();
        let mut p = Palette {
            input: TextInput::new("cpyto"),
            items,
            results: vec![],
            cursor: 0,
        };
        rank_palette(&mut p);
        assert_eq!(p.items[p.results[0].0].label, "Copy to…");
        assert_eq!(
            p.results[0].1,
            vec![0, 2, 3, 5, 6],
            "matched characters to highlight"
        );
        p.input.set("dual");
        rank_palette(&mut p);
        assert_eq!(p.items[p.results[0].0].label, "Toggle dual pane");
        assert_eq!(p.results.len(), 1);
        p.input.set("sftp://nas/home");
        rank_palette(&mut p);
        assert_eq!(p.items[p.results[0].0].label, "Go to “sftp://nas/home”");
        assert_eq!(
            p.items[p.results[0].0].action,
            MenuAction::Navigate {
                uri: "sftp://nas/home".into(),
                pane: None
            }
        );
        p.input.set("");
        rank_palette(&mut p);
        assert_eq!(
            p.results.len(),
            5,
            "empty query lists everything, no stale Go to"
        );
    }

    #[test]
    fn completion_prefix() {
        assert_eq!(
            common_prefix(&["/a/bcd/".into(), "/a/bce/".into()]),
            "/a/bc"
        );
        assert_eq!(common_prefix(&[]), "");
    }
}
