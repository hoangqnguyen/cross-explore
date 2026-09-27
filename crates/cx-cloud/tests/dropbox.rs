//! Dropbox provider against the in-process Dropbox mock, plus the shared
//! token-refresh and rate-limit behavior (exercised through Dropbox).

mod common;

use common::dropbox_mock::DropboxMock;
use common::*;
use cx_cloud::{CloudProvider, DropboxProvider, Service, Tokens};
use cx_core::provider::list_all;
use cx_core::{CxError, Provider, WriteMode};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

async fn setup(page: usize) -> (Arc<DropboxMock>, Mock, Arc<DropboxProvider>) {
    let dbx = Arc::new(DropboxMock::new(page));
    let mock = serve(dbx.clone(), Arc::new(Auth::new(ACCESS, REFRESH))).await;
    let CloudProvider::Dropbox(p) = connect(Service::Dropbox, &mock).await else { panic!("not Dropbox") };
    (dbx, mock, p)
}

#[tokio::test]
async fn listing_follows_cursors() {
    let (dbx, mock, p) = setup(3).await;
    dbx.add("/Docs", true, b"");
    for i in 0..7 {
        dbx.add(&format!("/Docs/f{i}.txt"), false, b"12345");
    }
    dbx.add("/Docs/.dotfile", false, b"");
    dbx.add("/Docs/Sub", true, b"");
    let e = entries(p.as_ref(), "/Docs").await;
    assert_eq!(e.len(), 9);
    assert_eq!(mock.requests("POST", "/api/files/list_folder/continue"), 2);
    assert!(e[".dotfile"].hidden && !e["f0.txt"].hidden);
    assert!(e["Sub"].is_dir);
    assert_eq!(e["f3.txt"].size, 5);
    assert!(e["f3.txt"].modified.is_some());
    assert_eq!(names(p.as_ref(), "/").await, ["Docs"]);
    assert!(matches!(list_all(p.as_ref(), &loc("/Nope")).await, Err(CxError::NotFound(_))));
}

#[tokio::test]
async fn stat_mkdir_rename_move_delete() {
    let (dbx, _mock, p) = setup(100).await;
    dbx.add("/a.txt", false, b"aaa");
    dbx.add("/b.txt", false, b"b");
    dbx.add("/Dir", true, b"");
    assert!(p.stat(&loc("/")).await.unwrap().is_dir);
    assert_eq!(p.stat(&loc("/a.txt")).await.unwrap().size, 3);
    assert!(matches!(p.stat(&loc("/zzz")).await, Err(CxError::NotFound(_))));

    assert_eq!(p.create_dir(&loc("/Dir"), None).await.unwrap().name, "New folder");
    assert_eq!(p.create_dir(&loc("/Dir"), None).await.unwrap().name, "New folder (2)");
    assert!(matches!(p.create_dir(&loc("/"), Some("Dir")).await, Err(CxError::AlreadyExists(_))));

    assert!(matches!(p.rename(&loc("/"), "a.txt", "b.txt").await, Err(CxError::AlreadyExists(_))));
    assert_eq!(p.rename(&loc("/"), "a.txt", "c.txt").await.unwrap().name, "c.txt");
    p.move_to(&loc("/c.txt"), &loc("/Dir/c.txt")).await.unwrap();
    assert_eq!(dbx.get("/Dir/c.txt").unwrap().content, b"aaa");
    // Case-only rename is allowed.
    p.move_to(&loc("/b.txt"), &loc("/B.txt")).await.unwrap();
    assert_eq!(dbx.get("/b.txt").unwrap().path, "/B.txt");

    p.remove(&loc("/Dir")).await.unwrap();
    assert!(dbx.get("/Dir/c.txt").is_none());
    assert!(matches!(p.remove(&loc("/Dir")).await, Err(CxError::NotFound(_))));
    assert!(matches!(p.trash(&loc("/"), &["B.txt".into()]).await, Err(CxError::Unsupported(_))));
}

#[tokio::test]
async fn ranged_downloads_and_unicode_paths() {
    let (dbx, _mock, p) = setup(100).await;
    dbx.add("/Ünïcödé 😀.bin", false, &pattern(5000));
    assert_eq!(read(p.as_ref(), "/Ünïcödé 😀.bin", 0).await, pattern(5000));
    assert_eq!(read(p.as_ref(), "/Ünïcödé 😀.bin", 4000).await, pattern(5000)[4000..]);
    assert!(read(p.as_ref(), "/Ünïcödé 😀.bin", 5000).await.is_empty());
}

#[tokio::test]
async fn uploads_single_and_session() {
    let (dbx, mock, p) = setup(100).await;
    write(p.as_ref(), "/small.txt", WriteMode::CreateNew, b"tiny").await.unwrap();
    assert_eq!(dbx.get("/small.txt").unwrap().content, b"tiny");
    assert_eq!(mock.requests("POST", "/content/files/upload_session"), 0, "small files go up in one request");

    p.set_upload_chunk(100 * 1024);
    let big = pattern(250 * 1024);
    write(p.as_ref(), "/big.bin", WriteMode::CreateNew, &big).await.unwrap();
    assert_eq!(dbx.get("/big.bin").unwrap().content, big);
    assert_eq!(mock.requests("POST", "/content/files/upload_session/start"), 1);
    assert_eq!(mock.requests("POST", "/content/files/upload_session/append_v2"), 1);
    assert_eq!(mock.requests("POST", "/content/files/upload_session/finish"), 1);

    assert!(matches!(write(p.as_ref(), "/small.txt", WriteMode::CreateNew, b"x").await, Err(CxError::AlreadyExists(_))));
    write(p.as_ref(), "/small.txt", WriteMode::Truncate, b"overwritten").await.unwrap();
    assert_eq!(dbx.get("/small.txt").unwrap().content, b"overwritten");
    write(p.as_ref(), "/big.bin", WriteMode::Truncate, &pattern(1000)).await.unwrap();
    assert_eq!(dbx.get("/big.bin").unwrap().content, pattern(1000));
    assert!(matches!(p.open_write(&loc("/small.txt"), WriteMode::Append).await, Err(CxError::Unsupported(_))));
}

#[tokio::test]
async fn copy_space_and_modified() {
    let (dbx, _mock, p) = setup(100).await;
    dbx.add("/F", true, b"");
    dbx.add("/F/x.txt", false, b"x");
    assert!(p.copy_within(&loc("/F"), &loc("/G")).await.unwrap(), "folders copy server-side too");
    assert_eq!(dbx.get("/G/x.txt").unwrap().content, b"x");
    assert!(matches!(p.copy_within(&loc("/F"), &loc("/G")).await, Err(CxError::AlreadyExists(_))));
    let s = p.free_space(&loc("/")).await.unwrap().unwrap();
    assert_eq!((s.total, s.free), (10_000_000_000, 10_000_000_000 - 314_159_265));
    p.set_modified(&loc("/F/x.txt"), 0).await.unwrap(); // read-only on Dropbox: a no-op
}

#[tokio::test]
async fn expired_access_token_refreshes_and_retries() {
    let (dbx, mock, _) = setup(100).await;
    dbx.add("/a", false, b"");
    let saved: Arc<Mutex<Vec<cx_core::Credentials>>> = Arc::default();
    let hook_saved = saved.clone();
    let hook: cx_cloud::TokenHook = Arc::new(move |c| hook_saved.lock().unwrap().push(c));
    let creds = tokens(Service::Dropbox, ACCESS, 3_600_000).to_credentials("me@example.com");
    let p = cx_cloud::open(Service::Dropbox, &endpoint(), Some(creds), Some(&config(Service::Dropbox, &mock)), Some(hook)).await.unwrap().into_provider();

    // The server stops accepting the token: 401 → refresh → retry.
    mock.auth.revoke_all_access();
    assert_eq!(names(p.as_ref(), "/").await, ["a"]);
    assert_eq!(mock.auth.refreshes.load(Ordering::SeqCst), 1);
    let saved = saved.lock().unwrap();
    assert_eq!(saved.len(), 1, "refreshed tokens are handed to the store");
    assert_eq!(saved[0].user, "me@example.com");
    let t = Tokens::from_credentials(&saved[0]).unwrap();
    assert_eq!(t.access_token, "access-1");
    assert_eq!(t.refresh_token.as_deref(), Some(REFRESH), "the refresh token is kept");
}

#[tokio::test]
async fn refresh_failure_asks_for_sign_in() {
    let (_dbx, mock, p) = setup(100).await;
    mock.auth.revoke_all_access();
    mock.auth.refresh_ok.lock().unwrap().clear();
    match p.stat(&loc("/x")).await {
        Err(CxError::AuthRequired { user, reason, uri }) => {
            assert_eq!(user.as_deref(), Some("me@example.com"));
            assert!(reason.contains("sign in again"), "{reason}");
            assert_eq!(uri, endpoint().uri());
        }
        other => panic!("expected AuthRequired, got {other:?}"),
    }
}

#[tokio::test]
async fn connect_needs_client_tokens_and_a_live_grant() {
    let dbx = Arc::new(DropboxMock::new(100));
    let mock = serve(dbx, Arc::new(Auth::new(ACCESS, REFRESH))).await;
    let cfg = config(Service::Dropbox, &mock);
    let ep = endpoint();
    let auth_required = |r: cx_core::Result<cx_cloud::CloudProvider>| matches!(r, Err(CxError::AuthRequired { .. }));

    assert!(auth_required(cx_cloud::open(Service::Dropbox, &ep, None, Some(&cfg), None).await), "no tokens");
    let creds = tokens(Service::Dropbox, ACCESS, 3_600_000).to_credentials("me@example.com");
    assert!(auth_required(cx_cloud::open(Service::Dropbox, &ep, Some(creds.clone()), None, None).await), "no client id");
    let other = tokens(Service::GDrive, ACCESS, 3_600_000).to_credentials("me@example.com");
    assert!(auth_required(cx_cloud::open(Service::Dropbox, &ep, Some(other), Some(&cfg), None).await), "another service's tokens");

    // An expired access token is refreshed while connecting…
    let expired = tokens(Service::Dropbox, "stale", -1000).to_credentials("me@example.com");
    cx_cloud::open(Service::Dropbox, &ep, Some(expired.clone()), Some(&cfg), None).await.unwrap();
    assert_eq!(mock.auth.refreshes.load(Ordering::SeqCst), 1);
    // …and a revoked grant is AuthRequired right away.
    mock.auth.refresh_ok.lock().unwrap().clear();
    assert!(auth_required(cx_cloud::open(Service::Dropbox, &ep, Some(expired), Some(&cfg), None).await));
}

#[tokio::test]
async fn rate_limits_back_off_with_retry_after() {
    let (dbx, mock, p) = setup(100).await;
    dbx.add("/a", false, b"");
    mock.throttle.store(1, Ordering::SeqCst);
    *mock.retry_after.lock().unwrap() = "1".into();
    let t = Instant::now();
    assert_eq!(names(p.as_ref(), "/").await, ["a"]);
    assert!(t.elapsed() >= Duration::from_secs(1), "waited for Retry-After");
    assert_eq!(mock.requests("POST", "/api/files/list_folder"), 2);

    // Without Retry-After the policy's backoff applies; it gives up eventually.
    *mock.retry_after.lock().unwrap() = "0".into();
    mock.throttle.store(100, Ordering::SeqCst);
    let before = mock.requests("POST", "/api/files/get_metadata");
    assert!(matches!(p.stat(&loc("/a")).await, Err(CxError::Connection(_))));
    assert_eq!(mock.requests("POST", "/api/files/get_metadata") - before, 4, "one try + 3 retries");
}
