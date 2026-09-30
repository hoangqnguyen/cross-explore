//! Drawing. Reads the app state and draws it; the only thing it writes is
//! each tab's scroll offset (through a `Cell`), to keep the cursor in view
//! for the height it was actually given.

pub mod dialogs;
pub mod layout;
pub mod pane;
pub mod panels;
pub mod theme;

use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Block;
use ratatui::Frame;
use theme::Theme;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn render(f: &mut Frame, app: &App) {
    let theme = Theme::resolve(app.settings.theme);
    let area = f.area();
    f.render_widget(Block::default().style(theme.base()), area);
    let l = layout::compute(area, app);
    for pr in &l.panes {
        pane::draw(f, app, &theme, pr);
    }
    if let Some(r) = l.preview {
        panels::preview(f, app, &theme, r, false);
    }
    if let Some(r) = l.transfers {
        panels::transfers(f, app, &theme, r);
    }
    panels::footer(f, app, &theme, l.footer);
    if let Some(r) = l.fkeys {
        panels::fkeys(f, app, &theme, r);
    }
    if let Some(r) = l.quicklook {
        panels::preview(f, app, &theme, r, true);
    }
    for d in &app.dialogs {
        dialogs::draw(f, app, &theme, d, area);
    }
}

/// Cut `s` to `width` display cells, ending with "…" when shortened.
pub fn truncate(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > width {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Keep the end of `s` (paths): "…/deep/folder".
pub fn truncate_left(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_string();
    }
    let mut chars: Vec<char> = Vec::new();
    let mut w = 1;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if w + cw > width {
            break;
        }
        chars.push(c);
        w += cw;
    }
    let mut out = String::from("…");
    out.extend(chars.into_iter().rev());
    out
}

/// Pad or cut to exactly `width` cells.
pub fn fit(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    let pad = width.saturating_sub(t.width());
    format!("{t}{}", " ".repeat(pad))
}

pub fn fit_right(s: &str, width: usize) -> String {
    let t = truncate(s, width);
    let pad = width.saturating_sub(t.width());
    format!("{}{t}", " ".repeat(pad))
}

/// A centered rectangle for popups.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2)).max(1);
    let h = height.min(area.height.saturating_sub(2)).max(1);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

/// "Key hint" spans: key in accent, label dim.
pub fn hints(theme: &Theme, pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", Style::default()));
        }
        spans.push(Span::styled(k.to_string(), theme.accent()));
        spans.push(Span::styled(format!(" {v}"), theme.dim()));
    }
    Line::from(spans)
}

/// Render the whole screen into a string (tests and snapshots).
pub fn render_to_string(app: &App, width: u16, height: u16) -> String {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut term = ratatui::Terminal::new(backend).expect("test backend");
    term.draw(|f| render(f, app)).expect("draw");
    buffer_text(term.backend().buffer())
}

pub fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..buf.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            line.push_str(sym);
            skip = sym.width().saturating_sub(1);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation() {
        assert_eq!(truncate("hello world", 8), "hello w…");
        assert_eq!(truncate("hi", 8), "hi");
        assert_eq!(truncate_left("/a/very/long/path", 8), "…ng/path");
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(fit_right("ab", 4), "  ab");
        assert_eq!(truncate("日本語テキスト", 7), "日本語…");
    }
}
