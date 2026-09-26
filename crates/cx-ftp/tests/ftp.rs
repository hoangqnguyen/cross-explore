//! Integration tests against the servers in `docker/ftp`:
//! Pure-FTPd on 2121 (MLSD, explicit TLS) and vsftpd on 2122 (LIST only).
//!
//! Skipped unless `CX_TEST_FTP=1`; run them with `docker/test-remote.sh ftp`.

use cx_core::provider::list_all;
use cx_core::{Credentials, CxError, Endpoint, EntryKind, Location, MemoryCredentials, Provider, Scheme, Vfs, WriteMode};
use cx_ftp::{trust_host_key, FtpConnector, FtpProvider};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

const HOST: &str = "127.0.0.1";
const PURE: u16 = 2121;
const VSFTPD: u16 = 2122;

fn enabled() -> bool {
    let on = std::env::var("CX_TEST_FTP").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set CX_TEST_FTP=1 (see docker/test-remote.sh)");
    }
    on
}

fn endpoint(scheme: Scheme, port: u16) -> Endpoint {
    Endpoint { scheme, user: Some("cx".into()), host: HOST.into(), port: Some(port) }
}

fn creds() -> Option<Credentials> {
    Some(Credentials::password("cx", "cxpass"))
}

/// Connect, accepting the certificate the first time (what the UI does).
async fn open(scheme: Scheme, port: u16, store: &std::path::Path) -> cx_core::Result<FtpProvider> {
    let conn = match scheme {
        Scheme::Ftps => FtpConnector::ftps(store.join("certs")),
        _ => FtpConnector::ftp(store.join("certs")),
    };
    match conn.open(&endpoint(scheme, port), creds()).await {
        Err(CxError::HostKeyUnknown { key_type, fingerprint, .. }) => {
            trust_host_key(conn.trust_store_path(), HOST, port, &key_type, &fingerprint)?;
            conn.open(&endpoint(scheme, port), creds()).await
        }
        other => other,
    }
}

struct Fixture {
    _store: tempfile::TempDir,
    p: FtpProvider,
    dir: Location,
    /// MLST available: exact times.
    exact_times: bool,
}

async fn fixture(scheme: Scheme, port: u16, name: &str) -> Fixture {
    let store = tempfile::tempdir().unwrap();
    let p = open(scheme, port, store.path()).await.unwrap();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = Location::remote(endpoint(scheme, port), p.home());
    let dir = home.join(&format!("{name}-{scheme}-{port}-{nanos}"));
    p.create_dir(&dir.parent().unwrap(), Some(&dir.name())).await.unwrap();
    Fixture { _store: store, p, dir, exact_times: port == PURE }
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
        assert!(matches!(self.p.stat(&self.dir).await, Err(CxError::NotFound(_))));
    }
}

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

const SERVERS: [(Scheme, u16); 3] = [(Scheme::Ftp, PURE), (Scheme::Ftps, PURE), (Scheme::Ftp, VSFTPD)];

#[tokio::test]
async fn auth_failures_ask_for_credentials() {
    if !enabled() {
        return;
    }
    for port in [PURE, VSFTPD] {
        let conn = FtpConnector::ftp("/nonexistent");
        match conn.open(&endpoint(Scheme::Ftp, port), Some(Credentials::password("cx", "wrong"))).await {
            Err(CxError::AuthRequired { user, reason, .. }) => {
                assert_eq!(user.as_deref(), Some("cx"));
                assert_eq!(reason, "Wrong password");
            }
            other => panic!("{port}: expected AuthRequired, got {:?}", other.err()),
        }
        // Named user, no password.
        assert!(matches!(conn.open(&endpoint(Scheme::Ftp, port), None).await, Err(CxError::AuthRequired { .. })));
        // No user at all: anonymous is tried, and refused by these servers.
        let anon = Endpoint { user: None, ..endpoint(Scheme::Ftp, port) };
        match conn.open(&anon, None).await {
            Err(CxError::AuthRequired { reason, .. }) => assert_eq!(reason, "Sign-in required"),
            other => panic!("{port}: expected AuthRequired, got {:?}", other.err()),
        }
    }
}

#[tokio::test]
async fn ftps_certificate_trust_on_first_use() {
    if !enabled() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let conn = FtpConnector::ftps(store.path().join("certs"));
    let ep = endpoint(Scheme::Ftps, PURE);
    let fingerprint = match conn.open(&ep, creds()).await {
        Err(CxError::HostKeyUnknown { uri, host, key_type, fingerprint, changed }) => {
            assert_eq!(uri, "ftps://cx@127.0.0.1:2121");
            assert_eq!(host, "[127.0.0.1]:2121");
            assert_eq!(key_type, "x509");
            assert!(!changed);
            fingerprint
        }
        other => panic!("expected HostKeyUnknown, got {:?}", other.err()),
    };
    trust_host_key(conn.trust_store_path(), HOST, PURE, "x509", &fingerprint).unwrap();
    let p = conn.open(&ep, creds()).await.unwrap();
    assert_eq!(p.scheme(), "ftps");
    list_all(&p, &Location::remote(ep.clone(), "/")).await.unwrap();

    trust_host_key(conn.trust_store_path(), HOST, PURE, "x509", "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA").unwrap();
    match conn.open(&ep, creds()).await {
        Err(CxError::HostKeyUnknown { changed, .. }) => assert!(changed),
        other => panic!("expected changed certificate, got {:?}", other.err()),
    }
}

#[tokio::test]
async fn lists_many_entries_in_batches() {
    if !enabled() {
        return;
    }
    for (scheme, port) in SERVERS {
        let f = fixture(scheme, port, "list").await;
        let mut expected = vec!["héllo wörld ✓.txt".to_string(), "日本語 フォルダ".to_string(), ".hidden".to_string(), "with  two spaces".to_string()];
        for i in 0..230 {
            expected.push(format!("file {i:03}.txt"));
        }
        for name in &expected {
            if name.contains("フォルダ") || name.contains("two") {
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
        assert_eq!(total, expected.len(), "{scheme}:{port}");
        assert!(batches.len() >= 2 && batches[0].len() < total, "{scheme}:{port}: batches {}", batches.len());
        let all: Vec<_> = batches.concat();
        let find = |n: &str| all.iter().find(|e| e.name == n).unwrap_or_else(|| panic!("{scheme}:{port}: {n} missing"));
        assert!(find("日本語 フォルダ").is_dir);
        assert!(find("with  two spaces").is_dir);
        assert_eq!(find("héllo wörld ✓.txt").size, 3);
        assert!(find(".hidden").hidden);
        assert!(find("file 000.txt").modified.is_some());
        let mut names: Vec<_> = all.iter().map(|e| e.name.clone()).collect();
        names.sort();
        expected.sort();
        assert_eq!(names, expected, "{scheme}:{port}");
        f.cleanup().await;
    }
}

#[tokio::test]
async fn stat_mkdir_rename_remove() {
    if !enabled() {
        return;
    }
    for (scheme, port) in SERVERS {
        let f = fixture(scheme, port, "ops").await;
        let a = f.p.create_dir(&f.dir, None).await.unwrap();
        let b = f.p.create_dir(&f.dir, None).await.unwrap();
        let c = f.p.create_dir(&f.dir, None).await.unwrap();
        assert_eq!((a.name.as_str(), b.name.as_str(), c.name.as_str()), ("New folder", "New folder (2)", "New folder (3)"));
        assert!(a.is_dir);
        assert!(matches!(f.p.create_dir(&f.dir, Some("New folder")).await, Err(CxError::AlreadyExists(_))));
        assert!(matches!(f.p.create_dir(&f.dir, Some("a/b")).await, Err(CxError::InvalidName(_))));

        f.put("x.txt", b"hello").await;
        let st = f.p.stat(&f.dir.join("x.txt")).await.unwrap();
        assert_eq!((st.name.as_str(), st.size, st.is_dir, st.kind), ("x.txt", 5, false, EntryKind::File));
        assert!(f.p.stat(&f.dir).await.unwrap().is_dir);
        assert!(matches!(f.p.stat(&f.dir.join("missing")).await, Err(CxError::NotFound(_))), "{scheme}:{port}");
        let (tx, _rx) = mpsc::channel(1);
        assert!(matches!(f.p.list(&f.dir.join("missing"), tx).await, Err(CxError::NotFound(_))), "{scheme}:{port}");

        f.put("y.txt", b"y").await;
        assert!(matches!(f.p.rename(&f.dir, "x.txt", "y.txt").await, Err(CxError::AlreadyExists(_))));
        assert!(matches!(f.p.rename(&f.dir, "x.txt", "New folder").await, Err(CxError::AlreadyExists(_))));
        let e = f.p.rename(&f.dir, "x.txt", "renamed é.txt").await.unwrap();
        assert_eq!((e.name.as_str(), e.size), ("renamed é.txt", 5));
        f.p.move_to(&f.dir.join("y.txt"), &f.dir.join("New folder").join("y.txt")).await.unwrap();

        let deep = f.dir.join("New folder (2)");
        f.p.create_dir(&deep, Some("sub")).await.unwrap();
        for i in 0..10 {
            let mut w = f.p.open_write(&deep.join("sub").join(&format!("{i}.bin")), WriteMode::CreateNew).await.unwrap();
            w.write_all(b"data").await.unwrap();
            w.shutdown().await.unwrap();
        }
        f.p.remove(&deep).await.unwrap();
        assert!(matches!(f.p.stat(&deep).await, Err(CxError::NotFound(_))));
        assert_eq!(f.names().await, vec!["New folder", "New folder (3)", "renamed é.txt"], "{scheme}:{port}");
        f.cleanup().await;
    }
}

#[tokio::test]
async fn streams_round_trip_offset_append_mtime() {
    if !enabled() {
        return;
    }
    for (scheme, port) in SERVERS {
        let f = fixture(scheme, port, "io").await;
        let data = random_bytes(5 * 1024 * 1024 + 77, port as u64);
        let t = std::time::Instant::now();
        f.put("big.bin", &data).await;
        let up = t.elapsed();
        let t = std::time::Instant::now();
        let back = f.get("big.bin", 0).await;
        eprintln!("{scheme}:{port} 5 MiB up {up:?}, down {:?}", t.elapsed());
        assert!(back == data, "{scheme}:{port}: round trip differs");
        assert!(f.get("big.bin", 4_000_000).await == data[4_000_000..], "{scheme}:{port}: offset read differs");
        assert!(matches!(f.p.open_write(&f.dir.join("big.bin"), WriteMode::CreateNew).await, Err(CxError::AlreadyExists(_))));
        assert!(matches!(f.p.open_read(&f.dir.join("nope.bin"), 0).await.err(), Some(CxError::NotFound(_))));

        let loc = f.dir.join("resume.bin");
        let mut w = f.p.open_write(&loc, WriteMode::CreateNew).await.unwrap();
        w.write_all(&data[..1_000_000]).await.unwrap();
        w.shutdown().await.unwrap();
        let mut w = f.p.open_write(&loc, WriteMode::Append).await.unwrap();
        w.write_all(&data[1_000_000..]).await.unwrap();
        w.shutdown().await.unwrap();
        assert!(f.get("resume.bin", 0).await == data, "{scheme}:{port}: append differs");
        assert_eq!(f.p.stat(&loc).await.unwrap().size, data.len() as u64);

        f.put("resume.bin", b"short").await;
        assert_eq!(f.get("resume.bin", 0).await, b"short");

        f.p.set_modified(&loc, 1_600_000_000_000).await.unwrap();
        let m = f.p.stat(&loc).await.unwrap().modified.unwrap();
        if f.exact_times {
            assert_eq!(m, 1_600_000_000_000, "{scheme}:{port}");
        } else {
            // LIST shows only the day for old files.
            assert_eq!(m.div_euclid(86_400_000), 1_600_000_000_000i64.div_euclid(86_400_000), "{scheme}:{port}");
        }
        f.cleanup().await;
    }
}

#[tokio::test]
async fn transfer_and_listing_run_side_by_side() {
    if !enabled() {
        return;
    }
    for (scheme, port) in SERVERS {
        let f = fixture(scheme, port, "pool").await;
        let data = random_bytes(2 * 1024 * 1024, 3);
        f.put("a.bin", &data).await;
        // A download holds its connection while a listing and an upload run.
        let mut r = f.p.open_read(&f.dir.join("a.bin"), 0).await.unwrap();
        let mut head = vec![0u8; 1000];
        r.read_exact(&mut head).await.unwrap();
        let mut w = f.p.open_write(&f.dir.join("b.bin"), WriteMode::CreateNew).await.unwrap();
        assert_eq!(f.names().await, vec!["a.bin", "b.bin"]);
        w.write_all(b"bee").await.unwrap();
        w.shutdown().await.unwrap();
        let mut rest = Vec::new();
        r.read_to_end(&mut rest).await.unwrap();
        head.extend(rest);
        assert!(head == data);

        // Abandon downloads half-way, more times than the pool is big: the
        // broken connections must not be reused.
        for _ in 0..(cx_ftp::POOL_SIZE + 2) {
            let mut r = f.p.open_read(&f.dir.join("a.bin"), 0).await.unwrap();
            let mut buf = vec![0u8; 10_000];
            r.read_exact(&mut buf).await.unwrap();
            drop(r);
        }
        assert_eq!(f.get("b.bin", 0).await, b"bee");
        // An upload dropped without shutdown doesn't wedge the pool either.
        let mut w = f.p.open_write(&f.dir.join("c.bin"), WriteMode::Truncate).await.unwrap();
        w.write_all(b"partial").await.unwrap();
        drop(w);
        assert!(f.names().await.contains(&"a.bin".to_string()));
        f.cleanup().await;
    }
}

#[tokio::test]
async fn vfs_resolves_ftp_uris() {
    if !enabled() {
        return;
    }
    let store = tempfile::tempdir().unwrap();
    let creds_store = Arc::new(MemoryCredentials::default());
    let vfs = Vfs::new(Arc::new(NoLocal), creds_store.clone());
    vfs.register(Arc::new(FtpConnector::ftp(store.path().join("certs"))));
    vfs.register(Arc::new(FtpConnector::ftps(store.path().join("certs"))));
    let loc = Location::parse("ftp://cx@127.0.0.1:2122/").unwrap();
    assert!(matches!(vfs.provider(&loc).await.err(), Some(CxError::AuthRequired { .. })));
    cx_core::CredentialStore::set(creds_store.as_ref(), loc.endpoint().unwrap(), &Credentials::password("cx", "cxpass"), false).unwrap();
    let p = vfs.provider(&loc).await.unwrap();
    assert_eq!(p.scheme(), "ftp");
    assert!(p.capabilities().polling && !p.capabilities().live_watch);
    list_all(p.as_ref(), &loc).await.unwrap();

    let loc = Location::parse("ftps://cx@127.0.0.1:2121/").unwrap();
    cx_core::CredentialStore::set(creds_store.as_ref(), loc.endpoint().unwrap(), &Credentials::password("cx", "cxpass"), false).unwrap();
    match vfs.provider(&loc).await.err() {
        Some(CxError::HostKeyUnknown { key_type, fingerprint, .. }) => trust_host_key(&store.path().join("certs"), HOST, PURE, &key_type, &fingerprint).unwrap(),
        other => panic!("expected HostKeyUnknown, got {other:?}"),
    }
    let p = vfs.provider(&loc).await.unwrap();
    list_all(p.as_ref(), &loc).await.unwrap();
}

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
