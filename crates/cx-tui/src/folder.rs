//! One listed location: its rows (kept sorted), load status, watch state
//! and the "just appeared" highlights. Batches from the engine are merged
//! in O(n) and watch patches applied in place, so a 100 000-row folder
//! never gets re-sorted from scratch while it streams in or changes.

use crate::sort::{compare, SortSpec};
use cx_core::{Capabilities, Change, CxError, Entry, LocationInfo};
use cx_engine::WatchMode;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How long a new row stays highlighted.
pub const FRESH_FOR: Duration = Duration::from_millis(1600);

/// A row's data. `uri` is set when the item does not live directly in the
/// folder being shown (search results, home page, tag lists).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub entry: Entry,
    /// Lower-cased name, for sorting and filtering without allocating.
    pub lname: String,
    ext_at: usize,
    pub uri: Option<String>,
    /// Folder holding the item, when it isn't the listed folder.
    pub parent: Option<String>,
    /// Secondary text: a search hit's path or line, a device's address…
    pub detail: Option<String>,
}

impl Item {
    pub fn new(entry: Entry) -> Item {
        let lname = entry.name.to_lowercase();
        let ext_at = if entry.is_dir {
            lname.len()
        } else {
            match lname.rfind('.') {
                Some(i) if i > 0 => i + 1,
                _ => lname.len(),
            }
        };
        Item { entry, lname, ext_at, uri: None, parent: None, detail: None }
    }

    pub fn located(entry: Entry, uri: String, parent: Option<String>, detail: Option<String>) -> Item {
        Item { uri: Some(uri), parent, detail, ..Item::new(entry) }
    }

    /// Lower-cased extension, "" for folders.
    pub fn lext(&self) -> &str {
        &self.lname[self.ext_at..]
    }

    /// Unique within a listing: the name, or the URI for located items
    /// (search results can contain the same name many times).
    pub fn key(&self) -> &str {
        self.uri.as_deref().unwrap_or(&self.entry.name)
    }

    pub fn name(&self) -> &str {
        &self.entry.name
    }
}

#[derive(Debug, Clone)]
pub enum Status {
    Loading,
    Ready,
    Error(CxError),
}

impl PartialEq for Status {
    fn eq(&self, other: &Status) -> bool {
        match (self, other) {
            (Status::Loading, Status::Loading) | (Status::Ready, Status::Ready) => true,
            (Status::Error(a), Status::Error(b)) => a.to_string() == b.to_string(),
            _ => false,
        }
    }
}

pub struct Folder {
    /// Identifies this listing in messages from background tasks; a new
    /// listing of the same URI gets a new token, so stale results are dropped.
    pub token: u64,
    pub uri: String,
    pub info: Option<LocationInfo>,
    pub caps: Option<Capabilities>,
    pub status: Status,
    pub items: Vec<Item>,
    pub sort: SortSpec,
    /// Keep insertion order (the home page's sections).
    pub keep_order: bool,
    /// Set while re-listing a folder that is already shown: the new rows go
    /// here and replace `items` at the end, so the list doesn't flash empty.
    reloading: Option<Vec<Item>>,
    pub watch: Option<(u64, WatchMode)>,
    pub watch_pending: bool,
    pub fresh: HashMap<String, Instant>,
    /// Bumped on every change; views recompute their rows when it moves.
    pub generation: u64,
    pub elapsed_ms: Option<f64>,
    /// Entries seen while loading (the "Loading… 12,345 items" counter).
    pub loaded: usize,
}

impl Folder {
    pub fn new(token: u64, uri: impl Into<String>, sort: SortSpec) -> Folder {
        Folder {
            token,
            uri: uri.into(),
            info: None,
            caps: None,
            status: Status::Loading,
            items: Vec::new(),
            sort,
            keep_order: false,
            reloading: None,
            watch: None,
            watch_pending: false,
            fresh: HashMap::new(),
            generation: 0,
            elapsed_ms: None,
            loaded: 0,
        }
    }

    /// A finished, static listing (home page, search results, tags).
    pub fn with_items(token: u64, uri: impl Into<String>, sort: SortSpec, items: Vec<Item>, keep_order: bool) -> Folder {
        let mut f = Folder::new(token, uri, sort);
        f.keep_order = keep_order;
        f.status = Status::Ready;
        f.set_items(items);
        f
    }

    /// The canonical URI (after the provider normalised it), for building
    /// child URIs.
    pub fn dir_uri(&self) -> &str {
        self.info.as_ref().map(|i| i.uri.as_str()).unwrap_or(&self.uri)
    }

    pub fn is_ready(&self) -> bool {
        self.status == Status::Ready
    }

    pub fn writable(&self) -> bool {
        self.is_ready() && self.caps.map(|c| c.writable).unwrap_or(true)
    }

    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    fn sort_items(&self, items: &mut [Item]) {
        if !self.keep_order {
            let spec = self.sort;
            items.sort_unstable_by(|a, b| compare(spec, a, b));
        }
    }

    pub fn set_items(&mut self, mut items: Vec<Item>) {
        self.sort_items(&mut items);
        self.items = items;
        self.bump();
    }

    /// Start listing again. With `keep` the current rows stay on screen
    /// until the new listing is complete.
    pub fn begin_load(&mut self, keep: bool) {
        self.loaded = 0;
        if keep && self.status == Status::Ready {
            self.reloading = Some(Vec::new());
        } else {
            self.reloading = None;
            self.items.clear();
            self.status = Status::Loading;
        }
        self.bump();
    }

    pub fn set_meta(&mut self, info: LocationInfo, caps: Capabilities) {
        self.info = Some(info);
        self.caps = Some(caps);
        self.bump();
    }

    /// Merge a streamed batch (sorted, then merged in one pass).
    pub fn add_batch(&mut self, entries: Vec<Entry>) {
        self.loaded += entries.len();
        let mut batch: Vec<Item> = entries.into_iter().map(Item::new).collect();
        if let Some(r) = self.reloading.as_mut() {
            r.append(&mut batch);
            return;
        }
        self.sort_items(&mut batch);
        if self.items.is_empty() || self.keep_order {
            self.items.append(&mut batch);
        } else {
            let old = std::mem::take(&mut self.items);
            let spec = self.sort;
            let mut out = Vec::with_capacity(old.len() + batch.len());
            let (mut a, mut b) = (old.into_iter().peekable(), batch.into_iter().peekable());
            loop {
                match (a.peek(), b.peek()) {
                    (Some(x), Some(y)) => {
                        if compare(spec, x, y).is_le() {
                            out.push(a.next().unwrap());
                        } else {
                            out.push(b.next().unwrap());
                        }
                    }
                    (Some(_), None) => out.push(a.next().unwrap()),
                    (None, Some(_)) => out.push(b.next().unwrap()),
                    (None, None) => break,
                }
            }
            self.items = out;
        }
        self.bump();
    }

    /// The listing finished. Returns names that are new compared with the
    /// rows shown before a reload (they get highlighted).
    pub fn finish_load(&mut self, elapsed_ms: f64) -> usize {
        self.elapsed_ms = Some(elapsed_ms);
        self.status = Status::Ready;
        let mut added = 0;
        if let Some(mut new) = self.reloading.take() {
            self.sort_items(&mut new);
            let before: HashSet<&str> = self.items.iter().map(|i| i.entry.name.as_str()).collect();
            let now = Instant::now();
            let fresh: Vec<String> = new.iter().filter(|i| !before.contains(i.entry.name.as_str())).map(|i| i.entry.name.clone()).collect();
            added = fresh.len();
            // A reload that replaced everything (first listing after an
            // error) isn't "new files", just a listing.
            if !self.items.is_empty() && added < 200 {
                for n in fresh {
                    self.fresh.insert(n, now);
                }
            }
            self.items = new;
        }
        self.bump();
        added
    }

    pub fn fail(&mut self, e: CxError) {
        self.reloading = None;
        self.status = Status::Error(e);
        self.bump();
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.items.iter().position(|i| i.entry.name == name)
    }

    pub fn get(&self, name: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.entry.name == name)
    }

    /// Insert or replace an entry, keeping the order. New names are
    /// highlighted when `mark_fresh`.
    pub fn upsert(&mut self, entry: Entry, mark_fresh: bool) {
        let existed = match self.index_of(&entry.name) {
            Some(i) => {
                if self.items[i].entry == entry {
                    return;
                }
                self.items.remove(i);
                true
            }
            None => false,
        };
        if !existed && mark_fresh {
            self.fresh.insert(entry.name.clone(), Instant::now());
        }
        let item = Item::new(entry);
        let at = if self.keep_order {
            self.items.len()
        } else {
            let spec = self.sort;
            self.items.partition_point(|x| compare(spec, x, &item).is_le())
        };
        self.items.insert(at, item);
        self.bump();
    }

    pub fn remove(&mut self, name: &str) -> bool {
        match self.index_of(name) {
            Some(i) => {
                self.items.remove(i);
                self.fresh.remove(name);
                self.bump();
                true
            }
            None => false,
        }
    }

    /// Apply watch patches; returns true when the folder must be re-listed.
    pub fn apply(&mut self, changes: Vec<Change>) -> bool {
        if self.reloading.is_some() || self.status == Status::Loading {
            // Mid-listing: the listing will include these; a reset is the
            // only thing worth remembering.
            return changes.iter().any(|c| matches!(c, Change::Reset));
        }
        let mut reset = false;
        // Large batches (a copy of thousands of files landing): collect the
        // upserts and merge once instead of inserting row by row.
        if changes.len() > 64 {
            let mut names: HashSet<String> = HashSet::new();
            let mut ups = Vec::new();
            for c in changes {
                match c {
                    Change::Upsert { entry } => {
                        names.insert(entry.name.clone());
                        ups.push(entry);
                    }
                    Change::Remove { name } => {
                        names.insert(name);
                    }
                    Change::Reset => reset = true,
                }
            }
            let before: HashSet<String> = self.items.iter().map(|i| i.entry.name.clone()).collect();
            self.items.retain(|i| !names.contains(&i.entry.name));
            let now = Instant::now();
            for e in &ups {
                if !before.contains(&e.name) {
                    self.fresh.insert(e.name.clone(), now);
                }
            }
            let mut add: Vec<Item> = ups.into_iter().map(Item::new).collect();
            self.sort_items(&mut add);
            self.items.append(&mut add);
            let mut all = std::mem::take(&mut self.items);
            self.sort_items(&mut all);
            self.items = all;
            self.bump();
            return reset;
        }
        for c in changes {
            match c {
                Change::Upsert { entry } => self.upsert(entry, true),
                Change::Remove { name } => {
                    self.remove(&name);
                }
                Change::Reset => reset = true,
            }
        }
        reset
    }

    pub fn set_sort(&mut self, sort: SortSpec) {
        if self.sort == sort {
            return;
        }
        self.sort = sort;
        let mut items = std::mem::take(&mut self.items);
        self.sort_items(&mut items);
        self.items = items;
        self.bump();
    }

    /// Drop expired highlights; true while some remain (keep animating).
    pub fn expire_fresh(&mut self) -> bool {
        if self.fresh.is_empty() {
            return false;
        }
        let before = self.fresh.len();
        self.fresh.retain(|_, t| t.elapsed() < FRESH_FOR);
        if self.fresh.len() != before {
            self.bump();
        }
        !self.fresh.is_empty()
    }

    pub fn is_fresh(&self, name: &str) -> bool {
        !self.fresh.is_empty() && self.fresh.get(name).is_some_and(|t| t.elapsed() < FRESH_FOR)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sort::SortKey;
    use cx_core::EntryKind;

    pub fn file(name: &str, size: u64, modified: i64) -> Entry {
        Entry { name: name.into(), kind: EntryKind::File, is_dir: false, size, modified: Some(modified), created: None, hidden: name.starts_with('.'), readonly: false }
    }

    pub fn dir(name: &str) -> Entry {
        Entry { name: name.into(), kind: EntryKind::Dir, is_dir: true, size: 0, modified: Some(0), created: None, hidden: name.starts_with('.'), readonly: false }
    }

    fn names(f: &Folder) -> Vec<&str> {
        f.items.iter().map(|i| i.name()).collect()
    }

    #[test]
    fn batches_merge_sorted_with_folders_first() {
        let mut f = Folder::new(1, "file:///x", SortSpec::default());
        f.add_batch(vec![file("b10", 1, 0), dir("zdir"), file("a", 1, 0)]);
        f.add_batch(vec![file("b2", 1, 0), dir("adir")]);
        f.finish_load(1.0);
        assert_eq!(names(&f), vec!["adir", "zdir", "a", "b2", "b10"]);
        f.set_sort(SortSpec { key: SortKey::Size, desc: true });
        assert_eq!(names(&f)[..2], ["adir", "zdir"]);
    }

    #[test]
    fn changes_patch_in_place_and_mark_fresh() {
        let mut f = Folder::with_items(1, "file:///x", SortSpec::default(), vec![Item::new(file("a", 1, 0)), Item::new(file("c", 1, 0))], false);
        let reset = f.apply(vec![Change::Upsert { entry: file("b", 1, 0) }, Change::Remove { name: "a".into() }]);
        assert!(!reset);
        assert_eq!(names(&f), vec!["b", "c"]);
        assert!(f.is_fresh("b"));
        assert!(!f.is_fresh("c"));
        assert!(f.apply(vec![Change::Reset]));
    }

    #[test]
    fn reload_keeps_rows_until_done_and_highlights_new_ones() {
        let mut f = Folder::with_items(1, "file:///x", SortSpec::default(), vec![Item::new(file("a", 1, 0))], false);
        f.begin_load(true);
        f.add_batch(vec![file("a", 1, 0), file("new", 1, 0)]);
        assert_eq!(names(&f), vec!["a"], "old rows stay while reloading");
        f.finish_load(1.0);
        assert_eq!(names(&f), vec!["a", "new"]);
        assert!(f.is_fresh("new"));
    }
}
