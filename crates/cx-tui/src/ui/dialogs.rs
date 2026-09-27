//! Drawing dialogs: centered popups over the file panes.

use super::theme::Theme;
use super::{centered, fit, hints, truncate, truncate_left};
use crate::app::App;
use crate::commands::{keys_for, COMMANDS};
use crate::dialog::*;
use crate::format;
use crate::input::TextInput;
use crate::keys::KeyCombo;
use crate::settings::Keymap;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

fn popup(f: &mut Frame, theme: &Theme, area: Rect, w: u16, h: u16, title: &str, foot: Option<Line<'static>>) -> Rect {
    let r = centered(area, w, h);
    f.render_widget(Clear, r);
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .title(Line::from(vec![Span::raw(" "), Span::styled(title.to_string(), Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)), Span::raw(" ")]))
        .style(Style::default().bg(theme.popup_bg).fg(theme.fg));
    if let Some(foot) = foot {
        block = block.title_bottom(foot);
    }
    let inner = block.inner(r);
    f.render_widget(block, r);
    Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner }
}

/// A text field: label on the left, the value with a visible cursor.
fn field(theme: &Theme, label: &str, label_w: usize, input: &TextInput, focused: bool, width: usize) -> Line<'static> {
    let mut spans = vec![Span::styled(fit(label, label_w), if focused { Style::default().fg(theme.accent).add_modifier(Modifier::BOLD) } else { theme.dim() })];
    let text = input.display();
    let avail = width.saturating_sub(label_w + 1).max(1);
    let chars: Vec<char> = text.chars().collect();
    // Scroll so the cursor stays visible.
    let start = (input.cursor + 1).saturating_sub(avail);
    let visible: String = chars.iter().skip(start).take(avail).collect();
    let bg = if theme.rich { theme.header_bg } else { theme.bg };
    let base = Style::default().fg(theme.fg).bg(bg);
    if focused {
        let cur = input.cursor - start;
        let before: String = visible.chars().take(cur).collect();
        let at: String = visible.chars().nth(cur).map(|c| c.to_string()).unwrap_or_else(|| " ".into());
        let after: String = visible.chars().skip(cur + 1).collect();
        spans.push(Span::styled(before, base));
        spans.push(Span::styled(at, base.add_modifier(Modifier::REVERSED)));
        let used = visible.width().max(cur + 1);
        spans.push(Span::styled(format!("{after}{}", " ".repeat(avail.saturating_sub(used))), base));
    } else {
        spans.push(Span::styled(fit(&visible, avail), base));
    }
    Line::from(spans)
}

fn check(theme: &Theme, label: &str, on: bool, focused: bool) -> Line<'static> {
    let style = if focused { Style::default().fg(theme.accent).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.fg) };
    Line::from(vec![Span::styled(if on { "[✓] " } else { "[ ] " }, style), Span::styled(label.to_string(), style)])
}

fn button(theme: &Theme, label: &str, focused: bool, danger: bool) -> Span<'static> {
    let color = if danger { theme.err } else { theme.accent };
    if focused {
        Span::styled(format!(" {label} "), Style::default().fg(theme.accent_fg).bg(color).add_modifier(Modifier::BOLD))
    } else {
        Span::styled(format!(" {label} "), Style::default().fg(color))
    }
}

fn error_line(theme: &Theme, e: &Option<String>) -> Line<'static> {
    match e {
        Some(e) => Line::from(Span::styled(e.clone(), Style::default().fg(theme.err))),
        None => Line::default(),
    }
}

pub fn draw(f: &mut Frame, app: &App, theme: &Theme, d: &Dialog, area: Rect) {
    match d {
        Dialog::Confirm(c) => {
            let w = (c.title.width().max(c.body.width()) as u16 + 8).clamp(40, 72);
            let inner = popup(f, theme, area, w, 8, &c.title, None);
            let lines = vec![
                Line::default(),
                Line::from(Span::styled(c.body.clone(), Style::default().fg(theme.fg))),
                Line::default(),
                Line::from(vec![button(theme, &c.ok, !c.on_cancel, c.danger), Span::raw("  "), button(theme, "Cancel", c.on_cancel, false)]),
            ];
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
        }
        Dialog::Prompt(p) => {
            let h = 7 + p.completions.len().min(6) as u16;
            let inner = popup(f, theme, area, 70, h, &p.title, Some(hints(theme, &[("Enter", &p.ok), ("Esc", "cancel")])));
            let mut lines = vec![Line::from(Span::styled(truncate(&p.label, inner.width as usize), theme.dim())), Line::default(), field(theme, "", 0, &p.input, true, inner.width as usize)];
            if !p.completions.is_empty() {
                lines.push(Line::default());
                for c in p.completions.iter().take(6) {
                    lines.push(Line::from(Span::styled(truncate_left(c, inner.width as usize), theme.dim())));
                }
            }
            f.render_widget(Paragraph::new(lines), inner);
        }
        Dialog::Menu(m) => menu(f, theme, area, m),
        Dialog::Palette(p) => palette(f, theme, area, p),
        Dialog::Help { scroll } => help(f, app, theme, area, *scroll),
        Dialog::Connect(c) => connect(f, theme, area, c),
        Dialog::SignIn(s) => {
            let inner = popup(f, theme, area, 64, 14, "Sign in", Some(hints(theme, &[("Enter", "sign in"), ("Tab", "next field"), ("Esc", "cancel")])));
            let w = inner.width as usize;
            let lines = vec![
                Line::from(Span::styled(truncate(&crate::util::display(&s.uri), w), Style::default().fg(theme.fg).add_modifier(Modifier::BOLD))),
                Line::from(Span::styled(truncate(if s.reason.is_empty() { "This server needs a user name and password." } else { &s.reason }, w), theme.dim())),
                Line::default(),
                field(theme, "User", 12, &s.user, s.focus == 0, w),
                field(theme, "Password", 12, &s.password, s.focus == 1, w),
                field(theme, "Key file", 12, &s.key_file, s.focus == 2, w),
                check(theme, "Remember in the keychain", s.remember, s.focus == 3),
                Line::default(),
                if s.busy { Line::from(Span::styled("Signing in…", theme.dim())) } else { error_line(theme, &s.error) },
            ];
            f.render_widget(Paragraph::new(lines), inner);
        }
        Dialog::HostKey(h) => {
            let title = if h.changed { "Server key changed!" } else { "Unknown server" };
            let inner = popup(f, theme, area, 76, 14, title, None);
            let mut lines = vec![Line::default()];
            if h.changed {
                lines.push(Line::from(Span::styled(format!("WARNING: {} presented a different key than before.", h.host), Style::default().fg(theme.err).add_modifier(Modifier::BOLD))));
                lines.push(Line::from(Span::styled("Someone may be intercepting the connection, or the server was reinstalled.", Style::default().fg(theme.err))));
            } else {
                lines.push(Line::from(Span::styled(format!("First connection to {}. Check that this fingerprint matches the server's:", h.host), Style::default().fg(theme.fg))));
            }
            lines.push(Line::default());
            lines.push(Line::from(vec![Span::styled(format!("{:>12}  ", "Key type"), theme.dim()), Span::styled(h.key_type.clone(), Style::default().fg(theme.fg))]));
            lines.push(Line::from(vec![Span::styled(format!("{:>12}  ", "Fingerprint"), theme.dim()), Span::styled(h.fingerprint.clone(), Style::default().fg(theme.warn).add_modifier(Modifier::BOLD))]));
            lines.push(Line::default());
            lines.push(Line::from(vec![button(theme, "Trust and connect", h.on_trust, h.changed), Span::raw("  "), button(theme, "Cancel", !h.on_trust, false)]));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        }
        Dialog::Conflict(c) => conflict(f, theme, area, c),
        Dialog::DestPicker(p) => dest_picker(f, theme, area, p),
        Dialog::MultiRename(m) => multi_rename(f, theme, area, m),
        Dialog::Search(s) => {
            let inner = popup(f, theme, area, 72, 11, "Search", Some(hints(theme, &[("Enter", "search"), ("Tab", "next field"), ("Esc", "cancel")])));
            let w = inner.width as usize;
            let lines = vec![
                Line::from(vec![Span::styled("In  ", theme.dim()), Span::styled(truncate_left(&crate::util::display(&s.root), w.saturating_sub(4)), Style::default().fg(theme.fg))]),
                Line::default(),
                field(theme, "Name", 14, &s.name, s.focus == 0, w),
                Line::from(Span::styled(fit("", 14) + "Text, *.pdf, IMG_????, or /regex/", theme.faint())),
                field(theme, "Containing", 14, &s.content, s.focus == 1, w),
                Line::from(Span::styled(fit("", 14) + "Text inside files (overrides Name)", theme.faint())),
                check(theme, "Include hidden files", s.include_hidden, s.focus == 2),
            ];
            f.render_widget(Paragraph::new(lines), inner);
        }
        Dialog::Tags(t) => {
            let h = (t.tags.len() as u16 + 7).min(area.height.saturating_sub(2));
            let inner = popup(f, theme, area, 50, h, "Tags", Some(hints(theme, &[("Space", "toggle"), ("Enter", "apply"), ("Esc", "cancel")])));
            let mut lines = vec![Line::from(Span::styled(if t.uris.len() == 1 { crate::util::name_of(&t.uris[0]) } else { format!("{} items", t.uris.len()) }, theme.dim())), Line::default()];
            for (i, (name, state)) in t.tags.iter().enumerate() {
                let mark = match state {
                    TagState::All => "[✓]",
                    TagState::Some => "[–]",
                    TagState::None => "[ ]",
                };
                let sel = i == t.cursor && t.input.text.is_empty();
                let st = if sel { Style::default().fg(theme.cursor_fg).bg(theme.cursor_bg) } else { Style::default().fg(theme.fg) };
                lines.push(Line::from(vec![Span::styled(format!("{mark} "), st), Span::styled("● ", Style::default().fg(theme.tag(name)).bg(if sel { theme.cursor_bg } else { theme.popup_bg })), Span::styled(fit(name, inner.width as usize - 6), st)]));
            }
            lines.push(Line::default());
            lines.push(field(theme, "New tag ", 8, &t.input, true, inner.width as usize));
            f.render_widget(Paragraph::new(lines), inner);
        }
        Dialog::Pair(p) => {
            let inner = popup(f, theme, area, 64, 11, "Pair a device", Some(hints(theme, &[("Enter", "pair"), ("Tab", "next field"), ("Esc", "cancel")])));
            let w = inner.width as usize;
            let lines = vec![
                Line::from(Span::styled("On the other device: Sharing & devices → Show pairing code.", theme.dim())),
                Line::default(),
                field(theme, "Address", 10, &p.address, p.focus == 0, w),
                field(theme, "Code", 10, &p.code, p.focus == 1, w),
                Line::default(),
                if p.busy { Line::from(Span::styled("Pairing…", theme.dim())) } else { error_line(theme, &p.error) },
            ];
            f.render_widget(Paragraph::new(lines), inner);
        }
        Dialog::Peer { cursor } => {
            let items = app.peer_items();
            let h = (items.len() as u16 + 5).min(area.height.saturating_sub(2));
            let inner = popup(f, theme, area, 72, h, "Sharing & paired devices", Some(hints(theme, &[("Enter", "choose"), ("r", "read-only"), ("Del", "remove"), ("Esc", "close")])));
            let mut lines = Vec::new();
            if let Some(p) = &app.peer {
                lines.push(Line::from(vec![Span::styled("This device  ", theme.dim()), Span::styled(format!("{}  ", p.name), Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)), Span::styled(truncate(&p.device_id, 20), theme.faint())]));
                lines.push(Line::default());
            }
            for (i, it) in items.iter().enumerate() {
                if it.header {
                    lines.push(Line::from(Span::styled(it.label.clone(), theme.accent().add_modifier(Modifier::BOLD))));
                    continue;
                }
                let sel = i == *cursor;
                let st = if sel { Style::default().fg(theme.cursor_fg).bg(theme.cursor_bg) } else { Style::default().fg(theme.fg) };
                let w = inner.width as usize;
                let lw = (w / 2).max(20).min(w);
                lines.push(Line::from(vec![Span::styled(fit(&format!(" {}", it.label), lw), st), Span::styled(fit(&it.detail, w - lw), if sel { st } else { theme.dim() })]));
            }
            f.render_widget(Paragraph::new(lines), inner);
        }
        Dialog::Offer { offer, cursor } => {
            let n = offer.files.len();
            let h = (n.min(6) as u16 + 11).min(area.height.saturating_sub(2));
            let inner = popup(f, theme, area, 64, h, "Incoming files", None);
            let mut lines = vec![Line::from(Span::styled(format!("{} wants to send you {} ({})", offer.from.name, format::plural(n, "file", "files"), format::size(offer.total)), Style::default().fg(theme.fg))), Line::default()];
            for file in offer.files.iter().take(6) {
                lines.push(Line::from(vec![Span::styled(fit(&format!("  {}", file.name), inner.width as usize - 10), Style::default().fg(theme.fg)), Span::styled(format::size(file.size), theme.dim())]));
            }
            if n > 6 {
                lines.push(Line::from(Span::styled(format!("  … and {} more", n - 6), theme.dim())));
            }
            lines.push(Line::default());
            for (i, label) in ["Accept into Downloads", "Accept into the current folder", "Decline"].iter().enumerate() {
                lines.push(Line::from(button(theme, label, i == *cursor, i == 2)));
            }
            f.render_widget(Paragraph::new(lines), inner);
        }
        Dialog::Settings { cursor } => {
            let s = &app.settings;
            let on = |b: bool| if b { "on" } else { "off" };
            let rows = [
                ("Keyboard style", s.keymap.label().to_string()),
                ("Theme", s.theme.label().to_string()),
                ("Nerd Font icons", on(s.nerd_icons).into()),
                ("Show hidden items", on(s.show_hidden).into()),
                ("Alternating row colors", on(s.stripes).into()),
                ("Preview pane", on(s.preview_pane).into()),
                ("Confirm moving to Trash", on(s.confirm_trash).into()),
                ("Confirm permanent delete", on(s.confirm_permanent_delete).into()),
                ("Confirm F5/F6 copy & move", on(s.confirm_transfer).into()),
                ("Restore tabs on start", on(s.restore_session).into()),
                ("Mouse (restart to apply)", on(s.mouse).into()),
                ("Dual pane", on(s.dual).into()),
            ];
            let inner = popup(f, theme, area, 56, rows.len() as u16 + 4, "Settings", Some(hints(theme, &[("Enter", "change"), ("Esc", "close")])));
            let w = inner.width as usize;
            let lines: Vec<Line> = rows
                .iter()
                .enumerate()
                .map(|(i, (k, v))| {
                    let sel = i == *cursor;
                    let st = if sel { Style::default().fg(theme.cursor_fg).bg(theme.cursor_bg) } else { Style::default().fg(theme.fg) };
                    Line::from(vec![Span::styled(fit(&format!(" {k}"), w.saturating_sub(18)), st), Span::styled(super::fit_right(&format!("{v} "), 18.min(w)), if sel { st } else { theme.accent() })])
                })
                .collect();
            f.render_widget(Paragraph::new(lines), inner);
        }
        Dialog::Diff(d) => diff(f, theme, area, d),
        Dialog::Info { title, lines } => {
            let inner = popup(f, theme, area, 64, lines.len() as u16 + 4, title, Some(hints(theme, &[("any key", "close")])));
            let text: Vec<Line> = lines.iter().map(|l| Line::from(Span::styled(l.clone(), if l.trim().chars().all(|c| c.is_ascii_digit() || c == ' ') && !l.trim().is_empty() { Style::default().fg(theme.accent).add_modifier(Modifier::BOLD) } else { Style::default().fg(theme.fg) }))).collect();
            f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
        }
    }
}

fn menu(f: &mut Frame, theme: &Theme, area: Rect, m: &Menu) {
    let vis = m.visible();
    let h = (vis.len() as u16 + 4).clamp(6, area.height.saturating_sub(2));
    let w = (m.items.iter().map(|i| i.label.width() + i.detail.width() + 6).max().unwrap_or(30) as u16).clamp(36, 96);
    let foot = if m.filter.is_empty() { hints(theme, &[("Enter", "open"), ("type", "filter"), ("Esc", "close")]) } else { hints(theme, &[("filter", &m.filter)]) };
    let inner = popup(f, theme, area, w, h, &m.title, Some(foot));
    let rows = inner.height as usize;
    let pos = vis.iter().position(|&i| i == m.cursor).unwrap_or(0);
    let start = pos.saturating_sub(rows.saturating_sub(1));
    let width = inner.width as usize;
    let lines: Vec<Line> = vis
        .iter()
        .skip(start)
        .take(rows)
        .map(|&i| {
            let it = &m.items[i];
            if it.header {
                return Line::from(Span::styled(it.label.clone(), theme.accent().add_modifier(Modifier::BOLD)));
            }
            let sel = i == m.cursor;
            let st = if sel { Style::default().fg(theme.cursor_fg).bg(theme.cursor_bg) } else { Style::default().fg(theme.fg) };
            let dw = it.detail.width().min(width / 2);
            Line::from(vec![Span::styled(fit(&format!(" {}", it.label), width.saturating_sub(dw + 1)), st), Span::styled(format!("{} ", truncate_left(&it.detail, dw)), if sel { st } else { theme.dim() })])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn palette(f: &mut Frame, theme: &Theme, area: Rect, p: &Palette) {
    let w = (area.width * 2 / 3).clamp(50, 100);
    let h = (area.height * 2 / 3).clamp(10, 30);
    let r = Rect { y: area.y + area.height / 8, ..centered(area, w, h) };
    f.render_widget(Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .title(Span::styled(" Command palette ", Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)))
        .title_bottom(hints(theme, &[("↑↓", "choose"), ("Enter", "run"), ("Esc", "close")]))
        .style(Style::default().bg(theme.popup_bg));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let inner = Rect { x: inner.x + 1, width: inner.width.saturating_sub(2), ..inner };
    let width = inner.width as usize;
    let mut lines = vec![field(theme, "› ", 2, &p.input, true, width), Line::from(Span::styled("─".repeat(width), theme.faint()))];
    let rows = inner.height.saturating_sub(2) as usize;
    let start = p.cursor.saturating_sub(rows.saturating_sub(1));
    for (k, (idx, pos)) in p.results.iter().enumerate().skip(start).take(rows) {
        let it = &p.items[*idx];
        let sel = k == p.cursor;
        let bg = if sel { theme.cursor_bg } else { theme.popup_bg };
        let base = Style::default().fg(if sel { theme.cursor_fg } else { theme.fg }).bg(bg);
        let hit = Style::default().fg(theme.accent).bg(bg).add_modifier(Modifier::BOLD);
        let dw = it.detail.width().min(width / 2);
        let lw = width.saturating_sub(dw + 2);
        let label = truncate(&it.label, lw);
        let mut spans = vec![Span::styled(" ", base)];
        // Highlight matched characters (positions are UTF-16 offsets; fine for BMP text).
        let mut u16pos = 0u32;
        for c in label.chars() {
            let matched = pos.contains(&u16pos);
            spans.push(Span::styled(c.to_string(), if matched { hit } else { base }));
            u16pos += c.len_utf16() as u32;
        }
        spans.push(Span::styled(" ".repeat(lw.saturating_sub(label.width())), base));
        spans.push(Span::styled(format!(" {} ", truncate_left(&it.detail, dw)), Style::default().fg(if sel { theme.cursor_fg } else { theme.dim }).bg(bg)));
        lines.push(Line::from(spans));
    }
    if p.results.is_empty() {
        lines.push(Line::from(Span::styled(" No matches", theme.dim())));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Every shortcut, grouped, for the current keymap.
pub fn help_lines(app: &App, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let km = app.settings.keymap;
    let mut lines = vec![
        Line::from(Span::styled(format!("Keyboard: {} style  (switch in Settings or the palette)", km.label()), theme.dim())),
        Line::default(),
        Line::from(Span::styled("Lists", theme.accent().add_modifier(Modifier::BOLD))),
    ];
    let mut list_keys: Vec<(&str, &str)> = vec![
        ("↑ ↓ PgUp PgDn Home End", "move the cursor"),
        ("Shift + arrows / Home / End", "select a range"),
        ("→ / ←", "Details: expand / collapse a folder in place (Brief: next / previous column)"),
        ("type letters", "filter this folder (Esc clears, Backspace edits)"),
        ("Esc", "clear the filter, then the selection"),
        ("mouse", "click selects, Ctrl+click toggles, Shift+click extends, double-click opens,"),
        ("", "wheel scrolls, header click sorts, right click shows actions"),
    ];
    if km == Keymap::Commander {
        list_keys.insert(2, ("Insert", "select / deselect and move down"));
        list_keys.insert(3, ("Space", "select / deselect (folders: count their size)"));
    }
    let kw = 30.min(width / 3);
    for (k, v) in list_keys {
        lines.push(Line::from(vec![Span::styled(fit(&format!("  {k}"), kw), Style::default().fg(theme.fg)), Span::styled(v.to_string(), theme.dim())]));
    }
    let mut groups: Vec<&str> = Vec::new();
    for c in COMMANDS {
        if !groups.contains(&c.group) {
            groups.push(c.group);
        }
    }
    for g in groups {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(g.to_string(), theme.accent().add_modifier(Modifier::BOLD))));
        for c in COMMANDS.iter().filter(|c| c.group == g) {
            let keys: Vec<String> = keys_for(c, km).iter().filter_map(|k| KeyCombo::parse(k)).map(|k| k.to_string()).collect();
            let keys = if keys.is_empty() { "palette".to_string() } else { keys.join(", ") };
            lines.push(Line::from(vec![Span::styled(fit(&format!("  {keys}"), kw), Style::default().fg(if keys == "palette" { theme.faint } else { theme.fg })), Span::styled(c.label.to_string(), theme.dim())]));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("Kitty-protocol terminals (kitty, WezTerm, foot, Ghostty, recent iTerm2) also get Ctrl+Shift and Ctrl+Tab keys.", theme.faint())));
    lines
}

fn help(f: &mut Frame, app: &App, theme: &Theme, area: Rect, scroll: usize) {
    let w = area.width.saturating_sub(4).min(110);
    let h = area.height.saturating_sub(2);
    let inner = popup(f, theme, area, w, h, "Keyboard shortcuts", Some(hints(theme, &[("↑↓ PgUp PgDn", "scroll"), ("Esc", "close")])));
    let lines = help_lines(app, theme, inner.width as usize);
    let scroll = scroll.min(lines.len().saturating_sub(inner.height as usize)) as u16;
    f.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

fn connect(f: &mut Frame, theme: &Theme, area: Rect, c: &ConnectForm) {
    let inner = popup(f, theme, area, 72, 20, "Connect to server", Some(hints(theme, &[("Enter", "connect"), ("Tab", "next field"), ("←→", "protocol"), ("Space", "toggle"), ("Esc", "cancel")])));
    let w = inner.width as usize;
    let lw = 16;
    let proto_style = if c.focus == 0 { Style::default().fg(theme.accent).add_modifier(Modifier::BOLD) } else { theme.dim() };
    let (path_label, user_label, pass_label) = if c.is_s3() { ("Bucket/folder", "Access key", "Secret key") } else { ("Folder", "User", "Password") };
    let mut lines = vec![
        Line::from(vec![Span::styled(fit("Protocol", lw), proto_style), Span::styled(format!("◂ {} ▸", PROTOCOLS[c.scheme].1), Style::default().fg(theme.fg).add_modifier(Modifier::BOLD))]),
        Line::default(),
        field(theme, "Server", lw, &c.host, c.focus == 1, w),
        field(theme, &format!("Port ({})", PROTOCOLS[c.scheme].2), lw, &c.port, c.focus == 2, w),
        field(theme, path_label, lw, &c.path, c.focus == 3, w),
        Line::default(),
        field(theme, user_label, lw, &c.user, c.focus == 4, w),
        field(theme, pass_label, lw, &c.password, c.focus == 5, w),
    ];
    if c.scheme() == "sftp" {
        lines.push(field(theme, "Key file", lw, &c.key_file, c.focus == 6, w));
    } else {
        lines.push(Line::default());
    }
    lines.push(Line::default());
    lines.push(check(theme, "Connect as guest / anonymously", c.anonymous, c.focus == 7));
    lines.push(check(theme, "Remember the password in the keychain", c.remember, c.focus == 8));
    lines.push(check(theme, "Add to saved servers", c.save, c.focus == 9));
    lines.push(Line::default());
    lines.push(Line::from(vec![button(theme, if c.busy { "Connecting…" } else { "Connect" }, c.focus == 10, false), Span::raw("  "), Span::styled(truncate(&c.uri().unwrap_or_default(), w.saturating_sub(16)), theme.faint())]));
    lines.push(error_line(theme, &c.error));
    f.render_widget(Paragraph::new(lines), inner);
}

fn conflict(f: &mut Frame, theme: &Theme, area: Rect, c: &ConflictDlg) {
    let inner = popup(f, theme, area, 76, 15, "A file with this name already exists", Some(hints(theme, &[("←→", "choose"), ("Enter", "apply"), ("Space", "apply to all"), ("c", "cancel job"), ("Esc", "later")])));
    let w = inner.width as usize;
    let side = |label: &str, e: &cx_core::Entry, uri: &str, newer: bool| -> Vec<Line<'static>> {
        vec![
            Line::from(vec![Span::styled(fit(label, 10), theme.dim()), Span::styled(truncate(&e.name, w - 10), Style::default().fg(theme.fg).add_modifier(Modifier::BOLD))]),
            Line::from(vec![
                Span::styled(fit("", 10), theme.dim()),
                Span::styled(format!("{}  ·  {}", format::size(e.size), format::date_long(e.modified)), Style::default().fg(if newer { theme.ok } else { theme.fg })),
                Span::styled(if newer { "  newer" } else { "" }, Style::default().fg(theme.ok)),
            ]),
            Line::from(vec![Span::styled(fit("", 10), theme.dim()), Span::styled(truncate_left(&crate::util::display(uri), w - 10), theme.faint())]),
        ]
    };
    let (s, d) = (&c.conflict.source, &c.conflict.dest);
    let s_newer = s.modified.unwrap_or(0) > d.modified.unwrap_or(0);
    let d_newer = d.modified.unwrap_or(0) > s.modified.unwrap_or(0);
    let mut lines = vec![Line::default()];
    lines.extend(side("Copying", s, &c.conflict.source_uri, s_newer));
    lines.push(Line::default());
    lines.extend(side("Existing", d, &c.conflict.dest_uri, d_newer));
    lines.push(Line::default());
    let mut buttons = Vec::new();
    for (i, label) in CONFLICT_CHOICES.iter().enumerate() {
        buttons.push(button(theme, label, i == c.cursor, i == 0));
        buttons.push(Span::raw(" "));
    }
    lines.push(Line::from(buttons));
    lines.push(check(theme, "Apply to all conflicts in this job", c.apply_all, false));
    f.render_widget(Paragraph::new(lines), inner);
}

fn dest_picker(f: &mut Frame, theme: &Theme, area: Rect, p: &DestPicker) {
    let title = format!("{} {} to…", if p.moving { "Move" } else { "Copy" }, if p.sources.len() == 1 { format!("“{}”", crate::util::name_of(&p.sources[0])) } else { format!("{} items", p.sources.len()) });
    let foot = if p.moving { hints(theme, &[("↑↓", "choose"), ("Enter", "move here"), ("type", "path or filter"), ("Tab", "complete"), ("Esc", "cancel")]) } else { hints(theme, &[("Space", "tick several"), ("Enter", "copy"), ("type", "path or filter"), ("Tab", "complete"), ("Esc", "cancel")]) };
    let h = area.height.saturating_sub(4).clamp(10, 30);
    let inner = popup(f, theme, area, 84, h, &title, Some(foot));
    let w = inner.width as usize;
    let mut lines = vec![field(theme, "Destination ", 12, &p.input, true, w), Line::from(Span::styled("─".repeat(w), theme.faint()))];
    let vis = p.visible();
    let rows = inner.height.saturating_sub(3) as usize;
    let start = p.cursor.saturating_sub(rows.saturating_sub(1));
    let mut last_section = "";
    let mut used = 0;
    for (k, &i) in vis.iter().enumerate().skip(start) {
        if used >= rows {
            break;
        }
        let it = &p.items[i];
        if it.section != last_section {
            if used + 1 >= rows {
                break;
            }
            lines.push(Line::from(Span::styled(it.section.to_string(), theme.accent().add_modifier(Modifier::BOLD))));
            last_section = it.section;
            used += 1;
        }
        let sel = k == p.cursor;
        let st = if sel { Style::default().fg(theme.cursor_fg).bg(theme.cursor_bg) } else { Style::default().fg(theme.fg) };
        let mark = if p.checked.contains(&i) { "[✓] " } else if p.moving { "    " } else { "[ ] " };
        let lw = (w / 2).max(20).min(w);
        lines.push(Line::from(vec![Span::styled(mark, if p.checked.contains(&i) { Style::default().fg(theme.selected).bg(if sel { theme.cursor_bg } else { theme.popup_bg }) } else { st }), Span::styled(fit(&it.label, lw.saturating_sub(4)), st), Span::styled(fit(&truncate_left(&it.detail, w - lw), w - lw), if sel { st } else { theme.dim() })]));
        used += 1;
    }
    let chosen = p.chosen();
    lines.push(Line::from(Span::styled(truncate(&format!("→ {}", chosen.iter().map(|u| crate::util::display(u)).collect::<Vec<_>>().join(", ")), w), theme.dim())));
    f.render_widget(Paragraph::new(lines), inner);
}

fn multi_rename(f: &mut Frame, theme: &Theme, area: Rect, m: &MultiRename) {
    let h = area.height.saturating_sub(2).clamp(16, 40);
    let inner = popup(f, theme, area, 96, h, &format!("Rename {} items", m.items.len()), Some(hints(theme, &[("Tab", "next field"), ("Space", "toggle"), ("PgUp/PgDn", "scroll preview"), ("Enter", "rename"), ("Esc", "cancel")])));
    let w = inner.width as usize;
    let half = w / 2;
    let two = |a: Line<'static>, b: Line<'static>| {
        let mut spans = a.spans;
        let aw: usize = spans.iter().map(|s| s.content.width()).sum();
        spans.push(Span::raw(" ".repeat(half.saturating_sub(aw))));
        spans.extend(b.spans);
        Line::from(spans)
    };
    let mut lines = vec![
        two(field(theme, "Name", 10, &m.name_mask, m.focus == 0, half - 1), field(theme, "Extension", 10, &m.ext_mask, m.focus == 1, w - half)),
        two(field(theme, "Find", 10, &m.search, m.focus == 2, half - 1), field(theme, "Replace", 10, &m.replace, m.focus == 3, w - half)),
        two(check(theme, "Regular expression", m.regex, m.focus == 4), Line::from(vec![Span::styled(fit("Case", 10), if m.focus == 5 { theme.accent().add_modifier(Modifier::BOLD) } else { theme.dim() }), Span::styled(format!("◂ {} ▸", m.case.label()), Style::default().fg(theme.fg))])),
        two(field(theme, "Counter", 10, &m.start, m.focus == 6, 18), two(field(theme, "Step", 6, &m.step, m.focus == 7, 12), field(theme, "Digits", 8, &m.digits, m.focus == 8, 12))),
        Line::from(Span::styled("Tokens: [N] name  [E] extension  [N2-5] characters  [C] counter  [YMD] date  [hms] time  [P] folder", theme.faint())),
        Line::from(Span::styled("─".repeat(w), theme.faint())),
    ];
    let plan = m.plan();
    let changes = plan.iter().filter(|p| p.to != p.from).count();
    let problems = plan.iter().filter(|p| !p.problem.is_empty()).count();
    let rows = inner.height.saturating_sub(lines.len() as u16 + 1) as usize;
    for p in plan.iter().skip(m.scroll).take(rows) {
        let color = if !p.problem.is_empty() { theme.err } else if p.to != p.from { theme.fg } else { theme.dim };
        let arrow = if p.to != p.from { " → " } else { "   " };
        let fw = (w.saturating_sub(3)) / 2;
        let mut spans = vec![Span::styled(fit(&p.from, fw), theme.dim()), Span::styled(arrow, theme.faint()), Span::styled(fit(&p.to, fw.saturating_sub(10)), Style::default().fg(color))];
        if !p.problem.is_empty() {
            spans.push(Span::styled(format!(" {}", p.problem), Style::default().fg(theme.err)));
        }
        lines.push(Line::from(spans));
    }
    let summary = if problems > 0 { Span::styled(format!("{problems} names can't be used"), Style::default().fg(theme.err)) } else { Span::styled(format!("{changes} of {} will change", plan.len()), theme.dim()) };
    let y = inner.y + inner.height.saturating_sub(1);
    f.render_widget(Paragraph::new(lines), Rect { height: inner.height.saturating_sub(1), ..inner });
    f.render_widget(Paragraph::new(Line::from(summary)).alignment(Alignment::Right), Rect { y, height: 1, ..inner });
}

fn diff(f: &mut Frame, theme: &Theme, area: Rect, d: &DiffView) {
    let w = area.width.saturating_sub(4);
    let h = area.height.saturating_sub(2);
    let title = format!("{}  ↔  {}", crate::util::name_of(&d.left), crate::util::name_of(&d.right));
    let inner = popup(f, theme, area, w, h, &title, Some(hints(theme, &[("n / p", "next / previous change"), ("↑↓ PgUp PgDn", "scroll"), ("Esc", "close")])));
    let width = inner.width as usize;
    let mut lines = vec![Line::from(vec![Span::styled(format!("+{} ", d.added), Style::default().fg(theme.ok)), Span::styled(format!("-{} ", d.removed), Style::default().fg(theme.err)), Span::styled(truncate(&format!("  {}  ↔  {}", crate::util::display(&d.left), crate::util::display(&d.right)), width.saturating_sub(12)), theme.faint())])];
    let rows = inner.height.saturating_sub(1) as usize;
    for l in d.lines.iter().skip(d.scroll).take(rows) {
        let num = |n: Option<usize>| n.map(|n| format!("{n:>5}")).unwrap_or_else(|| "     ".into());
        let (color, bg) = match l.tag {
            '+' => (theme.ok, if theme.rich { theme.fresh_bg } else { theme.bg }),
            '-' => (theme.err, theme.bg),
            _ => (theme.fg, theme.bg),
        };
        lines.push(Line::from(vec![Span::styled(format!("{} {} ", num(l.left), num(l.right)), theme.faint()), Span::styled(format!("{} ", l.tag), Style::default().fg(color).bg(bg)), Span::styled(fit(&l.text.replace('\t', "    "), width.saturating_sub(15)), Style::default().fg(color).bg(bg))]));
    }
    if d.lines.is_empty() {
        lines.push(Line::from(Span::styled("Both files are empty", theme.dim())));
    } else if d.added == 0 && d.removed == 0 {
        lines.insert(1, Line::from(Span::styled("The files are identical", Style::default().fg(theme.ok))));
    }
    f.render_widget(Paragraph::new(lines), inner);
}
