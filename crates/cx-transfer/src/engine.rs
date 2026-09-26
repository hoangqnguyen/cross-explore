//! Running a job: scan sources for totals, then walk them, creating folders
//! in order and handing files to concurrent tasks.

use crate::copy::{self, FileTask};
use crate::manager::Job;
use crate::naming::{copy_name, free_name, numbered_name};
use crate::undo::MovedItem;
use crate::{ConflictPolicy, JobKind, JobState, Resolution, TransferManager};
use cx_core::provider::list_all;
use cx_core::{CxError, Entry, EntryKind, Location, Provider, Result};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::task::JoinSet;

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Files below this size are grouped so one stream slot copies many of them
/// back to back instead of paying task and permit overhead per file.
const SMALL_FILE: u64 = 1024 * 1024;
const BATCH_FILES: usize = 32;
const BATCH_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct Ctx {
    pub mgr: Arc<TransferManager>,
    pub job: Arc<Job>,
}

impl Ctx {
    async fn checkpoint(&self) -> Result<()> {
        self.job.ctrl.checkpoint().await
    }

    fn file_error(&self, uri: String, err: &CxError) {
        self.mgr.file_error(&self.job, uri, err);
    }
}

pub(crate) async fn run(mgr: &Arc<TransferManager>, job: &Arc<Job>) -> Result<()> {
    let cx = Ctx { mgr: mgr.clone(), job: job.clone() };
    cx.checkpoint().await?;
    match job.req.kind {
        JobKind::Copy | JobKind::Move => transfer(&cx).await,
        JobKind::Delete | JobKind::Trash => remove(&cx).await,
    }
}

/// True when `inner` is `outer` or somewhere below it.
pub(crate) fn is_within(inner: &Location, outer: &Location) -> bool {
    if !inner.same_provider(outer) {
        return false;
    }
    match (inner, outer) {
        (Location::Local(a), Location::Local(b)) => a.starts_with(b),
        _ => {
            let a = inner.posix_path().unwrap_or("");
            let b = outer.posix_path().unwrap_or("").trim_end_matches('/');
            b.is_empty() || a == b || a.starts_with(&format!("{b}/"))
        }
    }
}

/// A scanned source. `children` is `None` for files and unreadable folders.
struct Item {
    loc: Location,
    entry: Entry,
    children: Option<Vec<Item>>,
}

impl Item {
    fn is_real_dir(&self) -> bool {
        self.entry.is_dir && self.entry.kind == EntryKind::Dir
    }

    fn totals(&self) -> (u64, u64) {
        match &self.children {
            Some(kids) => kids.iter().map(Item::totals).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1)),
            None if self.is_real_dir() => (0, 0),
            None => (1, self.entry.size),
        }
    }
}

fn scan<'a>(cx: &'a Ctx, p: &'a Arc<dyn Provider>, loc: Location, entry: Entry) -> BoxFut<'a, Result<Item>> {
    Box::pin(async move {
        cx.checkpoint().await?;
        // Symlinked folders are not followed: they can loop.
        if !(entry.is_dir && entry.kind == EntryKind::Dir) {
            cx.job.counters.add_total(1, if entry.is_dir { 0 } else { entry.size });
            return Ok(Item { loc, entry, children: None });
        }
        let children = match list_all(p.as_ref(), &loc).await {
            Ok(list) => {
                let mut kids = Vec::with_capacity(list.len());
                for e in list {
                    kids.push(scan(cx, p, loc.join(&e.name), e).await?);
                }
                Some(kids)
            }
            Err(e) => {
                cx.file_error(loc.uri(), &e);
                None
            }
        };
        Ok(Item { loc, entry, children })
    })
}

enum Plan {
    /// Same provider move: try a rename first.
    Rename,
    Stream(Item),
}

async fn transfer(cx: &Ctx) -> Result<()> {
    let (job, vfs) = (&cx.job, cx.mgr.vfs());
    let is_move = job.req.kind == JobKind::Move;
    let dest = Location::parse(job.req.dest.as_deref().ok_or_else(|| CxError::InvalidLocation("no destination folder".into()))?)?;
    let dst_p = vfs.provider(&dest).await?;
    if !dst_p.stat(&dest).await?.is_dir {
        return Err(CxError::InvalidLocation(format!("{} is not a folder", dest.info().display)));
    }
    cx.mgr.set_phase(job, JobState::Scanning);

    let mut plans = Vec::new();
    for uri in &job.req.sources {
        cx.checkpoint().await?;
        let found = async {
            let src = Location::parse(uri)?;
            let p = vfs.provider(&src).await?;
            let e = p.stat(&src).await?;
            Ok::<_, CxError>((src, p, e))
        }
        .await;
        let (src, src_p, entry) = match found {
            Ok(x) => x,
            // A resumed move already moved it.
            Err(CxError::NotFound(_)) if job.restored => continue,
            Err(e) => {
                cx.file_error(uri.clone(), &e);
                continue;
            }
        };
        if entry.is_dir && is_within(&dest, &src) {
            let verb = if is_move { "move" } else { "copy" };
            return Err(CxError::InvalidLocation(format!("can't {verb} \"{}\" into itself", entry.name)));
        }
        let same_dir = src.parent().as_ref() == Some(&dest);
        if is_move && same_dir {
            continue;
        }
        let target = job.record.lock().unwrap().targets.get(&src.uri()).cloned();
        let name = match target {
            Some(t) => Location::parse(&t)?.name(),
            None if same_dir => free_name(dst_p.as_ref(), &dest, 1, |n| copy_name(&entry.name, entry.is_dir, n)).await?,
            None => entry.name.clone(),
        };
        job.record(|r| r.targets.insert(src.uri(), dest.join(&name).uri()));
        let plan = if is_move && src.same_provider(&dest) {
            job.counters.add_total(1, if entry.is_dir { 0 } else { entry.size });
            Plan::Rename
        } else {
            Plan::Stream(scan(cx, &src_p, src.clone(), entry.clone()).await?)
        };
        plans.push((src, src_p, entry, name, plan));
    }

    cx.mgr.set_phase(job, JobState::Running);
    let mut w = Walker::new(cx.clone(), dst_p, is_move);
    let walked = async {
        for (src, p, entry, name, plan) in plans {
            match plan {
                Plan::Rename => w.move_fast(&p, src, entry, dest.clone(), name).await?,
                Plan::Stream(item) => w.copy_item(&item, &dest, name, &View::Unknown, false).await?,
            }
        }
        w.flush().await
    }
    .await;
    w.drain().await;
    walked?;
    cx.checkpoint().await?;
    w.finalize().await;
    Ok(())
}

/// What the walker knows about a destination folder's contents.
enum View {
    /// Not listed: ask the provider per name.
    Unknown,
    Known(HashMap<String, Entry>),
}

struct DirDone {
    src: Location,
    dst: Location,
    modified: Option<i64>,
}

struct Walker {
    cx: Ctx,
    dst_p: Arc<dyn Provider>,
    delete_src: bool,
    tasks: JoinSet<()>,
    batch: Vec<FileTask>,
    batch_bytes: u64,
    dirs: Vec<DirDone>,
    /// Destinations handed to tasks that may not exist yet.
    planned: HashMap<String, Entry>,
    /// Source URIs not transferred (skipped or failed); their folders stay.
    problems: Arc<Mutex<Vec<String>>>,
    /// Answer decided before falling back from a rename to a copy.
    preset: Option<Resolution>,
}

impl Walker {
    fn new(cx: Ctx, dst_p: Arc<dyn Provider>, delete_src: bool) -> Self {
        Walker {
            cx,
            dst_p,
            delete_src,
            tasks: JoinSet::new(),
            batch: Vec::new(),
            batch_bytes: 0,
            dirs: Vec::new(),
            planned: HashMap::new(),
            problems: Arc::new(Mutex::new(Vec::new())),
            preset: None,
        }
    }

    async fn lookup(&self, view: &View, dst: &Location) -> Result<Option<Entry>> {
        if let Some(e) = self.planned.get(&dst.uri()) {
            return Ok(Some(e.clone()));
        }
        match view {
            View::Known(map) => Ok(map.get(&dst.name()).cloned()),
            View::Unknown => match self.dst_p.stat(dst).await {
                Ok(e) => Ok(Some(e)),
                Err(CxError::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            },
        }
    }

    /// Record an item that won't be transferred, counting it as done.
    fn give_up(&self, item: &Item, err: Option<&CxError>) {
        if let Some(e) = err {
            self.cx.file_error(item.loc.uri(), e);
        }
        self.problems.lock().unwrap().push(item.loc.uri());
        let (files, bytes) = item.totals();
        self.cx.job.counters.skip(files, bytes);
    }

    fn record_top(&self, src: &Location, dst: &Location) {
        let delete_src = self.delete_src;
        self.cx.job.record(|r| {
            if delete_src {
                r.moved.push(MovedItem { from: src.uri(), to: dst.uri() });
            } else {
                r.created.push(dst.uri());
            }
        });
    }

    /// Settle a name clash to Replace, Skip or KeepBoth.
    async fn decide(&mut self, src: &Location, src_e: &Entry, dst: &Location, dst_e: &Entry) -> Result<Resolution> {
        let res = match (self.preset.take(), self.cx.job.req.conflict) {
            (Some(r), _) => r,
            (None, ConflictPolicy::Ask) => {
                // Let queued small files go while the user thinks.
                self.flush().await?;
                self.cx.mgr.ask(&self.cx.job, (src.uri(), src_e.clone()), (dst.uri(), dst_e.clone())).await?
            }
            (None, ConflictPolicy::Replace) => Resolution::Replace,
            (None, ConflictPolicy::Skip) => Resolution::Skip,
            (None, ConflictPolicy::KeepBoth) => Resolution::KeepBoth,
            (None, ConflictPolicy::ReplaceIfNewer) => Resolution::ReplaceIfNewer,
        };
        Ok(match res {
            Resolution::ReplaceIfNewer => match (src_e.modified, dst_e.modified) {
                (Some(s), Some(d)) if s > d => Resolution::Replace,
                (Some(_), None) => Resolution::Replace,
                _ => Resolution::Skip,
            },
            r => r,
        })
    }

    async fn keep_both_name(&self, dir: &Location, name: &str, is_dir: bool) -> Result<String> {
        for n in 2..10_000 {
            let candidate = numbered_name(name, is_dir, n);
            let loc = dir.join(&candidate);
            if self.planned.contains_key(&loc.uri()) {
                continue;
            }
            match self.dst_p.stat(&loc).await {
                Err(CxError::NotFound(_)) => return Ok(candidate),
                Ok(_) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(CxError::AlreadyExists(name.to_string()))
    }

    fn copy_item<'a>(&'a mut self, item: &'a Item, dst_dir: &'a Location, name: String, view: &'a View, parent_created: bool) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            self.cx.checkpoint().await?;
            let dst = dst_dir.join(&name);
            let existing = match self.lookup(view, &dst).await {
                Ok(e) => e,
                Err(e) => {
                    self.give_up(item, Some(&e));
                    return Ok(());
                }
            };
            if item.entry.is_dir {
                self.copy_dir(item, dst_dir, name, existing, parent_created).await
            } else {
                self.copy_file(item, dst_dir, name, existing, parent_created).await
            }
        })
    }

    async fn copy_dir(&mut self, item: &Item, dst_dir: &Location, name: String, existing: Option<Entry>, parent_created: bool) -> Result<()> {
        if !item.is_real_dir() {
            self.give_up(item, Some(&CxError::Unsupported(format!("\"{}\" is a link to a folder and was not copied", item.entry.name))));
            return Ok(());
        }
        let Some(children) = &item.children else {
            self.give_up(item, None);
            return Ok(());
        };
        let dst = dst_dir.join(&name);
        let (target, created, view) = match existing {
            // Folders merge.
            Some(e) if e.is_dir => match list_all(self.dst_p.as_ref(), &dst).await {
                Ok(list) => (dst, false, View::Known(list.into_iter().map(|e| (e.name.clone(), e)).collect())),
                Err(e) => {
                    self.give_up(item, Some(&e));
                    return Ok(());
                }
            },
            other => {
                let name = match other {
                    None => name,
                    Some(e) => match self.decide(&item.loc, &item.entry, &dst, &e).await? {
                        Resolution::Skip => {
                            self.give_up(item, None);
                            return Ok(());
                        }
                        // A file is in the way; never delete it for a folder.
                        _ => self.keep_both_name(dst_dir, &name, true).await?,
                    },
                };
                match self.dst_p.create_dir(dst_dir, Some(&name)).await {
                    Ok(_) => (dst_dir.join(&name), true, View::Known(HashMap::new())),
                    Err(e) => {
                        self.give_up(item, Some(&e));
                        return Ok(());
                    }
                }
            }
        };
        if created && !parent_created {
            self.record_top(&item.loc, &target);
        }
        for child in children {
            self.copy_item(child, &target, child.entry.name.clone(), &view, created).await?;
        }
        self.dirs.push(DirDone { src: item.loc.clone(), dst: target, modified: item.entry.modified });
        Ok(())
    }

    async fn copy_file(&mut self, item: &Item, dst_dir: &Location, mut name: String, existing: Option<Entry>, parent_created: bool) -> Result<()> {
        if item.entry.kind == EntryKind::Other {
            self.give_up(item, Some(&CxError::Unsupported(format!("\"{}\" is not a regular file", item.entry.name))));
            return Ok(());
        }
        let src_uri = item.loc.uri();
        if self.cx.job.restored && self.cx.job.record.lock().unwrap().completed.contains(&src_uri) {
            self.cx.job.counters.skip(1, item.entry.size);
            return Ok(());
        }
        let mut replace = false;
        if let Some(e) = existing {
            match self.decide(&item.loc, &item.entry, &dst_dir.join(&name), &e).await? {
                Resolution::Skip => {
                    self.give_up(item, None);
                    return Ok(());
                }
                Resolution::Replace if !e.is_dir => replace = true,
                _ => name = self.keep_both_name(dst_dir, &name, false).await?,
            }
        }
        let dst = dst_dir.join(&name);
        self.planned.insert(dst.uri(), item.entry.clone());
        let task = FileTask { src: item.loc.clone(), dst, entry: item.entry.clone(), replace, top: !parent_created && !replace };
        self.enqueue(task).await
    }

    async fn enqueue(&mut self, task: FileTask) -> Result<()> {
        if task.entry.size >= SMALL_FILE {
            return self.spawn(vec![task]).await;
        }
        self.batch_bytes += task.entry.size;
        self.batch.push(task);
        if self.batch.len() >= BATCH_FILES || self.batch_bytes >= BATCH_BYTES {
            self.flush().await?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<()> {
        if self.batch.is_empty() {
            return Ok(());
        }
        self.batch_bytes = 0;
        let batch = std::mem::take(&mut self.batch);
        self.spawn(batch).await
    }

    /// Start a task once a stream slot on both ends is free.
    async fn spawn(&mut self, tasks: Vec<FileTask>) -> Result<()> {
        while self.tasks.try_join_next().is_some() {}
        let limits = &self.cx.mgr.limits;
        let permits = tokio::select! {
            p = limits.acquire(&tasks[0].src, &tasks[0].dst) => p?,
            _ = self.cx.job.ctrl.cancelled() => return Err(CxError::Cancelled),
        };
        let (cx, problems, delete_src) = (self.cx.clone(), self.problems.clone(), self.delete_src);
        self.tasks.spawn(async move {
            let _permits = permits;
            for t in tasks {
                if run_file(&cx, &t, delete_src, &problems).await.is_err() {
                    break;
                }
            }
        });
        Ok(())
    }

    async fn drain(&mut self) {
        while self.tasks.join_next().await.is_some() {}
    }

    /// After all files: folder times (writing files into a folder bumps
    /// its mtime), and for moves, remove source folders that were emptied.
    async fn finalize(&mut self) {
        let problems = std::mem::take(&mut *self.problems.lock().unwrap());
        let vfs = self.cx.mgr.vfs().clone();
        // `dirs` is in post-order: children before their parents.
        for d in std::mem::take(&mut self.dirs) {
            if let Some(ms) = d.modified {
                let _ = self.dst_p.set_modified(&d.dst, ms).await;
            }
            if !self.delete_src {
                continue;
            }
            let prefix = format!("{}/", d.src.uri().trim_end_matches('/'));
            if problems.iter().any(|p| p.starts_with(&prefix) || *p == d.src.uri()) {
                continue;
            }
            if let Ok(p) = vfs.provider(&d.src).await {
                if matches!(list_all(p.as_ref(), &d.src).await, Ok(l) if l.is_empty()) {
                    if let Err(e) = p.remove(&d.src).await {
                        self.cx.file_error(d.src.uri(), &e);
                    }
                }
            }
        }
    }

    /// Move within one provider: rename when possible, merge into an
    /// existing folder item by item, fall back to copy + delete when the
    /// provider can't rename across (e.g. volumes).
    fn move_fast<'a>(&'a mut self, p: &'a Arc<dyn Provider>, src: Location, entry: Entry, dst_dir: Location, name: String) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            let cx = self.cx.clone();
            cx.checkpoint().await?;
            let counters = &cx.job.counters;
            let item_size = if entry.is_dir { 0 } else { entry.size };
            let mut dst = dst_dir.join(&name);
            let existing = match p.stat(&dst).await {
                Ok(e) => Some(e),
                Err(CxError::NotFound(_)) => None,
                Err(e) => {
                    cx.file_error(src.uri(), &e);
                    counters.skip(1, item_size);
                    return Ok(());
                }
            };
            let mut aside = None;
            let mut decided = None;
            if let Some(ex) = existing {
                if entry.kind == EntryKind::Dir && ex.is_dir {
                    return self.merge_move(p, src, dst).await;
                }
                let res = self.decide(&src, &entry, &dst, &ex).await?;
                decided = Some(res);
                match res {
                    Resolution::Skip => {
                        self.problems.lock().unwrap().push(src.uri());
                        counters.skip(1, item_size);
                        return Ok(());
                    }
                    Resolution::Replace if !ex.is_dir && !entry.is_dir => {
                        // Park the old file until the new one is in place.
                        let parked = dst_dir.join(&format!("{name}.cxold"));
                        if let Err(e) = p.move_to(&dst, &parked).await {
                            cx.file_error(src.uri(), &e);
                            counters.skip(1, item_size);
                            return Ok(());
                        }
                        aside = Some(parked);
                    }
                    _ => dst = dst_dir.join(&self.keep_both_name(&dst_dir, &name, entry.is_dir).await?),
                }
            }
            let moved = p.move_to(&src, &dst).await;
            if let Some(parked) = &aside {
                let _ = if moved.is_ok() { p.remove(parked).await } else { p.move_to(parked, &dst).await };
            }
            match moved {
                Ok(()) => {
                    counters.skip(1, item_size);
                    cx.job.record(|r| r.moved.push(MovedItem { from: src.uri(), to: dst.uri() }));
                }
                Err(CxError::Unsupported(_)) => {
                    counters.sub_total(1, item_size);
                    let item = scan(&cx, p, src, entry).await?;
                    self.preset = decided;
                    let copied = self.copy_item(&item, &dst_dir, name, &View::Unknown, false).await;
                    self.preset = None;
                    copied?;
                }
                Err(e) => {
                    cx.file_error(src.uri(), &e);
                    self.problems.lock().unwrap().push(src.uri());
                    counters.skip(1, item_size);
                }
            }
            Ok(())
        })
    }

    async fn merge_move(&mut self, p: &Arc<dyn Provider>, src: Location, dst: Location) -> Result<()> {
        let counters = &self.cx.job.counters;
        let kids = match list_all(p.as_ref(), &src).await {
            Ok(k) => k,
            Err(e) => {
                self.cx.file_error(src.uri(), &e);
                counters.skip(1, 0);
                return Ok(());
            }
        };
        counters.add_total(kids.len() as u64, kids.iter().filter(|k| !k.is_dir).map(|k| k.size).sum());
        for k in kids {
            let name = k.name.clone();
            self.move_fast(p, src.join(&name), k, dst.clone(), name).await?;
        }
        self.cx.job.counters.skip(1, 0);
        if matches!(list_all(p.as_ref(), &src).await, Ok(l) if l.is_empty()) {
            let _ = p.remove(&src).await;
        }
        Ok(())
    }
}

/// Copy one file inside a task. Only cancellation is returned as an error;
/// other failures are reported and the job goes on.
async fn run_file(cx: &Ctx, t: &FileTask, delete_src: bool, problems: &Mutex<Vec<String>>) -> Result<()> {
    cx.job.counters.set_current(Some(t.entry.name.clone()));
    match copy::transfer(cx, t).await {
        Ok(()) => {
            cx.job.counters.file_done();
            let src_uri = t.src.uri();
            if delete_src {
                let removed = async { cx.mgr.vfs().provider(&t.src).await?.remove(&t.src).await }.await;
                if let Err(e) = removed {
                    cx.file_error(src_uri.clone(), &CxError::Io(format!("copied, but the original could not be removed: {e}")));
                    problems.lock().unwrap().push(src_uri.clone());
                }
            }
            cx.job.record(|r| {
                if t.top {
                    if delete_src {
                        r.moved.push(MovedItem { from: src_uri.clone(), to: t.dst.uri() });
                    } else {
                        r.created.push(t.dst.uri());
                    }
                }
                r.completed.insert(src_uri);
            });
            Ok(())
        }
        Err(CxError::Cancelled) => Err(CxError::Cancelled),
        Err(e) => {
            cx.file_error(t.src.uri(), &e);
            problems.lock().unwrap().push(t.src.uri());
            cx.job.counters.skip(1, t.entry.size);
            Ok(())
        }
    }
}

async fn remove(cx: &Ctx) -> Result<()> {
    let job = &cx.job;
    job.counters.add_total(job.req.sources.len() as u64, 0);
    cx.mgr.set_phase(job, JobState::Running);
    for uri in &job.req.sources {
        cx.checkpoint().await?;
        let r = async {
            let loc = Location::parse(uri)?;
            let p = cx.mgr.vfs().provider(&loc).await?;
            job.counters.set_current(Some(loc.name()));
            match job.req.kind {
                JobKind::Trash => {
                    let dir = loc.parent().ok_or_else(|| CxError::InvalidLocation(uri.clone()))?;
                    let items = p.trash(&dir, &[loc.name()]).await?;
                    job.record(|r| r.trashed.extend(items));
                    Ok(())
                }
                _ => p.remove(&loc).await,
            }
        }
        .await;
        match r {
            Ok(()) => {}
            // A resumed job already removed it.
            Err(CxError::NotFound(_)) if job.restored => {}
            Err(e) => cx.file_error(uri.clone(), &e),
        }
        job.counters.skip(1, 0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within() {
        let a = Location::parse("sftp://h/a").unwrap();
        assert!(is_within(&Location::parse("sftp://h/a/b").unwrap(), &a));
        assert!(is_within(&a, &a));
        assert!(!is_within(&Location::parse("sftp://h/ab").unwrap(), &a));
        assert!(!is_within(&Location::parse("sftp://other/a/b").unwrap(), &a));
        assert!(is_within(&a, &Location::parse("sftp://h/").unwrap()));
        let l = Location::local("/x/y");
        assert!(is_within(&Location::local("/x/y/z"), &l));
        assert!(!is_within(&Location::local("/x/yz"), &l));
    }
}
