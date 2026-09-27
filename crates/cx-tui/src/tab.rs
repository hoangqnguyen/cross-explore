//! A tab: a location with history, its rows (hidden files and the quick
//! filter applied, expanded folders spliced in as an outline), cursor and
//! selection. Pure state; loading and watching are driven by the app.

use crate::folder::{Folder, Item};
use crate::pattern::Wildcards;
use crate::settings::ViewMode;
use cx_core::Location;
use cx_transfer::DiffItem;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};

pub const HOME_URI: &str = "cx:home";

/// The most folders an outline expands (past this it stops being useful,
/// and each one is listed and watched).
pub const MAX_EXPANDED: usize = 200;

/// What a tab shows besides plain folders.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Folder,
    Home,
    Search { root: String, text: String, content: bool, task: Option<u64>, scanned: u64, truncated: bool },
    Compare { left: String, right: String, by_content: bool, diff: Vec<DiffItem> },
    Tag { name: String },
}

/// One visible line. `folder` is 0 for the tab's own listing, k for
/// `expanded[k - 1]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub folder: u32,
    pub idx: u32,
    pub depth: u16,
    /// URI of rows inside expanded folders (their key).
    pub uri: Option<String>,
}

pub struct Expanded {
    pub uri: String,
    pub folder: Folder,
}

/// Where the tab was in a folder, restored when coming back.
#[derive(Debug, Clone, Default)]
struct Memory {
    cursor: Option<String>,
    scroll: usize,
}

pub struct Tab {
    pub id: u64,
    pub history: Vec<String>,
    pub index: usize,
    pub folder: Folder,
    pub source: Source,
    pub expanded: Vec<Expanded>,
    pub selection: HashSet<String>,
    /// Index into `rows`.
    pub cursor: usize,
    cursor_key: Option<String>,
    anchor: Option<String>,
    pub filter: String,
    pub view: ViewMode,
    /// First visible row; kept in a Cell so rendering can scroll the cursor
    /// into view.
    pub scroll: Cell<usize>,
    /// Rows that fit on screen (last render), for PageUp/PageDown.
    pub page: Cell<usize>,
    /// Brief view: rows per column (last render).
    pub column_rows: Cell<usize>,
    rows: Vec<Row>,
    rows_sig: Option<(u64, String, bool)>,
    memory: HashMap<String, Memory>,
}

/// URI of `name` inside the folder `dir`.
pub fn child_uri(dir: &str, name: &str) -> String {
    Location::parse(dir).map(|l| l.join(name).uri()).unwrap_or_else(|_| format!("{}/{}", dir.trim_end_matches('/'), name))
}

impl Tab {
    pub fn new(id: u64, folder: Folder, source: Source, view: ViewMode) -> Tab {
        let uri = folder.uri.clone();
        Tab {
            id,
            history: vec![uri],
            index: 0,
            folder,
            source,
            expanded: Vec::new(),
            selection: HashSet::new(),
            cursor: 0,
            cursor_key: None,
            anchor: None,
            filter: String::new(),
            view,
            scroll: Cell::new(0),
            page: Cell::new(20),
            column_rows: Cell::new(20),
            rows: Vec::new(),
            rows_sig: None,
            memory: HashMap::new(),
        }
    }

    pub fn uri(&self) -> &str {
        &self.folder.uri
    }

    /// The canonical folder URI (child URIs are built from it).
    pub fn dir_uri(&self) -> &str {
        self.folder.dir_uri()
    }

    pub fn title(&self) -> String {
        match &self.source {
            Source::Home => "Home".into(),
            Source::Search { text, .. } => format!("Search “{text}”"),
            Source::Compare { .. } => "Compare folders".into(),
            Source::Tag { name } => format!("Tagged {name}"),
            Source::Folder => self.folder.info.as_ref().map(|i| if i.name.is_empty() { i.display.clone() } else { i.name.clone() }).unwrap_or_else(|| self.folder.uri.clone()),
        }
    }

    pub fn is_folder(&self) -> bool {
        self.source == Source::Folder
    }

    pub fn writable(&self) -> bool {
        self.is_folder() && self.folder.writable()
    }

    pub fn can_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_forward(&self) -> bool {
        self.index + 1 < self.history.len()
    }

    // ---- navigation (the app lists and watches the new folder) ----

    fn remember(&mut self) {
        let key = self.folder.uri.clone();
        self.memory.insert(key, Memory { cursor: self.cursor_key.clone(), scroll: self.scroll.get() });
    }

    /// Show `folder` (already created by the app) and select `select`.
    fn show(&mut self, folder: Folder, source: Source, select: Option<String>) {
        let mem = self.memory.get(&folder.uri).cloned().unwrap_or_default();
        self.folder = folder;
        self.source = source;
        self.expanded.clear();
        self.filter.clear();
        self.selection.clear();
        self.cursor = 0;
        self.cursor_key = select.clone().or(mem.cursor);
        self.anchor = self.cursor_key.clone();
        if let Some(s) = select {
            self.selection.insert(s);
        }
        self.scroll.set(mem.scroll);
        self.rows_sig = None;
        self.rows.clear();
    }

    /// Go to a new location, dropping forward history.
    pub fn navigate(&mut self, folder: Folder, source: Source, select: Option<String>) {
        self.remember();
        let uri = folder.uri.clone();
        self.show(folder, source, select);
        self.history.truncate(self.index + 1);
        self.history.push(uri);
        self.index = self.history.len() - 1;
    }

    /// Step through history; the app supplies the folder for the target.
    pub fn go_history(&mut self, delta: isize, folder: Folder, source: Source) {
        self.remember();
        self.index = (self.index as isize + delta).clamp(0, self.history.len() as isize - 1) as usize;
        self.show(folder, source, None);
    }

    pub fn history_target(&self, delta: isize) -> Option<&str> {
        let i = self.index as isize + delta;
        (i >= 0 && (i as usize) < self.history.len()).then(|| self.history[i as usize].as_str())
    }

    // ---- rows ----

    /// Recompute rows if anything they depend on changed. Cheap when not.
    pub fn refresh_rows(&mut self, show_hidden: bool) {
        for f in self.folders_mut() {
            f.settle();
        }
        let gens = self.expanded.iter().fold(self.folder.generation.wrapping_mul(31), |a, e| a.wrapping_mul(31).wrapping_add(e.folder.generation).wrapping_add(e.uri.len() as u64));
        let sig = (gens.wrapping_add(self.expanded.len() as u64), self.filter.clone(), show_hidden);
        if self.rows_sig.as_ref() == Some(&sig) {
            return;
        }
        self.rows_sig = Some(sig);
        let q = self.filter.trim().to_lowercase();
        let keep = |i: &Item| (show_hidden || !i.entry.hidden) && (q.is_empty() || i.lname.contains(&q));
        let mut rows = Vec::with_capacity(self.folder.items.len());
        if self.expanded.is_empty() {
            rows.extend(self.folder.items.iter().enumerate().filter(|(_, i)| keep(i)).map(|(idx, _)| Row { folder: 0, idx: idx as u32, depth: 0, uri: None }));
        } else {
            let by_uri: HashMap<&str, usize> = self.expanded.iter().enumerate().map(|(k, e)| (e.uri.as_str(), k + 1)).collect();
            self.walk(0, 0, None, &by_uri, &keep, &mut rows);
        }
        self.rows = rows;
        self.fix_cursor();
    }

    fn walk(&self, fi: usize, depth: u16, parent: Option<&str>, by_uri: &HashMap<&str, usize>, keep: &dyn Fn(&Item) -> bool, out: &mut Vec<Row>) {
        let folder = if fi == 0 { &self.folder } else { &self.expanded[fi - 1].folder };
        let base = parent.and_then(|p| Location::parse(p).ok()).or_else(|| if fi == 0 { Location::parse(self.folder.dir_uri()).ok() } else { None });
        for (idx, item) in folder.items.iter().enumerate() {
            let uri = if item.entry.is_dir || parent.is_some() {
                Some(match (&item.uri, &base) {
                    (Some(u), _) if parent.is_none() => u.clone(),
                    (_, Some(b)) => b.join(&item.entry.name).uri(),
                    _ => item.key().to_string(),
                })
            } else {
                None
            };
            let sub = if item.entry.is_dir { uri.as_deref().and_then(|u| by_uri.get(u).copied()) } else { None };
            if !keep(item) && sub.is_none() {
                continue;
            }
            out.push(Row { folder: fi as u32, idx: idx as u32, depth, uri: if parent.is_some() { uri.clone() } else { None } });
            if let (Some(k), Some(u)) = (sub, uri.as_deref()) {
                self.walk(k, depth + 1, Some(u), by_uri, keep, out);
            }
        }
    }

    /// After rows change, put the cursor back on the same key.
    fn fix_cursor(&mut self) {
        if let Some(k) = &self.cursor_key {
            if let Some(i) = self.rows.iter().position(|r| self.key_of(r) == k.as_str()) {
                self.cursor = i;
                return;
            }
        }
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        self.cursor_key = self.rows.get(self.cursor).map(|r| self.key_of(r).to_string());
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn folder_of(&self, r: &Row) -> &Folder {
        if r.folder == 0 {
            &self.folder
        } else {
            &self.expanded[r.folder as usize - 1].folder
        }
    }

    pub fn item(&self, r: &Row) -> &Item {
        &self.folder_of(r).items[r.idx as usize]
    }

    pub fn key_of<'a>(&'a self, r: &'a Row) -> &'a str {
        r.uri.as_deref().unwrap_or_else(|| self.item(r).key())
    }

    /// The folder a row lives in.
    pub fn parent_of(&self, r: &Row) -> String {
        if r.folder > 0 {
            return self.expanded[r.folder as usize - 1].uri.clone();
        }
        let item = self.item(r);
        item.parent.clone().unwrap_or_else(|| self.dir_uri().to_string())
    }

    pub fn uri_of(&self, r: &Row) -> String {
        if let Some(u) = &r.uri {
            return u.clone();
        }
        let item = self.item(r);
        item.uri.clone().unwrap_or_else(|| child_uri(self.dir_uri(), &item.entry.name))
    }

    pub fn cursor_row(&self) -> Option<&Row> {
        self.rows.get(self.cursor)
    }

    pub fn cursor_item(&self) -> Option<&Item> {
        self.cursor_row().map(|r| self.item(r))
    }

    pub fn is_selected(&self, r: &Row) -> bool {
        !self.selection.is_empty() && self.selection.contains(self.key_of(r))
    }

    /// Selected rows in order.
    pub fn selected_rows(&self) -> Vec<&Row> {
        if self.selection.is_empty() {
            return Vec::new();
        }
        self.rows.iter().filter(|r| self.is_selected(r)).collect()
    }

    /// Selected rows, or the cursor row when nothing is selected.
    pub fn targets(&self) -> Vec<&Row> {
        let sel = self.selected_rows();
        if !sel.is_empty() {
            return sel;
        }
        self.cursor_row().into_iter().collect()
    }

    pub fn target_uris(&self) -> Vec<String> {
        self.targets().into_iter().map(|r| self.uri_of(r)).collect()
    }

    pub fn is_expanded(&self, uri: &str) -> bool {
        self.expanded.iter().any(|e| e.uri == uri)
    }

    // ---- cursor & selection ----

    fn set_cursor(&mut self, i: usize) {
        if self.rows.is_empty() {
            self.cursor = 0;
            self.cursor_key = None;
            return;
        }
        self.cursor = i.min(self.rows.len() - 1);
        self.cursor_key = Some(self.key_of(&self.rows[self.cursor]).to_string());
    }

    /// Move the cursor. `extend` grows a range from the anchor (Shift);
    /// otherwise, like Explorer, a plain move selects nothing new but clears
    /// a selection only when `clear` is set.
    pub fn move_to(&mut self, i: isize, extend: bool) {
        if self.rows.is_empty() {
            return;
        }
        let i = i.clamp(0, self.rows.len() as isize - 1) as usize;
        if extend {
            if self.anchor.is_none() {
                self.anchor = self.cursor_key.clone();
            }
            self.set_cursor(i);
            self.select_range_to(i, false);
        } else {
            self.set_cursor(i);
            self.anchor = self.cursor_key.clone();
        }
    }

    pub fn move_by(&mut self, delta: isize, extend: bool) {
        self.move_to(self.cursor as isize + delta, extend);
    }

    pub fn select_key(&mut self, key: &str) {
        if let Some(i) = self.rows.iter().position(|r| self.key_of(r) == key) {
            self.set_cursor(i);
            self.anchor = self.cursor_key.clone();
        } else {
            self.cursor_key = Some(key.to_string());
        }
    }

    /// Select only this row (a click).
    pub fn select_only(&mut self, i: usize) {
        self.set_cursor(i);
        self.selection.clear();
        if let Some(k) = self.cursor_key.clone() {
            self.selection.insert(k);
        }
        self.anchor = self.cursor_key.clone();
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// Toggle the cursor row (Insert / Space in Commander, Ctrl+click).
    pub fn toggle_at(&mut self, i: usize) {
        self.set_cursor(i);
        if let Some(k) = self.cursor_key.clone() {
            if !self.selection.remove(&k) {
                self.selection.insert(k);
            }
        }
        self.anchor = self.cursor_key.clone();
    }

    pub fn select_range_to(&mut self, to: usize, additive: bool) {
        let a = self.anchor.as_ref().and_then(|k| self.rows.iter().position(|r| self.key_of(r) == k.as_str())).unwrap_or(to);
        let (lo, hi) = (a.min(to), a.max(to));
        if !additive {
            self.selection.clear();
        }
        for i in lo..=hi.min(self.rows.len().saturating_sub(1)) {
            let k = self.key_of(&self.rows[i]).to_string();
            self.selection.insert(k);
        }
    }

    pub fn select_all(&mut self) {
        self.selection = self.rows.iter().map(|r| self.key_of(r).to_string()).collect();
    }

    pub fn invert_selection(&mut self) {
        let next: HashSet<String> = self.rows.iter().map(|r| self.key_of(r)).filter(|k| !self.selection.contains(*k)).map(str::to_owned).collect();
        self.selection = next;
    }

    /// Total Commander's + and -: select or deselect by wildcard. Returns
    /// how many rows matched.
    pub fn select_pattern(&mut self, pattern: &str, select: bool) -> usize {
        let w = Wildcards::new(pattern);
        let keys: Vec<String> = self.rows.iter().filter(|r| w.matches(self.item(r).name())).map(|r| self.key_of(r).to_string()).collect();
        for k in &keys {
            if select {
                self.selection.insert(k.clone());
            } else {
                self.selection.remove(k);
            }
        }
        keys.len()
    }

    // ---- outline ----

    /// Add an expanded folder (the app lists and watches it).
    pub fn add_expanded(&mut self, uri: String, folder: Folder) -> bool {
        if self.expanded.len() >= MAX_EXPANDED || self.is_expanded(&uri) {
            return false;
        }
        self.expanded.push(Expanded { uri, folder });
        true
    }

    /// Collapse `uri` and everything below it; returns the removed folders'
    /// tokens (to unwatch). Keeps the cursor visible if it was inside.
    pub fn collapse(&mut self, uri: &str) -> Vec<Expanded> {
        let prefix = format!("{}/", uri.trim_end_matches('/'));
        let (gone, keep): (Vec<Expanded>, Vec<Expanded>) = std::mem::take(&mut self.expanded).into_iter().partition(|e| e.uri == uri || e.uri.starts_with(&prefix));
        self.expanded = keep;
        if self.cursor_key.as_deref().is_some_and(|k| k.starts_with(&prefix)) {
            let top = self.rows.iter().position(|r| r.uri.as_deref() == Some(uri) || (r.uri.is_none() && self.uri_of(r) == uri));
            self.cursor_key = top.map(|i| self.key_of(&self.rows[i]).to_string()).or_else(|| Some(uri.to_string()));
        }
        self.selection.retain(|k| !k.starts_with(&prefix));
        gone
    }

    pub fn collapse_all(&mut self) -> Vec<Expanded> {
        let gone = std::mem::take(&mut self.expanded);
        if self.cursor_key.as_deref().is_some_and(|k| k.contains("://") && !self.folder.items.iter().any(|i| i.key() == k)) {
            self.cursor_key = None;
        }
        gone
    }

    /// Index of the row that holds row `i` (its parent in the outline).
    pub fn parent_row(&self, i: usize) -> Option<usize> {
        let depth = self.rows.get(i)?.depth;
        if depth == 0 {
            return None;
        }
        (0..i).rev().find(|&j| self.rows[j].depth < depth)
    }

    pub fn expanded_folder_mut(&mut self, token: u64) -> Option<&mut Folder> {
        self.expanded.iter_mut().map(|e| &mut e.folder).find(|f| f.token == token)
    }

    pub fn folder_by_token_mut(&mut self, token: u64) -> Option<&mut Folder> {
        if self.folder.token == token {
            return Some(&mut self.folder);
        }
        self.expanded_folder_mut(token)
    }

    /// Every folder this tab shows (for watching and highlights).
    pub fn folders(&self) -> impl Iterator<Item = &Folder> {
        std::iter::once(&self.folder).chain(self.expanded.iter().map(|e| &e.folder))
    }

    pub fn folders_mut(&mut self) -> impl Iterator<Item = &mut Folder> {
        std::iter::once(&mut self.folder).chain(self.expanded.iter_mut().map(|e| &mut e.folder))
    }

    /// Keep the cursor in view for a viewport of `height` rows.
    pub fn scroll_into_view(&self, height: usize) {
        let height = height.max(1);
        self.page.set(height);
        let mut s = self.scroll.get();
        if self.cursor < s {
            s = self.cursor;
        } else if self.cursor >= s + height {
            s = self.cursor + 1 - height;
        }
        let max = self.rows.len().saturating_sub(height);
        self.scroll.set(s.min(max));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder::tests::{dir, file};
    use crate::folder::Item;
    use crate::sort::SortSpec;

    pub fn tab_with(entries: Vec<cx_core::Entry>) -> Tab {
        let folder = Folder::with_items(1, "file:///root", SortSpec::default(), entries.into_iter().map(Item::new).collect(), false);
        let mut t = Tab::new(1, folder, Source::Folder, ViewMode::Details);
        t.refresh_rows(false);
        t
    }

    fn names(t: &Tab) -> Vec<String> {
        t.rows().iter().map(|r| format!("{}{}", "  ".repeat(r.depth as usize), t.item(r).name())).collect()
    }

    #[test]
    fn hidden_and_filter() {
        let mut t = tab_with(vec![file("a.txt", 1, 0), file(".hidden", 1, 0), file("b.md", 1, 0)]);
        assert_eq!(names(&t), vec!["a.txt", "b.md"]);
        t.refresh_rows(true);
        assert_eq!(t.rows().len(), 3);
        t.filter = "MD".into();
        t.refresh_rows(true);
        assert_eq!(names(&t), vec!["b.md"]);
    }

    #[test]
    fn cursor_follows_its_row_when_rows_change() {
        let mut t = tab_with(vec![file("a", 1, 0), file("b", 1, 0), file("c", 1, 0)]);
        t.move_to(2, false);
        assert_eq!(t.cursor_item().unwrap().name(), "c");
        t.folder.upsert(file("0first", 1, 0), true);
        t.refresh_rows(false);
        assert_eq!(t.cursor_item().unwrap().name(), "c");
        assert_eq!(t.cursor, 3);
    }

    #[test]
    fn selection_ranges_patterns_invert() {
        let mut t = tab_with(vec![file("a.jpg", 1, 0), file("b.png", 1, 0), file("c.jpg", 1, 0), file("d.txt", 1, 0)]);
        t.move_to(1, false);
        t.move_by(2, true);
        assert_eq!(t.selected_rows().len(), 3);
        t.move_by(-1, true);
        assert_eq!(t.selected_rows().len(), 2);
        t.clear_selection();
        assert_eq!(t.select_pattern("*.jpg", true), 2);
        assert_eq!(t.selected_rows().len(), 2);
        t.select_pattern("a*", false);
        assert_eq!(t.target_uris(), vec!["file:///root/c.jpg".to_string()]);
        t.invert_selection();
        assert_eq!(t.selected_rows().len(), 3);
        t.select_all();
        assert_eq!(t.selected_rows().len(), 4);
        t.clear_selection();
        t.move_to(0, false);
        assert_eq!(t.targets().len(), 1, "cursor row is the target without a selection");
        t.toggle_at(0);
        t.toggle_at(3);
        assert_eq!(t.selected_rows().len(), 2);
    }

    #[test]
    fn outline_flattens_expanded_folders() {
        let mut t = tab_with(vec![dir("docs"), dir("src"), file("z.txt", 1, 0)]);
        let docs = Folder::with_items(2, "file:///root/docs", SortSpec::default(), vec![Item::new(dir("img")), Item::new(file("a.md", 1, 0))], false);
        assert!(t.add_expanded("file:///root/docs".into(), docs));
        let img = Folder::with_items(3, "file:///root/docs/img", SortSpec::default(), vec![Item::new(file("p.png", 1, 0))], false);
        t.add_expanded("file:///root/docs/img".into(), img);
        t.refresh_rows(false);
        assert_eq!(names(&t), vec!["docs", "  img", "    p.png", "  a.md", "src", "z.txt"]);
        assert_eq!(t.uri_of(&t.rows()[2].clone()), "file:///root/docs/img/p.png");
        assert_eq!(t.parent_of(&t.rows()[2].clone()), "file:///root/docs/img");
        assert_eq!(t.parent_row(2), Some(1));
        assert_eq!(t.parent_row(3), Some(0));
        assert_eq!(t.parent_row(4), None);
        // The filter keeps expanded folders so their matches stay reachable.
        t.filter = "p.png".into();
        t.refresh_rows(false);
        assert_eq!(names(&t), vec!["docs", "  img", "    p.png"]);
        t.filter.clear();
        t.refresh_rows(false);
        t.move_to(2, false);
        let gone = t.collapse("file:///root/docs");
        assert_eq!(gone.len(), 2, "collapsing a folder collapses its subfolders");
        t.refresh_rows(false);
        assert_eq!(names(&t), vec!["docs", "src", "z.txt"]);
        assert_eq!(t.cursor_item().unwrap().name(), "docs", "cursor moves out of the collapsed folder");
    }

    #[test]
    fn history_and_memory() {
        let mut t = tab_with(vec![file("a", 1, 0), file("b", 1, 0)]);
        t.move_to(1, false);
        let next = Folder::with_items(9, "file:///other", SortSpec::default(), vec![Item::new(file("x", 1, 0))], false);
        t.navigate(next, Source::Folder, None);
        assert!(t.can_back());
        assert_eq!(t.history_target(-1), Some("file:///root"));
        let back = Folder::with_items(10, "file:///root", SortSpec::default(), vec![Item::new(file("a", 1, 0)), Item::new(file("b", 1, 0))], false);
        t.go_history(-1, back, Source::Folder);
        t.refresh_rows(false);
        assert_eq!(t.cursor_item().unwrap().name(), "b", "cursor restored");
        assert!(t.can_forward());
    }

    #[test]
    fn scrolling_keeps_cursor_visible() {
        let mut t = tab_with((0..100).map(|i| file(&format!("f{i:03}"), 1, 0)).collect());
        t.move_to(50, false);
        t.scroll_into_view(10);
        assert_eq!(t.scroll.get(), 41);
        t.move_to(0, false);
        t.scroll_into_view(10);
        assert_eq!(t.scroll.get(), 0);
    }
}
