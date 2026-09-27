//! File operations: open, rename, new folder, trash / delete, clipboard,
//! copy and move (to the other pane or anywhere), duplicate, undo,
//! archives, tags, sizes, diffs, sync, shell and editor.

use super::{App, External, UndoEntry};
use crate::dialog::{Confirm, DestItem, DestPicker, Dialog, DiffLine, DiffView, Menu, MenuAction, MenuItem, MultiRename, Pending, Prompt, PromptKind, RenameItem, TagState, TagsDlg};
use crate::input::TextInput;
use crate::msg::Update;
use crate::tab::{child_uri, Source};
use crate::util::name_of;
use cx_core::{CxError, Location};
use cx_engine::SubmitRequest;
use cx_transfer::{ConflictPolicy, SyncDirection, UndoOp};
use std::collections::{BTreeMap, BTreeSet, HashSet};

fn describe(uris: &[String]) -> String {
    if uris.len() == 1 {
        format!("“{}”", name_of(&uris[0]))
    } else {
        format!("{} items", uris.len())
    }
}

impl App {
    pub(crate) fn submit(&mut self, kind: &str, sources: Vec<String>, dest: Option<String>, conflict: ConflictPolicy) -> Option<u64> {
        if sources.is_empty() {
            return None;
        }
        let r = self.engine.submit(SubmitRequest::new(kind, sources, dest).with_conflict(conflict));
        self.report(r)
    }

    /// Enter: folders and archives navigate (several open in tabs), files
    /// open in their default apps.
    pub(crate) fn open_targets(&mut self, new_tab: bool) {
        let t = self.tab();
        let targets: Vec<(String, bool, bool)> = t
            .targets()
            .into_iter()
            .map(|r| {
                let i = t.item(r);
                (t.uri_of(r), i.entry.is_dir && !i.name().ends_with(".app"), cx_archive::is_archive(i.name()))
            })
            .collect();
        let navigable = targets.iter().filter(|(_, d, a)| *d || *a).count();
        for (uri, is_dir, is_archive) in targets.iter().cloned() {
            let target = if is_dir {
                Some(uri.clone())
            } else if is_archive {
                Some(format!("archive://{uri}!/"))
            } else {
                None
            };
            match target {
                Some(t) if new_tab || navigable > 1 || targets.len() > 1 => {
                    let p = self.active;
                    self.open_tab(p, &t, !new_tab || navigable == 1);
                }
                Some(t) => {
                    // Compare view: Enter on a changed file shows the diff.
                    self.navigate(&t);
                }
                None => {
                    if let Source::Compare { left, right, .. } = &self.tab().source {
                        let rel = self.tab().cursor_item().map(|i| i.entry.name.clone()).unwrap_or_default();
                        let (l, r) = (join_rel(left, &rel), join_rel(right, &rel));
                        self.show_diff(l, r);
                        return;
                    }
                    self.spawn(move |engine| async move {
                        let r = engine.open_entry(&uri).await;
                        Box::new(move |app: &mut App| {
                            app.report(r);
                        }) as Update
                    });
                }
            }
        }
    }

    pub(crate) fn open_external(&mut self) {
        let uris = self.tab().target_uris();
        for uri in uris {
            self.spawn(move |engine| async move {
                let r = engine.open_entry(&uri).await;
                Box::new(move |app: &mut App| {
                    app.report(r);
                }) as Update
            });
        }
    }

    /// F4: edit in `$VISUAL` / `$EDITOR` with the TUI suspended. Remote
    /// files are downloaded first and offered for upload when changed.
    pub(crate) fn edit(&mut self) {
        let t = self.tab();
        let Some(r) = t.cursor_row() else { return };
        if t.item(r).entry.is_dir {
            return;
        }
        let uri = t.uri_of(r);
        let parent = t.parent_of(r);
        self.spawn(move |engine| async move {
            let local = engine.local_copy(&uri).await;
            Box::new(move |app: &mut App| {
                let Some(path) = app.report(local) else { return };
                let remote = !uri.starts_with("file:");
                let upload = remote.then(|| (parent, std::fs::metadata(&path).and_then(|m| m.modified()).ok()));
                app.external = Some(External::Edit { path, upload });
            }) as Update
        });
    }

    /// Called by the loop after the editor exits.
    pub fn edited(&mut self, path: std::path::PathBuf, upload: Option<(String, Option<std::time::SystemTime>)>) {
        let Some((dir, before)) = upload else { return };
        let after = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if after != before {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.dialogs.push(Dialog::Confirm(Confirm {
                title: format!("Upload “{name}”?"),
                body: format!("You changed the downloaded copy. Replace the file in {}?", crate::util::display(&dir)),
                ok: "Upload".into(),
                danger: false,
                then: Pending::Upload { local: Location::local(&path).uri(), dest_dir: dir },
                on_cancel: false,
            }));
        }
    }

    // ---- rename & new folder ----

    pub(crate) fn rename_prompt(&mut self) {
        let t = self.tab();
        if t.targets().len() > 1 {
            self.multi_rename();
            return;
        }
        let Some(r) = t.cursor_row() else { return };
        if matches!(t.source, Source::Home | Source::Compare { .. }) {
            return;
        }
        let name = t.item(r).name().to_string();
        let dir = t.parent_of(r);
        let mut input = TextInput::new(name.clone());
        // Cursor before the extension, like Finder's rename field.
        if !t.item(r).entry.is_dir {
            if let Some(i) = name.rfind('.').filter(|&i| i > 0) {
                input.cursor = name[..i].chars().count();
            }
        }
        self.dialogs.push(Dialog::Prompt(Prompt { title: "Rename".into(), label: format!("New name for “{name}”"), input, ok: "Rename".into(), kind: PromptKind::Rename { dir, from: name }, completions: Vec::new() }));
    }

    pub(crate) fn rename(&mut self, dir: String, from: String, to: String) {
        let to = to.trim().to_string();
        if to.is_empty() || to == from {
            return;
        }
        self.spawn(move |engine| async move {
            let r = engine.rename(&dir, &from, &to).await;
            Box::new(move |app: &mut App| {
                let Some(entry) = app.report(r) else { return };
                app.undo.push(UndoEntry { label: format!("Rename “{from}”"), ops: vec![UndoOp::Rename { dir: dir.clone(), from: from.clone(), to: to.clone() }] });
                app.after_local_change(&dir, &[from.as_str()], Some(entry));
            }) as Update
        });
    }

    /// Show a change we made ourselves right away (polled folders would
    /// otherwise lag; live ones get the same patch again, harmlessly).
    pub(crate) fn after_local_change(&mut self, dir: &str, removed: &[&str], added: Option<cx_core::Entry>) {
        let dir = dir.trim_end_matches('/');
        let mut select = None;
        for p in 0..2 {
            for t in self.panes[p].tabs.iter_mut() {
                let is_main = t.is_folder() && t.dir_uri().trim_end_matches('/') == dir;
                let key = added.as_ref().map(|e| e.name.clone());
                for f in t.folders_mut() {
                    if f.dir_uri().trim_end_matches('/') != dir || matches!(f.status, crate::folder::Status::Error(_)) {
                        continue;
                    }
                    let mut changes: Vec<cx_core::Change> = removed.iter().map(|r| cx_core::Change::Remove { name: r.to_string() }).collect();
                    if let Some(e) = &added {
                        changes.push(cx_core::Change::Upsert { entry: e.clone() });
                    }
                    f.apply_local(changes);
                }
                if is_main && p == self.active {
                    select = key;
                }
            }
        }
        if let Some(k) = select {
            let hidden = self.settings.show_hidden;
            self.tab_mut().refresh_rows(hidden);
            self.tab_mut().select_key(&k);
            let c = self.tab().cursor;
            self.tab_mut().select_only(c);
        }
    }

    pub(crate) fn new_folder_prompt(&mut self) {
        if !self.tab().writable() {
            return;
        }
        let dir = self.tab().dir_uri().to_string();
        let taken: HashSet<String> = self.tab().folder.items.iter().map(|i| i.lname.clone()).collect();
        let mut name = "New folder".to_string();
        let mut n = 2;
        while taken.contains(&name.to_lowercase()) {
            name = format!("New folder ({n})");
            n += 1;
        }
        self.dialogs.push(Dialog::Prompt(Prompt { title: "New folder".into(), label: "Name".into(), input: TextInput::new(name), ok: "Create".into(), kind: PromptKind::NewFolder { dir }, completions: Vec::new() }));
    }

    pub(crate) fn create_folder(&mut self, dir: String, name: String) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        self.spawn(move |engine| async move {
            let r = engine.create_folder(&dir, Some(&name)).await;
            Box::new(move |app: &mut App| {
                let Some(entry) = app.report(r) else { return };
                app.undo.push(UndoEntry { label: format!("New folder “{}”", entry.name), ops: vec![UndoOp::NewFolder { uri: child_uri(&dir, &entry.name) }] });
                app.tab_mut().filter.clear();
                app.after_local_change(&dir, &[], Some(entry));
            }) as Update
        });
    }

    pub(crate) fn multi_rename(&mut self) {
        let t = self.tab();
        if matches!(t.source, Source::Home | Source::Compare { .. }) {
            return;
        }
        let rows: Vec<_> = if t.selected_rows().len() > 1 { t.selected_rows() } else { t.rows().iter().collect() };
        if rows.is_empty() {
            return;
        }
        let items: Vec<RenameItem> = rows.iter().map(|r| RenameItem { dir: t.parent_of(r), name: t.item(r).name().to_string(), is_dir: t.item(r).entry.is_dir, modified: t.item(r).entry.modified }).collect();
        let renaming: HashSet<&str> = items.iter().map(|i| i.name.as_str()).collect();
        let others = t.folder.items.iter().map(|i| i.name().to_string()).filter(|n| !renaming.contains(n.as_str())).collect();
        let parent = t.title();
        self.dialogs.push(Dialog::MultiRename(MultiRename {
            items,
            others,
            parent,
            name_mask: TextInput::new("[N]"),
            ext_mask: TextInput::new("[E]"),
            search: TextInput::default(),
            replace: TextInput::default(),
            regex: false,
            case: Default::default(),
            start: TextInput::new("1"),
            step: TextInput::new("1"),
            digits: TextInput::new("2"),
            focus: 0,
            scroll: 0,
            busy: false,
        }));
    }

    /// Apply a multi-rename in two phases (through temporary names) so
    /// swaps like a→b, b→a never collide; undo is one batch.
    pub(crate) fn apply_multi_rename(&mut self, mr: &MultiRename) {
        let plan = mr.plan();
        if plan.iter().any(|p| !p.problem.is_empty()) {
            self.error("Some new names are invalid or clash; fix them first");
            return;
        }
        let changes: Vec<(String, String, String)> = mr.items.iter().zip(plan).filter(|(_, p)| p.to != p.from).map(|(i, p)| (i.dir.clone(), p.from, p.to)).collect();
        if changes.is_empty() {
            return;
        }
        self.spawn(move |engine| async move {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
            let mut done: Vec<UndoOp> = Vec::new();
            let mut error = None;
            let mut temp = Vec::new();
            for (i, (dir, from, _)) in changes.iter().enumerate() {
                let tmp = format!(".cx-rename-{stamp}-{i}");
                match engine.rename(dir, from, &tmp).await {
                    Ok(_) => temp.push((i, tmp)),
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                }
            }
            for (i, tmp) in &temp {
                let (dir, from, to) = &changes[*i];
                // After a failure in phase one, put the moved ones back.
                let target = if error.is_some() { from } else { to };
                match engine.rename(dir, tmp, target).await {
                    Ok(_) if error.is_none() => done.push(UndoOp::Rename { dir: dir.clone(), from: from.clone(), to: to.clone() }),
                    Ok(_) => {}
                    Err(e) => error = error.or(Some(e)),
                }
            }
            let dirs: HashSet<String> = changes.iter().map(|(d, _, _)| d.clone()).collect();
            Box::new(move |app: &mut App| {
                if !done.is_empty() {
                    let n = done.len();
                    app.undo.push(UndoEntry { label: format!("Rename {n} items"), ops: done });
                    app.toast(format!("Renamed {n} {}", if n == 1 { "item" } else { "items" }));
                }
                if let Some(e) = error {
                    app.error(format!("Rename stopped: {e}"));
                }
                app.reload_dirs(&dirs);
            }) as Update
        });
    }

    /// Re-list every shown folder in `dirs`.
    pub(crate) fn reload_dirs(&mut self, dirs: &HashSet<String>) {
        let dirs: HashSet<String> = dirs.iter().map(|d| d.trim_end_matches('/').to_string()).collect();
        let tokens: Vec<u64> = self.panes.iter().flat_map(|p| p.tabs.iter()).flat_map(|t| t.folders()).filter(|f| dirs.contains(f.dir_uri().trim_end_matches('/'))).map(|f| f.token).collect();
        for t in tokens {
            self.reload_token(t);
        }
        if matches!(self.tab().source, Source::Search { .. } | Source::Tag { .. }) {
            self.reload();
        }
    }

    // ---- trash & delete ----

    pub(crate) fn trash(&mut self) {
        let t = self.tab();
        if matches!(t.source, Source::Home | Source::Compare { .. }) || t.targets().is_empty() {
            return;
        }
        if t.folder.caps.is_some_and(|c| !c.trash) && t.is_folder() {
            self.delete_confirm(true);
            return;
        }
        let mut by_dir: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for r in t.targets() {
            by_dir.entry(t.parent_of(r)).or_default().push(t.item(r).name().to_string());
        }
        let groups: Vec<(String, Vec<String>)> = by_dir.into_iter().collect();
        if self.settings.confirm_trash {
            let n: usize = groups.iter().map(|g| g.1.len()).sum();
            let body = if n == 1 { format!("“{}”", groups[0].1[0]) } else { format!("{n} items") };
            self.dialogs.push(Dialog::Confirm(Confirm { title: "Move to Trash?".into(), body, ok: "Move to Trash".into(), danger: false, then: Pending::Trash(groups), on_cancel: false }));
        } else {
            self.trash_now(groups);
        }
    }

    pub(crate) fn trash_now(&mut self, groups: Vec<(String, Vec<String>)>) {
        // Move the cursor to the row after the deleted block, like Explorer.
        let t = self.tab_mut();
        let gone: HashSet<String> = t.targets().into_iter().map(|r| t.key_of(r).to_string()).collect();
        let last = t.rows().iter().rposition(|r| gone.contains(t.key_of(r))).unwrap_or(0);
        let next = t.rows().iter().skip(last + 1).map(|r| t.key_of(r).to_string()).find(|k| !gone.contains(k)).or_else(|| t.rows()[..last.min(t.rows().len())].iter().rev().map(|r| t.key_of(r).to_string()).find(|k| !gone.contains(k)));
        t.clear_selection();
        if let Some(k) = next {
            t.select_key(&k);
        }
        let total: usize = groups.iter().map(|g| g.1.len()).sum();
        let label = if total == 1 { format!("“{}”", groups[0].1[0]) } else { format!("{total} items") };
        for (dir, names) in &groups {
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            self.after_local_change(dir, &names, None);
        }
        self.spawn(move |engine| async move {
            let mut items = Vec::new();
            let mut err = None;
            for (dir, names) in &groups {
                match engine.trash(dir, names).await {
                    Ok(v) => items.extend(v),
                    Err(e) => err = Some(e),
                }
            }
            let dirs: HashSet<String> = groups.iter().map(|g| g.0.clone()).collect();
            Box::new(move |app: &mut App| {
                if !items.is_empty() {
                    app.undo.push(UndoEntry { label: format!("Move {label} to Trash"), ops: vec![UndoOp::Trash { items }] });
                    app.toast(format!("Moved {label} to Trash"));
                }
                if let Some(e) = err {
                    app.error(e);
                    app.reload_dirs(&dirs);
                }
            }) as Update
        });
    }

    pub(crate) fn delete(&mut self) {
        self.delete_confirm(false);
    }

    fn delete_confirm(&mut self, no_trash: bool) {
        let uris = self.tab().target_uris();
        if uris.is_empty() || matches!(self.tab().source, Source::Home | Source::Compare { .. }) {
            return;
        }
        if self.settings.confirm_permanent_delete || no_trash {
            self.dialogs.push(Dialog::Confirm(Confirm {
                title: if no_trash { "This location has no trash. Delete permanently?".into() } else { "Delete permanently?".into() },
                body: format!("{} will be deleted immediately. You can't undo this.", describe(&uris)),
                ok: "Delete".into(),
                danger: true,
                then: Pending::Delete(uris),
                on_cancel: true,
            }));
        } else {
            self.submit("delete", uris, None, ConflictPolicy::Ask);
        }
    }

    // ---- clipboard & transfers ----

    pub(crate) fn copy(&mut self, cut: bool) {
        let uris = self.tab().target_uris();
        if uris.is_empty() {
            return;
        }
        self.toast(format!("{} {}", if cut { "Cut" } else { "Copied" }, describe(&uris)));
        if self.settings.os_clipboard {
            let local = uris.clone();
            // Local files also go on the system clipboard for Finder / Explorer.
            tokio::task::spawn_blocking(move || {
                let _ = cx_engine::system::os_clipboard_set(&local);
            });
        }
        self.clipboard = Some((uris, cut));
    }

    pub(crate) fn paste(&mut self) {
        if !self.tab().writable() {
            return;
        }
        let dest = self.tab().dir_uri().to_string();
        let ours: Vec<String> = self.clipboard.as_ref().map(|c| c.0.iter().filter(|u| u.starts_with("file:")).cloned().collect()).unwrap_or_default();
        let use_os = self.settings.os_clipboard;
        self.spawn(move |_| async move {
            // Files copied in another app win when they differ from ours.
            let os = if !use_os {
                Vec::new()
            } else {
                tokio::time::timeout(std::time::Duration::from_millis(500), tokio::task::spawn_blocking(cx_engine::system::os_clipboard_get)).await.ok().and_then(|r| r.ok()).unwrap_or_default()
            };
            Box::new(move |app: &mut App| {
                if !os.is_empty() && (os.len() != ours.len() || os.iter().any(|u| !ours.contains(u))) {
                    app.submit("copy", os, Some(dest), ConflictPolicy::Ask);
                    return;
                }
                let Some((uris, cut)) = app.clipboard.clone() else {
                    app.toast("Nothing to paste");
                    return;
                };
                app.submit(if cut { "move" } else { "copy" }, uris, Some(dest), ConflictPolicy::Ask);
                if cut {
                    app.clipboard = None;
                }
            }) as Update
        });
    }

    pub(crate) fn duplicate(&mut self) {
        if !self.tab().writable() {
            return;
        }
        let uris = self.tab().target_uris();
        let dest = self.tab().dir_uri().to_string();
        self.submit("copy", uris, Some(dest), ConflictPolicy::KeepBoth);
    }

    pub(crate) fn undo_last(&mut self) {
        let Some(entry) = self.undo.pop() else {
            self.toast("Nothing to undo");
            return;
        };
        self.spawn(move |engine| async move {
            let mut err = None;
            let mut dirs = HashSet::new();
            for op in entry.ops.iter().rev() {
                dirs.extend(op_dirs(op));
                if let Err(e) = engine.undo(op.clone()).await {
                    err = Some(e);
                }
            }
            Box::new(move |app: &mut App| {
                match err {
                    Some(e) => app.error(format!("Couldn't undo {}: {e}", entry.label)),
                    None => app.toast(format!("Undid: {}", entry.label)),
                }
                app.refresh_polled(&dirs);
            }) as Update
        });
    }

    /// F5 / F6: copy or move the selection to the other pane.
    pub(crate) fn to_other_pane(&mut self, moving: bool) {
        let uris = self.tab().target_uris();
        if uris.is_empty() || matches!(self.tab().source, Source::Home) {
            return;
        }
        if matches!(self.tab().source, Source::Compare { .. }) {
            self.sync(if moving { SyncDirection::RightToLeft } else { SyncDirection::LeftToRight });
            return;
        }
        let Some(other) = self.other_tab() else {
            // Single pane: pick a destination instead.
            self.destination_picker(moving);
            return;
        };
        let dest = other.dir_uri().to_string();
        if !other.writable() {
            self.error("The other pane isn't a writable folder");
            return;
        }
        let kind = if moving { "move" } else { "copy" };
        if self.settings.confirm_transfer {
            self.dialogs.push(Dialog::Confirm(Confirm {
                title: format!("{} {}?", if moving { "Move" } else { "Copy" }, describe(&uris)),
                body: format!("To {}", crate::util::display(&dest)),
                ok: if moving { "Move".into() } else { "Copy".into() },
                danger: false,
                then: Pending::Transfer { kind, sources: uris, dests: vec![dest] },
                on_cancel: false,
            }));
        } else {
            self.submit(kind, uris, Some(dest), ConflictPolicy::Ask);
        }
    }

    /// Copy to… / Move to…: every place the files could go.
    pub(crate) fn destination_picker(&mut self, moving: bool) {
        let sources = self.tab().target_uris();
        if sources.is_empty() || matches!(self.tab().source, Source::Home | Source::Compare { .. }) {
            return;
        }
        let here = self.tab().dir_uri().to_string();
        let mut items: Vec<DestItem> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        seen.insert(here.clone());
        let mut add = |label: String, uri: String, section: &'static str| {
            if !uri.starts_with("cx:") && seen.insert(uri.clone()) {
                items.push(DestItem { detail: crate::util::display(&uri), label, uri, section });
            }
        };
        if let Some(o) = self.other_tab() {
            add(format!("Other pane: {}", o.title()), o.dir_uri().to_string(), "Panes");
        }
        for (p, pane) in self.panes.iter().enumerate() {
            for t in &pane.tabs {
                if t.is_folder() && (p != self.active || t.id != self.tab().id) {
                    add(t.title(), t.dir_uri().to_string(), "Tabs");
                }
            }
        }
        for u in &self.settings.recent_destinations {
            add(name_of(u), u.clone(), "Recent destinations");
        }
        for b in &self.settings.bookmarks {
            add(b.name.clone(), b.uri.clone(), "Favorites");
        }
        for u in self.settings.recent.iter().take(10) {
            add(name_of(u), u.clone(), "Recent folders");
        }
        for s in &self.settings.servers {
            add(s.name.clone(), s.uri.clone(), "Servers");
        }
        for c in self.engine.connections() {
            add(crate::util::display(&c), c, "Servers");
        }
        for d in self.devices.iter().filter(|d| !d.is_self()) {
            for s in &d.shares {
                add(format!("{} — {}", d.name, s.name), s.uri.clone(), "Devices");
            }
        }
        if let Some(pl) = &self.places {
            add(pl.home.name.clone(), pl.home.uri.clone(), "Drives");
            for f in &pl.favorites {
                add(f.name.clone(), f.uri.clone(), "Drives");
            }
            for v in &pl.volumes {
                add(v.name.clone(), v.uri.clone(), "Drives");
            }
        }
        self.dialogs.push(Dialog::DestPicker(DestPicker { moving, sources, items, checked: BTreeSet::new(), cursor: 0, input: TextInput::default() }));
    }

    /// Run the picker's choice: one job per destination.
    pub(crate) fn transfer_to(&mut self, moving: bool, sources: Vec<String>, dests: Vec<String>) {
        let kind = if moving { "move" } else { "copy" };
        for d in dests {
            let uri = match Location::parse(&d) {
                Ok(l) => l.uri(),
                Err(e) => {
                    self.error(e);
                    continue;
                }
            };
            self.settings.add_recent_destination(&uri);
            self.submit(kind, sources.clone(), Some(uri), ConflictPolicy::Ask);
        }
    }

    pub(crate) fn copy_path(&mut self) {
        let t = self.tab();
        let uris = if t.targets().is_empty() { vec![t.dir_uri().to_string()] } else { t.target_uris() };
        let text: Vec<String> = uris.iter().map(|u| Location::parse(u).ok().and_then(|l| l.local_path().map(|p| p.display().to_string())).unwrap_or_else(|| u.clone())).collect();
        self.osc52 = Some(text.join("\n"));
        self.toast(if text.len() > 1 { format!("Copied {} paths", text.len()) } else { "Copied path".to_string() });
    }

    pub(crate) fn reveal(&mut self) {
        let t = self.tab();
        let uri = t.cursor_row().map(|r| t.uri_of(r)).unwrap_or_else(|| t.dir_uri().to_string());
        let r = cx_engine::system::reveal_entry(&uri);
        self.report(r);
    }

    /// Ctrl+O: a shell (or ssh session) in this folder, TUI suspended.
    pub(crate) fn shell(&mut self) {
        let t = self.tab();
        let uri = match &t.source {
            Source::Folder => t.dir_uri().to_string(),
            Source::Search { root, .. } => root.clone(),
            _ => cx_core::location::home_dir().map(|h| Location::local(h).uri()).unwrap_or_default(),
        };
        match Location::parse(&uri).and_then(|l| cx_term::ShellCommand::for_location(&l)) {
            Ok(cmd) => self.external = Some(External::Shell { argv: cmd.argv, cwd: cmd.cwd }),
            Err(e) => self.error(e),
        }
    }

    // ---- sizes, archives, tags ----

    pub(crate) fn calc_sizes(&mut self) {
        let t = self.tab();
        let sel = t.selected_rows();
        let rows = if sel.is_empty() { t.rows().iter().collect() } else { sel };
        let uris: Vec<String> = rows.into_iter().filter(|r| t.item(r).entry.is_dir).map(|r| t.uri_of(r)).collect();
        for u in uris {
            self.compute_size(u);
        }
    }

    pub(crate) fn compute_size(&mut self, uri: String) {
        if self.sizes.get(&uri).is_some_and(|s| !s.1) {
            return; // already counting
        }
        self.sizes.insert(uri.clone(), (0, false));
        let tx = self.sender();
        let limit = self.size_limit.clone();
        let engine = self.engine.clone();
        tokio::spawn(async move {
            let _permit = limit.acquire_owned().await;
            let u = uri.clone();
            let r = engine
                .dir_size(&uri, move |p| {
                    let u = u.clone();
                    tx.send(crate::msg::Msg::Apply(Box::new(move |app: &mut App| {
                        app.sizes.insert(u, (p.bytes, p.done));
                    })))
                    .is_ok()
                })
                .await;
            drop(r);
        });
    }

    pub(crate) fn compress_prompt(&mut self) {
        if !self.tab().writable() {
            return;
        }
        let sources = self.tab().target_uris();
        if sources.is_empty() {
            return;
        }
        let base = if sources.len() == 1 { cx_archive::strip_archive_ext(&name_of(&sources[0])).to_string() } else { "Archive".into() };
        let base = if sources.len() == 1 && !self.tab().cursor_item().is_some_and(|i| i.entry.is_dir) { base.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or(base) } else { base };
        let dir = self.tab().dir_uri().to_string();
        self.dialogs.push(Dialog::Prompt(Prompt { title: "Compress to ZIP".into(), label: "Archive name".into(), input: TextInput::new(format!("{base}.zip")), ok: "Compress".into(), kind: PromptKind::Compress { sources, dir }, completions: Vec::new() }));
    }

    pub(crate) fn compress(&mut self, sources: Vec<String>, dir: String, name: String) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let name = if name.to_lowercase().ends_with(".zip") { name.to_string() } else { format!("{name}.zip") };
        self.submit("compress", sources, Some(child_uri(&dir, &name)), ConflictPolicy::KeepBoth);
    }

    pub(crate) fn extract(&mut self) {
        let t = self.tab();
        let uris: Vec<String> = t.targets().into_iter().filter(|r| cx_archive::is_archive(t.item(r).name())).map(|r| t.uri_of(r)).collect();
        if uris.is_empty() || !t.writable() {
            return;
        }
        let dest = t.dir_uri().to_string();
        self.submit("extract", uris, Some(dest), ConflictPolicy::KeepBoth);
    }

    pub(crate) fn tags_dialog(&mut self) {
        let uris = self.tab().target_uris();
        if uris.is_empty() {
            return;
        }
        let mut names = self.engine.tags.known();
        for u in &uris {
            for t in self.tags.get(u).cloned().unwrap_or_default() {
                if !names.contains(&t) {
                    names.push(t);
                }
            }
        }
        let tags = names
            .into_iter()
            .map(|n| {
                let have = uris.iter().filter(|u| self.tags.get(*u).is_some_and(|t| t.contains(&n))).count();
                let state = if have == 0 { TagState::None } else if have == uris.len() { TagState::All } else { TagState::Some };
                (n, state)
            })
            .collect();
        self.dialogs.push(Dialog::Tags(TagsDlg { uris, tags, cursor: 0, input: TextInput::default(), busy: false }));
    }

    pub(crate) fn apply_tags(&mut self, dlg: TagsDlg) {
        let current: Vec<(String, Vec<String>)> = dlg.uris.iter().map(|u| (u.clone(), self.tags.get(u).cloned().unwrap_or_default())).collect();
        let plan: Vec<(String, Vec<String>)> = current
            .into_iter()
            .map(|(u, mut have)| {
                for (name, state) in &dlg.tags {
                    match state {
                        TagState::All if !have.contains(name) => have.push(name.clone()),
                        TagState::None => have.retain(|t| t != name),
                        _ => {}
                    }
                }
                (u, have)
            })
            .collect();
        self.spawn(move |engine| async move {
            let result = tokio::task::spawn_blocking(move || {
                let mut out = Vec::new();
                for (u, t) in plan {
                    engine.tags.set(&u, t.clone())?;
                    out.push((u, t));
                }
                Ok::<_, CxError>(out)
            })
            .await
            .map_err(|e| CxError::Io(e.to_string()))
            .and_then(|r| r);
            Box::new(move |app: &mut App| {
                if let Some(done) = app.report(result) {
                    for (u, t) in done {
                        app.tags.insert(u, t);
                    }
                }
            }) as Update
        });
    }

    pub(crate) fn send_to_menu(&mut self) {
        let uris = self.tab().target_uris();
        if uris.is_empty() || !uris.iter().all(|u| u.starts_with("file:")) {
            self.toast("Only local files can be sent");
            return;
        }
        let mut items = Vec::new();
        let mut seen = HashSet::new();
        for d in self.devices.iter().filter(|d| !d.is_self()) {
            for s in d.services.iter().filter(|s| s.scheme == cx_core::Scheme::Peer) {
                if seen.insert(s.uri.clone()) {
                    items.push(MenuItem::new(d.name.clone(), s.uri.clone(), MenuAction::SendTo(s.uri.clone())));
                }
            }
        }
        if let Some(p) = &self.peer {
            for t in &p.trusted {
                let uri = format!("peer://{}/", t.id);
                if seen.insert(uri.clone()) {
                    items.push(MenuItem::new(t.name.clone(), uri.clone(), MenuAction::SendTo(uri)));
                }
            }
        }
        if items.is_empty() {
            self.toast("No devices to send to. Pair one first (palette → Pair a device)");
            return;
        }
        self.pending_send = Some(uris);
        self.dialogs.push(Dialog::Menu(Menu::new("Send to device", items)));
    }

    pub(crate) fn send_to(&mut self, device: String) {
        let Some(uris) = self.pending_send.take() else { return };
        self.spawn(move |engine| async move {
            let r = engine.peer_send(&device, &uris).await;
            Box::new(move |app: &mut App| {
                if app.report(r).is_some() {
                    app.toast("Offer sent; waiting for the other device to accept");
                }
            }) as Update
        });
    }

    // ---- diff & sync ----

    pub(crate) fn diff_files(&mut self) {
        let t = self.tab();
        let files: Vec<String> = t.selected_rows().into_iter().filter(|r| !t.item(r).entry.is_dir).map(|r| t.uri_of(r)).collect();
        let (a, b) = if files.len() == 2 {
            (files[0].clone(), files[1].clone())
        } else {
            let Some(r) = t.cursor_row() else { return };
            let Some(o) = self.other_tab() else {
                self.toast("Select two files, or put the cursor on a file in each pane");
                return;
            };
            let Some(or) = o.cursor_row() else { return };
            (t.uri_of(r), o.uri_of(or))
        };
        self.show_diff(a, b);
    }

    pub fn show_diff(&mut self, left: String, right: String) {
        self.spawn(move |engine| async move {
            const MAX: usize = 4 << 20;
            let (a, b) = tokio::join!(engine.preview_text(&left, MAX), engine.preview_text(&right, MAX));
            let result = a.and_then(|a| b.map(|b| (a.text, b.text)));
            Box::new(move |app: &mut App| {
                let Some((a, b)) = app.report(result) else { return };
                app.dialogs.push(Dialog::Diff(diff_view(left, right, &a, &b)));
            }) as Update
        });
    }

    pub(crate) fn sync(&mut self, direction: SyncDirection) {
        let Source::Compare { left, right, diff, .. } = self.tab().source.clone() else { return };
        let r = self.engine.sync_dirs(&left, &right, &diff, direction);
        if let Some(ids) = self.report(r) {
            if ids.is_empty() {
                self.toast("Nothing to copy");
            } else {
                self.toast(format!("Syncing: {} copy {}", ids.len(), if ids.len() == 1 { "job" } else { "jobs" }));
            }
        }
    }
}

fn join_rel(root: &str, rel: &str) -> String {
    rel.split('/').filter(|s| !s.is_empty()).fold(root.to_string(), |acc, s| child_uri(&acc, s))
}

/// Folders an undo touches (to refresh polled views).
fn op_dirs(op: &UndoOp) -> Vec<String> {
    let parent = |u: &str| Location::parse(u).ok().and_then(|l| l.parent()).map(|p| p.uri());
    match op {
        UndoOp::Copy { created } => created.iter().filter_map(|u| parent(u)).collect(),
        UndoOp::Move { items } => items.iter().flat_map(|i| [parent(&i.from), parent(&i.to)]).flatten().collect(),
        UndoOp::Rename { dir, .. } => vec![dir.clone()],
        UndoOp::Trash { items } => items.iter().filter_map(|i| parent(&i.original)).collect(),
        UndoOp::NewFolder { uri } => parent(uri).into_iter().collect(),
    }
}

/// Line diff of two texts.
pub fn diff_view(left: String, right: String, a: &str, b: &str) -> DiffView {
    let diff = similar::TextDiff::from_lines(a, b);
    let mut lines = Vec::new();
    let (mut added, mut removed) = (0, 0);
    for change in diff.iter_all_changes() {
        let tag = match change.tag() {
            similar::ChangeTag::Delete => {
                removed += 1;
                '-'
            }
            similar::ChangeTag::Insert => {
                added += 1;
                '+'
            }
            similar::ChangeTag::Equal => ' ',
        };
        lines.push(DiffLine { tag, left: change.old_index().map(|i| i + 1), right: change.new_index().map(|i| i + 1), text: change.value().trim_end_matches(['\n', '\r']).to_string() });
    }
    DiffView { left, right, lines, added, removed, scroll: 0 }
}
