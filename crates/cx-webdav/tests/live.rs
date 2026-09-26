//! Integration tests against real WebDAV servers (docker/webdav/compose.yml):
//! rclone (Basic auth) and Apache mod_dav (Digest auth). Every test runs
//! against both.
//!
//! Skipped unless `CX_TEST_DAV=1`. Run them with `docker/test-remote-2.sh
//! webdav`, or by hand:
//!
//!   docker compose -f docker/webdav/compose.yml up -d
//!   CX_TEST_DAV=1 cargo test -p cx-webdav --test live
//!
//! Overrides: CX_DAV_HOST (127.0.0.1), CX_DAV_PORTS ("8088,8089"),
//! CX_DAV_USER (cx), CX_DAV_PASS (cxpass).

use cx_core::provider::list_all;
use cx_core::{Credentials, CxError, Endpoint, Entry, Location, MemoryCredentials, Provider, Scheme, Vfs, WriteMode};
use cx_webdav::{DavConnector, DavProvider};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn enabled() -> bool {
    let on = std::env::var("CX_TEST_DAV").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set CX_TEST_DAV=1 (see docker/webdav/compose.yml)");
    }
    on
}

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn endpoints() -> Vec<Endpoint> {
    env("CX_DAV_PORTS", "8088,8089")
        .split(',')
        .map(|p| Endpoint { scheme: Scheme::Dav, user: Some(env("CX_DAV_USER", "cx")), host: env("CX_DAV_HOST", "127.0.0.1"), port: Some(p.trim().parse().unwrap()) })
        .collect()
}

fn creds() -> Credentials {
    Credentials::password(env("CX_DAV_USER", "cx"), env("CX_DAV_PASS", "cxpass"))
}

async fn connect(ep: &Endpoint) -> DavProvider {
    DavProvider::connect(ep, Some(creds())).await.unwrap_or_else(|e| panic!("connect {ep}: {e:?}"))
}

async fn scratch(p: &DavProvider, tag: &str) -> Location {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let root = Location::remote(p.endpoint().clone(), "/");
    let e = p.create_dir(&root, Some(&format!("cx-{tag}-{nanos}"))).await.expect("scratch dir");
    root.join(&e.name)
}

fn random_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

async fn write_file(p: &dyn Provider, at: &Location, data: &[u8], mode: WriteMode) {
    let mut w = p.open_write(at, mode).await.expect("open_write");
    w.write_all(data).await.expect("write");
    w.shutdown().await.expect("shutdown");
}

async fn read_file(p: &dyn Provider, at: &Location, offset: u64) -> Vec<u8> {
    let mut r = p.open_read(at, offset).await.expect("open_read");
    let mut out = Vec::new();
    r.read_to_end(&mut out).await.expect("read");
    out
}

#[tokio::test]
async fn connect_and_auth_errors() {
    if !enabled() {
        return;
    }
    for ep in endpoints() {
        let p = connect(&ep).await;
        assert!(p.stat(&Location::remote(ep.clone(), "/")).await.unwrap().is_dir);
        let bad = DavProvider::connect(&ep, Some(Credentials::password("cx", "wrong"))).await;
        assert!(matches!(bad, Err(CxError::AuthRequired { .. })), "{ep}: {:?}", bad.err());
        let anon = DavProvider::connect(&ep, None).await;
        match anon {
            Err(CxError::AuthRequired { user, .. }) => assert_eq!(user.as_deref(), Some("cx")),
            other => panic!("{ep}: expected AuthRequired, got {:?}", other.err()),
        }
    }
}

#[tokio::test]
async fn large_listing_in_batches_with_odd_names() {
    if !enabled() {
        return;
    }
    for ep in endpoints() {
        let p = connect(&ep).await;
        let dir = scratch(&p, "list").await;
        let odd = ["héllo wörld 日本.txt", "with space.txt", ".dotfile", "a+b&c=d%e #1.txt", "emoji 🎉.md"];
        for i in 0..230 {
            write_file(&p, &dir.join(&format!("f{i:03}.txt")), b"x", WriteMode::Truncate).await;
        }
        for n in odd {
            write_file(&p, &dir.join(n), n.as_bytes(), WriteMode::CreateNew).await;
        }
        p.create_dir(&dir, Some("sub dir")).await.unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let (d, p2) = (dir.clone(), connect(&ep).await);
        let task = tokio::spawn(async move { p2.list(&d, tx).await });
        let mut batches = Vec::new();
        while let Some(b) = rx.recv().await {
            batches.push(b);
        }
        let total = task.await.unwrap().unwrap();
        let all: Vec<Entry> = batches.concat();
        assert_eq!(total, 230 + odd.len() + 1, "{ep}");
        assert_eq!(all.len(), total);
        assert!(batches.len() >= 2 && batches[0].len() == 128, "{ep}: batches {:?}", batches.iter().map(Vec::len).collect::<Vec<_>>());
        assert!(!all.iter().any(|e| e.name == dir.name()), "{ep}: listing contains the folder itself");
        for n in odd {
            let e = all.iter().find(|e| e.name == n).unwrap_or_else(|| panic!("{ep}: {n} missing"));
            assert_eq!(e.size, n.len() as u64, "{ep}: {n}");
            assert!(e.modified.is_some());
            assert!(!e.is_dir);
        }
        assert!(all.iter().find(|e| e.name == ".dotfile").unwrap().hidden);
        let sub = all.iter().find(|e| e.name == "sub dir").unwrap();
        assert!(sub.is_dir && sub.size == 0);

        let e = p.stat(&dir.join("héllo wörld 日本.txt")).await.unwrap();
        assert_eq!(e.name, "héllo wörld 日本.txt");
        assert_eq!(e.size, "héllo wörld 日本.txt".len() as u64);
        assert!(matches!(p.stat(&dir.join("nope")).await, Err(CxError::NotFound(_))), "{ep}");
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        assert!(matches!(p.list(&dir.join("nope"), tx).await, Err(CxError::NotFound(_))), "{ep}");
        p.remove(&dir).await.unwrap();
    }
}

#[tokio::test]
async fn mkdir_rename_copy_remove() {
    if !enabled() {
        return;
    }
    for ep in endpoints() {
        let p = connect(&ep).await;
        let dir = scratch(&p, "ops").await;
        let a = p.create_dir(&dir, None).await.unwrap();
        let b = p.create_dir(&dir, None).await.unwrap();
        assert_eq!((a.name.as_str(), b.name.as_str()), ("New folder", "New folder (2)"), "{ep}");
        assert!(matches!(p.create_dir(&dir, Some("New folder")).await, Err(CxError::AlreadyExists(_))), "{ep}");

        write_file(&p, &dir.join("a.txt"), b"a", WriteMode::CreateNew).await;
        write_file(&p, &dir.join("b.txt"), b"b", WriteMode::CreateNew).await;
        assert!(matches!(p.rename(&dir, "a.txt", "b.txt").await, Err(CxError::AlreadyExists(_))), "{ep}");
        assert!(matches!(p.open_write(&dir.join("a.txt"), WriteMode::CreateNew).await, Err(CxError::AlreadyExists(_))), "{ep}");
        assert!(matches!(p.open_write(&dir.join("a.txt"), WriteMode::Append).await, Err(CxError::Unsupported(_))));
        let c = p.rename(&dir, "a.txt", "c.txt").await.unwrap();
        assert_eq!(c.name, "c.txt");
        assert_eq!(read_file(&p, &dir.join("b.txt"), 0).await, b"b");
        // Folders move too (with their contents).
        write_file(&p, &dir.join("New folder").join("inner.txt"), b"in", WriteMode::CreateNew).await;
        p.rename(&dir, "New folder", "Renamed").await.unwrap();
        assert_eq!(read_file(&p, &dir.join("Renamed").join("inner.txt"), 0).await, b"in");

        // Server-side copy of a file and of a folder; never overwrites.
        assert!(p.copy_within(&dir.join("c.txt"), &dir.join("d.txt")).await.unwrap(), "{ep}");
        assert_eq!(read_file(&p, &dir.join("d.txt"), 0).await, b"a");
        assert!(matches!(p.copy_within(&dir.join("c.txt"), &dir.join("b.txt")).await, Err(CxError::AlreadyExists(_))), "{ep}");
        assert!(p.copy_within(&dir.join("Renamed"), &dir.join("Copied")).await.unwrap());
        assert_eq!(read_file(&p, &dir.join("Copied").join("inner.txt"), 0).await, b"in");

        // Best effort only, but must not fail.
        p.set_modified(&dir.join("b.txt"), 1_600_000_000_000).await.unwrap();
        let _ = p.free_space(&dir).await.unwrap();

        // Recursive delete.
        p.remove(&dir.join("Copied")).await.unwrap();
        assert!(matches!(p.stat(&dir.join("Copied")).await, Err(CxError::NotFound(_))));
        p.remove(&dir).await.unwrap();
        assert!(matches!(p.stat(&dir).await, Err(CxError::NotFound(_))), "{ep}");
    }
}

#[tokio::test]
async fn streams_round_trip_and_offsets() {
    if !enabled() {
        return;
    }
    for ep in endpoints() {
        let p = connect(&ep).await;
        let dir = scratch(&p, "io").await;
        let data = random_bytes(5 * 1024 * 1024 + 4321, 7);
        let f = dir.join("big file.bin");
        let t = Instant::now();
        write_file(&p, &f, &data, WriteMode::CreateNew).await;
        let wrote = t.elapsed();
        let t = Instant::now();
        let back = read_file(&p, &f, 0).await;
        eprintln!("{ep}: 5 MB write {wrote:?}, read {:?}", t.elapsed());
        assert_eq!(back.len(), data.len(), "{ep}");
        assert!(back == data, "{ep}: content differs");
        assert_eq!(p.stat(&f).await.unwrap().size, data.len() as u64);

        let off = 2 * 1024 * 1024 + 3;
        assert!(read_file(&p, &f, off as u64).await == data[off..], "{ep}: offset read");
        assert!(read_file(&p, &f, data.len() as u64 + 5).await.is_empty(), "{ep}: read past end");

        // Truncate replaces.
        write_file(&p, &f, b"short", WriteMode::Truncate).await;
        assert_eq!(read_file(&p, &f, 0).await, b"short");

        // Writing into a missing folder fails on shutdown with NotFound.
        let mut w = p.open_write(&dir.join("missing").join("x.txt"), WriteMode::Truncate).await.unwrap();
        w.write_all(b"x").await.unwrap();
        let err = w.shutdown().await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound, "{ep}: {err}");

        // Dropping a writer without shutdown aborts without hanging.
        let mut w = p.open_write(&dir.join("abandoned.bin"), WriteMode::Truncate).await.unwrap();
        w.write_all(b"partial").await.unwrap();
        drop(w);

        assert!(matches!(p.open_read(&dir.join("nope"), 0).await, Err(CxError::NotFound(_))));
        p.remove(&dir).await.unwrap();
    }
}

#[tokio::test]
async fn vfs_parses_uri_and_connects() {
    if !enabled() {
        return;
    }
    let store = Arc::new(MemoryCredentials::default());
    let vfs = Vfs::new(Arc::new(cx_local::LocalProvider), store.clone());
    vfs.register(Arc::new(DavConnector::http()));
    vfs.register(Arc::new(DavConnector::https()));
    for ep in endpoints() {
        // http:// parses to the dav scheme.
        let loc = Location::parse(&format!("http://cx@{}:{}/", ep.host, ep.port.unwrap())).unwrap();
        assert_eq!(loc.endpoint(), Some(&ep));
        // Without a stored password the Vfs reports AuthRequired...
        assert!(matches!(vfs.provider(&loc).await, Err(CxError::AuthRequired { .. })));
        // ...and connects once the sign-in dialog has stored one.
        cx_core::CredentialStore::set(store.as_ref(), &ep, &creds(), false).unwrap();
        let provider = vfs.provider(&loc).await.unwrap();
        assert_eq!(provider.scheme(), "dav");
        assert!(!provider.capabilities().live_watch && provider.capabilities().polling);
        let dir = loc.join(&format!("vfs-{}", std::process::id()));
        provider.create_dir(&loc, Some(&dir.name())).await.unwrap();
        write_file(provider.as_ref(), &dir.join("x.txt"), b"x", WriteMode::CreateNew).await;
        let entries = list_all(provider.as_ref(), &dir).await.unwrap();
        assert_eq!(entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["x.txt"]);
        assert!(provider.watch(&dir, Arc::new(|_| {})).await.unwrap().is_none());
        provider.remove(&dir).await.unwrap();
    }
}
