//! Integration tests against a real MinIO (docker/s3/compose.yml), which
//! provides the private bucket `cx-test` and the anonymously readable
//! `cx-public` (holding `readme.txt`).
//!
//! Skipped unless `CX_TEST_S3=1`. Run them with `docker/test-s3.sh`, or:
//!
//!   docker compose -f docker/s3/compose.yml up -d
//!   CX_TEST_S3=1 cargo test -p cx-s3 --test live
//!
//! Overrides: CX_S3_HOST (127.0.0.1), CX_S3_PORT (9900), CX_S3_KEY
//! (cxadmin), CX_S3_SECRET (cxsecret123), CX_S3_BUCKET (cx-test).

use cx_core::provider::list_all;
use cx_core::{CredentialStore, Credentials, CxError, Endpoint, Entry, Location, MemoryCredentials, Provider, Scheme, Vfs, WriteMode};
use cx_s3::{S3Connector, S3Provider};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn enabled() -> bool {
    let on = std::env::var("CX_TEST_S3").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set CX_TEST_S3=1 (see docker/s3/compose.yml)");
    }
    on
}

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

fn endpoint(user: Option<&str>) -> Endpoint {
    Endpoint { scheme: Scheme::S3, user: user.map(str::to_owned), host: env("CX_S3_HOST", "127.0.0.1"), port: Some(env("CX_S3_PORT", "9900").parse().unwrap()) }
}

fn creds() -> Credentials {
    Credentials::password(env("CX_S3_KEY", "cxadmin"), env("CX_S3_SECRET", "cxsecret123"))
}

fn bucket() -> String {
    env("CX_S3_BUCKET", "cx-test")
}

async fn connect() -> S3Provider {
    let ep = endpoint(Some(&env("CX_S3_KEY", "cxadmin")));
    S3Provider::connect(&ep, Some(creds())).await.unwrap_or_else(|e| panic!("connect {ep}: {e:?}"))
}

/// A fresh folder in the test bucket.
async fn scratch(p: &S3Provider, tag: &str) -> Location {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let root = Location::remote(p.endpoint().clone(), format!("/{}", bucket()));
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

fn names(entries: &[Entry]) -> Vec<String> {
    let mut v: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    v.sort();
    v
}

#[tokio::test]
async fn lists_buckets() {
    if !enabled() {
        return;
    }
    let p = connect().await;
    let root = Location::remote(p.endpoint().clone(), "/");
    let all = list_all(&p, &root).await.unwrap();
    let b = all.iter().find(|e| e.name == bucket()).expect("test bucket listed");
    assert!(b.is_dir);
    assert!(b.modified.is_some());
    assert!(all.iter().any(|e| e.name == "cx-public"));
    assert!(p.stat(&root).await.unwrap().is_dir);
    assert!(p.stat(&root.join(&bucket())).await.unwrap().is_dir);
    assert!(matches!(p.stat(&root.join("cx-no-such-bucket")).await, Err(CxError::NotFound(_))));
    let caps = p.capabilities();
    assert!(caps.polling && caps.server_copy && caps.writable && !caps.posix && !caps.trash);
}

#[tokio::test]
async fn paginated_listing_with_nested_prefixes() {
    if !enabled() {
        return;
    }
    let p = Arc::new(connect().await);
    let dir = scratch(&p, "many").await;
    const N: usize = 1105;
    // Upload in parallel: 1105 one-by-one PUTs would make the test slow.
    let mut tasks = tokio::task::JoinSet::new();
    let sem = Arc::new(tokio::sync::Semaphore::new(32));
    for i in 0..N {
        let (p, at, sem) = (p.clone(), dir.join(&format!("f{i:04}.txt")), sem.clone());
        tasks.spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            write_file(p.as_ref(), &at, b"x", WriteMode::Truncate).await;
        });
    }
    while let Some(r) = tasks.join_next().await {
        r.unwrap();
    }
    write_file(p.as_ref(), &dir.join("sub").join("deeper").join("leaf.txt"), b"leaf", WriteMode::Truncate).await;
    write_file(p.as_ref(), &dir.join("sub").join("a.txt"), b"a", WriteMode::Truncate).await;
    write_file(p.as_ref(), &dir.join(".hidden"), b"h", WriteMode::Truncate).await;
    write_file(p.as_ref(), &dir.join("odd name +&=%#?.txt"), b"odd", WriteMode::Truncate).await;

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let (d, p2) = (dir.clone(), p.clone());
    let task = tokio::spawn(async move { p2.list(&d, tx).await });
    let mut batches = Vec::new();
    while let Some(b) = rx.recv().await {
        batches.push(b);
    }
    let total = task.await.unwrap().unwrap();
    let all: Vec<Entry> = batches.concat();
    assert_eq!(total, N + 3, "files + sub + .hidden + odd");
    assert_eq!(all.len(), total);
    assert!(batches.len() >= 3, "batches {:?}", batches.iter().map(Vec::len).collect::<Vec<_>>());
    assert!(batches[0].len() <= 100, "first batch should be small: {}", batches[0].len());
    assert!(!all.iter().any(|e| e.name.is_empty() || e.name == dir.name()), "folder marker leaked into the listing");
    let sub = all.iter().find(|e| e.name == "sub").unwrap();
    assert!(sub.is_dir);
    assert!(all.iter().find(|e| e.name == ".hidden").unwrap().hidden);
    let odd = all.iter().find(|e| e.name == "odd name +&=%#?.txt").unwrap();
    assert_eq!(odd.size, 3);
    assert!(odd.modified.is_some());
    assert_eq!(read_file(p.as_ref(), &dir.join("odd name +&=%#?.txt"), 0).await, b"odd");

    // Nested: the implied folder "sub/deeper" has no marker.
    let sub_list = list_all(p.as_ref(), &dir.join("sub")).await.unwrap();
    assert_eq!(names(&sub_list), ["a.txt", "deeper"]);
    let deeper = list_all(p.as_ref(), &dir.join("sub").join("deeper")).await.unwrap();
    assert_eq!(names(&deeper), ["leaf.txt"]);
    assert!(matches!(list_all(p.as_ref(), &dir.join("nope")).await, Err(CxError::NotFound(_))));
    assert!(matches!(list_all(p.as_ref(), &dir.join("f0001.txt")).await, Err(CxError::InvalidLocation(_))));

    // Recursive remove of > 1000 keys (DeleteObjects in batches).
    p.remove(&dir).await.unwrap();
    assert!(matches!(p.stat(&dir).await, Err(CxError::NotFound(_))));
}

#[tokio::test]
async fn stat_and_mkdir() {
    if !enabled() {
        return;
    }
    let p = connect().await;
    let dir = scratch(&p, "stat").await;
    // An empty folder (marker only) exists and lists as empty.
    assert!(p.stat(&dir).await.unwrap().is_dir);
    assert!(list_all(&p, &dir).await.unwrap().is_empty());

    write_file(&p, &dir.join("file.bin"), b"12345", WriteMode::CreateNew).await;
    let f = p.stat(&dir.join("file.bin")).await.unwrap();
    assert!(!f.is_dir);
    assert_eq!((f.name.as_str(), f.size), ("file.bin", 5));
    assert!(f.modified.is_some());

    write_file(&p, &dir.join("implied").join("x.txt"), b"x", WriteMode::Truncate).await;
    let d = p.stat(&dir.join("implied")).await.unwrap();
    assert!(d.is_dir && d.name == "implied");
    assert!(matches!(p.stat(&dir.join("missing")).await, Err(CxError::NotFound(_))));

    let named = p.create_dir(&dir, Some("Photos")).await.unwrap();
    assert!(named.is_dir && named.name == "Photos");
    assert!(matches!(p.create_dir(&dir, Some("Photos")).await, Err(CxError::AlreadyExists(_))));
    assert!(matches!(p.create_dir(&dir, Some("implied")).await, Err(CxError::AlreadyExists(_))));
    assert!(matches!(p.create_dir(&dir, Some("file.bin")).await, Err(CxError::AlreadyExists(_))));
    assert!(matches!(p.create_dir(&dir, Some("a/b")).await, Err(CxError::InvalidName(_))));
    let n1 = p.create_dir(&dir, None).await.unwrap();
    let n2 = p.create_dir(&dir, None).await.unwrap();
    assert_eq!((n1.name.as_str(), n2.name.as_str()), ("New folder", "New folder (2)"));
    let listed = list_all(&p, &dir).await.unwrap();
    assert_eq!(names(&listed), ["New folder", "New folder (2)", "Photos", "file.bin", "implied"]);
    assert!(list_all(&p, &dir.join("Photos")).await.unwrap().is_empty());

    p.remove(&dir).await.unwrap();
}

#[tokio::test]
async fn upload_small_and_multipart_and_read_back() {
    if !enabled() {
        return;
    }
    let p = connect().await;
    let dir = scratch(&p, "io").await;

    let small = random_bytes(10_000, 7);
    write_file(&p, &dir.join("small.bin"), &small, WriteMode::CreateNew).await;
    assert_eq!(read_file(&p, &dir.join("small.bin"), 0).await, small);
    assert_eq!(read_file(&p, &dir.join("small.bin"), 9_000).await, &small[9_000..]);
    assert!(read_file(&p, &dir.join("small.bin"), 20_000).await.is_empty());

    // 12 MB: two parts (8 MiB + rest), written in odd-sized chunks.
    let big = random_bytes(12 * 1000 * 1000, 42);
    let mut w = p.open_write(&dir.join("big.bin"), WriteMode::Truncate).await.unwrap();
    for chunk in big.chunks(777_777) {
        w.write_all(chunk).await.unwrap();
    }
    w.shutdown().await.unwrap();
    assert_eq!(p.stat(&dir.join("big.bin")).await.unwrap().size, big.len() as u64);
    assert!(read_file(&p, &dir.join("big.bin"), 0).await == big, "multipart content differs");
    let off = 9_876_543;
    assert!(read_file(&p, &dir.join("big.bin"), off).await == big[off as usize..], "ranged read differs");

    // Empty file.
    write_file(&p, &dir.join("empty"), b"", WriteMode::CreateNew).await;
    assert_eq!(p.stat(&dir.join("empty")).await.unwrap().size, 0);
    assert!(read_file(&p, &dir.join("empty"), 0).await.is_empty());

    // Modes.
    assert!(matches!(p.open_write(&dir.join("small.bin"), WriteMode::CreateNew).await, Err(CxError::AlreadyExists(_))));
    assert!(matches!(p.open_write(&dir.join("small.bin"), WriteMode::Append).await, Err(CxError::Unsupported(_))));
    write_file(&p, &dir.join("small.bin"), b"replaced", WriteMode::Truncate).await;
    assert_eq!(read_file(&p, &dir.join("small.bin"), 0).await, b"replaced");
    assert!(p.set_modified(&dir.join("small.bin"), 0).await.is_ok());

    // Dropped without shutdown: nothing appears (the multipart upload is
    // aborted, the single PUT never sent).
    for (name, len) in [("dropped-small", 1000), ("dropped-big", 20 * 1024 * 1024)] {
        let mut w = p.open_write(&dir.join(name), WriteMode::CreateNew).await.unwrap();
        w.write_all(&random_bytes(len, 3)).await.unwrap();
        drop(w);
    }
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    for name in ["dropped-small", "dropped-big"] {
        assert!(matches!(p.stat(&dir.join(name)).await, Err(CxError::NotFound(_))), "{name} should not exist");
    }
    assert!(matches!(p.open_read(&dir.join("missing"), 0).await, Err(CxError::NotFound(_))));

    p.remove(&dir).await.unwrap();
}

#[tokio::test]
async fn rename_copy_and_remove() {
    if !enabled() {
        return;
    }
    let p = connect().await;
    let dir = scratch(&p, "mv").await;
    write_file(&p, &dir.join("a.txt"), b"alpha", WriteMode::CreateNew).await;
    write_file(&p, &dir.join("b.txt"), b"beta", WriteMode::CreateNew).await;

    // File rename, and refusing to overwrite.
    let e = p.rename(&dir, "a.txt", "renamed.txt").await.unwrap();
    assert_eq!((e.name.as_str(), e.size), ("renamed.txt", 5));
    assert!(matches!(p.stat(&dir.join("a.txt")).await, Err(CxError::NotFound(_))));
    assert!(matches!(p.move_to(&dir.join("renamed.txt"), &dir.join("b.txt")).await, Err(CxError::AlreadyExists(_))));
    assert_eq!(read_file(&p, &dir.join("b.txt"), 0).await, b"beta");

    // Folder rename: every key under it moves, including the marker and
    // nested levels.
    p.create_dir(&dir, Some("folder")).await.unwrap();
    write_file(&p, &dir.join("folder").join("one.txt"), b"1", WriteMode::CreateNew).await;
    write_file(&p, &dir.join("folder").join("nested").join("two.txt"), b"22", WriteMode::CreateNew).await;
    p.create_dir(&dir.join("folder"), Some("empty")).await.unwrap();
    p.rename(&dir, "folder", "moved").await.unwrap();
    assert!(matches!(p.stat(&dir.join("folder")).await, Err(CxError::NotFound(_))));
    assert_eq!(names(&list_all(&p, &dir.join("moved")).await.unwrap()), ["empty", "nested", "one.txt"]);
    assert_eq!(read_file(&p, &dir.join("moved").join("nested").join("two.txt"), 0).await, b"22");
    assert!(p.stat(&dir.join("moved").join("empty")).await.unwrap().is_dir);
    assert!(matches!(p.move_to(&dir.join("moved"), &dir.join("moved").join("nested").join("x")).await, Err(CxError::InvalidLocation(_))));
    assert!(matches!(p.move_to(&dir.join("moved"), &dir.join("b.txt")).await, Err(CxError::AlreadyExists(_))));

    // Server-side copy.
    assert!(p.copy_within(&dir.join("b.txt"), &dir.join("b copy.txt")).await.unwrap());
    assert_eq!(read_file(&p, &dir.join("b copy.txt"), 0).await, b"beta");
    assert!(matches!(p.copy_within(&dir.join("b.txt"), &dir.join("renamed.txt")).await, Err(CxError::AlreadyExists(_))));
    assert!(!p.copy_within(&dir.join("moved"), &dir.join("moved2")).await.unwrap(), "folders are walked by the caller");

    // Multipart copy (UploadPartCopy), forced by lowering the limits.
    let big = random_bytes(12 * 1000 * 1000, 9);
    write_file(&p, &dir.join("big.bin"), &big, WriteMode::CreateNew).await;
    p.set_copy_limits(5 * 1024 * 1024, 5 * 1024 * 1024);
    assert!(p.copy_within(&dir.join("big.bin"), &dir.join("big copy.bin")).await.unwrap());
    p.move_to(&dir.join("big.bin"), &dir.join("big moved.bin")).await.unwrap();
    for name in ["big copy.bin", "big moved.bin"] {
        assert!(read_file(&p, &dir.join(name), 0).await == big, "{name} differs");
    }

    // Removing a single file, a folder, and something missing.
    p.remove(&dir.join("b copy.txt")).await.unwrap();
    assert!(matches!(p.stat(&dir.join("b copy.txt")).await, Err(CxError::NotFound(_))));
    p.remove(&dir.join("moved")).await.unwrap();
    assert!(matches!(p.stat(&dir.join("moved")).await, Err(CxError::NotFound(_))));
    assert!(matches!(p.remove(&dir.join("moved")).await, Err(CxError::NotFound(_))));
    assert_eq!(names(&list_all(&p, &dir).await.unwrap()), ["b.txt", "big copy.bin", "big moved.bin", "renamed.txt"]);

    p.remove(&dir).await.unwrap();
    assert!(matches!(p.stat(&dir).await, Err(CxError::NotFound(_))));
}

#[tokio::test]
async fn auth_errors_and_anonymous_access() {
    if !enabled() {
        return;
    }
    let key = env("CX_S3_KEY", "cxadmin");
    let wrong_secret = S3Provider::connect(&endpoint(Some(&key)), Some(Credentials::password(&key, "wrong-secret"))).await;
    match wrong_secret {
        Err(CxError::AuthRequired { user, reason, .. }) => {
            assert_eq!(user.as_deref(), Some(key.as_str()));
            assert!(reason.contains("secret"), "{reason}");
        }
        other => panic!("expected AuthRequired, got {:?}", other.err()),
    }
    let wrong_key = S3Provider::connect(&endpoint(None), Some(Credentials::password("nobody", "whatever"))).await;
    assert!(matches!(wrong_key, Err(CxError::AuthRequired { .. })), "{:?}", wrong_key.err());
    // A key in the URI but no secret: ask for it.
    assert!(matches!(S3Provider::connect(&endpoint(Some(&key)), None).await, Err(CxError::AuthRequired { .. })));

    // Anonymous: public buckets work, everything else asks to sign in.
    let anon = S3Provider::connect(&endpoint(None), None).await.unwrap();
    let root = Location::remote(endpoint(None), "/");
    let public = list_all(&anon, &root.join("cx-public")).await.unwrap();
    assert!(public.iter().any(|e| e.name == "readme.txt"), "{public:?}");
    assert_eq!(read_file(&anon, &root.join("cx-public").join("readme.txt"), 0).await, b"hello\n");
    assert!(matches!(list_all(&anon, &root).await, Err(CxError::AuthRequired { .. })));
    let private = list_all(&anon, &root.join(&bucket())).await;
    assert!(matches!(private, Err(CxError::AuthRequired { .. })), "{:?}", private.err());
    let mut write = anon.open_write(&root.join("cx-public").join("nope.txt"), WriteMode::Truncate).await.unwrap();
    write.write_all(b"x").await.unwrap();
    assert!(write.shutdown().await.is_err(), "anonymous upload must fail");
}

#[tokio::test]
async fn through_the_vfs() {
    if !enabled() {
        return;
    }
    let store = Arc::new(MemoryCredentials::default());
    let vfs = Vfs::new(Arc::new(cx_local::LocalProvider), store.clone());
    vfs.register(Arc::new(S3Connector));
    assert!(vfs.supports(Scheme::S3));

    let host = env("CX_S3_HOST", "127.0.0.1");
    let port = env("CX_S3_PORT", "9900");
    let key = env("CX_S3_KEY", "cxadmin");
    let uri = format!("s3://{key}@{host}:{port}/{}/cx-vfs-{}", bucket(), std::process::id());
    let loc = Location::parse(&uri).unwrap();
    let ep = loc.endpoint().unwrap().clone();
    assert_eq!(ep.scheme, Scheme::S3);
    assert_eq!(loc.info().crumbs[1].icon, "share");

    // No credentials stored yet: sign-in required.
    assert!(matches!(vfs.provider(&loc).await, Err(CxError::AuthRequired { .. })));
    store.set(&ep, &creds(), false).unwrap();
    let p = vfs.provider(&loc).await.unwrap();
    let bucket_loc = loc.parent().unwrap();
    let dir = bucket_loc.join(&p.create_dir(&bucket_loc, Some(&loc.name())).await.unwrap().name);
    assert_eq!(dir, loc);
    write_file(p.as_ref(), &dir.join("hello world.txt"), b"hi via vfs", WriteMode::CreateNew).await;
    let parsed = Location::parse(&format!("{uri}/hello%20world.txt")).unwrap();
    let p2 = vfs.provider(&parsed).await.unwrap();
    assert!(Arc::ptr_eq(&p, &p2), "one cached connection per endpoint");
    assert_eq!(read_file(p2.as_ref(), &parsed, 3).await, b"via vfs");
    assert_eq!(names(&list_all(p.as_ref(), &dir).await.unwrap()), ["hello world.txt"]);
    p.remove(&dir).await.unwrap();
}
