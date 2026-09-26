//! Folder comparison and synchronisation plans (Total Commander's
//! "Synchronize dirs").

use crate::{ConflictPolicy, JobRequest};
use cx_core::provider::list_all;
use cx_core::{Entry, Location, Provider, Result, Vfs};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Timestamps closer than this count as equal: FAT stores 2-second times and
/// many servers round to the second.
const MTIME_SLACK_MS: i64 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompareBy {
    #[default]
    SizeAndTime,
    /// Also hash files whose size matches (reads both sides fully).
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareOptions {
    pub recursive: bool,
    pub by: CompareBy,
}

impl Default for CompareOptions {
    fn default() -> Self {
        CompareOptions { recursive: true, by: CompareBy::SizeAndTime }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiffKind {
    LeftOnly,
    RightOnly,
    /// Both exist and the left one was modified later.
    NewerLeft,
    NewerRight,
    /// Differ, but the timestamps don't say which is newer (or one is a
    /// folder and the other a file).
    Different,
    Same,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffItem {
    /// Path below the compared roots, `/`-separated.
    pub rel_path: String,
    pub kind: DiffKind,
    pub left: Option<Entry>,
    pub right: Option<Entry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SyncDirection {
    LeftToRight,
    RightToLeft,
    /// Newer and one-sided items flow both ways; `different` items are left
    /// alone because there is no safe winner.
    Both,
}

type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

struct Side {
    p: Arc<dyn Provider>,
    root: Location,
}

fn at(root: &Location, rel: &str) -> Location {
    rel.split('/').filter(|s| !s.is_empty()).fold(root.clone(), |l, s| l.join(s))
}

/// Compare two folders. Items only on one side are reported once (their
/// contents are not listed); folders on both sides are descended into when
/// `recursive`, otherwise reported as `same`. Sorted by path.
pub async fn compare(vfs: &Vfs, left: &str, right: &str, opts: CompareOptions) -> Result<Vec<DiffItem>> {
    let (l, r) = (Location::parse(left)?, Location::parse(right)?);
    let left = Side { p: vfs.provider(&l).await?, root: l };
    let right = Side { p: vfs.provider(&r).await?, root: r };
    let mut out = Vec::new();
    walk(&left, &right, String::new(), opts, &mut out).await?;
    Ok(out)
}

fn walk<'a>(l: &'a Side, r: &'a Side, rel: String, opts: CompareOptions, out: &'a mut Vec<DiffItem>) -> BoxFut<'a, Result<()>> {
    Box::pin(async move {
        let (ld, rd) = (at(&l.root, &rel), at(&r.root, &rel));
        let (a, b) = tokio::try_join!(list_all(l.p.as_ref(), &ld), list_all(r.p.as_ref(), &rd))?;
        let a: BTreeMap<String, Entry> = a.into_iter().map(|e| (e.name.clone(), e)).collect();
        let b: BTreeMap<String, Entry> = b.into_iter().map(|e| (e.name.clone(), e)).collect();
        let names: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
        for name in names {
            let path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
            let (le, re) = (a.get(name).cloned(), b.get(name).cloned());
            let kind = match (&le, &re) {
                (Some(_), None) => DiffKind::LeftOnly,
                (None, Some(_)) => DiffKind::RightOnly,
                (Some(x), Some(y)) if x.is_dir && y.is_dir => {
                    if opts.recursive {
                        walk(l, r, path, opts, out).await?;
                        continue;
                    }
                    DiffKind::Same
                }
                (Some(x), Some(y)) if x.is_dir != y.is_dir => DiffKind::Different,
                (Some(x), Some(y)) => compare_files(l, r, &path, x, y, opts.by).await?,
                (None, None) => unreachable!(),
            };
            out.push(DiffItem { rel_path: path, kind, left: le, right: re });
        }
        Ok(())
    })
}

async fn compare_files(l: &Side, r: &Side, rel: &str, x: &Entry, y: &Entry, by: CompareBy) -> Result<DiffKind> {
    let by_time = match (x.modified, y.modified) {
        (Some(a), Some(b)) if a - b > MTIME_SLACK_MS => Some(DiffKind::NewerLeft),
        (Some(a), Some(b)) if b - a > MTIME_SLACK_MS => Some(DiffKind::NewerRight),
        _ => None,
    };
    if x.size != y.size {
        return Ok(by_time.unwrap_or(DiffKind::Different));
    }
    match by {
        CompareBy::SizeAndTime => Ok(by_time.unwrap_or(DiffKind::Same)),
        CompareBy::Content => {
            const CHUNK: usize = 1024 * 1024;
            let (lf, rf) = (at(&l.root, rel), at(&r.root, rel));
            let (lh, rh) = tokio::try_join!(crate::copy::hash(l.p.as_ref(), &lf, CHUNK), crate::copy::hash(r.p.as_ref(), &rf, CHUNK))?;
            Ok(if lh == rh { DiffKind::Same } else { by_time.unwrap_or(DiffKind::Different) })
        }
    }
}

/// Turn a comparison into copy jobs (one per destination folder, replacing
/// older files). Nothing is ever deleted: items only on the target side stay.
pub fn sync_plan(left: &str, right: &str, diff: &[DiffItem], direction: SyncDirection) -> Result<Vec<JobRequest>> {
    let (l, r) = (Location::parse(left)?, Location::parse(right)?);
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for d in diff {
        let to_right = match direction {
            SyncDirection::LeftToRight => matches!(d.kind, DiffKind::LeftOnly | DiffKind::NewerLeft | DiffKind::Different),
            SyncDirection::RightToLeft => false,
            SyncDirection::Both => matches!(d.kind, DiffKind::LeftOnly | DiffKind::NewerLeft),
        };
        let to_left = match direction {
            SyncDirection::RightToLeft => matches!(d.kind, DiffKind::RightOnly | DiffKind::NewerRight | DiffKind::Different),
            SyncDirection::LeftToRight => false,
            SyncDirection::Both => matches!(d.kind, DiffKind::RightOnly | DiffKind::NewerRight),
        };
        let (from, to) = match (to_right, to_left) {
            (true, _) => (&l, &r),
            (_, true) => (&r, &l),
            _ => continue,
        };
        let parent = d.rel_path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        groups.entry(at(to, parent).uri()).or_default().push(at(from, &d.rel_path).uri());
    }
    Ok(groups
        .into_iter()
        .map(|(dest, sources)| JobRequest::copy(sources, dest).with_conflict(ConflictPolicy::Replace))
        .collect())
}
