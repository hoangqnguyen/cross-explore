//! One file pane: breadcrumb title with the live indicator, tab strip,
//! column header, the rows (details outline or brief columns) and the
//! pane's status line. Only the rows on screen are formatted, so a 100 000
//! row folder costs the same to draw as a 30 row one.

use super::layout::{brief_columns, crumbs, tab_label, ColKind, PaneRects};
use super::theme::Theme;
use super::{fit, fit_right, truncate};
use crate::app::{App, Focus};
use crate::folder::Status;
use crate::format;
use crate::settings::ViewMode;
use crate::tab::{Row, Source, Tab};
use cx_core::CxError;
use cx_engine::WatchMode;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub fn draw(f: &mut Frame, app: &App, theme: &Theme, pr: &PaneRects) {
    let t = app.panes[pr.pane].tab();
    let active = pr.pane == app.active;
    let highlight = active && app.dual();
    let border = if highlight { theme.accent } else { theme.border };

    // Title: breadcrumb, current folder in bold; right: live indicator.
    let mut title = vec![Span::raw(" ")];
    let parts = crumbs(t, pr.outer.width.saturating_sub(16));
    let n = parts.len();
    for (i, (label, _)) in parts.into_iter().enumerate() {
        if i > 0 {
            title.push(Span::styled(" › ", theme.faint()));
        }
        let style = if i + 1 == n { Style::default().fg(theme.fg).add_modifier(Modifier::BOLD) } else { theme.dim() };
        title.push(Span::styled(label, style));
    }
    title.push(Span::raw(" "));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .title(Line::from(title))
        .title(Line::from(indicator(t, theme)).alignment(Alignment::Right))
        .style(theme.base());
    f.render_widget(block, pr.outer);

    tabs(f, app, theme, pr, active);
    if t.view == ViewMode::Details {
        header(f, app, theme, pr, t);
    }
    let focused = active && matches!(app.focus, Focus::List | Focus::Filter) && app.dialogs.is_empty();
    if !body_message(f, theme, pr.list, t) {
        match t.view {
            ViewMode::Details => details(f, app, theme, pr, t, focused, active),
            ViewMode::Brief => brief(f, app, theme, pr, t, focused, active),
        }
    }
    status(f, app, theme, pr, t, active);
}

fn indicator(t: &Tab, theme: &Theme) -> Vec<Span<'static>> {
    let (text, color) = match (&t.folder.status, t.folder.watch, &t.source) {
        (Status::Loading, _, _) => ("◌ loading", theme.dim),
        (Status::Error(_), _, _) => ("✕ error", theme.err),
        (_, _, Source::Search { task: Some(_), .. }) => ("◌ searching", theme.warn),
        (_, Some((_, WatchMode::Live)), _) => ("● live", theme.ok),
        (_, Some((_, WatchMode::Polling)), _) => ("◍ polling", theme.warn),
        _ => return vec![],
    };
    vec![Span::styled(format!(" {text} "), Style::default().fg(color))]
}

fn tabs(f: &mut Frame, app: &App, theme: &Theme, pr: &PaneRects, active_pane: bool) {
    let pane = &app.panes[pr.pane];
    let mut spans = Vec::new();
    let mut used = 0u16;
    for (r, i) in &pr.tab_hits {
        let label = tab_label(&pane.tabs[*i], *i);
        let style = if *i == pane.active {
            if active_pane {
                Style::default().fg(theme.accent_fg).bg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.fg).bg(theme.cursor_inactive_bg).add_modifier(Modifier::BOLD)
            }
        } else {
            theme.dim()
        };
        if !spans.is_empty() {
            spans.push(Span::styled("│", theme.faint()));
        }
        spans.push(Span::styled(label, style));
        used = r.x + r.width - pr.tabs.x;
    }
    if pr.tab_hits.len() < pane.tabs.len() && used + 2 < pr.tabs.width {
        spans.push(Span::styled(format!(" +{}", pane.tabs.len() - pr.tab_hits.len()), theme.dim()));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.header_bg)), pr.tabs);
}

fn header(f: &mut Frame, app: &App, theme: &Theme, pr: &PaneRects, t: &Tab) {
    let sort = app.settings.sort;
    let mut spans = Vec::new();
    let mut x = pr.header.x;
    for c in &pr.columns {
        if c.x > x {
            spans.push(Span::raw(" ".repeat((c.x - x) as usize)));
        }
        let mut title = c.title(&t.source).to_string();
        let sorted = c.sort_key() == Some(sort.key) && t.is_folder();
        if sorted {
            title.push_str(if sort.desc { " ▼" } else { " ▲" });
        }
        let text = if matches!(c.kind, ColKind::Size) { fit_right(&title, c.width as usize) } else { fit(&format!(" {title}"), c.width as usize) };
        let style = if sorted { Style::default().fg(theme.accent).add_modifier(Modifier::BOLD) } else { theme.dim() };
        spans.push(Span::styled(text, style));
        x = c.x + c.width;
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.header_bg)), pr.header);
}

/// Loading / error / empty states. Returns true when it drew one.
fn body_message(f: &mut Frame, theme: &Theme, area: Rect, t: &Tab) -> bool {
    let (lines, color): (Vec<String>, _) = match &t.folder.status {
        Status::Loading if t.rows().is_empty() => {
            let n = t.folder.loaded;
            (vec![if n > 0 { format!("Loading… {} items", format::count(n)) } else { "Loading…".into() }], theme.dim)
        }
        Status::Error(e) => {
            let hint = match e {
                CxError::AuthRequired { .. } => "Sign-in required. Press Ctrl+R to try again.",
                CxError::HostKeyUnknown { .. } => "The server's key isn't trusted yet. Press Ctrl+R to review it.",
                CxError::NotFound(_) => "This folder doesn't exist (any more).",
                CxError::PermissionDenied(_) => "You don't have permission to open this folder.",
                _ => "Press Ctrl+R to try again.",
            };
            (vec!["Couldn't open this location".into(), String::new(), e.to_string(), String::new(), hint.into()], theme.err)
        }
        Status::Ready if t.rows().is_empty() => {
            let msg = if !t.filter.is_empty() {
                format!("No items match “{}”", t.filter)
            } else if matches!(t.source, Source::Search { .. }) {
                "No results".into()
            } else if matches!(t.source, Source::Compare { .. }) {
                "The folders are the same".into()
            } else if !t.folder.items.is_empty() {
                "Only hidden items here (Alt+H shows them)".into()
            } else {
                "Empty folder".into()
            };
            (vec![msg], theme.dim)
        }
        _ => return false,
    };
    let top = area.height.saturating_sub(lines.len() as u16) / 3;
    let r = Rect { y: area.y + top, height: area.height.saturating_sub(top), ..area };
    let text: Vec<Line> = lines.into_iter().enumerate().map(|(i, l)| Line::from(Span::styled(l, if i == 0 { Style::default().fg(color).add_modifier(Modifier::BOLD) } else { theme.dim() }))).collect();
    f.render_widget(Paragraph::new(text).alignment(Alignment::Center).wrap(Wrap { trim: true }), r);
    true
}

struct RowLook {
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    bold: bool,
    selected: bool,
}

fn row_look(app: &App, theme: &Theme, t: &Tab, row: &Row, i: usize, focused: bool, active: bool) -> RowLook {
    let item = t.item(row);
    let selected = t.is_selected(row);
    let cursor = i == t.cursor && (active || app.dual());
    let mut fg = if item.entry.hidden { theme.faint } else { theme.category(format::category(&item.entry)) };
    if selected {
        fg = theme.selected;
    }
    let bg = if cursor && focused {
        theme.cursor_bg
    } else if cursor && active {
        theme.cursor_inactive_bg
    } else if t.folder_of(row).is_fresh(item.name()) {
        theme.fresh_bg
    } else if app.settings.stripes && theme.rich && i % 2 == 1 {
        theme.stripe_bg
    } else {
        theme.bg
    };
    if cursor && focused && !selected {
        fg = theme.cursor_fg;
    }
    RowLook { fg, bg, bold: item.entry.is_dir || selected, selected }
}

fn name_cell(app: &App, theme: &Theme, t: &Tab, row: &Row, width: usize, details: bool, look: &RowLook) -> Vec<Span<'static>> {
    let item = t.item(row);
    let mut spans = Vec::new();
    let mark = if look.selected { "✓" } else { " " };
    let mut prefix = String::from(mark);
    if details {
        prefix.push_str(&"  ".repeat(row.depth as usize));
        let outline = item.entry.is_dir && !matches!(t.source, Source::Home | Source::Compare { .. });
        if outline {
            prefix.push_str(if t.is_expanded(&t.uri_of(row)) { "▾ " } else { "▸ " });
        } else {
            prefix.push_str("  ");
        }
    }
    if app.settings.nerd_icons {
        prefix.push_str(format::nerd_icon(&item.entry));
        prefix.push(' ');
    }
    let tags = if t.is_folder() || matches!(t.source, Source::Search { .. } | Source::Tag { .. }) { app.tags.get(&t.uri_of(row)).cloned().unwrap_or_default() } else { Vec::new() };
    let tag_w = if tags.is_empty() { 0 } else { tags.len().min(3) + 1 };
    let name_w = width.saturating_sub(prefix.width() + tag_w);
    let name = truncate(item.name(), name_w);
    let pad = width.saturating_sub(prefix.width() + name.width() + tag_w);
    let mut style = Style::default().fg(look.fg).bg(look.bg);
    if look.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    spans.push(Span::styled(prefix, Style::default().fg(if look.selected { theme.selected } else { theme.faint }).bg(look.bg)));
    spans.push(Span::styled(name, style));
    if !tags.is_empty() {
        spans.push(Span::styled(" ", Style::default().bg(look.bg)));
        for tg in tags.iter().take(3) {
            spans.push(Span::styled("●", Style::default().fg(theme.tag(tg)).bg(look.bg)));
        }
    }
    spans.push(Span::styled(" ".repeat(pad), Style::default().bg(look.bg)));
    spans
}

fn details(f: &mut Frame, app: &App, theme: &Theme, pr: &PaneRects, t: &Tab, focused: bool, active: bool) {
    let h = pr.list.height as usize;
    t.scroll_into_view(h);
    let start = t.scroll.get();
    let mut lines = Vec::with_capacity(h);
    for (k, row) in t.rows().iter().enumerate().skip(start).take(h) {
        let look = row_look(app, theme, t, row, k, focused, active);
        let item = t.item(row);
        let mut spans = Vec::new();
        let mut x = pr.list.x;
        let cell_style = Style::default().fg(if look.selected { theme.selected } else if k == t.cursor && focused { theme.cursor_fg } else { theme.dim }).bg(look.bg);
        for c in &pr.columns {
            if c.x > x {
                spans.push(Span::styled(" ".repeat((c.x - x) as usize), Style::default().bg(look.bg)));
            }
            let w = c.width as usize;
            match c.kind {
                ColKind::Name => spans.extend(name_cell(app, theme, t, row, w, true, &look)),
                ColKind::Modified => spans.push(Span::styled(fit(&format::date(item.entry.modified), w), cell_style)),
                ColKind::Type => spans.push(Span::styled(fit(&format::type_label(&item.entry), w), cell_style)),
                ColKind::Size => {
                    let text = if item.entry.is_dir {
                        match app.sizes.get(&t.uri_of(row)) {
                            Some((b, true)) => format::size(*b),
                            Some((b, false)) => format!("{}…", format::size(*b)),
                            None => "—".into(),
                        }
                    } else {
                        format::size(item.entry.size)
                    };
                    spans.push(Span::styled(fit_right(&text, w), cell_style));
                }
                ColKind::Detail => spans.push(Span::styled(fit(item.detail.as_deref().unwrap_or(""), w), cell_style)),
            }
            x = c.x + c.width;
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), pr.list);
}

fn brief(f: &mut Frame, app: &App, theme: &Theme, pr: &PaneRects, t: &Tab, focused: bool, active: bool) {
    let rpc = pr.list.height.max(1) as usize;
    let cols = brief_columns(pr.list.width) as usize;
    let cw = (pr.list.width as usize / cols).max(1);
    t.column_rows.set(rpc);
    t.page.set(rpc);
    let ccol = t.cursor / rpc;
    let mut scol = t.scroll.get() / rpc;
    if ccol < scol {
        scol = ccol;
    } else if ccol >= scol + cols {
        scol = ccol + 1 - cols;
    }
    t.scroll.set(scol * rpc);
    let rows = t.rows();
    let mut lines: Vec<Line> = Vec::with_capacity(rpc);
    for y in 0..rpc {
        let mut spans = Vec::new();
        for c in 0..cols {
            let i = (scol + c) * rpc + y;
            let w = if c + 1 == cols { pr.list.width as usize - cw * (cols - 1) } else { cw };
            if c > 0 {
                spans.push(Span::styled("│", theme.faint()));
            }
            let w = if c > 0 { w.saturating_sub(1) } else { w };
            match rows.get(i) {
                Some(row) => {
                    let look = row_look(app, theme, t, row, i, focused, active);
                    spans.extend(name_cell(app, theme, t, row, w, false, &look));
                }
                None => spans.push(Span::raw(" ".repeat(w))),
            }
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), pr.list);
}

fn status(f: &mut Frame, app: &App, theme: &Theme, pr: &PaneRects, t: &Tab, active: bool) {
    let mut left: Vec<Span> = Vec::new();
    let filtering = active && app.focus == Focus::Filter;
    if !t.filter.is_empty() || filtering {
        left.push(Span::styled(" ⌕ ", Style::default().fg(theme.accent_fg).bg(theme.accent)));
        left.push(Span::styled(format!(" {}", t.filter), Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)));
        if filtering {
            left.push(Span::styled("▏", theme.accent()));
        }
        left.push(Span::styled("  ", Style::default()));
    }
    let n = t.rows().len();
    let loading = t.folder.status == Status::Loading;
    let mut text = if loading && n > 0 { format!("Loading… {}", format::count(t.folder.loaded.max(n))) } else { format::plural(n, "item", "items") };
    if matches!(t.source, Source::Compare { .. }) {
        let km = app.settings.keymap;
        text.push_str(if km == crate::settings::Keymap::Commander { " · Enter diff · F5 sync → · F6 sync ← · palette: both ways" } else { " · Enter diff · palette: Sync" });
    }
    if let Source::Search { scanned, truncated, task, .. } = &t.source {
        if task.is_none() {
            text.push_str(&format!(" · {} scanned{}", format::count(*scanned as usize), if *truncated { " (stopped at the limit)" } else { "" }));
        }
    }
    let sel = t.selected_rows();
    if !sel.is_empty() {
        let bytes: u64 = sel.iter().map(|r| {
            let i = t.item(r);
            if i.entry.is_dir { app.sizes.get(&t.uri_of(r)).map(|s| s.0).unwrap_or(0) } else { i.entry.size }
        }).sum();
        text.push_str(&format!(" · {} selected ({})", format::count(sel.len()), format::size(bytes)));
    }
    left.push(Span::styled(text, theme.dim()));
    let mut right = String::new();
    if t.is_folder() {
        if let Some(s) = app.free.get(t.dir_uri()) {
            right = format!("{} free ", format::size(s.free));
        }
    }
    if let Some(ms) = t.folder.elapsed_ms.filter(|_| t.is_folder() && t.folder.items.len() > 5000) {
        right = format!("listed in {:.0} ms · {right}", ms);
    }
    let rw = right.width() as u16;
    let lw = pr.status.width.saturating_sub(rw);
    f.render_widget(Paragraph::new(Line::from(left)), Rect { width: lw, ..pr.status });
    f.render_widget(Paragraph::new(Span::styled(right, theme.dim())).alignment(Alignment::Right), Rect { x: pr.status.x + lw, width: rw, ..pr.status });
}
