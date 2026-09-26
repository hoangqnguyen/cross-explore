use async_trait::async_trait;
use cx_core::{Capabilities, Connector, Credentials, CxError, Endpoint, Entry, Location, MemoryCredentials, Provider, ReadStream, Result, Scheme, Vfs, WriteMode, WriteStream};
use cx_local::LocalProvider;
use cx_search::{search, search_content, Cancel, ContentHit, ContentQuery, KindFilter, SearchHit, SearchQuery, SearchStats};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;

fn vfs() -> Arc<Vfs> {
    Vfs::new(Arc::new(LocalProvider), Arc::new(MemoryCredentials::default()))
}

fn write(root: &Path, rel: &str, data: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

/// root/
///   Report.PDF, notes.txt, .hidden.txt, Ảnh đẹp.jpg, bin.dat, old.txt
///   a/deep1.txt, a/b/c/report-final.pdf
///   node_modules/report.pdf
fn tree() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    write(r, "Report.PDF", &[b'x'; 100]);
    write(r, "notes.txt", b"hello world\nfoo Bar\nbarbell\n");
    write(r, ".hidden.txt", b"hello hidden");
    write(r, "Ảnh đẹp.jpg", b"jpg");
    write(r, "bin.dat", b"hello\0\x01\x02binary");
    write(r, "old.txt", b"hello from the past");
    write(r, "a/deep1.txt", b"deep hello");
    write(r, "a/b/c/report-final.pdf", &[b'y'; 5000]);
    write(r, "node_modules/report.pdf", b"nm");
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    fs::File::options().write(true).open(r.join("old.txt")).unwrap().set_modified(old).unwrap();
    tmp
}

async fn run_at(vfs: Arc<Vfs>, root: Location, q: SearchQuery) -> (Vec<SearchHit>, SearchStats) {
    let (tx, mut rx) = mpsc::channel(4);
    let task = tokio::spawn(search(vfs, root, q, tx, Cancel::new()));
    let mut hits = Vec::new();
    while let Some(b) = rx.recv().await {
        hits.extend(b);
    }
    (hits, task.await.unwrap().unwrap())
}

async fn run(root: &Path, q: SearchQuery) -> Vec<String> {
    let (hits, stats) = run_at(vfs(), Location::local(root), q).await;
    assert_eq!(stats.hits as usize, hits.len());
    hits.into_iter().map(|h| h.rel_path).collect()
}

fn q(text: &str) -> SearchQuery {
    SearchQuery { text: text.into(), ..Default::default() }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[tokio::test]
async fn substring_is_case_and_diacritic_insensitive() {
    let t = tree();
    let hits = run(t.path(), q("report")).await;
    assert_eq!(sorted(hits.clone()), ["Report.PDF", "a/b/c/report-final.pdf", "node_modules/report.pdf"]);
    // Breadth-first: the root-level hit comes before the deep one.
    let pos = |n: &str| hits.iter().position(|h| h == n).unwrap();
    assert!(pos("Report.PDF") < pos("a/b/c/report-final.pdf"));
    assert_eq!(run(t.path(), q("ANH DEP")).await, ["Ảnh đẹp.jpg"]);
    assert_eq!(run(t.path(), q("ảnh")).await, ["Ảnh đẹp.jpg"]);
}

#[tokio::test]
async fn glob_and_regex() {
    let t = tree();
    assert_eq!(run(t.path(), q("*.pdf")).await.len(), 3);
    assert_eq!(run(t.path(), q("report-*")).await, ["a/b/c/report-final.pdf"]);
    assert_eq!(run(t.path(), q(r"/^report-.*\.pdf$/")).await, ["a/b/c/report-final.pdf"]);
    assert_eq!(sorted(run(t.path(), q(r"/^(notes|old)\./")).await), ["notes.txt", "old.txt"]);
    let mut bad = q(r"/(unclosed/");
    bad.mode = cx_search::MatchMode::Auto;
    let (tx, _rx) = mpsc::channel(1);
    assert!(matches!(search(vfs(), Location::local(t.path()), bad, tx, Cancel::new()).await, Err(CxError::InvalidName(_))));
}

#[tokio::test]
async fn filters() {
    let t = tree();
    let dirs = run(t.path(), SearchQuery { kind: KindFilter::Dir, ..Default::default() }).await;
    assert_eq!(sorted(dirs), ["a", "a/b", "a/b/c", "node_modules"]);
    let txt = run(t.path(), SearchQuery { extensions: vec![".TXT".into()], ..Default::default() }).await;
    assert_eq!(sorted(txt), ["a/deep1.txt", "notes.txt", "old.txt"]);
    let big = run(t.path(), SearchQuery { min_size: Some(1000), ..Default::default() }).await;
    assert_eq!(big, ["a/b/c/report-final.pdf"]);
    let small_pdf = run(t.path(), SearchQuery { text: "*.pdf".into(), max_size: Some(100), ..Default::default() }).await;
    assert_eq!(sorted(small_pdf), ["Report.PDF", "node_modules/report.pdf"]);
    let cutoff = 1_500_000_000_000;
    let old = run(t.path(), SearchQuery { modified_before: Some(cutoff), ..Default::default() }).await;
    assert_eq!(old, ["old.txt"]);
    let recent = run(t.path(), SearchQuery { text: "txt".into(), modified_after: Some(cutoff), ..Default::default() }).await;
    assert_eq!(sorted(recent), ["a/deep1.txt", "notes.txt"]);
}

#[tokio::test]
async fn hidden_depth_limits_and_excludes() {
    let t = tree();
    assert!(!run(t.path(), q("hidden")).await.contains(&".hidden.txt".to_string()));
    assert_eq!(run(t.path(), SearchQuery { include_hidden: true, ..q("hidden") }).await, [".hidden.txt"]);

    assert_eq!(run(t.path(), SearchQuery { max_depth: Some(0), ..q("*.pdf") }).await, ["Report.PDF"]);
    assert_eq!(sorted(run(t.path(), SearchQuery { max_depth: Some(1), ..q("*.pdf") }).await), ["Report.PDF", "node_modules/report.pdf"]);
    let excl = run(t.path(), SearchQuery { exclude_dirs: vec!["NODE_MODULES".into()], ..q("*.pdf") }).await;
    assert_eq!(sorted(excl), ["Report.PDF", "a/b/c/report-final.pdf"]);

    let (hits, stats) = run_at(vfs(), Location::local(t.path()), SearchQuery { max_results: Some(2), ..Default::default() }).await;
    assert_eq!(hits.len(), 2);
    assert!(stats.truncated && !stats.cancelled);
}

#[tokio::test]
async fn hits_carry_uris_and_entries() {
    let t = tree();
    let (hits, stats) = run_at(vfs(), Location::local(t.path()), q("report-final")).await;
    let h = &hits[0];
    assert_eq!(Location::parse(&h.uri).unwrap(), Location::local(t.path().join("a/b/c/report-final.pdf")));
    assert_eq!(Location::parse(&h.parent_uri).unwrap(), Location::local(t.path().join("a/b/c")));
    assert_eq!(h.entry.size, 5000);
    assert_eq!(stats.dirs_scanned, 5);
    let json = serde_json::to_value(h).unwrap();
    assert!(json.get("parentUri").is_some() && json.get("relPath").is_some());
}

#[tokio::test]
async fn breadth_first_order_on_a_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let mut dir = tmp.path().to_path_buf();
    for i in 0..6 {
        write(&dir, &format!("match{i}"), b"");
        write(&dir, &format!("zz_other{i}"), b"");
        dir = dir.join(format!("d{i}"));
    }
    let hits = run(tmp.path(), q("match")).await;
    let depths: Vec<usize> = hits.iter().map(|h| h.matches('/').count()).collect();
    assert_eq!(depths, [0, 1, 2, 3, 4, 5]);
}

#[tokio::test]
async fn cancel_stops_the_walk() {
    let tmp = tempfile::tempdir().unwrap();
    for d in 0..100 {
        for f in 0..50 {
            write(tmp.path(), &format!("dir{d}/sub/file{f}.txt"), b"");
        }
    }
    // Cancelled up front: nothing is reported.
    let cancel = Cancel::new();
    cancel.cancel();
    let (tx, _rx) = mpsc::channel(4);
    let stats = search(vfs(), Location::local(tmp.path()), q("file"), tx, cancel).await.unwrap();
    assert!(stats.cancelled);
    assert_eq!(stats.hits, 0);

    // Cancelled after the first batch: stops well before the 5000 files.
    let cancel = Cancel::new();
    let (tx, mut rx) = mpsc::channel(1);
    let task = tokio::spawn(search(vfs(), Location::local(tmp.path()), q("file"), tx, cancel.clone()));
    let first = rx.recv().await.unwrap();
    assert!(!first.is_empty() && first.len() <= 512);
    cancel.cancel();
    let stats = tokio::time::timeout(Duration::from_secs(5), async {
        while rx.recv().await.is_some() {}
        task.await.unwrap().unwrap()
    })
    .await
    .unwrap();
    assert!(stats.cancelled);
    assert!(stats.hits < 5000, "{stats:?}");

    // A dropped receiver also stops it.
    let (tx, rx) = mpsc::channel(1);
    drop(rx);
    let stats = search(vfs(), Location::local(tmp.path()), q("file"), tx, Cancel::new()).await.unwrap();
    assert!(stats.cancelled);
}

#[tokio::test]
async fn missing_root_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (tx, _rx) = mpsc::channel(1);
    let r = search(vfs(), Location::local(tmp.path().join("nope")), q("x"), tx, Cancel::new()).await;
    assert!(matches!(r, Err(CxError::NotFound(_))));
}

// ------------------------------------------------------------ content

async fn grep_at(vfs: Arc<Vfs>, root: Location, cq: ContentQuery) -> (Vec<ContentHit>, SearchStats) {
    let (tx, mut rx) = mpsc::channel(4);
    let task = tokio::spawn(search_content(vfs, root, cq, tx, Cancel::new()));
    let mut hits = Vec::new();
    while let Some(b) = rx.recv().await {
        hits.extend(b);
    }
    hits.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    (hits, task.await.unwrap().unwrap())
}

fn cq(pattern: &str) -> ContentQuery {
    ContentQuery { pattern: pattern.into(), ..Default::default() }
}

fn paths(h: &[ContentHit]) -> Vec<&str> {
    h.iter().map(|h| h.rel_path.as_str()).collect()
}

async fn check_content(vfs: Arc<Vfs>, root: Location) {
    // Binary files and hidden files are skipped.
    let (hits, stats) = grep_at(vfs.clone(), root.clone(), cq("hello")).await;
    assert_eq!(paths(&hits), ["a/deep1.txt", "notes.txt", "old.txt"]);
    assert_eq!(stats.hits, 3);
    let notes = &hits[1];
    assert_eq!(notes.matches.len(), 1);
    assert_eq!(notes.matches[0].line_number, 1);
    assert_eq!(notes.matches[0].line, "hello world");
    assert_eq!(notes.matches[0].ranges, vec![[0, 5]]);

    // Smart case: lowercase matches both "Bar" and "barbell"; uppercase is exact.
    let (hits, _) = grep_at(vfs.clone(), root.clone(), cq("bar")).await;
    let lines: Vec<u64> = hits[0].matches.iter().map(|m| m.line_number).collect();
    assert_eq!(lines, [2, 3]);
    let (hits, _) = grep_at(vfs.clone(), root.clone(), cq("Bar")).await;
    assert_eq!(hits[0].matches.len(), 1);
    assert_eq!(hits[0].matches[0].ranges, vec![[4, 7]]);
    let (hits, _) = grep_at(vfs.clone(), root.clone(), ContentQuery { whole_word: true, ..cq("bar") }).await;
    assert_eq!(hits[0].matches.len(), 1);

    // Regex, file filters and per-file caps.
    let (hits, _) = grep_at(vfs.clone(), root.clone(), ContentQuery { regex: true, ..cq(r"^(foo|deep)\b") }).await;
    assert_eq!(paths(&hits), ["a/deep1.txt", "notes.txt"]);
    let files = SearchQuery { text: "*.txt".into(), max_depth: Some(0), include_hidden: true, ..Default::default() };
    let (hits, _) = grep_at(vfs.clone(), root.clone(), ContentQuery { files, ..cq("hello") }).await;
    assert_eq!(paths(&hits), [".hidden.txt", "notes.txt", "old.txt"]);
    let (hits, _) = grep_at(vfs.clone(), root.clone(), ContentQuery { max_matches_per_file: 1, ..cq("bar") }).await;
    assert!(hits[0].truncated && hits[0].matches.len() == 1);
    let (hits, stats) = grep_at(vfs.clone(), root.clone(), ContentQuery { max_files: Some(1), ..cq("hello") }).await;
    assert_eq!(hits.len(), 1);
    assert!(stats.truncated);
}

#[tokio::test]
async fn content_search_local() {
    let t = tree();
    check_content(vfs(), Location::local(t.path())).await;
    let (tx, _rx) = mpsc::channel(1);
    assert!(search_content(vfs(), Location::local(t.path()), cq(""), tx, Cancel::new()).await.is_err());
}

#[tokio::test]
async fn content_search_respects_gitignore_on_request() {
    let t = tree();
    write(t.path(), ".gitignore", b"old.txt\n");
    let (hits, _) = grep_at(vfs(), Location::local(t.path()), cq("hello")).await;
    assert!(paths(&hits).contains(&"old.txt"));
    let (hits, _) = grep_at(vfs(), Location::local(t.path()), ContentQuery { respect_gitignore: true, ..cq("hello") }).await;
    assert!(!paths(&hits).contains(&"old.txt"));
}

// ------------------------------------------------------------ remote

/// A "remote" server that serves a local folder, to cover the code paths used
/// for SFTP/SMB/… (the provider API instead of direct file access).
struct FakeRemote {
    base: PathBuf,
}

impl FakeRemote {
    fn map(&self, loc: &Location) -> Result<Location> {
        let path = loc.posix_path().ok_or_else(|| CxError::InvalidLocation(loc.uri()))?;
        Ok(Location::local(self.base.join(path.trim_start_matches('/'))))
    }
}

#[async_trait]
impl Provider for FakeRemote {
    fn scheme(&self) -> &'static str {
        "sftp"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities { polling: true, ..Default::default() }
    }
    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        LocalProvider.list(&self.map(dir)?, sink).await
    }
    async fn stat(&self, loc: &Location) -> Result<Entry> {
        LocalProvider.stat(&self.map(loc)?).await
    }
    async fn create_dir(&self, _: &Location, _: Option<&str>) -> Result<Entry> {
        Err(CxError::Unsupported("read-only".into()))
    }
    async fn move_to(&self, _: &Location, _: &Location) -> Result<()> {
        Err(CxError::Unsupported("read-only".into()))
    }
    async fn remove(&self, _: &Location) -> Result<()> {
        Err(CxError::Unsupported("read-only".into()))
    }
    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        LocalProvider.open_read(&self.map(loc)?, offset).await
    }
    async fn open_write(&self, _: &Location, _: WriteMode) -> Result<WriteStream> {
        Err(CxError::Unsupported("read-only".into()))
    }
}

struct FakeConnector(PathBuf);

#[async_trait]
impl Connector for FakeConnector {
    fn scheme(&self) -> Scheme {
        Scheme::Sftp
    }
    async fn connect(&self, _: &Endpoint, _: Option<Credentials>) -> Result<Arc<dyn Provider>> {
        Ok(Arc::new(FakeRemote { base: self.0.clone() }))
    }
}

fn remote_vfs(base: &Path) -> (Arc<Vfs>, Location) {
    let vfs = vfs();
    vfs.register(Arc::new(FakeConnector(base.to_path_buf())));
    let ep = Endpoint { scheme: Scheme::Sftp, user: None, host: "fake".into(), port: None };
    (vfs, Location::remote(ep, "/"))
}

#[tokio::test]
async fn name_search_through_a_remote_provider() {
    let t = tree();
    let (vfs, root) = remote_vfs(t.path());
    let (hits, _) = run_at(vfs, root.clone(), q("*.pdf")).await;
    let mut rels: Vec<_> = hits.iter().map(|h| h.rel_path.clone()).collect();
    rels.sort();
    assert_eq!(rels, ["Report.PDF", "a/b/c/report-final.pdf", "node_modules/report.pdf"]);
    let deep = hits.iter().find(|h| h.rel_path.starts_with("a/")).unwrap();
    assert_eq!(deep.uri, root.join("a").join("b").join("c").join("report-final.pdf").uri());
    assert!(deep.uri.starts_with("sftp://fake/"));
}

#[tokio::test]
async fn content_search_through_a_remote_provider() {
    let t = tree();
    let (vfs, root) = remote_vfs(t.path());
    check_content(vfs.clone(), root.clone()).await;
    // Files above the size cap are not read.
    let (hits, stats) = grep_at(vfs, root, ContentQuery { max_file_size: Some(12), ..cq("hello") }).await;
    assert_eq!(paths(&hits), ["a/deep1.txt"]);
    assert!(stats.files_searched < 5, "{stats:?}");
}
