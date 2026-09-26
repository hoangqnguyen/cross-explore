//! Integration tests against a real Samba server (docker/smb/compose.yml).
//!
//! Skipped unless `CX_TEST_SMB=1`. Run them with `docker/test-remote-2.sh smb`,
//! or by hand:
//!
//!   docker compose -f docker/smb/compose.yml up -d
//!   CX_TEST_SMB=1 cargo test -p cx-smb --test live
//!
//! Server overrides: CX_SMB_HOST (127.0.0.1), CX_SMB_PORT (1445),
//! CX_SMB_USER (cx), CX_SMB_PASS (cxpass).

use cx_core::provider::list_all;
use cx_core::{Change, Credentials, CxError, Endpoint, Entry, Location, MemoryCredentials, Provider, Scheme, Vfs, WriteMode};
use cx_smb::{SmbConnector, SmbProvider};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn enabled() -> bool {
    let on = std::env::var("CX_TEST_SMB").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set CX_TEST_SMB=1 (see docker/smb/compose.yml)");
    }
    on
}

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn endpoint() -> Endpoint {
    Endpoint {
        scheme: Scheme::Smb,
        user: Some(env("CX_SMB_USER", "cx")),
        host: env("CX_SMB_HOST", "127.0.0.1"),
        port: Some(env("CX_SMB_PORT", "1445").parse().unwrap()),
    }
}

fn creds() -> Credentials {
    Credentials::password(env("CX_SMB_USER", "cx"), env("CX_SMB_PASS", "cxpass"))
}

async fn provider() -> SmbProvider {
    SmbProvider::connect(&endpoint(), Some(creds())).await.expect("connect")
}

fn loc(path: &str) -> Location {
    Location::remote(endpoint(), path)
}

/// A fresh folder in the private share, unique per test run.
async fn scratch(p: &SmbProvider, tag: &str) -> Location {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = loc(&format!("/private/cx-{tag}-{nanos}"));
    p.create_dir(&dir.parent().unwrap(), Some(&dir.name())).await.expect("scratch dir");
    dir
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
    let _ = provider().await;
    let bad = SmbProvider::connect(&endpoint(), Some(Credentials::password("cx", "wrong"))).await;
    match bad {
        Err(CxError::AuthRequired { user, reason, .. }) => {
            assert_eq!(user.as_deref(), Some("cx"));
            assert!(!reason.is_empty());
        }
        other => panic!("expected AuthRequired, got {:?}", other.err()),
    }
    // Guest works on this server, but the private share must refuse it.
    let guest = SmbProvider::connect(&endpoint(), None).await.expect("guest session");
    let public = list_all(&guest, &loc("/public")).await;
    assert!(public.is_ok(), "{public:?}");
    let private = list_all(&guest, &loc("/private")).await;
    assert!(matches!(private, Err(CxError::PermissionDenied(_)) | Err(CxError::NotFound(_))), "{private:?}");
}

#[tokio::test]
async fn lists_shares_at_root() {
    if !enabled() {
        return;
    }
    let p = provider().await;
    let shares = list_all(&p, &loc("/")).await.unwrap();
    let names: Vec<_> = shares.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"public") && names.contains(&"private"), "{names:?}");
    assert!(shares.iter().all(|e| e.is_dir));
    assert!(shares.iter().filter(|e| e.name.ends_with('$')).all(|e| e.hidden));
    let root = p.stat(&loc("/")).await.unwrap();
    assert!(root.is_dir);
    let share = p.stat(&loc("/private")).await.unwrap();
    assert!(share.is_dir);
    assert_eq!(share.name, "private");
    let space = p.free_space(&loc("/private")).await.unwrap().unwrap();
    assert!(space.total > 0);
}

#[tokio::test]
async fn large_listing_in_batches_with_odd_names() {
    if !enabled() {
        return;
    }
    let p = provider().await;
    let dir = scratch(&p, "list").await;
    let odd = ["héllo wörld 日本.txt", "with space.txt", ".dotfile", "emoji 🎉.md"];
    for i in 0..230 {
        write_file(&p, &dir.join(&format!("f{i:03}.txt")), b"x", WriteMode::CreateNew).await;
    }
    for n in odd {
        write_file(&p, &dir.join(n), n.as_bytes(), WriteMode::CreateNew).await;
    }
    p.create_dir(&dir, Some("sub dir")).await.unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let d = dir.clone();
    let p2 = provider().await;
    let task = tokio::spawn(async move { p2.list(&d, tx).await });
    let mut batches = Vec::new();
    while let Some(b) = rx.recv().await {
        batches.push(b);
    }
    let total = task.await.unwrap().unwrap();
    let all: Vec<Entry> = batches.concat();
    assert_eq!(total, 230 + odd.len() + 1);
    assert_eq!(all.len(), total);
    assert!(batches.len() >= 2, "expected several batches, got {}", batches.len());
    assert_eq!(batches[0].len(), 128);
    for n in odd {
        let e = all.iter().find(|e| e.name == n).unwrap_or_else(|| panic!("{n} missing"));
        assert_eq!(e.size, n.len() as u64);
        assert!(e.modified.is_some());
    }
    assert!(all.iter().find(|e| e.name == ".dotfile").unwrap().hidden);
    assert!(all.iter().find(|e| e.name == "sub dir").unwrap().is_dir);

    let e = p.stat(&dir.join("héllo wörld 日本.txt")).await.unwrap();
    assert_eq!(e.name, "héllo wörld 日本.txt");
    assert!(!e.is_dir);
    assert!(matches!(p.stat(&dir.join("nope")).await, Err(CxError::NotFound(_))));
    p.remove(&dir).await.unwrap();
}

#[tokio::test]
async fn mkdir_rename_remove() {
    if !enabled() {
        return;
    }
    let p = provider().await;
    let dir = scratch(&p, "ops").await;
    let a = p.create_dir(&dir, None).await.unwrap();
    let b = p.create_dir(&dir, None).await.unwrap();
    assert_eq!(a.name, "New folder");
    assert_eq!(b.name, "New folder (2)");
    assert!(matches!(p.create_dir(&dir, Some("New folder")).await, Err(CxError::AlreadyExists(_))));

    write_file(&p, &dir.join("a.txt"), b"a", WriteMode::CreateNew).await;
    write_file(&p, &dir.join("b.txt"), b"b", WriteMode::CreateNew).await;
    assert!(matches!(p.rename(&dir, "a.txt", "b.txt").await, Err(CxError::AlreadyExists(_))));
    assert!(matches!(p.open_write(&dir.join("a.txt"), WriteMode::CreateNew).await, Err(CxError::AlreadyExists(_))));
    let c = p.rename(&dir, "a.txt", "c.txt").await.unwrap();
    assert_eq!(c.name, "c.txt");
    // Case-only rename is allowed.
    p.rename(&dir, "c.txt", "C.txt").await.unwrap();
    // Move into a subfolder.
    p.move_to(&dir.join("C.txt"), &dir.join("New folder").join("moved.txt")).await.unwrap();
    assert_eq!(read_file(&p, &dir.join("New folder").join("moved.txt"), 0).await, b"a");

    // Recursive remove of a nested tree.
    let deep = dir.join("New folder (2)");
    let inner = p.create_dir(&deep, Some("inner")).await.unwrap();
    write_file(&p, &deep.join(&inner.name).join("x.bin"), &[1, 2, 3], WriteMode::CreateNew).await;
    p.remove(&deep).await.unwrap();
    assert!(matches!(p.stat(&deep).await, Err(CxError::NotFound(_))));

    // Times.
    p.set_modified(&dir.join("b.txt"), 1_600_000_000_000).await.unwrap();
    assert_eq!(p.stat(&dir.join("b.txt")).await.unwrap().modified, Some(1_600_000_000_000));

    p.remove(&dir).await.unwrap();
    assert!(matches!(p.stat(&dir).await, Err(CxError::NotFound(_))));
}

#[tokio::test]
async fn streams_round_trip_offset_append_and_copy() {
    if !enabled() {
        return;
    }
    let p = provider().await;
    let dir = scratch(&p, "io").await;
    let data = random_bytes(6 * 1024 * 1024 + 12_345, 42);
    let f = dir.join("big.bin");
    let t = Instant::now();
    write_file(&p, &f, &data, WriteMode::CreateNew).await;
    let wrote = t.elapsed();
    let t = Instant::now();
    let back = read_file(&p, &f, 0).await;
    eprintln!("6 MB: write {wrote:?}, read {:?}", t.elapsed());
    assert_eq!(back.len(), data.len());
    assert!(back == data, "content differs");
    assert_eq!(p.stat(&f).await.unwrap().size, data.len() as u64);

    let off = 3 * 1024 * 1024 + 7;
    assert!(read_file(&p, &f, off as u64).await == data[off..]);
    assert!(read_file(&p, &f, data.len() as u64 + 10).await.is_empty());

    // Truncate replaces, Append continues.
    let small = dir.join("small.txt");
    write_file(&p, &small, b"hello world, long first version", WriteMode::Truncate).await;
    write_file(&p, &small, b"hello ", WriteMode::Truncate).await;
    write_file(&p, &small, b"world", WriteMode::Append).await;
    assert_eq!(read_file(&p, &small, 0).await, b"hello world");

    // Server-side copy.
    let copy = dir.join("copy.bin");
    let t = Instant::now();
    let copied = p.copy_within(&f, &copy).await.unwrap();
    eprintln!("copy_within → {copied} in {:?}", t.elapsed());
    if copied {
        assert!(read_file(&p, &copy, 0).await == data);
        assert!(matches!(p.copy_within(&f, &copy).await, Err(CxError::AlreadyExists(_))));
    }
    // Folders are left to the transfer engine.
    assert!(!p.copy_within(&dir, &dir.parent().unwrap().join("nope-copy")).await.unwrap());

    // Dropping a write stream without shutdown must not hang anything.
    let mut w = p.open_write(&dir.join("abandoned.bin"), WriteMode::CreateNew).await.unwrap();
    w.write_all(b"partial").await.unwrap();
    drop(w);

    p.remove(&dir).await.unwrap();
}

#[tokio::test]
async fn watch_sees_changes_from_another_connection() {
    if !enabled() {
        return;
    }
    let p = provider().await;
    let other = provider().await;
    let dir = scratch(&p, "watch").await;
    write_file(&p, &dir.join("gone.txt"), b"x", WriteMode::CreateNew).await;

    let seen: Arc<Mutex<Vec<Change>>> = Arc::default();
    let s = seen.clone();
    let guard = p.watch(&dir, Arc::new(move |c| s.lock().unwrap().extend(c))).await.unwrap();
    assert!(guard.is_some(), "CHANGE_NOTIFY should be available");
    tokio::time::sleep(Duration::from_millis(200)).await;

    write_file(&other, &dir.join("new.txt"), b"12345", WriteMode::CreateNew).await;
    other.remove(&dir.join("gone.txt")).await.unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state: HashMap<String, Option<u64>> = seen
            .lock()
            .unwrap()
            .iter()
            .filter_map(|c| match c {
                Change::Upsert { entry } => Some((entry.name.clone(), Some(entry.size))),
                Change::Remove { name } => Some((name.clone(), None)),
                Change::Reset => None,
            })
            .collect();
        if state.get("new.txt") == Some(&Some(5)) && state.get("gone.txt") == Some(&None) {
            break;
        }
        assert!(Instant::now() < deadline, "watch did not converge: {:?}", seen.lock().unwrap());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drop(guard);
    p.remove(&dir).await.unwrap();
}

#[tokio::test]
async fn vfs_parses_uri_and_connects() {
    if !enabled() {
        return;
    }
    let store = Arc::new(MemoryCredentials::default());
    let vfs = Vfs::new(Arc::new(cx_local::LocalProvider), store.clone());
    vfs.register(Arc::new(SmbConnector::new()));
    let ep = endpoint();
    let uri = format!("smb://{}@{}:{}/private/", ep.user.as_deref().unwrap(), ep.host, ep.port.unwrap());
    let loc = Location::parse(&uri).unwrap();

    assert_eq!(loc.endpoint(), Some(&ep));
    assert_eq!(loc.posix_path(), Some("/private"));

    // Store the password the way the sign-in dialog would; the Vfs then
    // connects on first use and caches the provider.
    cx_core::CredentialStore::set(store.as_ref(), &ep, &creds(), false).unwrap();
    let provider = vfs.provider(&loc).await.unwrap();
    let marker = loc.join(&format!("vfs-{}.txt", std::process::id()));
    write_file(provider.as_ref(), &marker, b"via vfs", WriteMode::Truncate).await;
    let entries = list_all(provider.as_ref(), &loc).await.unwrap();
    assert!(entries.iter().any(|e| e.name == marker.name()));
    provider.remove(&marker).await.unwrap();
    assert!(Arc::ptr_eq(&provider, &vfs.provider(&loc).await.unwrap()), "connection is reused");
    let root = Location::parse(&format!("smb://{}@{}:{}/", ep.user.as_deref().unwrap(), ep.host, ep.port.unwrap())).unwrap();
    let shares = list_all(vfs.provider(&root).await.unwrap().as_ref(), &root).await.unwrap();
    assert!(shares.iter().any(|e| e.name == "private"));

    // Wrong stored password surfaces AuthRequired through the Vfs.
    let bad = Endpoint { user: Some("cx".into()), ..ep.clone() };
    let res = vfs.connect(&bad, Some(Credentials::password("cx", "nope"))).await;
    assert!(matches!(res, Err(CxError::AuthRequired { .. })));
}

/// Needs `CX_SMB_RESTART=<container>` too: restarts the server under a live
/// provider and checks the next operation reconnects by itself.
#[tokio::test]
async fn reconnects_after_server_restart() {
    if !enabled() {
        return;
    }
    let Ok(container) = std::env::var("CX_SMB_RESTART") else {
        eprintln!("skipped: set CX_SMB_RESTART=<container name>");
        return;
    };
    let p = provider().await;
    let dir = scratch(&p, "restart").await;
    write_file(&p, &dir.join("before.txt"), b"1", WriteMode::CreateNew).await;
    let status = std::process::Command::new("docker").args(["restart", "-t", "1", &container]).status().unwrap();
    assert!(status.success());
    // The same provider must work again once the server is back.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match list_all(&p, &dir).await {
            Ok(entries) => {
                assert!(entries.iter().any(|e| e.name == "before.txt"));
                break;
            }
            Err(e) => {
                assert!(Instant::now() < deadline, "no reconnect: {e:?}");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    write_file(&p, &dir.join("after.txt"), b"2", WriteMode::CreateNew).await;
    p.remove(&dir).await.unwrap();
}
