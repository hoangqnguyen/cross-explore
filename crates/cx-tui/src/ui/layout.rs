//! Where everything goes on screen. Computed from the app state and the
//! terminal size alone, so drawing and mouse hit-testing agree exactly.

use crate::app::App;
use crate::settings::{Keymap, ViewMode};
use crate::sort::SortKey;
use crate::tab::{Source, Tab};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColKind {
    Name,
    Modified,
    Type,
    Size,
    /// Home section, search location, compare status.
    Detail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Col {
    pub kind: ColKind,
    pub x: u16,
    pub width: u16,
}

impl Col {
    pub fn sort_key(&self) -> Option<SortKey> {
        match self.kind {
            ColKind::Name => Some(SortKey::Name),
            ColKind::Modified => Some(SortKey::Modified),
            ColKind::Type => Some(SortKey::Type),
            ColKind::Size => Some(SortKey::Size),
            ColKind::Detail => None,
        }
    }

    pub fn title(&self, source: &Source) -> &'static str {
        match self.kind {
            ColKind::Name => "Name",
            ColKind::Modified => "Modified",
            ColKind::Type => "Kind",
            ColKind::Size => "Size",
            ColKind::Detail => match source {
                Source::Home => "Section",
                Source::Compare { .. } => "Status",
                Source::Search { content: true, .. } => "Match",
                _ => "Where",
            },
        }
    }
}

const DATE_W: u16 = 16;
const TYPE_W: u16 = 14;
const SIZE_W: u16 = 9;

/// Columns for a details list `width` cells wide (responsive: the name
/// keeps at least 16 cells, other columns drop right to left).
pub fn columns(x: u16, width: u16, source: &Source) -> Vec<Col> {
    let mut want: Vec<(ColKind, u16)> = match source {
        Source::Home => vec![(ColKind::Detail, 12)],
        Source::Search { content: true, .. } => vec![
            (ColKind::Detail, width.saturating_sub(24).clamp(10, 60)),
            (ColKind::Size, SIZE_W),
        ],
        Source::Search { .. } | Source::Tag { .. } => vec![
            (ColKind::Detail, (width / 3).clamp(10, 40)),
            (ColKind::Modified, DATE_W),
            (ColKind::Size, SIZE_W),
        ],
        Source::Compare { .. } => vec![
            (ColKind::Detail, 12),
            (ColKind::Modified, DATE_W),
            (ColKind::Size, SIZE_W),
        ],
        Source::Folder => vec![
            (ColKind::Modified, DATE_W),
            (ColKind::Type, TYPE_W),
            (ColKind::Size, SIZE_W),
        ],
    };
    // Drop the least important columns until the name has room.
    let order = [
        ColKind::Type,
        ColKind::Modified,
        ColKind::Detail,
        ColKind::Size,
    ];
    for drop in order {
        let used: u16 = want.iter().map(|(_, w)| w + 1).sum();
        if width >= used + 16 {
            break;
        }
        want.retain(|(k, _)| *k != drop);
    }
    let used: u16 = want.iter().map(|(_, w)| w + 1).sum();
    let mut out = vec![Col {
        kind: ColKind::Name,
        x,
        width: width.saturating_sub(used),
    }];
    let mut cx = x + width.saturating_sub(used);
    for (k, w) in want {
        cx += 1;
        out.push(Col {
            kind: k,
            x: cx,
            width: w,
        });
        cx += w;
    }
    out
}

/// Brief view: how many name columns fit.
pub fn brief_columns(width: u16) -> u16 {
    (width / 24).clamp(1, 8)
}

pub fn tab_label(t: &Tab, i: usize) -> String {
    let mut title = t.title();
    if title.width() > 22 {
        title = format!("{}…", title.chars().take(21).collect::<String>());
    }
    format!(" {} {} ", i + 1, title)
}

#[derive(Debug, Clone)]
pub struct PaneRects {
    pub pane: usize,
    pub outer: Rect,
    pub tabs: Rect,
    pub tab_hits: Vec<(Rect, usize)>,
    pub crumb_hits: Vec<(Rect, String)>,
    pub header: Rect,
    pub columns: Vec<Col>,
    pub list: Rect,
    pub status: Rect,
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub panes: Vec<PaneRects>,
    pub preview: Option<Rect>,
    pub transfers: Option<Rect>,
    pub footer: Rect,
    pub fkeys: Option<Rect>,
    pub quicklook: Option<Rect>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Hit {
    Tab {
        pane: usize,
        idx: usize,
    },
    Crumb {
        pane: usize,
        uri: String,
    },
    Column {
        pane: usize,
        key: SortKey,
    },
    Row {
        pane: usize,
        row: usize,
    },
    /// Blank space in a list.
    List {
        pane: usize,
    },
    Pane {
        pane: usize,
    },
    Preview,
    Transfers {
        row: usize,
    },
    None,
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

/// The crumbs shown in a pane's title, newest last, trimmed from the left
/// to fit `width`.
pub fn crumbs(t: &Tab, width: u16) -> Vec<(String, Option<String>)> {
    let mut parts: Vec<(String, Option<String>)> = match (&t.source, &t.folder.info) {
        (Source::Folder, Some(info)) => info
            .crumbs
            .iter()
            .map(|c| (c.label.clone(), Some(c.uri.clone())))
            .collect(),
        (Source::Folder, None) => vec![(crate::util::display(t.uri()), None)],
        (Source::Search { root, text, .. }, _) => vec![(
            format!("Search “{text}” in {}", crate::util::name_of(root)),
            None,
        )],
        (Source::Compare { left, right, .. }, _) => vec![(
            format!(
                "Compare {} ↔ {}",
                crate::util::display(left),
                crate::util::display(right)
            ),
            None,
        )],
        (Source::Tag { name }, _) => vec![(format!("Tagged {name}"), None)],
        (Source::Home, _) => vec![("Home".into(), None)],
    };
    let total =
        |p: &[(String, Option<String>)]| p.iter().map(|(l, _)| l.width() as u16 + 3).sum::<u16>();
    // Replace leading crumbs with "…" until it fits (keeping the last one).
    while total(&parts) + 2 > width {
        if parts[0].0 != "…" && parts.len() > 1 {
            parts[0] = ("…".into(), None);
        } else if parts.len() > 2 {
            parts.remove(1);
        } else {
            break;
        }
    }
    // Still too wide: cut the current folder's name itself.
    if total(&parts) + 2 > width {
        let others: u16 = total(&parts[..parts.len() - 1]);
        let room = width.saturating_sub(others + 5) as usize;
        if let Some(last) = parts.last_mut() {
            last.0 = super::truncate(&last.0, room.max(1));
        }
    }
    parts
}

pub fn compute(area: Rect, app: &App) -> Layout {
    let mut l = Layout::default();
    let commander = app.settings.keymap == Keymap::Commander;
    let mut main = area;
    // Bottom rows: footer, and the F-key bar in Commander mode.
    if commander && main.height > 6 {
        l.fkeys = Some(Rect {
            y: main.y + main.height - 1,
            height: 1,
            ..main
        });
        main.height -= 1;
    }
    l.footer = Rect {
        y: main.y + main.height.saturating_sub(1),
        height: 1.min(main.height),
        ..main
    };
    main.height = main.height.saturating_sub(1);
    if app.transfers_open && main.height > 10 {
        let h = (app.jobs.len() as u16 + 2)
            .clamp(4, (main.height / 3).max(4))
            .min(main.height);
        l.transfers = Some(Rect {
            y: main.y + main.height - h,
            height: h,
            ..main
        });
        main.height -= h;
    }
    if app.quicklook {
        l.quicklook = Some(main);
    }
    let mut panes_area = main;
    if app.settings.preview_pane && main.width >= 60 {
        let w = (main.width * 2 / 5).clamp(28, 80).min(main.width);
        l.preview = Some(Rect {
            x: main.x + main.width - w,
            width: w,
            ..main
        });
        panes_area.width -= w;
    }
    let panes = app.visible_panes();
    let n = panes.len() as u16;
    for (k, &p) in panes.iter().enumerate() {
        let k = k as u16;
        let w = panes_area.width / n;
        let x = panes_area.x + k * w;
        let width = if k + 1 == n {
            panes_area.width.saturating_sub(w * (n - 1))
        } else {
            w
        };
        let outer = Rect {
            x,
            y: panes_area.y,
            width,
            height: panes_area.height,
        };
        l.panes.push(pane_rects(app, p, outer));
    }
    l
}

fn pane_rects(app: &App, p: usize, outer: Rect) -> PaneRects {
    let inner = Rect {
        x: outer.x + 1,
        y: outer.y + 1,
        width: outer.width.saturating_sub(2),
        height: outer.height.saturating_sub(2),
    };
    let tabs = Rect {
        height: 1.min(inner.height),
        ..inner
    };
    let t = app.panes[p].tab();
    let details = t.view == ViewMode::Details;
    let header = Rect {
        y: inner.y + 1,
        height: if details { 1 } else { 0 }.min(inner.height.saturating_sub(1)),
        ..inner
    };
    let status = Rect {
        y: inner.y + inner.height.saturating_sub(1),
        height: 1.min(inner.height),
        ..inner
    };
    let list_y = header.y + header.height;
    let list = Rect {
        y: list_y,
        height: status.y.saturating_sub(list_y),
        ..inner
    };
    let mut tab_hits = Vec::new();
    let mut x = tabs.x;
    for (i, tab) in app.panes[p].tabs.iter().enumerate() {
        let w = tab_label(tab, i).width() as u16;
        if x + w > tabs.x + tabs.width {
            break;
        }
        tab_hits.push((
            Rect {
                x,
                y: tabs.y,
                width: w,
                height: 1,
            },
            i,
        ));
        x += w + 1;
    }
    let mut crumb_hits = Vec::new();
    let mut cx = outer.x + 2;
    for (label, uri) in crumbs(t, outer.width.saturating_sub(16)) {
        let w = label.width() as u16;
        if let Some(u) = uri {
            crumb_hits.push((
                Rect {
                    x: cx,
                    y: outer.y,
                    width: w,
                    height: 1,
                },
                u,
            ));
        }
        cx += w + 3;
    }
    PaneRects {
        pane: p,
        outer,
        tabs,
        tab_hits,
        crumb_hits,
        header,
        columns: columns(inner.x, inner.width, &t.source),
        list,
        status,
    }
}

impl Layout {
    pub fn hit(&self, x: u16, y: u16, app: &App) -> Hit {
        if let Some(r) = self.transfers {
            if contains(r, x, y) {
                return Hit::Transfers {
                    row: y.saturating_sub(r.y + 1) as usize,
                };
            }
        }
        if let Some(r) = self.preview {
            if contains(r, x, y) {
                return Hit::Preview;
            }
        }
        for pr in &self.panes {
            if !contains(pr.outer, x, y) {
                continue;
            }
            let pane = pr.pane;
            for (r, idx) in &pr.tab_hits {
                if contains(*r, x, y) {
                    return Hit::Tab { pane, idx: *idx };
                }
            }
            for (r, uri) in &pr.crumb_hits {
                if contains(*r, x, y) {
                    return Hit::Crumb {
                        pane,
                        uri: uri.clone(),
                    };
                }
            }
            if contains(pr.header, x, y) {
                for c in &pr.columns {
                    if x >= c.x && x < c.x + c.width + 1 {
                        if let Some(key) = c.sort_key() {
                            return Hit::Column { pane, key };
                        }
                    }
                }
            }
            if contains(pr.list, x, y) {
                let t = app.panes[pane].tab();
                let dy = (y - pr.list.y) as usize;
                let row = match t.view {
                    ViewMode::Details => t.scroll.get() + dy,
                    ViewMode::Brief => {
                        let cols = brief_columns(pr.list.width);
                        let cw = (pr.list.width / cols).max(1);
                        let col = ((x - pr.list.x) / cw) as usize;
                        t.scroll.get() + col * pr.list.height as usize + dy
                    }
                };
                return if row < t.rows().len() {
                    Hit::Row { pane, row }
                } else {
                    Hit::List { pane }
                };
            }
            return Hit::Pane { pane };
        }
        Hit::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crumbs_fit_any_width() {
        use crate::folder::Folder;
        let mut f = Folder::new(
            1,
            "file:///a/very-long-folder-name/another-long-one/current-folder-name",
            Default::default(),
        );
        f.info = Some(
            cx_core::Location::parse(
                "/a/very-long-folder-name/another-long-one/current-folder-name",
            )
            .unwrap()
            .info(),
        );
        let t = Tab::new(1, f, Source::Folder, ViewMode::Details);
        for w in [0u16, 3, 8, 12, 20, 30, 60, 200] {
            let parts = crumbs(&t, w);
            let last = &parts.last().unwrap().0;
            assert!(
                last.starts_with("current") || last.starts_with('c') || last == "…" || w < 12,
                "width {w}: {parts:?}"
            );
            if w >= 30 {
                let total: usize = parts.iter().map(|(l, _)| l.width() + 3).sum();
                assert!(total + 2 <= w as usize, "width {w}: {parts:?}");
            }
        }
    }

    #[test]
    fn columns_drop_when_narrow() {
        let kinds = |w| {
            columns(0, w, &Source::Folder)
                .into_iter()
                .map(|c| c.kind)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            kinds(100),
            vec![
                ColKind::Name,
                ColKind::Modified,
                ColKind::Type,
                ColKind::Size
            ]
        );
        assert_eq!(
            kinds(50),
            vec![ColKind::Name, ColKind::Modified, ColKind::Size]
        );
        assert_eq!(kinds(30), vec![ColKind::Name, ColKind::Size]);
        assert_eq!(kinds(20), vec![ColKind::Name]);
        let cols = columns(3, 100, &Source::Folder);
        let last = cols.last().unwrap();
        assert_eq!(last.x + last.width, 103, "columns fill the width exactly");
    }
}
