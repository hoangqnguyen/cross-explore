//! Content search ("find in files").
//!
//! Local trees go through ripgrep's building blocks: the `ignore` parallel
//! walker, `grep-searcher` (fast line search, UTF-16 BOM transcoding, binary
//! detection) and `grep-regex`. Anything else (SFTP, SMB, archives…) reuses
//! the breadth-first name walk and streams files below a size cap through
//! the provider, since downloading a whole remote tree to grep it would be
//! far too slow.

use crate::batch::Batcher;
use crate::filter::{is_noise, Filter, KindFilter};
use crate::name::{walk, SearchHit};
use crate::{bad_pattern, Cancel, SearchQuery, SearchStats};
use cx_core::{CxError, Entry, Location, Provider, Result, Vfs};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::{WalkBuilder, WalkState};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

/// Files larger than this are skipped on non-local providers by default.
pub const REMOTE_MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
/// Longest snippet sent to the UI (bytes of the source line).
const MAX_SNIPPET: usize = 300;
/// Context kept before the first match when a long line is cut.
const SNIPPET_LEAD: usize = 60;
/// Remote files read at once.
const REMOTE_PARALLEL_READS: usize = 4;

/// A "find in files" query. Every field has a default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContentQuery {
    pub pattern: String,
    /// Treat `pattern` as a regex; otherwise it is a literal string.
    pub regex: bool,
    /// `None` = smart case: case-insensitive unless the pattern has an uppercase letter.
    pub case_sensitive: Option<bool>,
    pub whole_word: bool,
    /// Which files to look in: name pattern (e.g. `*.rs`), extensions, size,
    /// dates, hidden, depth and excluded folders. `kind` is forced to files.
    pub files: SearchQuery,
    /// Skip what `.gitignore` / `.ignore` files exclude (local trees only).
    pub respect_gitignore: bool,
    /// Matching lines reported per file; the rest are counted as truncated.
    pub max_matches_per_file: usize,
    /// Stop after this many files with matches.
    pub max_files: Option<usize>,
    /// Skip larger files. Defaults to unlimited locally and
    /// [`REMOTE_MAX_FILE_SIZE`] elsewhere.
    pub max_file_size: Option<u64>,
}

impl Default for ContentQuery {
    fn default() -> Self {
        ContentQuery {
            pattern: String::new(),
            regex: false,
            case_sensitive: None,
            whole_word: false,
            files: SearchQuery::default(),
            respect_gitignore: false,
            max_matches_per_file: 100,
            max_files: None,
            max_file_size: None,
        }
    }
}

/// One file with matches.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ContentHit {
    pub uri: String,
    pub rel_path: String,
    pub entry: Entry,
    pub matches: Vec<LineMatch>,
    /// More lines matched than `maxMatchesPerFile`.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LineMatch {
    /// 1-based.
    pub line_number: u64,
    /// The line without its terminator and leading indentation; long lines
    /// are cut around the first match and marked with "…".
    pub line: String,
    /// `[start, end)` of each match in `line`, in UTF-16 code units so the UI
    /// can slice the JavaScript string directly.
    pub ranges: Vec<[u32; 2]>,
}

struct Compiled {
    pattern: String,
    case_insensitive: bool,
}

fn compile(q: &ContentQuery) -> Result<Compiled> {
    if q.pattern.is_empty() {
        return Err(bad_pattern("empty pattern"));
    }
    let mut pattern = if q.regex { q.pattern.clone() } else { regex::escape(&q.pattern) };
    if q.whole_word {
        pattern = format!(r"\b(?:{pattern})\b");
    }
    let case_insensitive = match q.case_sensitive {
        Some(cs) => !cs,
        None => !q.pattern.chars().any(char::is_uppercase),
    };
    Ok(Compiled { pattern, case_insensitive })
}

/// Search the contents of files under `root`, streaming one [`ContentHit`]
/// per file with matches. Binary files are skipped.
pub async fn search_content(vfs: Arc<Vfs>, root: Location, query: ContentQuery, sink: mpsc::Sender<Vec<ContentHit>>, cancel: Cancel) -> Result<SearchStats> {
    let compiled = compile(&query)?;
    let mut files = query.files.clone();
    files.kind = KindFilter::File;
    files.max_results = None;
    let filter = Filter::new(&files)?;
    match root.local_path() {
        Some(path) => {
            let matcher = RegexMatcherBuilder::new()
                .case_insensitive(compiled.case_insensitive)
                .line_terminator(Some(b'\n'))
                .build(&compiled.pattern)
                .map_err(bad_pattern)?;
            let job = LocalJob { root: path.to_path_buf(), query, files, filter, matcher, sink, cancel };
            tokio::task::spawn_blocking(move || job.run()).await.map_err(|e| CxError::Io(format!("search worker failed: {e}")))?
        }
        None => {
            let re = regex::bytes::RegexBuilder::new(&compiled.pattern)
                .case_insensitive(compiled.case_insensitive)
                .build()
                .map_err(bad_pattern)?;
            let provider = vfs.provider(&root).await?;
            search_remote(provider, root, query, files, filter, re, sink, cancel).await
        }
    }
}

// ---------------------------------------------------------------- local

struct LocalJob {
    root: PathBuf,
    query: ContentQuery,
    files: SearchQuery,
    filter: Filter,
    matcher: RegexMatcher,
    sink: mpsc::Sender<Vec<ContentHit>>,
    cancel: Cancel,
}

#[derive(Default)]
struct Counters {
    dirs: AtomicU64,
    entries: AtomicU64,
    files: AtomicU64,
    hits: AtomicU64,
    errors: AtomicU64,
    truncated: AtomicBool,
    closed: AtomicBool,
}

impl LocalJob {
    fn run(self) -> Result<SearchStats> {
        let started = Instant::now();
        if !self.root.is_dir() {
            return Err(CxError::NotFound(self.root.display().to_string()));
        }
        let respect = self.query.respect_gitignore;
        let mut wb = WalkBuilder::new(&self.root);
        wb.hidden(!self.files.include_hidden)
            .ignore(respect)
            .git_ignore(respect)
            .git_global(respect)
            .git_exclude(respect)
            .parents(respect)
            .require_git(false)
            .follow_links(false)
            .threads(std::thread::available_parallelism().map_or(4, |n| n.get()).min(8));
        if let Some(d) = self.files.max_depth {
            wb.max_depth(Some(d as usize + 1));
        }
        let job = Arc::new(self);
        let prune = job.clone();
        wb.filter_entry(move |de| {
            if de.depth() == 0 || !de.file_type().is_some_and(|t| t.is_dir()) {
                return true;
            }
            let name = de.file_name().to_string_lossy();
            !prune.filter.is_excluded(&name) && !is_noise(&Location::local(de.path()))
        });
        let counters = Arc::new(Counters::default());
        wb.build_parallel().run(|| {
            let mut worker = Worker {
                job: job.clone(),
                counters: counters.clone(),
                searcher: SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build(),
                buf: Vec::new(),
                last_flush: Instant::now(),
            };
            Box::new(move |res| worker.visit(res))
        });
        let c = &counters;
        Ok(SearchStats {
            dirs_scanned: c.dirs.load(Ordering::Relaxed),
            entries_scanned: c.entries.load(Ordering::Relaxed),
            files_searched: c.files.load(Ordering::Relaxed),
            hits: c.hits.load(Ordering::Relaxed),
            errors: c.errors.load(Ordering::Relaxed),
            elapsed_ms: started.elapsed().as_millis() as u64,
            truncated: c.truncated.load(Ordering::Relaxed),
            cancelled: job.cancel.is_cancelled() || c.closed.load(Ordering::Relaxed),
        })
    }
}

/// Per-thread state of the parallel walk. Buffers hits and flushes them in
/// small batches; whatever is left goes out when the thread finishes.
struct Worker {
    job: Arc<LocalJob>,
    counters: Arc<Counters>,
    searcher: Searcher,
    buf: Vec<ContentHit>,
    last_flush: Instant,
}

impl Worker {
    fn stopped(&self) -> bool {
        self.job.cancel.is_cancelled() || self.counters.closed.load(Ordering::Relaxed) || self.counters.truncated.load(Ordering::Relaxed)
    }

    fn visit(&mut self, res: std::result::Result<ignore::DirEntry, ignore::Error>) -> WalkState {
        if self.stopped() {
            return WalkState::Quit;
        }
        let de = match res {
            Ok(de) => de,
            Err(_) => {
                self.counters.errors.fetch_add(1, Ordering::Relaxed);
                return WalkState::Continue;
            }
        };
        let Some(ft) = de.file_type() else { return WalkState::Continue };
        if ft.is_dir() {
            self.counters.dirs.fetch_add(1, Ordering::Relaxed);
            return WalkState::Continue;
        }
        self.counters.entries.fetch_add(1, Ordering::Relaxed);
        if !ft.is_file() {
            return WalkState::Continue;
        }
        let path = de.path();
        let Ok(meta) = de.metadata() else {
            self.counters.errors.fetch_add(1, Ordering::Relaxed);
            return WalkState::Continue;
        };
        let entry = Entry::from_metadata(de.file_name().to_string_lossy().into_owned(), path, &meta);
        if !self.job.filter.matches(&entry) || self.job.query.max_file_size.is_some_and(|m| entry.size > m) {
            return WalkState::Continue;
        }
        self.counters.files.fetch_add(1, Ordering::Relaxed);
        let mut collect = Collect::new(&self.job.matcher, self.job.query.max_matches_per_file);
        if self.searcher.search_path(&self.job.matcher, path, &mut collect).is_err() {
            self.counters.errors.fetch_add(1, Ordering::Relaxed);
            return WalkState::Continue;
        }
        if collect.binary || collect.lines.is_empty() {
            return WalkState::Continue;
        }
        let n = self.counters.hits.fetch_add(1, Ordering::Relaxed) + 1;
        if let Some(max) = self.job.query.max_files {
            if n > max as u64 {
                self.counters.hits.fetch_sub(1, Ordering::Relaxed);
                self.counters.truncated.store(true, Ordering::Relaxed);
                return WalkState::Quit;
            }
        }
        self.buf.push(ContentHit {
            uri: Location::local(path).uri(),
            rel_path: rel_path(&self.job.root, path),
            entry,
            matches: collect.lines,
            truncated: collect.truncated,
        });
        if self.buf.len() >= 16 || self.last_flush.elapsed() >= Duration::from_millis(100) {
            self.flush();
        }
        if self.stopped() {
            WalkState::Quit
        } else {
            WalkState::Continue
        }
    }

    fn flush(&mut self) {
        self.last_flush = Instant::now();
        if self.buf.is_empty() || self.counters.closed.load(Ordering::Relaxed) {
            return;
        }
        // Walker threads are plain OS threads, so blocking here is fine and
        // gives natural back-pressure when the UI falls behind.
        if self.job.sink.blocking_send(std::mem::take(&mut self.buf)).is_err() {
            self.counters.closed.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if !self.job.cancel.is_cancelled() {
            self.flush();
        }
    }
}

fn rel_path(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// Collects matching lines of one file.
struct Collect<'m> {
    matcher: &'m RegexMatcher,
    cap: usize,
    lines: Vec<LineMatch>,
    truncated: bool,
    binary: bool,
}

impl<'m> Collect<'m> {
    fn new(matcher: &'m RegexMatcher, cap: usize) -> Self {
        Collect { matcher, cap: cap.max(1), lines: Vec::new(), truncated: false, binary: false }
    }
}

impl Sink for Collect<'_> {
    type Error = io::Error;

    fn matched(&mut self, _: &Searcher, m: &SinkMatch<'_>) -> io::Result<bool> {
        if self.lines.len() >= self.cap {
            self.truncated = true;
            return Ok(false);
        }
        let line = m.bytes();
        let mut ranges = Vec::new();
        self.matcher
            .find_iter(line, |r| {
                ranges.push((r.start(), r.end()));
                true
            })
            .map_err(io::Error::other)?;
        self.lines.push(line_match(m.line_number().unwrap_or(0), line, &ranges));
        Ok(true)
    }

    /// Matches found before the first NUL byte would be garbage: drop the file.
    fn binary_data(&mut self, _: &Searcher, _: u64) -> io::Result<bool> {
        self.binary = true;
        Ok(false)
    }
}

// ---------------------------------------------------------------- snippets

fn is_continuation(b: u8) -> bool {
    b & 0xC0 == 0x80
}

fn utf16_len(bytes: &[u8]) -> u32 {
    String::from_utf8_lossy(bytes).encode_utf16().count() as u32
}

/// Build the snippet for one line and convert byte ranges to UTF-16 offsets.
fn line_match(line_number: u64, line: &[u8], ranges: &[(usize, usize)]) -> LineMatch {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    let first = ranges.first().map_or(0, |r| r.0).min(end);
    // Drop indentation (but never part of a match).
    let mut start = 0;
    while start < first && matches!(line[start], b' ' | b'\t') {
        start += 1;
    }
    let mut cut_start = false;
    if end - start > MAX_SNIPPET && first > start + SNIPPET_LEAD {
        start = first - SNIPPET_LEAD;
        while start < first && is_continuation(line[start]) {
            start += 1;
        }
        cut_start = true;
    }
    let mut cut_end = false;
    if end - start > MAX_SNIPPET {
        end = start + MAX_SNIPPET;
        while end > start && end < line.len() && is_continuation(line[end]) {
            end -= 1;
        }
        cut_end = true;
    }
    let prefix = if cut_start { "…" } else { "" };
    let text = format!("{prefix}{}{}", String::from_utf8_lossy(&line[start..end]), if cut_end { "…" } else { "" });
    let base = prefix.encode_utf16().count() as u32;
    let ranges = ranges
        .iter()
        .filter(|(s, e)| *e > start && *s < end)
        .map(|&(s, e)| {
            let (s, e) = (s.max(start), e.min(end));
            let s16 = base + utf16_len(&line[start..s]);
            [s16, s16 + utf16_len(&line[s..e])]
        })
        .collect();
    LineMatch { line_number, line: text, ranges }
}

// ---------------------------------------------------------------- remote

#[allow(clippy::too_many_arguments)]
async fn search_remote(
    provider: Arc<dyn Provider>,
    root: Location,
    query: ContentQuery,
    files: SearchQuery,
    filter: Filter,
    re: regex::bytes::Regex,
    sink: mpsc::Sender<Vec<ContentHit>>,
    cancel: Cancel,
) -> Result<SearchStats> {
    let started = std::time::Instant::now();
    let max_size = query.max_file_size.unwrap_or(REMOTE_MAX_FILE_SIZE);
    let cap = query.max_matches_per_file.max(1);
    let re = Arc::new(re);

    // The name walk runs concurrently and feeds candidate files.
    let (tx, mut rx) = mpsc::channel::<Vec<SearchHit>>(8);
    let walk_cancel = Cancel::new();
    let walker = {
        let (provider, root, walk_cancel, depth) = (provider.clone(), root.clone(), walk_cancel.clone(), files.max_depth);
        tokio::spawn(async move { walk(provider, root, &filter, depth, None, tx, walk_cancel).await })
    };

    let mut stats = SearchStats::default();
    let mut batch = Batcher::new(sink);
    let mut pending: VecDeque<SearchHit> = VecDeque::new();
    let mut reads: JoinSet<Result<Option<ContentHit>>> = JoinSet::new();
    let mut walking = true;
    loop {
        while reads.len() < REMOTE_PARALLEL_READS {
            let Some(hit) = pending.pop_front() else { break };
            if hit.entry.size > max_size {
                continue;
            }
            stats.files_searched += 1;
            let (provider, re) = (provider.clone(), re.clone());
            reads.spawn(async move { grep_remote(provider.as_ref(), hit, &re, cap, max_size).await });
        }
        if !walking && reads.is_empty() && pending.is_empty() {
            break;
        }
        let deadline = batch.deadline();
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                stats.cancelled = true;
                break;
            }
            got = rx.recv(), if walking && pending.len() < 256 => match got {
                Some(v) => pending.extend(v),
                None => walking = false,
            },
            done = reads.join_next(), if !reads.is_empty() => match done {
                Some(Ok(Ok(Some(hit)))) => {
                    stats.hits += 1;
                    batch.push(hit);
                    if query.max_files.is_some_and(|m| stats.hits as usize >= m) {
                        stats.truncated = true;
                        break;
                    }
                }
                Some(Ok(Ok(None))) => {}
                _ => stats.errors += 1,
            },
            _ = tokio::time::sleep_until(deadline), if batch.has_pending() => {}
        }
        if batch.flush_if_due().await.is_err() {
            stats.cancelled = true;
            break;
        }
    }
    drop(reads);
    walk_cancel.cancel();
    drop(rx);
    let walked = walker.await.map_err(|e| CxError::Io(format!("search worker failed: {e}")))??;
    stats.dirs_scanned = walked.dirs_scanned;
    stats.entries_scanned = walked.entries_scanned;
    stats.errors += walked.errors;
    if !stats.cancelled && batch.flush().await.is_err() {
        stats.cancelled = true;
    }
    stats.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(stats)
}

/// Read one file through its provider and collect matching lines.
/// Returns `None` for binary files and files without matches.
async fn grep_remote(provider: &dyn Provider, hit: SearchHit, re: &regex::bytes::Regex, cap: usize, max_size: u64) -> Result<Option<ContentHit>> {
    let loc = Location::parse(&hit.uri)?;
    let stream = provider.open_read(&loc, 0).await?;
    let mut data = Vec::with_capacity(hit.entry.size.min(max_size) as usize);
    stream.take(max_size).read_to_end(&mut data).await.map_err(|e| CxError::from_io(e, &hit.uri))?;
    if data[..data.len().min(8192)].contains(&0) {
        return Ok(None);
    }
    let mut lines = Vec::new();
    let mut truncated = false;
    for (i, line) in data.split(|&b| b == b'\n').enumerate() {
        let ranges: Vec<(usize, usize)> = re.find_iter(line).map(|m| (m.start(), m.end())).filter(|(s, e)| e > s).collect();
        if ranges.is_empty() {
            continue;
        }
        if lines.len() >= cap {
            truncated = true;
            break;
        }
        lines.push(line_match(i as u64 + 1, line, &ranges));
    }
    if lines.is_empty() {
        return Ok(None);
    }
    Ok(Some(ContentHit { uri: hit.uri, rel_path: hit.rel_path, entry: hit.entry, matches: lines, truncated }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_ranges_are_utf16() {
        let text = "    let tên = \"đẹp\";\n";
        let s = text.find("đẹp").unwrap();
        let m = line_match(7, text.as_bytes(), &[(s, s + "đẹp".len())]);
        assert_eq!(m.line, "let tên = \"đẹp\";");
        let [a, b] = m.ranges[0];
        let utf16: Vec<u16> = m.line.encode_utf16().collect();
        assert_eq!(String::from_utf16(&utf16[a as usize..b as usize]).unwrap(), "đẹp");
    }

    #[test]
    fn long_lines_are_cut_around_the_match() {
        let mut line = "x".repeat(1000);
        line.push_str("NEEDLE");
        line.push_str(&"y".repeat(1000));
        let s = 1000;
        let m = line_match(1, line.as_bytes(), &[(s, s + 6)]);
        assert!(m.line.starts_with('…') && m.line.ends_with('…'));
        let [a, b] = m.ranges[0];
        assert_eq!(&m.line.encode_utf16().collect::<Vec<_>>()[a as usize..b as usize], "NEEDLE".encode_utf16().collect::<Vec<_>>().as_slice());
    }
}
