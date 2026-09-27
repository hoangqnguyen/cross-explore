//! OneDrive provider against the in-process Microsoft Graph mock.

mod common;

use common::onedrive_mock::GraphMock;
use common::*;
use cx_cloud::{CloudProvider, OneDriveProvider, Service};
use cx_core::provider::list_all;
use cx_core::{CxError, Provider, WriteMode};
use std::sync::Arc;

async fn setup(page: usize) -> (Arc<GraphMock>, Mock, Arc<OneDriveProvider>) {
    let graph = Arc::new(GraphMock::new(page));
    let mock = serve(graph.clone(), Arc::new(Auth::new(ACCESS, REFRESH))).await;
    let CloudProvider::OneDrive(p) = connect(Service::OneDrive, &mock).await else { panic!("not OneDrive") };
    (graph, mock, p)
}

#[tokio::test]
async fn listing_follows_next_links() {
    let (g, mock, p) = setup(4).await;
    g.add("/Docs", true, b"");
    for i in 0..9 {
        g.add(&format!("/Docs/f{i}.txt"), false, b"abc");
    }
    g.add("/Docs/.hidden", false, b"");
    g.add("/Docs/My Folder #1", true, b"");
    let e = entries(p.as_ref(), "/Docs").await;
    assert_eq!(e.len(), 11);
    assert_eq!(mock.requests("GET", "/api/me/drive/root:/Docs:/children"), 3);
    assert!(e["My Folder #1"].is_dir && e[".hidden"].hidden);
    assert_eq!(e["f1.txt"].size, 3);
    assert!(e["f1.txt"].modified.is_some() && e["f1.txt"].created.is_some());
    assert_eq!(names(p.as_ref(), "/").await, ["Docs"]);
    // Odd characters survive path addressing.
    g.add("/Docs/My Folder #1/x%y?.txt", false, b"odd");
    assert_eq!(names(p.as_ref(), "/Docs/My Folder #1").await, ["x%y?.txt"]);
    assert_eq!(read(p.as_ref(), "/Docs/My Folder #1/x%y?.txt", 0).await, b"odd");
    assert!(matches!(list_all(p.as_ref(), &loc("/Nope")).await, Err(CxError::NotFound(_))));
    assert!(matches!(list_all(p.as_ref(), &loc("/Docs/f1.txt")).await, Err(CxError::InvalidLocation(_))));
}

#[tokio::test]
async fn stat_mkdir_rename_move_delete() {
    let (g, _mock, p) = setup(100).await;
    g.add("/a.txt", false, b"aaa");
    g.add("/b.txt", false, b"b");
    g.add("/Dir", true, b"");
    assert!(p.stat(&loc("/")).await.unwrap().is_dir);
    assert_eq!(p.stat(&loc("/a.txt")).await.unwrap().size, 3);
    assert!(matches!(p.stat(&loc("/zzz")).await, Err(CxError::NotFound(_))));

    assert_eq!(p.create_dir(&loc("/Dir"), None).await.unwrap().name, "New folder");
    assert_eq!(p.create_dir(&loc("/Dir"), None).await.unwrap().name, "New folder (2)");
    assert!(matches!(p.create_dir(&loc("/"), Some("Dir")).await, Err(CxError::AlreadyExists(_))));

    assert!(matches!(p.rename(&loc("/"), "a.txt", "b.txt").await, Err(CxError::AlreadyExists(_))));
    assert_eq!(p.rename(&loc("/"), "a.txt", "c.txt").await.unwrap().name, "c.txt");
    p.move_to(&loc("/c.txt"), &loc("/Dir/c.txt")).await.unwrap();
    assert_eq!(g.get("/Dir/c.txt").unwrap().content, b"aaa");
    assert!(g.get("/c.txt").is_none());
    p.move_to(&loc("/Dir"), &loc("/Moved")).await.unwrap();
    assert!(g.get("/Moved/New folder (2)").is_some(), "folders move with their contents");

    p.remove(&loc("/Moved")).await.unwrap();
    assert!(g.get("/Moved/c.txt").is_none());
    assert!(matches!(p.remove(&loc("/Moved")).await, Err(CxError::NotFound(_))));
}

#[tokio::test]
async fn downloads_follow_redirect_without_token() {
    let (g, mock, p) = setup(100).await;
    g.add("/big.bin", false, &pattern(9000));
    assert_eq!(read(p.as_ref(), "/big.bin", 0).await, pattern(9000));
    assert_eq!(read(p.as_ref(), "/big.bin", 8000).await, pattern(9000)[8000..]);
    let log = mock.log.lock().unwrap();
    let dl: Vec<_> = log.iter().filter(|l| l.path.starts_with("/download/")).collect();
    assert_eq!(dl.len(), 2);
    assert!(dl.iter().all(|l| !l.headers.contains_key("authorization")));
    assert_eq!(dl[1].headers.get("range").map(String::as_str), Some("bytes=8000-"));
}

#[tokio::test]
async fn uploads_simple_and_session() {
    let (g, mock, p) = setup(100).await;
    write(p.as_ref(), "/small.txt", WriteMode::CreateNew, b"small").await.unwrap();
    assert_eq!(g.get("/small.txt").unwrap().content, b"small");
    assert_eq!(mock.requests("PUT", "/api/me/drive/root:/small.txt:/content"), 1);

    p.set_upload_limits(100 * 1024, 320 * 1024);
    let big = pattern(800 * 1024);
    write(p.as_ref(), "/big.bin", WriteMode::CreateNew, &big).await.unwrap();
    assert_eq!(g.get("/big.bin").unwrap().content, big);
    assert_eq!(g.st.lock().unwrap().chunk_ranges, ["bytes 0-327679/819200", "bytes 327680-655359/819200", "bytes 655360-819199/819200"]);
    assert!(mock.log.lock().unwrap().iter().filter(|l| l.path.starts_with("/upload/")).all(|l| !l.headers.contains_key("authorization")));

    assert!(matches!(write(p.as_ref(), "/small.txt", WriteMode::CreateNew, b"x").await, Err(CxError::AlreadyExists(_))));
    write(p.as_ref(), "/small.txt", WriteMode::Truncate, b"replaced").await.unwrap();
    assert_eq!(g.get("/small.txt").unwrap().content, b"replaced");
    write(p.as_ref(), "/big.bin", WriteMode::Truncate, &pattern(300 * 1024)).await.unwrap();
    assert_eq!(g.get("/big.bin").unwrap().content, pattern(300 * 1024));
    assert!(matches!(p.open_write(&loc("/x"), WriteMode::Append).await, Err(CxError::Unsupported(_))));
}

#[tokio::test]
async fn copy_waits_for_monitor_and_metadata() {
    let (g, mock, p) = setup(100).await;
    g.add("/F", true, b"");
    g.add("/F/x.txt", false, b"x");
    g.add("/Dest", true, b"");
    assert!(p.copy_within(&loc("/F"), &loc("/Dest/F copy")).await.unwrap());
    assert_eq!(g.get("/Dest/F copy/x.txt").unwrap().content, b"x");
    assert_eq!(mock.requests("GET", "/monitor/"), 2, "polled until completed");
    assert!(matches!(p.copy_within(&loc("/F"), &loc("/Dest/F copy")).await, Err(CxError::AlreadyExists(_))));

    let ms = 1_700_000_000_000;
    p.set_modified(&loc("/F/x.txt"), ms).await.unwrap();
    assert_eq!(g.get("/F/x.txt").unwrap().modified, "2023-11-14T22:13:20.000Z");
    assert_eq!(p.stat(&loc("/F/x.txt")).await.unwrap().modified, Some(ms));

    let s = p.free_space(&loc("/")).await.unwrap().unwrap();
    assert_eq!((s.total, s.free), (5_368_709_120, 4_294_967_296));
}
