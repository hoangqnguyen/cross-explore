//! Google Drive provider against the in-process Drive mock.

mod common;

use common::gdrive_mock::{DriveMock, DOC, FOLDER, FORM, SHEET};
use common::*;
use cx_cloud::{CloudProvider, GDriveProvider, Service};
use cx_core::provider::list_all;
use cx_core::{Connector, CredentialStore, CxError, EntryKind, MemoryCredentials, Provider, WriteMode};
use std::sync::Arc;

async fn setup() -> (Arc<DriveMock>, Mock, Arc<GDriveProvider>) {
    let drive = Arc::new(DriveMock::default());
    let mock = serve(drive.clone(), Arc::new(Auth::new(ACCESS, REFRESH))).await;
    let CloudProvider::GDrive(p) = connect(Service::GDrive, &mock).await else { panic!("not a Drive provider") };
    (drive, mock, p)
}

#[tokio::test]
async fn virtual_roots() {
    let (drive, _mock, p) = setup().await;
    drive.st.lock().unwrap().drives.push(("drive1".into(), "Team".into()));
    drive.add("drive1", "handbook.pdf", "application/pdf", b"pdf");
    drive.add_shared("From Alice.txt", "text/plain");

    assert_eq!(names(p.as_ref(), "/").await, ["My Drive", "Shared drives", "Shared with me"]);
    assert_eq!(names(p.as_ref(), "/Shared drives").await, ["Team"]);
    assert_eq!(names(p.as_ref(), "/Shared drives/Team").await, ["handbook.pdf"]);
    assert_eq!(read(p.as_ref(), "/Shared drives/Team/handbook.pdf", 0).await, b"pdf");
    assert_eq!(names(p.as_ref(), "/Shared with me").await, ["From Alice.txt"]);
    assert!(p.stat(&loc("/My Drive")).await.unwrap().is_dir);
    assert!(matches!(p.stat(&loc("/Elsewhere")).await, Err(CxError::NotFound(_))));
    // Shared-drive listings are scoped to the drive.
    let scoped = mock_query_has(&_mock, "driveId", "drive1");
    assert!(scoped);
}

fn mock_query_has(mock: &Mock, k: &str, v: &str) -> bool {
    mock.log.lock().unwrap().iter().any(|l| l.query.iter().any(|(a, b)| a == k && b == v))
}

#[tokio::test]
async fn listing_paginates_small_first_page() {
    let (drive, mock, p) = setup().await;
    for i in 0..250 {
        drive.add("root", &format!("f{i:03}.txt"), "text/plain", b"x");
    }
    let got = list_all(p.as_ref(), &loc("/My Drive")).await.unwrap();
    assert_eq!(got.len(), 250);
    let sizes: Vec<String> = mock.log.lock().unwrap().iter().filter(|l| l.path == "/api/files").filter_map(|l| l.query.iter().find(|(k, _)| k == "pageSize").map(|(_, v)| v.clone())).collect();
    assert_eq!(sizes, ["100", "1000"], "first page small, then big pages");
}

#[tokio::test]
async fn duplicates_native_docs_and_odd_names() {
    let (drive, _mock, p) = setup().await;
    let r1 = drive.add("root", "report.pdf", "application/pdf", b"first");
    let r2 = drive.add("root", "report.pdf", "application/pdf", b"second");
    let _r3 = drive.add("root", "report.pdf", "application/pdf", b"third");
    drive.add("root", "Plan", DOC, b"");
    drive.add("root", "Budget", SHEET, b"");
    drive.add("root", "Survey", FORM, b"");
    drive.add("root", "a/b", "text/plain", b"slash");
    drive.add("root", ".hidden", "text/plain", b"");

    let e = entries(p.as_ref(), "/My Drive").await;
    let mut n: Vec<&str> = e.keys().map(String::as_str).collect();
    n.sort();
    assert_eq!(n, [".hidden", "Budget.xlsx", "Plan.docx", "Survey", "a\u{2215}b", "report (2).pdf", "report (3).pdf", "report.pdf"]);
    assert_eq!(e["Plan.docx"].size, 0, "native docs have no size");
    assert!(!e["Plan.docx"].readonly);
    assert!(e["Survey"].readonly, "non-exportable natives are read-only");
    assert!(e[".hidden"].hidden);
    assert_eq!(e["report.pdf"].size, 5);

    // Suffixed names map back to the right file, in creation order.
    assert_eq!(read(p.as_ref(), "/My Drive/report.pdf", 0).await, b"first");
    assert_eq!(read(p.as_ref(), "/My Drive/report (2).pdf", 0).await, b"second");
    assert_eq!(read(p.as_ref(), "/My Drive/report (3).pdf", 0).await, b"third");
    assert_eq!(read(p.as_ref(), "/My Drive/a\u{2215}b", 0).await, b"slash");
    let _ = (r1, r2);

    // A fresh connection (empty cache) resolves the same names.
    p.clear_cache();
    assert_eq!(read(p.as_ref(), "/My Drive/report (2).pdf", 0).await, b"second");
    let st = p.stat(&loc("/My Drive/report (3).pdf")).await.unwrap();
    assert_eq!((st.name.as_str(), st.size), ("report (3).pdf", 5));

    // Google Docs are exported as Office files.
    let docx = read(p.as_ref(), "/My Drive/Plan.docx", 0).await;
    assert_eq!(String::from_utf8(docx).unwrap(), "EXPORT(application/vnd.openxmlformats-officedocument.wordprocessingml.document):Plan");
    let xlsx = read(p.as_ref(), "/My Drive/Budget.xlsx", 7).await;
    assert!(String::from_utf8(xlsx).unwrap().starts_with("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"), "offset skips into the export");
    assert!(matches!(p.open_read(&loc("/My Drive/Survey"), 0).await, Err(CxError::Unsupported(_))));
}

#[tokio::test]
async fn nested_paths_stat_and_range_reads() {
    let (drive, _mock, p) = setup().await;
    let a = drive.add("root", "A", FOLDER, b"");
    let b = drive.add(&a, "B", FOLDER, b"");
    drive.add(&b, "deep.bin", "application/octet-stream", &pattern(1000));
    let st = p.stat(&loc("/My Drive/A/B/deep.bin")).await.unwrap();
    assert_eq!((st.size, st.kind, st.is_dir), (1000, EntryKind::File, false));
    assert!(st.modified.is_some());
    assert!(p.stat(&loc("/My Drive/A/B")).await.unwrap().is_dir);
    assert_eq!(read(p.as_ref(), "/My Drive/A/B/deep.bin", 600).await, pattern(1000)[600..]);
    assert!(read(p.as_ref(), "/My Drive/A/B/deep.bin", 1000).await.is_empty());
    assert!(matches!(p.stat(&loc("/My Drive/A/nope")).await, Err(CxError::NotFound(_))));
    assert!(matches!(list_all(p.as_ref(), &loc("/My Drive/A/B/deep.bin")).await, Err(CxError::InvalidLocation(_))));
}

#[tokio::test]
async fn mkdir_auto_naming() {
    let (drive, _mock, p) = setup().await;
    let e1 = p.create_dir(&loc("/My Drive"), None).await.unwrap();
    let e2 = p.create_dir(&loc("/My Drive"), None).await.unwrap();
    assert_eq!((e1.name.as_str(), e2.name.as_str()), ("New folder", "New folder (2)"));
    assert!(e1.is_dir);
    assert!(matches!(p.create_dir(&loc("/My Drive"), Some("New folder")).await, Err(CxError::AlreadyExists(_))));
    let docs = p.create_dir(&loc("/My Drive"), Some("Docs")).await.unwrap();
    assert_eq!(docs.name, "Docs");
    assert_eq!(drive.find("root", "Docs")[0].mime, FOLDER);
    // Immediately usable.
    p.create_dir(&loc("/My Drive/Docs"), Some("Inner")).await.unwrap();
    assert_eq!(names(p.as_ref(), "/My Drive/Docs").await, ["Inner"]);
    assert!(matches!(p.create_dir(&loc("/"), Some("x")).await, Err(CxError::PermissionDenied(_))));
}

#[tokio::test]
async fn rename_and_move_never_overwrite() {
    let (drive, _mock, p) = setup().await;
    let dst = drive.add("root", "Dest", FOLDER, b"");
    let f = drive.add("root", "a.txt", "text/plain", b"a");
    drive.add("root", "b.txt", "text/plain", b"b");
    drive.add(&dst, "taken.txt", "text/plain", b"t");
    let doc = drive.add("root", "Plan", DOC, b"");

    let e = p.rename(&loc("/My Drive"), "a.txt", "c.txt").await.unwrap();
    assert_eq!(e.name, "c.txt");
    assert_eq!(drive.file(&f).name, "c.txt");
    assert!(matches!(p.rename(&loc("/My Drive"), "c.txt", "b.txt").await, Err(CxError::AlreadyExists(_))));

    p.move_to(&loc("/My Drive/c.txt"), &loc("/My Drive/Dest/c.txt")).await.unwrap();
    assert_eq!(drive.file(&f).parents, vec![dst.clone()], "old parent removed, new added");
    assert_eq!(names(p.as_ref(), "/My Drive/Dest").await, ["c.txt", "taken.txt"]);
    assert!(!names(p.as_ref(), "/My Drive").await.contains(&"c.txt".to_string()));
    assert!(matches!(p.move_to(&loc("/My Drive/b.txt"), &loc("/My Drive/Dest/taken.txt")).await, Err(CxError::AlreadyExists(_))));
    assert!(matches!(p.move_to(&loc("/My Drive/Dest"), &loc("/My Drive/Dest/x")).await, Err(CxError::InvalidLocation(_))));

    // Renaming a Google Doc keeps the export extension off its Drive name.
    p.rename(&loc("/My Drive"), "Plan.docx", "Plan 2.docx").await.unwrap();
    assert_eq!(drive.file(&doc).name, "Plan 2");
    assert!(names(p.as_ref(), "/My Drive").await.contains(&"Plan 2.docx".to_string()));
}

#[tokio::test]
async fn delete_and_trash_with_restore() {
    let (drive, _mock, p) = setup().await;
    let gone = drive.add("root", "gone.txt", "text/plain", b"g");
    let tr = drive.add("root", "trash-me.txt", "text/plain", b"t");
    names(p.as_ref(), "/My Drive").await;

    p.remove(&loc("/My Drive/gone.txt")).await.unwrap();
    assert!(!drive.st.lock().unwrap().files.contains_key(&gone), "permanently deleted");
    assert!(matches!(p.stat(&loc("/My Drive/gone.txt")).await, Err(CxError::NotFound(_))));

    assert!(p.capabilities().trash);
    let items = p.trash(&loc("/My Drive"), &["trash-me.txt".to_string()]).await.unwrap();
    assert_eq!(items.len(), 1);
    assert!(drive.file(&tr).trashed);
    assert_eq!(cx_cloud::trash_marker_id(items[0].trashed.as_deref().unwrap()), Some(tr.as_str()));
    assert_eq!(names(p.as_ref(), "/My Drive").await, Vec::<String>::new());

    p.restore(&items).await.unwrap();
    assert!(!drive.file(&tr).trashed);
    assert_eq!(names(p.as_ref(), "/My Drive").await, ["trash-me.txt"]);
    assert!(matches!(p.remove(&loc("/My Drive")).await, Err(CxError::Unsupported(_))));
}

#[tokio::test]
async fn uploads_small_and_resumable() {
    let (drive, mock, p) = setup().await;
    drive.add("root", "Up", FOLDER, b"");

    // Small: one session, one PUT with the total.
    write(p.as_ref(), "/My Drive/Up/small.txt", WriteMode::CreateNew, b"hello").await.unwrap();
    let small = drive.find(&drive.find("root", "Up")[0].id, "small.txt");
    assert_eq!(small[0].content, b"hello");
    assert_eq!(drive.st.lock().unwrap().chunk_puts, ["bytes 0-4/5"]);

    // Empty file.
    write(p.as_ref(), "/My Drive/Up/empty.txt", WriteMode::CreateNew, b"").await.unwrap();
    assert_eq!(drive.st.lock().unwrap().chunk_puts.last().unwrap(), "bytes */0");

    // Resumable in 256 KiB chunks: 700 KiB → 3 PUTs.
    p.set_upload_chunk(256 * 1024);
    drive.st.lock().unwrap().chunk_puts.clear();
    let big = pattern(700 * 1024);
    write(p.as_ref(), "/My Drive/Up/big.bin", WriteMode::CreateNew, &big).await.unwrap();
    let puts = drive.st.lock().unwrap().chunk_puts.clone();
    assert_eq!(puts, ["bytes 0-262143/*", "bytes 262144-524287/*", "bytes 524288-716799/716800"]);
    assert_eq!(read(p.as_ref(), "/My Drive/Up/big.bin", 0).await, big);
    assert_eq!(p.stat(&loc("/My Drive/Up/big.bin")).await.unwrap().size, big.len() as u64);

    // No overwrite for CreateNew; Truncate replaces the same file.
    assert!(matches!(write(p.as_ref(), "/My Drive/Up/small.txt", WriteMode::CreateNew, b"x").await, Err(CxError::AlreadyExists(_))));
    let id = small[0].id.clone();
    write(p.as_ref(), "/My Drive/Up/small.txt", WriteMode::Truncate, b"replaced").await.unwrap();
    assert_eq!(drive.file(&id).content, b"replaced");
    assert_eq!(mock.requests("PATCH", "/content/files/"), 1, "replacing updates the file's content");
    assert!(matches!(p.open_write(&loc("/My Drive/Up/small.txt"), WriteMode::Append).await, Err(CxError::Unsupported(_))));
}

#[tokio::test]
async fn copy_modified_and_quota() {
    let (drive, _mock, p) = setup().await;
    let dir = drive.add("root", "Dir", FOLDER, b"");
    drive.add("root", "src.txt", "text/plain", b"copy me");
    drive.add("root", "Plan", DOC, b"");

    assert!(p.copy_within(&loc("/My Drive/src.txt"), &loc("/My Drive/Dir/dst.txt")).await.unwrap());
    assert_eq!(drive.find(&dir, "dst.txt")[0].content, b"copy me");
    assert!(p.copy_within(&loc("/My Drive/Plan.docx"), &loc("/My Drive/Dir/Plan copy.docx")).await.unwrap());
    assert_eq!(drive.find(&dir, "Plan copy").len(), 1, "native copy keeps a Drive name without extension");
    assert!(!p.copy_within(&loc("/My Drive/Dir"), &loc("/My Drive/Dir2")).await.unwrap(), "folders are walked by the caller");
    assert!(matches!(p.copy_within(&loc("/My Drive/src.txt"), &loc("/My Drive/Dir/dst.txt")).await, Err(CxError::AlreadyExists(_))));

    let ms = 1_700_000_000_123;
    p.set_modified(&loc("/My Drive/src.txt"), ms).await.unwrap();
    assert_eq!(drive.find("root", "src.txt")[0].modified, "2023-11-14T22:13:20.123Z");
    assert_eq!(p.stat(&loc("/My Drive/src.txt")).await.unwrap().modified, Some(ms));

    let space = p.free_space(&loc("/My Drive")).await.unwrap().unwrap();
    assert_eq!((space.total, space.free), (16_106_127_360, 10_000_000_000));
    assert!(p.free_space(&loc("/Shared with me")).await.unwrap().is_none());
}

#[tokio::test]
async fn drive_403_rate_limit_is_retried() {
    let (drive, mock, p) = setup().await;
    drive.add("root", "a.txt", "text/plain", b"a");
    drive.st.lock().unwrap().rate_limit_403 = 2;
    assert_eq!(names(p.as_ref(), "/My Drive").await, ["a.txt"]);
    assert_eq!(mock.requests("GET", "/api/files"), 3);
}

#[tokio::test]
async fn connector_opens_saves_refreshed_tokens_and_restores_trash() {
    let drive = Arc::new(DriveMock::default());
    let mock = serve(drive.clone(), Arc::new(Auth::new(ACCESS, REFRESH))).await;
    let store = Arc::new(MemoryCredentials::default());
    let conn = cx_cloud::CloudConnector::new(Service::GDrive, cx_core::Scheme::Davs).with_client(Some(config(Service::GDrive, &mock))).with_store(store.clone());
    assert_eq!(conn.scheme(), cx_core::Scheme::Davs);
    assert!(matches!(conn.connect(&endpoint(), None).await, Err(CxError::AuthRequired { .. })));

    // Expired tokens: refreshed on connect and saved back to the store.
    let creds = tokens(Service::GDrive, "stale", -1).to_credentials("me@example.com");
    let p = conn.connect(&endpoint(), Some(creds)).await.unwrap();
    let saved = cx_cloud::Tokens::from_credentials(&store.get(&endpoint()).unwrap()).unwrap();
    assert_eq!(saved.access_token, "access-1");

    let id = drive.add("root", "oops.txt", "text/plain", b"o");
    let items = p.trash(&loc("/My Drive"), &["oops.txt".into()]).await.unwrap();
    assert!(drive.file(&id).trashed);
    conn.restore_trashed(&items).await.unwrap();
    assert!(!drive.file(&id).trashed);
    assert_eq!(names(p.as_ref(), "/My Drive").await, ["oops.txt"]);
}
