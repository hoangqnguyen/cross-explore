use async_trait::async_trait;
use cx_archive::{compress, extract, is_archive, ArchiveProvider, CancellationToken, Progress};
use cx_core::provider::list_all;
use cx_core::{Capabilities, Connector, Credentials, CxError, Endpoint, Entry, Location, MemoryCredentials, Provider, ReadStream, Result, Scheme, Vfs, WriteMode, WriteStream};
use cx_local::LocalProvider;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

// ------------------------------------------------------------ fixtures

const README: &[u8] = b"hello archive\n";
const MTIME: u64 = 1_600_000_000; // 2020-09-13, whole seconds

fn big() -> Vec<u8> {
    (0..300_000u32).map(|i| (i % 251) as u8).collect()
}

/// Members shared by every format. `docs/` and `docs/deep/` have no entries
/// of their own: they must be synthesised.
fn members() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("readme.txt", README.to_vec()),
        ("docs/a.txt", b"alpha".to_vec()),
        ("docs/deep/b.txt", b"bravo".to_vec()),
        ("big.bin", big()),
    ]
}

fn make_zip(path: &Path) {
    let mut z = zip::ZipWriter::new(fs::File::create(path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in members() {
        z.start_file(name, opts).unwrap();
        z.write_all(&data).unwrap();
    }
    z.add_directory("empty/", opts).unwrap();
    z.finish().unwrap();
}

fn tar_bytes() -> Vec<u8> {
    let mut b = tar::Builder::new(Vec::new());
    for (name, data) in members() {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(MTIME);
        h.set_cksum();
        b.append_data(&mut h, name, data.as_slice()).unwrap();
    }
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Directory);
    h.set_size(0);
    h.set_mtime(MTIME);
    h.set_cksum();
    b.append_data(&mut h, "empty/", std::io::empty()).unwrap();
    b.into_inner().unwrap()
}

fn make_tar(path: &Path) {
    let raw = tar_bytes();
    let name = path.to_string_lossy().to_string();
    let out = fs::File::create(path).unwrap();
    if name.ends_with(".tar.gz") {
        let mut e = flate2::write::GzEncoder::new(out, flate2::Compression::fast());
        e.write_all(&raw).unwrap();
        e.finish().unwrap();
    } else if name.ends_with(".tar.bz2") {
        let mut e = bzip2::write::BzEncoder::new(out, bzip2::Compression::fast());
        e.write_all(&raw).unwrap();
        e.finish().unwrap();
    } else if name.ends_with(".tar.xz") {
        let mut e = lzma_rust2::XzWriter::new(out, lzma_rust2::XzOptions::default()).unwrap();
        e.write_all(&raw).unwrap();
        e.finish().unwrap();
    } else if name.ends_with(".tar.zst") {
        // Two frames, like parallel compressors write.
        let (a, b) = raw.split_at(raw.len() / 2);
        let mut out = out;
        for part in [a, b] {
            out.write_all(&ruzstd::encoding::compress_to_vec(part, ruzstd::encoding::CompressionLevel::Fastest)).unwrap();
        }
    } else {
        let mut out = out;
        out.write_all(&raw).unwrap();
    }
}

fn make_7z(path: &Path, solid: bool) {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, SourceReader};
    let mut w = ArchiveWriter::create(path).unwrap();
    let entry = |name: &str| {
        let mut e = ArchiveEntry::new_file(name);
        e.has_last_modified_date = true;
        e.last_modified_date = (std::time::UNIX_EPOCH + std::time::Duration::from_secs(MTIME)).try_into().unwrap();
        e
    };
    if solid {
        let (entries, readers): (Vec<_>, Vec<SourceReader<std::io::Cursor<Vec<u8>>>>) =
            members().into_iter().map(|(n, d)| (entry(n), SourceReader::from(std::io::Cursor::new(d)))).unzip();
        w.push_archive_entries(entries, readers).unwrap();
    } else {
        for (name, data) in members() {
            w.push_archive_entry(entry(name), Some(data.as_slice())).unwrap();
        }
    }
    w.push_archive_entry::<&[u8]>(ArchiveEntry::new_directory("empty"), None).unwrap();
    w.finish().unwrap();
}

// ------------------------------------------------------------ helpers

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    vfs: Arc<Vfs>,
    _provider: Arc<ArchiveProvider>,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let vfs = Vfs::new(Arc::new(LocalProvider), Arc::new(MemoryCredentials::default()));
    let provider = ArchiveProvider::install(&vfs, root.join(".cache"));
    fs::create_dir_all(root.join("work")).unwrap();
    Env { _tmp: tmp, root, vfs, _provider: provider }
}

fn inside(container: &Location, inner: &str) -> Location {
    Location::Archive { container: Box::new(container.clone()), inner: inner.into() }
}

async fn names(vfs: &Vfs, dir: &Location) -> Vec<String> {
    let p = vfs.provider(dir).await.unwrap();
    let mut v: Vec<String> = list_all(p.as_ref(), dir).await.unwrap().into_iter().map(|e| e.name).collect();
    v.sort();
    v
}

async fn read(vfs: &Vfs, loc: &Location, offset: u64) -> Result<Vec<u8>> {
    let p = vfs.provider(loc).await?;
    let mut r = p.open_read(loc, offset).await?;
    let mut out = Vec::new();
    r.read_to_end(&mut out).await.map_err(|e| CxError::io("read", e))?;
    Ok(out)
}

async fn stat(vfs: &Vfs, loc: &Location) -> Result<Entry> {
    vfs.provider(loc).await?.stat(loc).await
}

/// The checks every readable format must pass.
async fn check_browse(vfs: &Vfs, container: &Location) {
    let root = inside(container, "/");
    assert_eq!(names(vfs, &root).await, vec!["big.bin", "docs", "empty", "readme.txt"], "{container}");
    assert_eq!(names(vfs, &inside(container, "/docs")).await, vec!["a.txt", "deep"]);
    assert_eq!(names(vfs, &inside(container, "/docs/deep")).await, vec!["b.txt"]);
    assert!(names(vfs, &inside(container, "/empty")).await.is_empty());

    let deep = stat(vfs, &inside(container, "/docs/deep")).await.unwrap();
    assert!(deep.is_dir, "implied folder");
    let root_entry = stat(vfs, &root).await.unwrap();
    assert!(root_entry.is_dir);
    let readme = stat(vfs, &inside(container, "/readme.txt")).await.unwrap();
    assert_eq!(readme.size, README.len() as u64);
    assert!(!readme.is_dir);

    assert_eq!(read(vfs, &inside(container, "/readme.txt"), 0).await.unwrap(), README);
    assert_eq!(read(vfs, &inside(container, "/docs/deep/b.txt"), 0).await.unwrap(), b"bravo");
    let big = big();
    assert_eq!(read(vfs, &inside(container, "/big.bin"), 0).await.unwrap(), big);
    assert_eq!(read(vfs, &inside(container, "/big.bin"), 100_000).await.unwrap(), &big[100_000..]);
    assert_eq!(read(vfs, &inside(container, "/readme.txt"), 5).await.unwrap(), README[5..]);

    assert!(matches!(stat(vfs, &inside(container, "/nope")).await, Err(CxError::NotFound(_))));
    assert!(read(vfs, &inside(container, "/docs"), 0).await.is_err());
    let (tx, _rx) = mpsc::channel(4);
    let p = vfs.provider(&root).await.unwrap();
    assert!(p.list(&inside(container, "/readme.txt"), tx).await.is_err());
}

// ------------------------------------------------------------ browsing

#[tokio::test]
async fn zip_browse_and_read() {
    let e = env();
    let path = e.root.join("work/test.zip");
    make_zip(&path);
    check_browse(&e.vfs, &Location::local(&path)).await;
}

#[tokio::test]
async fn tar_family_browse_and_read() {
    let e = env();
    for ext in ["tar", "tar.gz", "tar.bz2", "tar.xz", "tar.zst"] {
        let path = e.root.join(format!("work/test.{ext}"));
        make_tar(&path);
        let c = Location::local(&path);
        check_browse(&e.vfs, &c).await;
        let readme = stat(&e.vfs, &inside(&c, "/readme.txt")).await.unwrap();
        assert_eq!(readme.modified, Some(MTIME as i64 * 1000), "{ext}");
        assert!(readme.readonly, "tar members are read-only");
    }
}

#[tokio::test]
async fn sevenz_browse_and_read() {
    let e = env();
    for solid in [false, true] {
        let path = e.root.join(format!("work/test-{solid}.7z"));
        make_7z(&path, solid);
        let c = Location::local(&path);
        check_browse(&e.vfs, &c).await;
        let a = stat(&e.vfs, &inside(&c, "/docs/a.txt")).await.unwrap();
        assert_eq!(a.modified, Some(MTIME as i64 * 1000));
    }
}

#[tokio::test]
async fn detects_by_magic_when_name_lies() {
    let e = env();
    let path = e.root.join("work/really-a-zip.7z");
    make_zip(&path);
    assert_eq!(names(&e.vfs, &inside(&Location::local(&path), "/docs")).await, vec!["a.txt", "deep"]);
    assert!(is_archive("x.tar.gz") && is_archive("y.ZIP") && !is_archive("z.txt"));
}

#[tokio::test]
async fn index_is_refreshed_when_the_container_changes() {
    let e = env();
    let path = e.root.join("work/change.zip");
    make_zip(&path);
    let c = Location::local(&path);
    assert_eq!(names(&e.vfs, &inside(&c, "/")).await.len(), 4);
    let mut z = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    z.start_file("only.txt", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(b"x").unwrap();
    z.finish().unwrap();
    assert_eq!(names(&e.vfs, &inside(&c, "/")).await, vec!["only.txt"]);
}

#[tokio::test]
async fn nested_archive_is_browsable() {
    let e = env();
    let inner_path = e.root.join("work/inner.zip");
    make_zip(&inner_path);
    let outer_path = e.root.join("work/outer.zip");
    let mut z = zip::ZipWriter::new(fs::File::create(&outer_path).unwrap());
    z.start_file("nested/inner.zip", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(&fs::read(&inner_path).unwrap()).unwrap();
    z.finish().unwrap();
    let inner_container = inside(&Location::local(&outer_path), "/nested/inner.zip");
    let loc = inside(&inner_container, "/docs/deep/b.txt");
    assert_eq!(Location::parse(&loc.uri()).unwrap(), loc);
    assert_eq!(read(&e.vfs, &loc, 0).await.unwrap(), b"bravo");
}

// ------------------------------------------------------------ zip editing

#[tokio::test]
async fn zip_write_mkdir_rename_remove() {
    let e = env();
    let path = e.root.join("work/edit.zip");
    make_zip(&path);
    let c = Location::local(&path);
    let p = e.vfs.provider(&inside(&c, "/")).await.unwrap();
    assert!(p.capabilities().writable);
    assert!(ArchiveProvider::is_writable(&inside(&c, "/")));

    // mkdir, named and auto-named
    let d = p.create_dir(&inside(&c, "/docs"), Some("new")).await.unwrap();
    assert!(d.is_dir && d.name == "new");
    let auto = p.create_dir(&inside(&c, "/"), None).await.unwrap();
    assert_eq!(auto.name, "New folder");
    assert!(matches!(p.create_dir(&inside(&c, "/docs"), Some("new")).await, Err(CxError::AlreadyExists(_))));

    // new file
    let f = inside(&c, "/docs/new/hello.txt");
    let mut w = p.open_write(&f, WriteMode::CreateNew).await.unwrap();
    w.write_all(b"written into a zip").await.unwrap();
    w.shutdown().await.unwrap();
    assert_eq!(read(&e.vfs, &f, 0).await.unwrap(), b"written into a zip");
    assert!(matches!(p.open_write(&f, WriteMode::CreateNew).await, Err(CxError::AlreadyExists(_))));
    assert!(matches!(p.open_write(&f, WriteMode::Append).await, Err(CxError::Unsupported(_))));
    assert!(matches!(p.open_write(&inside(&c, "/missing/x.txt"), WriteMode::CreateNew).await, Err(CxError::NotFound(_))));

    // replace
    let mut w = p.open_write(&f, WriteMode::Truncate).await.unwrap();
    w.write_all(b"v2").await.unwrap();
    w.shutdown().await.unwrap();
    assert_eq!(read(&e.vfs, &f, 0).await.unwrap(), b"v2");

    // a write that is dropped without shutdown changes nothing
    let mut w = p.open_write(&inside(&c, "/dropped.txt"), WriteMode::CreateNew).await.unwrap();
    w.write_all(b"x").await.unwrap();
    drop(w);
    assert!(stat(&e.vfs, &inside(&c, "/dropped.txt")).await.is_err());

    // rename a folder (with an implied child folder)
    let renamed = p.rename(&inside(&c, "/"), "docs", "papers").await.unwrap();
    assert!(renamed.is_dir);
    assert_eq!(names(&e.vfs, &inside(&c, "/papers")).await, vec!["a.txt", "deep", "new"]);
    assert_eq!(read(&e.vfs, &inside(&c, "/papers/deep/b.txt"), 0).await.unwrap(), b"bravo");
    assert_eq!(read(&e.vfs, &inside(&c, "/papers/new/hello.txt"), 0).await.unwrap(), b"v2");
    assert!(matches!(p.rename(&inside(&c, "/"), "papers", "readme.txt").await, Err(CxError::AlreadyExists(_))));

    // remove a file and a folder tree
    p.remove(&inside(&c, "/readme.txt")).await.unwrap();
    p.remove(&inside(&c, "/papers")).await.unwrap();
    assert_eq!(names(&e.vfs, &inside(&c, "/")).await, vec!["New folder", "big.bin", "empty"]);
    assert!(matches!(p.remove(&inside(&c, "/papers")).await, Err(CxError::NotFound(_))));

    // the edited file is still a valid zip for other tools
    let z = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
    assert!(z.file_names().any(|n| n == "big.bin"));
}

#[tokio::test]
async fn read_only_formats_reject_writes() {
    let e = env();
    let path = e.root.join("work/ro.tar.gz");
    make_tar(&path);
    let c = Location::local(&path);
    let p = e.vfs.provider(&inside(&c, "/")).await.unwrap();
    assert!(!ArchiveProvider::is_writable(&inside(&c, "/")));
    assert!(matches!(p.create_dir(&inside(&c, "/"), Some("x")).await, Err(CxError::Unsupported(_))));
    assert!(matches!(p.remove(&inside(&c, "/readme.txt")).await, Err(CxError::Unsupported(_))));
    assert!(matches!(p.open_write(&inside(&c, "/n.txt"), WriteMode::CreateNew).await, Err(CxError::Unsupported(_))));
}

// ------------------------------------------------------------ compress / extract

fn set_mtime(path: &Path, secs: u64) {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    fs::File::options().write(true).open(path).unwrap().set_modified(t).unwrap();
}

fn make_tree(base: &Path) {
    fs::create_dir_all(base.join("project/src/deep")).unwrap();
    fs::create_dir_all(base.join("project/empty")).unwrap();
    fs::write(base.join("project/src/main.rs"), b"fn main() {}\n").unwrap();
    fs::write(base.join("project/src/deep/data.bin"), big()).unwrap();
    fs::write(base.join("project/photo.jpg"), b"not really a jpeg").unwrap();
    fs::write(base.join("notes.txt"), b"loose file").unwrap();
    set_mtime(&base.join("project/src/main.rs"), MTIME);
    set_mtime(&base.join("notes.txt"), MTIME + 60);
}

#[tokio::test]
async fn compress_then_extract_round_trip() {
    let e = env();
    let src = e.root.join("src");
    make_tree(&src);
    let dest = Location::local(e.root.join("work/bundle.zip"));
    let seen = Arc::new(Mutex::new(Vec::<Progress>::new()));
    let s = seen.clone();
    let entry = compress(
        &e.vfs,
        vec![Location::local(src.join("project")), Location::local(src.join("notes.txt"))],
        dest.clone(),
        move |p: &Progress| s.lock().unwrap().push(p.clone()),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(entry.name, "bundle.zip");
    let last = seen.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last.files_total, 4);
    assert_eq!(last.files_done, 4);
    assert_eq!(last.bytes_done, last.bytes_total);

    // Browsable, with times kept.
    assert_eq!(names(&e.vfs, &inside(&dest, "/")).await, vec!["notes.txt", "project"]);
    assert_eq!(names(&e.vfs, &inside(&dest, "/project")).await, vec!["empty", "photo.jpg", "src"]);
    let main = stat(&e.vfs, &inside(&dest, "/project/src/main.rs")).await.unwrap();
    assert_eq!(main.modified, Some(MTIME as i64 * 1000));

    // Refuses to overwrite.
    let again = compress(&e.vfs, vec![Location::local(src.join("notes.txt"))], dest.clone(), |_: &Progress| {}, CancellationToken::new()).await;
    assert!(matches!(again, Err(CxError::AlreadyExists(_))));

    // Extract twice: the second goes to "bundle (2)".
    let out_dir = Location::local(e.root.join("out"));
    fs::create_dir_all(e.root.join("out")).unwrap();
    let t1 = extract(&e.vfs, dest.clone(), out_dir.clone(), |_: &Progress| {}, CancellationToken::new()).await.unwrap();
    let t2 = extract(&e.vfs, dest.clone(), out_dir.clone(), |_: &Progress| {}, CancellationToken::new()).await.unwrap();
    assert_eq!(t1.name(), "bundle");
    assert_eq!(t2.name(), "bundle (2)");
    let x = e.root.join("out/bundle");
    assert_eq!(fs::read(x.join("project/src/deep/data.bin")).unwrap(), big());
    assert_eq!(fs::read(x.join("notes.txt")).unwrap(), b"loose file");
    assert!(x.join("project/empty").is_dir());
    let m = fs::metadata(x.join("project/src/main.rs")).unwrap().modified().unwrap();
    assert_eq!(m.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(), MTIME);
}

#[tokio::test]
async fn extract_tar_and_7z() {
    let e = env();
    let out = Location::local(e.root.join("work"));
    for name in ["t.tar.gz", "t.tar.zst", "s.7z"] {
        let path = e.root.join("work").join(name);
        if name.ends_with(".7z") {
            make_7z(&path, true);
        } else {
            make_tar(&path);
        }
        let target = extract(&e.vfs, Location::local(&path), out.clone(), |_: &Progress| {}, CancellationToken::new()).await.unwrap();
        let dir = target.local_path().unwrap().to_path_buf();
        assert_eq!(fs::read(dir.join("docs/deep/b.txt")).unwrap(), b"bravo", "{name}");
        assert_eq!(fs::read(dir.join("big.bin")).unwrap(), big(), "{name}");
        assert!(dir.join("empty").is_dir(), "{name}");
    }
}

#[tokio::test]
async fn zip_slip_is_rejected() {
    let e = env();
    let path = e.root.join("work/evil.zip");
    let mut z = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    z.start_file("ok.txt", opts).unwrap();
    z.write_all(b"fine").unwrap();
    z.start_file("../escaped.txt", opts).unwrap();
    z.write_all(b"evil").unwrap();
    z.finish().unwrap();
    let c = Location::local(&path);

    // Browsing hides the bad member.
    assert_eq!(names(&e.vfs, &inside(&c, "/")).await, vec!["ok.txt"]);

    let out = Location::local(e.root.join("work"));
    let res = extract(&e.vfs, c, out, |_: &Progress| {}, CancellationToken::new()).await;
    assert!(matches!(res, Err(CxError::InvalidName(_))), "{res:?}");
    assert!(!e.root.join("escaped.txt").exists());
    assert!(!e.root.join("work/escaped.txt").exists());
    assert!(!e.root.join("work/evil").exists(), "partial folder removed");
}

#[tokio::test]
async fn cancelled_jobs_leave_nothing_behind() {
    let e = env();
    let src = e.root.join("src");
    make_tree(&src);
    let dest = Location::local(e.root.join("work/cancel.zip"));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let res = compress(&e.vfs, vec![Location::local(src.join("project"))], dest.clone(), |_: &Progress| {}, cancel.clone()).await;
    assert!(matches!(res, Err(CxError::Cancelled)));
    assert!(!e.root.join("work/cancel.zip").exists());

    let zip_path = e.root.join("work/ok.zip");
    make_zip(&zip_path);
    let res = extract(&e.vfs, Location::local(&zip_path), Location::local(e.root.join("work")), |_: &Progress| {}, cancel).await;
    assert!(matches!(res, Err(CxError::Cancelled)));
    assert!(!e.root.join("work/ok").exists());
}

// ------------------------------------------------------------ remote containers

/// A writable "remote" server backed by a local folder, so containers go
/// through the download/upload paths used for SFTP, SMB, …
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
        Capabilities { polling: true, writable: true, ..Default::default() }
    }
    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        LocalProvider.list(&self.map(dir)?, sink).await
    }
    async fn stat(&self, loc: &Location) -> Result<Entry> {
        LocalProvider.stat(&self.map(loc)?).await
    }
    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        LocalProvider.create_dir(&self.map(dir)?, name).await
    }
    async fn move_to(&self, a: &Location, b: &Location) -> Result<()> {
        LocalProvider.move_to(&self.map(a)?, &self.map(b)?).await
    }
    async fn remove(&self, loc: &Location) -> Result<()> {
        LocalProvider.remove(&self.map(loc)?).await
    }
    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        LocalProvider.open_read(&self.map(loc)?, offset).await
    }
    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        LocalProvider.open_write(&self.map(loc)?, mode).await
    }
    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        LocalProvider.set_modified(&self.map(loc)?, ms).await
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

fn remote(e: &Env, path: &str) -> Location {
    e.vfs.register(Arc::new(FakeConnector(e.root.join("server"))));
    let ep = Endpoint { scheme: Scheme::Sftp, user: None, host: "fake".into(), port: None };
    Location::remote(ep, path)
}

#[tokio::test]
async fn remote_container_browse_edit_extract() {
    let e = env();
    fs::create_dir_all(e.root.join("server/pub")).unwrap();
    make_zip(&e.root.join("server/pub/r.zip"));
    make_tar(&e.root.join("server/pub/r.tar.xz"));
    let zip_loc = remote(&e, "/pub/r.zip");
    check_browse(&e.vfs, &zip_loc).await;
    check_browse(&e.vfs, &remote(&e, "/pub/r.tar.xz")).await;
    let cached = fs::read_dir(e.root.join(".cache/containers")).unwrap().count();
    assert_eq!(cached, 2, "one cached download per container");

    // Edit: rewritten locally, uploaded back.
    let p = e.vfs.provider(&inside(&zip_loc, "/")).await.unwrap();
    let f = inside(&zip_loc, "/up.txt");
    let mut w = p.open_write(&f, WriteMode::CreateNew).await.unwrap();
    w.write_all(b"uploaded").await.unwrap();
    w.shutdown().await.unwrap();
    assert_eq!(read(&e.vfs, &f, 0).await.unwrap(), b"uploaded");
    let z = zip::ZipArchive::new(fs::File::open(e.root.join("server/pub/r.zip")).unwrap()).unwrap();
    assert!(z.file_names().any(|n| n == "up.txt"), "server copy updated");

    // Compress onto the server, extract from it into a server folder.
    fs::write(e.root.join("work/local.txt"), b"local").unwrap();
    let remote_zip = remote(&e, "/pub/made.zip");
    compress(&e.vfs, vec![Location::local(e.root.join("work/local.txt"))], remote_zip.clone(), |_: &Progress| {}, CancellationToken::new()).await.unwrap();
    let target = extract(&e.vfs, remote_zip, remote(&e, "/pub"), |_: &Progress| {}, CancellationToken::new()).await.unwrap();
    assert_eq!(target.posix_path(), Some("/pub/made"));
    assert_eq!(fs::read(e.root.join("server/pub/made/local.txt")).unwrap(), b"local");
}
