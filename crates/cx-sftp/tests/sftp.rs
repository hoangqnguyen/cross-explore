//! Integration tests against the OpenSSH server in `docker/sftp`.
//!
//! Skipped unless `CX_TEST_SFTP=1`; run them with `docker/test-remote.sh sftp`.

use cx_core::provider::list_all;
use cx_core::{Credentials, CxError, Endpoint, EntryKind, Location, MemoryCredentials, Provider, Scheme, Secret, Vfs, WriteMode};
use cx_sftp::{trust_host_key, SftpConnector, SftpProvider};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

const HOST: &str = "127.0.0.1";
const PORT: u16 = 2222;

fn enabled() -> bool {
    let on = std::env::var("CX_TEST_SFTP").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set CX_TEST_SFTP=1 (see docker/test-remote.sh)");
    }
    on
}

fn endpoint(user: &str) -> Endpoint {
    Endpoint { scheme: Scheme::Sftp, user: Some(user.into()), host: HOST.into(), port: Some(PORT) }
}

fn keys_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docker/sftp/keys")
}

/// A connector isolated from the developer's own ~/.ssh and agent.
fn connector(store: &std::path::Path) -> SftpConnector {
    SftpConnector::new(store.join("known_hosts")).with_ssh_dir(None).with_agent(false)
}

/// Connect, accepting the host key the first time (what the UI does).
async fn open_with(conn: &SftpConnector, user: &str, creds: Option<Credentials>) -> cx_core::Result<SftpProvider> {
    match conn.open(&endpoint(user), creds.clone()).await {
        Err(CxError::HostKeyUnknown { key_type, fingerprint, .. }) => {
            trust_host_key(conn.known_hosts_path(), HOST, PORT, &key_type, &fingerprint)?;
            conn.open(&endpoint(user), creds).await
        }
        other => other,
    }
}

struct Fixture {
    _store: tempfile::TempDir,
    p: SftpProvider,
    dir: Location,
}

async fn fixture(name: &str) -> Fixture {
    let store = tempfile::tempdir().unwrap();
    let p = open_with(&connector(store.path()), "cx", Some(Credentials::password("cx", "cxpass"))).await.unwrap();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = Location::remote(endpoint("cx"), format!("/upload/{name}-{nanos}"));
    let parent = dir.parent().unwrap();
    p.create_dir(&parent, Some(&dir.name())).await.unwrap();
    Fixture { _store: store, p, dir }
}

impl Fixture {
    async fn put(&self, name: &str, data: &[u8]) {
        let mut w = self.p.open_write(&self.dir.join(name), WriteMode::Truncate).await.unwrap();
        w.write_all(data).await.unwrap();
        w.shutdown().await.unwrap();
    }

    async fn get(&self, name: &str, offset: u64) -> Vec<u8> {
        let mut r = self.p.open_read(&self.dir.join(name), offset).await.unwrap();
        let mut out = Vec::new();
        r.read_to_end(&mut out).await.unwrap();
        out
    }

    async fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = list_all(&self.p, &self.dir).await.unwrap().into_iter().map(|e| e.name).collect();
        v.sort();
        v
    }

    async fn cleanup(self) {
        self.p.remove(&self.dir).await.unwrap();
    }
}

/// Deterministic pseudo-random bytes (xorshift), so failures are reproducible.
fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

#[tokio::test]
async fn host_key_flow_unknown_then_trusted_then_changed() {
    if !enabled() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let conn = connector(store.path());
    let creds = Some(Credentials::password("cx", "cxpass"));
    let (key_type, fingerprint) = match conn.open(&endpoint("cx"), creds.clone()).await {
        Err(CxError::HostKeyUnknown { uri, host, key_type, fingerprint, changed }) => {
            assert_eq!(uri, "sftp://cx@127.0.0.1:2222");
            assert_eq!(host, "[127.0.0.1]:2222");
            assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
            assert!(!changed);
            (key_type, fingerprint)
        }
        Err(e) => panic!("expected HostKeyUnknown, got {e:?}"),
        Ok(_) => panic!("expected HostKeyUnknown, got a connection"),
    };
    trust_host_key(conn.known_hosts_path(), HOST, PORT, &key_type, &fingerprint).unwrap();
    let p = conn.open(&endpoint("cx"), creds.clone()).await.unwrap();
    assert!(p.home().await.unwrap().starts_with('/'));

    // Another key recorded for the host → "changed" warning.
    trust_host_key(conn.known_hosts_path(), HOST, PORT, &key_type, "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    match conn.open(&endpoint("cx"), creds).await {
        Err(CxError::HostKeyUnknown { changed, fingerprint: fp, .. }) => {
            assert!(changed);
            assert_eq!(fp, fingerprint);
        }
        other => panic!("expected changed host key, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn auth_failures_ask_for_credentials() {
    if !enabled() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let conn = connector(store.path());
    match open_with(&conn, "cx", Some(Credentials::password("cx", "nope"))).await {
        Err(CxError::AuthRequired { user, reason, .. }) => {
            assert_eq!(user.as_deref(), Some("cx"));
            assert_eq!(reason, "Wrong password");
        }
        other => panic!("expected AuthRequired, got {:?}", other.err()),
    }
    match open_with(&conn, "cx", None).await {
        Err(CxError::AuthRequired { user, .. }) => assert_eq!(user.as_deref(), Some("cx")),
        other => panic!("expected AuthRequired, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn key_auth_explicit_and_default_keys() {
    if !enabled() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let key = keys_dir().join("id_ed25519");
    let creds = Credentials { user: "cxkey".into(), secret: Secret::Key { path: key.to_string_lossy().into_owned(), passphrase: None } };
    let p = open_with(&connector(store.path()), "cxkey", Some(creds)).await.unwrap();
    list_all(&p, &Location::remote(endpoint("cxkey"), "/upload")).await.unwrap();

    // No credentials: the default identity in the ssh dir is used.
    let conn = connector(store.path()).with_ssh_dir(Some(keys_dir()));
    let p = open_with(&conn, "cxkey", None).await.unwrap();
    list_all(&p, &Location::remote(endpoint("cxkey"), "/upload")).await.unwrap();

    // Key auth for a password-only user fails with a reason.
    let bad = Credentials { user: "cx".into(), secret: Secret::Key { path: key.to_string_lossy().into_owned(), passphrase: None } };
    match open_with(&connector(store.path()), "cx", Some(bad)).await {
        Err(CxError::AuthRequired { reason, .. }) => assert_eq!(reason, "Key rejected by the server"),
        other => panic!("expected AuthRequired, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn lists_many_entries_in_batches() {
    if !enabled() {
        return;
    }
    let f = fixture("list").await;
    let mut expected = vec!["héllo wörld ✓.txt".to_string(), "日本語 フォルダ".to_string(), ".hidden".to_string(), "with space".to_string()];
    for i in 0..250 {
        expected.push(format!("file {i:03}.txt"));
    }
    for name in &expected {
        if name.contains("フォルダ") || name == "with space" {
            f.p.create_dir(&f.dir, Some(name)).await.unwrap();
        } else {
            f.put(name, b"abc").await;
        }
    }
    let (tx, mut rx) = mpsc::channel(64);
    let (res, batches) = tokio::join!(f.p.list(&f.dir, tx), async {
        let mut v = Vec::new();
        while let Some(b) = rx.recv().await {
            v.push(b);
        }
        v
    });
    let total = res.unwrap();
    assert_eq!(total, expected.len());
    assert!(batches.len() >= 2, "expected several batches, got {}", batches.len());
    assert!(batches[0].len() < total);
    let all: Vec<_> = batches.concat();
    let find = |n: &str| all.iter().find(|e| e.name == n).unwrap_or_else(|| panic!("{n} missing"));
    assert!(find("日本語 フォルダ").is_dir);
    assert_eq!(find("héllo wörld ✓.txt").size, 3);
    assert!(find(".hidden").hidden);
    assert!(!find("file 000.txt").hidden);
    assert!(find("file 000.txt").modified.is_some());
    let mut names: Vec<_> = all.iter().map(|e| e.name.clone()).collect();
    names.sort();
    expected.sort();
    assert_eq!(names, expected);
    f.cleanup().await;
}

#[tokio::test]
async fn stat_mkdir_rename_remove() {
    if !enabled() {
        return;
    }
    let f = fixture("ops").await;
    let a = f.p.create_dir(&f.dir, None).await.unwrap();
    let b = f.p.create_dir(&f.dir, None).await.unwrap();
    let c = f.p.create_dir(&f.dir, None).await.unwrap();
    assert_eq!((a.name.as_str(), b.name.as_str(), c.name.as_str()), ("New folder", "New folder (2)", "New folder (3)"));
    assert!(matches!(f.p.create_dir(&f.dir, Some("New folder")).await, Err(CxError::AlreadyExists(_))));
    assert!(matches!(f.p.create_dir(&f.dir, Some("a/b")).await, Err(CxError::InvalidName(_))));

    f.put("x.txt", b"hello").await;
    let st = f.p.stat(&f.dir.join("x.txt")).await.unwrap();
    assert_eq!((st.name.as_str(), st.size, st.is_dir), ("x.txt", 5, false));
    assert!(matches!(f.p.stat(&f.dir.join("missing")).await, Err(CxError::NotFound(_))));

    // Rename never overwrites.
    f.put("y.txt", b"y").await;
    assert!(matches!(f.p.rename(&f.dir, "x.txt", "y.txt").await, Err(CxError::AlreadyExists(_))));
    assert!(matches!(f.p.rename(&f.dir, "x.txt", "New folder").await, Err(CxError::AlreadyExists(_))));
    let e = f.p.rename(&f.dir, "x.txt", "renamed é.txt").await.unwrap();
    assert_eq!((e.name.as_str(), e.size), ("renamed é.txt", 5));
    // Move into a sub folder.
    f.p.move_to(&f.dir.join("y.txt"), &f.dir.join("New folder").join("y.txt")).await.unwrap();

    // Recursive delete of a nested tree.
    let deep = f.dir.join("New folder (2)");
    let sub = f.p.create_dir(&deep, Some("sub")).await.unwrap();
    for i in 0..20 {
        let mut w = f.p.open_write(&deep.join(&sub.name).join(&format!("{i}.bin")), WriteMode::CreateNew).await.unwrap();
        w.write_all(b"data").await.unwrap();
        w.shutdown().await.unwrap();
    }
    f.p.remove(&deep).await.unwrap();
    assert!(matches!(f.p.stat(&deep).await, Err(CxError::NotFound(_))));
    assert_eq!(f.names().await, vec!["New folder", "New folder (3)", "renamed é.txt"]);
    f.cleanup().await;
}

#[tokio::test]
async fn streams_round_trip_offset_append_mtime() {
    if !enabled() {
        return;
    }
    let f = fixture("io").await;
    let data = random_bytes(6 * 1024 * 1024 + 123, 42);
    let t = std::time::Instant::now();
    f.put("big.bin", &data).await;
    let up = t.elapsed();
    let t = std::time::Instant::now();
    let back = f.get("big.bin", 0).await;
    let down = t.elapsed();
    eprintln!("6 MiB up {up:?}, down {down:?}");
    assert!(back == data, "round trip differs");
    assert!(f.get("big.bin", 5_000_000).await == data[5_000_000..], "offset read differs");

    // CreateNew refuses to overwrite.
    assert!(matches!(f.p.open_write(&f.dir.join("big.bin"), WriteMode::CreateNew).await, Err(CxError::AlreadyExists(_))));

    // Resume: write half, then append the rest.
    let loc = f.dir.join("resume.bin");
    let mut w = f.p.open_write(&loc, WriteMode::CreateNew).await.unwrap();
    w.write_all(&data[..1_000_000]).await.unwrap();
    w.shutdown().await.unwrap();
    let mut w = f.p.open_write(&loc, WriteMode::Append).await.unwrap();
    w.write_all(&data[1_000_000..]).await.unwrap();
    w.shutdown().await.unwrap();
    assert!(f.get("resume.bin", 0).await == data, "append differs");
    assert_eq!(f.p.stat(&loc).await.unwrap().size, data.len() as u64);

    // Truncate replaces.
    f.put("resume.bin", b"short").await;
    assert_eq!(f.get("resume.bin", 0).await, b"short");

    f.p.set_modified(&loc, 1_600_000_000_000).await.unwrap();
    assert_eq!(f.p.stat(&loc).await.unwrap().modified, Some(1_600_000_000_000));

    // Server-side copy when the server supports it (OpenSSH 9.0+).
    eprintln!("server_copy: {}", f.p.capabilities().server_copy);
    if f.p.capabilities().server_copy {
        assert!(f.p.copy_within(&f.dir.join("big.bin"), &f.dir.join("copy.bin")).await.unwrap());
        assert!(f.get("copy.bin", 0).await == data);
        // Existing target: caller must stream instead.
        assert!(!f.p.copy_within(&f.dir.join("big.bin"), &f.dir.join("copy.bin")).await.unwrap());
    } else {
        assert!(!f.p.copy_within(&f.dir.join("big.bin"), &f.dir.join("copy.bin")).await.unwrap());
    }
    let space = f.p.free_space(&f.dir).await.unwrap();
    if let Some(s) = space {
        assert!(s.total >= s.free && s.total > 0);
    }
    f.cleanup().await;
}

#[tokio::test]
async fn symlinks_are_followed_for_folders() {
    if !enabled() {
        return;
    }
    let f = fixture("links").await;
    f.p.create_dir(&f.dir, Some("real")).await.unwrap();
    f.put("file.txt", b"12345").await;
    let dir = f.dir.posix_path().unwrap().to_string();
    f.p.symlink(&f.dir.join("to-dir"), &format!("{dir}/real")).await.unwrap();
    f.p.symlink(&f.dir.join("to-file"), "file.txt").await.unwrap();
    f.p.symlink(&f.dir.join("broken"), "nowhere").await.unwrap();
    let all = list_all(&f.p, &f.dir).await.unwrap();
    let find = |n: &str| all.iter().find(|e| e.name == n).unwrap().clone();
    let d = find("to-dir");
    assert_eq!((d.kind, d.is_dir), (EntryKind::Symlink, true));
    let fl = find("to-file");
    assert_eq!((fl.kind, fl.is_dir, fl.size), (EntryKind::Symlink, false, 5));
    let b = find("broken");
    assert_eq!((b.kind, b.is_dir), (EntryKind::Symlink, false));
    assert!(f.p.stat(&f.dir.join("to-dir")).await.unwrap().is_dir);
    // Removing a link to a folder removes the link, not the folder's contents.
    f.p.remove(&f.dir.join("to-dir")).await.unwrap();
    assert!(f.p.stat(&f.dir.join("real")).await.unwrap().is_dir);
    f.cleanup().await;
}

#[tokio::test]
async fn reconnects_after_the_connection_drops() {
    if !enabled() {
        return;
    }
    let f = fixture("reconnect").await;
    f.put("a.txt", b"a").await;
    f.p.disconnect().await;
    assert_eq!(f.names().await, vec!["a.txt"]);
    f.cleanup().await;
}

#[tokio::test]
async fn vfs_resolves_sftp_uris() {
    if !enabled() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let creds = Arc::new(MemoryCredentials::default());
    let vfs = Vfs::new(Arc::new(NoLocal), creds.clone());
    let conn = connector(store.path());
    vfs.register(Arc::new(conn.clone()));
    let loc = Location::parse("sftp://cx@127.0.0.1:2222/upload").unwrap();
    let ep = loc.endpoint().unwrap().clone();

    // No credentials yet → the UI would prompt.
    let err = vfs.provider(&loc).await.err().unwrap();
    let err = match err {
        CxError::HostKeyUnknown { key_type, fingerprint, .. } => {
            trust_host_key(conn.known_hosts_path(), HOST, PORT, &key_type, &fingerprint).unwrap();
            vfs.provider(&loc).await.err().unwrap()
        }
        other => other,
    };
    assert!(matches!(err, CxError::AuthRequired { .. }), "{err:?}");
    cx_core::CredentialStore::set(creds.as_ref(), &ep, &Credentials::password("cx", "cxpass"), false).unwrap();
    let p = vfs.provider(&loc).await.unwrap();
    assert_eq!(p.scheme(), "sftp");
    assert!(p.capabilities().polling && !p.capabilities().live_watch);
    list_all(p.as_ref(), &loc).await.unwrap();
    // Cached: same provider instance.
    assert!(Arc::ptr_eq(&p, &vfs.provider(&loc.join("x")).await.unwrap()));
}

/// Current OpenSSH (docker `sftp-modern`, port 2223): server-side copy
/// through `copy-data`, free space through `statvfs@openssh.com`.
#[tokio::test]
async fn modern_server_copies_server_side() {
    if !enabled() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let conn = connector(store.path());
    let ep = Endpoint { port: Some(2223), ..endpoint("cx") };
    let creds = Some(Credentials::password("cx", "cxpass"));
    let p = match conn.open(&ep, creds.clone()).await {
        Err(CxError::HostKeyUnknown { key_type, fingerprint, .. }) => {
            trust_host_key(conn.known_hosts_path(), HOST, 2223, &key_type, &fingerprint).unwrap();
            conn.open(&ep, creds).await.unwrap()
        }
        other => other.unwrap(),
    };
    assert!(p.capabilities().server_copy);
    let home = Location::remote(ep.clone(), p.home().await.unwrap());
    let dir = p.create_dir(&home, None).await.unwrap();
    let dir = home.join(&dir.name);
    let data = random_bytes(3 * 1024 * 1024, 7);
    let mut w = p.open_write(&dir.join("a.bin"), WriteMode::CreateNew).await.unwrap();
    w.write_all(&data).await.unwrap();
    w.shutdown().await.unwrap();
    assert!(p.copy_within(&dir.join("a.bin"), &dir.join("b.bin")).await.unwrap());
    let mut back = Vec::new();
    p.open_read(&dir.join("b.bin"), 0).await.unwrap().read_to_end(&mut back).await.unwrap();
    assert!(back == data);
    assert!(!p.copy_within(&dir.join("a.bin"), &dir.join("b.bin")).await.unwrap());
    let space = p.free_space(&dir).await.unwrap().expect("statvfs");
    assert!(space.total > 0 && space.free <= space.total);
    p.remove(&dir).await.unwrap();
}

/// The Vfs needs a local provider; these tests never touch it.
struct NoLocal;

#[async_trait::async_trait]
impl Provider for NoLocal {
    fn scheme(&self) -> &'static str {
        "file"
    }
    fn capabilities(&self) -> cx_core::Capabilities {
        cx_core::Capabilities::default()
    }
    async fn list(&self, _: &Location, _: mpsc::Sender<Vec<cx_core::Entry>>) -> cx_core::Result<usize> {
        unimplemented!()
    }
    async fn stat(&self, _: &Location) -> cx_core::Result<cx_core::Entry> {
        unimplemented!()
    }
    async fn create_dir(&self, _: &Location, _: Option<&str>) -> cx_core::Result<cx_core::Entry> {
        unimplemented!()
    }
    async fn move_to(&self, _: &Location, _: &Location) -> cx_core::Result<()> {
        unimplemented!()
    }
    async fn remove(&self, _: &Location) -> cx_core::Result<()> {
        unimplemented!()
    }
    async fn open_read(&self, _: &Location, _: u64) -> cx_core::Result<cx_core::ReadStream> {
        unimplemented!()
    }
    async fn open_write(&self, _: &Location, _: WriteMode) -> cx_core::Result<cx_core::WriteStream> {
        unimplemented!()
    }
}
