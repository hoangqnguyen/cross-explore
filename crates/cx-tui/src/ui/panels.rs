//! The preview pane / Quick Look, the transfers panel, the footer and the
//! Commander F-key bar.

use super::theme::Theme;
use super::{fit, fit_right, hints, truncate};
use crate::app::{App, Focus};
use crate::format;
use crate::highlight::Tok;
use crate::preview::Content;
use crate::settings::Keymap;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

pub fn preview(f: &mut Frame, app: &App, theme: &Theme, area: Rect, full: bool) {
    if full {
        f.render_widget(Clear, area);
    }
    let title = app.preview.as_ref().map(|p| p.entry.name.clone()).unwrap_or_else(|| "Preview".into());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if full { theme.accent } else { theme.border }))
        .title(Line::from(vec![Span::raw(" "), Span::styled(truncate(&title, area.width.saturating_sub(6) as usize), Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)), Span::raw(" ")]))
        .title_bottom(if full { hints(theme, &[("Esc", "close"), ("↑↓", "next file"), ("PgUp/PgDn", "scroll"), ("Enter", "open")]) } else { Line::default() })
        .style(theme.base());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(p) = &app.preview else {
        f.render_widget(Paragraph::new(Span::styled("Nothing selected", theme.dim())).alignment(Alignment::Center), inner);
        return;
    };
    let tags = app.tags.get(&p.uri).cloned().unwrap_or_default();
    let size = app.sizes.get(&p.uri).map(|s| s.0);
    let facts = crate::preview::facts(&p.uri, &p.entry, &tags, size);
    let key_w = facts.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
    let mut lines: Vec<Line> = Vec::new();
    // Full-screen Quick Look puts the content first; the pane puts facts first.
    let fact_lines: Vec<Line> = facts
        .into_iter()
        .map(|(k, v)| {
            let color = if k == "Tags" { tags.first().map(|t| theme.tag(t)).unwrap_or(theme.fg) } else { theme.fg };
            Line::from(vec![Span::styled(format!("{:>w$}  ", k, w = key_w), theme.dim()), Span::styled(v, Style::default().fg(color))])
        })
        .collect();
    let content: Vec<Line> = match &p.content {
        Content::Loading => vec![Line::from(Span::styled("Loading…", theme.dim()))],
        Content::Error(e) => vec![Line::from(Span::styled(e.clone(), Style::default().fg(theme.err)))],
        Content::None => vec![Line::from(Span::styled("No preview for this kind of file.", theme.dim())), Line::from(Span::styled("Enter opens it in its app, F4 edits it.", theme.faint()))],
        Content::Image { info } => {
            let w = info.iter().map(|(k, _)| k.width()).max().unwrap_or(0);
            info.iter().map(|(k, v)| Line::from(vec![Span::styled(format!("{:>w$}  ", k, w = w), theme.dim()), Span::styled(v.clone(), Style::default().fg(theme.image))])).collect()
        }
        Content::Listing { entries, total } => {
            let mut v: Vec<Line> = entries
                .iter()
                .map(|e| {
                    let color = theme.category(format::category(e));
                    let size = if e.is_dir { String::new() } else { format::size(e.size) };
                    let nw = (inner.width as usize).saturating_sub(12);
                    Line::from(vec![Span::styled(fit(&format!("{}{}", e.name, if e.is_dir { "/" } else { "" }), nw), Style::default().fg(color)), Span::styled(fit_right(&size, 10), theme.dim())])
                })
                .collect();
            if *total > entries.len() {
                v.push(Line::from(Span::styled(format!("… and {} more", format::count(total - entries.len())), theme.dim())));
            }
            if *total == 0 {
                v.push(Line::from(Span::styled("Empty", theme.dim())));
            }
            v
        }
        Content::Text { lines: text, language, truncated, encoding } => {
            let gutter = format!("{}", text.len()).len();
            let mut v: Vec<Line> = text
                .iter()
                .enumerate()
                .map(|(i, toks)| {
                    let mut spans = vec![Span::styled(format!("{:>gutter$} ", i + 1), theme.faint())];
                    for (tok, s) in toks {
                        let st = match tok {
                            Tok::Plain => Style::default().fg(theme.fg),
                            Tok::Keyword => Style::default().fg(theme.keyword),
                            Tok::Str => Style::default().fg(theme.string),
                            Tok::Comment => Style::default().fg(theme.comment).add_modifier(Modifier::ITALIC),
                            Tok::Number => Style::default().fg(theme.number),
                            Tok::Heading => Style::default().fg(theme.accent).add_modifier(Modifier::BOLD),
                            Tok::Code => Style::default().fg(theme.string),
                            Tok::Punct => Style::default().fg(theme.dim),
                        };
                        spans.push(Span::styled(s.replace('\t', "    "), st));
                    }
                    Line::from(spans)
                })
                .collect();
            let mut foot = format!("{} lines · {}", format::count(text.len()), encoding);
            if let Some(l) = language {
                foot.push_str(&format!(" · {l}"));
            }
            if *truncated {
                foot.push_str(" · preview truncated");
            }
            v.insert(0, Line::from(Span::styled(foot, theme.faint())));
            v
        }
    };
    if full {
        lines.extend(content);
        lines.push(Line::default());
        lines.extend(fact_lines);
    } else {
        lines.extend(fact_lines);
        lines.push(Line::from(Span::styled("─".repeat(inner.width as usize), theme.faint())));
        lines.extend(content);
    }
    let scroll = p.scroll.min(lines.len().saturating_sub(1)) as u16;
    let para = Paragraph::new(lines).scroll((scroll, 0));
    let para = if matches!(p.content, Content::Text { .. }) { para } else { para.wrap(Wrap { trim: false }) };
    f.render_widget(para, inner);
}

fn bar(theme: &Theme, frac: f64, width: usize) -> Vec<Span<'static>> {
    const PARTS: [&str; 8] = ["▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];
    let cells = frac.clamp(0.0, 1.0) * width as f64;
    let full = cells.floor() as usize;
    let mut s = "█".repeat(full.min(width));
    let rest = ((cells - full as f64) * 8.0).floor() as usize;
    if full < width && rest > 0 {
        s.push_str(PARTS[rest - 1]);
    }
    let filled = s.width();
    vec![Span::styled(s, Style::default().fg(theme.accent)), Span::styled("─".repeat(width.saturating_sub(filled)), theme.faint())]
}

pub fn transfers(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let focused = app.focus == Focus::Transfers;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if focused { theme.accent } else { theme.border }))
        .title(Line::from(vec![Span::styled(" Transfers ", Style::default().fg(theme.fg).add_modifier(Modifier::BOLD))]))
        .title_bottom(hints(theme, &[("Space", "pause/resume"), ("c", "cancel"), ("Enter", "resolve conflict"), ("C", "clear finished"), ("Esc", "back")]))
        .style(theme.base());
    let inner = block.inner(area);
    f.render_widget(block, area);
    if app.jobs.is_empty() {
        f.render_widget(Paragraph::new(Span::styled("No transfers", theme.dim())).alignment(Alignment::Center), inner);
        return;
    }
    let h = inner.height as usize;
    let start = app.transfers_cursor.saturating_sub(h.saturating_sub(1));
    let w = inner.width as usize;
    let mut lines = Vec::new();
    for (i, j) in app.jobs.iter().enumerate().skip(start).take(h) {
        let sel = focused && i == app.transfers_cursor;
        let (icon, color) = match j.state.as_str() {
            "done" => ("✓", theme.ok),
            "failed" => ("✕", theme.err),
            "cancelled" => ("⊘", theme.dim),
            "paused" => ("⏸", theme.warn),
            "waitingForConflict" => ("?", theme.warn),
            _ => ("↻", theme.accent),
        };
        let what = if j.sources.len() == 1 { crate::util::name_of(&j.sources[0]) } else { format!("{} items", j.sources.len()) };
        let dest = j.dest.as_deref().map(|d| format!(" → {}", crate::util::name_of(d))).unwrap_or_default();
        let label = format!("{} {what}{dest}", capital(&j.kind));
        let pct = (j.fraction() * 100.0).round() as u64;
        let detail = match j.state.as_str() {
            "running" | "scanning" => {
                let mut s = format!("{pct:>3}%");
                if j.speed > 0.0 {
                    s.push_str(&format!(" {}/s", format::size(j.speed as u64)));
                }
                if let Some(eta) = j.eta {
                    s.push_str(&format!(" · {}", format::duration(eta)));
                }
                s
            }
            "waitingForConflict" => "needs your answer".into(),
            "done" if !j.errors.is_empty() => format!("{} errors", j.errors.len()),
            "failed" => j.errors.first().map(|e| e.message.clone()).unwrap_or_else(|| "failed".into()),
            s => s.to_string(),
        };
        let label_w = (w / 2).saturating_sub(3).max(10);
        let detail_w = 28.min(w.saturating_sub(label_w + 4));
        let bar_w = w.saturating_sub(label_w + detail_w + 5);
        let bg = if sel { theme.cursor_bg } else { theme.bg };
        let mut spans = vec![Span::styled(format!(" {icon} "), Style::default().fg(color).bg(bg)), Span::styled(fit(&label, label_w), Style::default().fg(if sel { theme.cursor_fg } else { theme.fg }).bg(bg)), Span::styled(" ", Style::default().bg(bg))];
        spans.extend(bar(theme, j.fraction(), bar_w));
        spans.push(Span::styled(" ", Style::default().bg(bg)));
        spans.push(Span::styled(fit_right(&detail, detail_w), Style::default().fg(if j.state == "failed" { theme.err } else { theme.dim }).bg(bg)));
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

pub fn footer(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    // Right: transfer summary and keymap.
    let active = app.active_jobs();
    let mut right: Vec<Span> = Vec::new();
    if !active.is_empty() {
        let (done, total): (u64, u64) = active.iter().fold((0, 0), |a, j| (a.0 + j.bytes_done, a.1 + j.bytes_total));
        let speed: f64 = active.iter().map(|j| j.speed).sum();
        let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
        right.push(Span::styled(format!(" ⇅ {} ", active.len()), theme.accent()));
        right.extend(bar(theme, frac, 10));
        right.push(Span::styled(format!(" {:.0}%", frac * 100.0), theme.dim()));
        if speed > 0.0 {
            right.push(Span::styled(format!(" {}/s", format::size(speed as u64)), theme.dim()));
        }
        if active.iter().any(|j| j.conflict.is_some()) {
            right.push(Span::styled(" · needs answer", Style::default().fg(theme.warn)));
        }
        right.push(Span::raw("  "));
    }
    if !app.clipboard.as_ref().is_none_or(|c| c.0.is_empty()) {
        let (uris, cut) = app.clipboard.as_ref().unwrap();
        right.push(Span::styled(format!("{} {} ", if *cut { "✂" } else { "⧉" }, uris.len()), theme.dim()));
    }
    right.push(Span::styled(format!(" {} ", app.settings.keymap.label()), Style::default().fg(theme.accent_fg).bg(theme.accent)));
    let rw: u16 = right.iter().map(|s| s.content.width() as u16).sum();
    let lw = area.width.saturating_sub(rw);
    let left: Line = match app.current_toast() {
        Some(t) => Line::from(Span::styled(truncate(&format!(" {}", t.text), lw as usize), Style::default().fg(if t.error { theme.err } else { theme.ok }))),
        None => {
            let km = app.settings.keymap;
            let s = |a| crate::commands::shortcut(a, km).unwrap_or_default();
            use crate::commands::Action as A;
            let pairs = [(s(A::Help), "help"), (s(A::Palette), "commands"), (s(A::GoTo), "go to"), (s(A::ToggleDual), "dual"), (s(A::Connect), "connect"), (s(A::Transfers), "transfers"), (s(A::Quit), "quit")];
            let pairs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            let mut l = hints(theme, &pairs);
            l.spans.insert(0, Span::raw(" "));
            l
        }
    };
    f.render_widget(Paragraph::new(left).style(Style::default().bg(theme.header_bg)), Rect { width: lw, ..area });
    f.render_widget(Paragraph::new(Line::from(right)).style(Style::default().bg(theme.header_bg)), Rect { x: area.x + lw, width: rw, ..area });
}

/// Total Commander's F-key bar.
pub fn fkeys(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    debug_assert_eq!(app.settings.keymap, Keymap::Commander);
    let keys = [("1", "Help"), ("2", "Rename"), ("3", "View"), ("4", "Edit"), ("5", "Copy"), ("6", "Move"), ("7", "MkDir"), ("8", "Delete"), ("9", "Dual"), ("10", "Quit")];
    let w = (area.width as usize / keys.len()).max(4);
    let mut spans = Vec::new();
    for (k, label) in keys {
        spans.push(Span::styled(k, Style::default().fg(theme.fg).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(fit(label, w.saturating_sub(k.len())), Style::default().fg(theme.accent_fg).bg(theme.accent)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}
