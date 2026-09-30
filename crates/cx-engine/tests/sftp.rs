//! The engine against the OpenSSH server in `docker/sftp`: unknown host
//! key → trust → sign-in required → connect with a password → browse,
//! upload, polled watch, compare, disconnect.
//!
//! Skipped unless `CX_TEST_SFTP=1`. Start the server with
//! `docker compose -f docker/sftp/compose.yml up -d` (after
//! `docker/test-remote.sh sftp --keep` created its keys).

use cx_core::{Change, Credentials, CxError};
use cx_engine::{Engine, EngineConfig, ListEvent, PollConfig, SubmitRequest, WatchMode};
use std::time::Duration;

const BASE: &str = "sftp://cx@127.0.0.1:2222/upload";

fn enabled() -> bool {
    let on = std::env::var("CX_TEST_SFTP").is_ok_and(|v| v == "1");
    if !on {
        eprintln!("skipped: set CX_TEST_SFTP=1 with docker/sftp running");
    }
    on
}

#[tokio::test(flavor = "multi_thread")]
async fn sftp_through_the_engine() {
    if !enabled() {
        return;
    }
    let state = tempfile::tempdir().unwrap();
    let mut cfg = EngineConfig::isolated(state.path());
    cfg.poll = PollConfig {
        min: Duration::from_millis(200),
        max: Duration::from_millis(800),
    };
    let engine = Engine::new(cfg).unwrap();

    // First contact: the key is unknown and must be reviewed.
    let err = engine.connect_server(BASE, None, false).await.unwrap_err();
    let CxError::HostKeyUnknown {
        uri,
        key_type,
        fingerprint,
        changed,
        ..
    } = err
    else {
        panic!("expected an unknown host key, got {err:?}")
    };
    assert!(!changed);
    assert!(fingerprint.starts_with("SHA256:"), "{fingerprint}");
    // The failure is remembered: asking again doesn't dial.
    assert!(matches!(
        engine.connect_server(BASE, None, false).await,
        Err(CxError::HostKeyUnknown { .. })
    ));
    engine
        .trust_host_key(
            if uri.is_empty() { BASE } else { &uri },
            &key_type,
            &fingerprint,
        )
        .unwrap();
    assert!(state.path().join("data/known_hosts").exists());

    // Trusted now; a wrong password is an auth error, the right one works.
    let bad = engine
        .connect_server(BASE, Some(Credentials::password("cx", "nope")), false)
        .await;
    assert!(matches!(bad, Err(CxError::AuthRequired { .. })), "{bad:?}");
    engine
        .connect_server(BASE, Some(Credentials::password("cx", "cxpass")), false)
        .await
        .unwrap();
    assert!(engine
        .connections()
        .iter()
        .any(|c| c.starts_with("sftp://cx@127.0.0.1:2222")));

    // A scratch folder.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let made = engine
        .create_folder(BASE, Some(&format!("engine-{stamp}")))
        .await
        .unwrap();
    let dir = format!("{BASE}/{}", made.name);

    // Upload a local file with a copy job.
    let local = tempfile::tempdir().unwrap();
    std::fs::write(local.path().join("hello.txt"), b"hello over ssh").unwrap();
    let src = cx_core::Location::local(local.path().join("hello.txt")).uri();
    let id = engine
        .submit(SubmitRequest::new("copy", vec![src], Some(dir.clone())))
        .unwrap();
    assert_eq!(engine.wait_job(id).await.unwrap().state, "done");

    // Listing streams, and remembers the baseline for the poller.
    let mut names = Vec::new();
    engine
        .list_dir(&dir, |e| {
            if let ListEvent::Batch { entries } = e {
                names.extend(entries.into_iter().map(|e| e.name));
            }
            true
        })
        .await
        .unwrap();
    assert_eq!(names, vec!["hello.txt".to_string()]);
    let text = engine
        .preview_text(&format!("{dir}/hello.txt"), 1024)
        .await
        .unwrap();
    assert_eq!(text.text, "hello over ssh");

    // SFTP can't push changes: the watch polls, diffing against that listing.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<Change>>();
    let w = engine
        .watch_dir_with(&dir, move |c| drop(tx.send(c)))
        .await
        .unwrap();
    assert_eq!(w.mode, WatchMode::Polling);
    engine
        .rename(&dir, "hello.txt", "renamed.txt")
        .await
        .unwrap();
    let saw = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let batch = rx.recv().await.unwrap();
            if batch
                .iter()
                .any(|c| matches!(c, Change::Upsert { entry } if entry.name == "renamed.txt"))
            {
                return true;
            }
        }
    })
    .await
    .unwrap_or(false);
    assert!(saw, "polling reported the rename");
    engine.unwatch_dir(w.id);

    // Compare with a local folder, then clean up and disconnect.
    let diff = engine
        .compare_dirs(&cx_core::Location::local(local.path()).uri(), &dir, false)
        .await
        .unwrap();
    assert_eq!(
        diff.len(),
        2,
        "hello.txt only left, renamed.txt only right: {diff:?}"
    );
    let id = engine
        .submit(SubmitRequest::new("delete", vec![dir.clone()], None))
        .unwrap();
    assert_eq!(engine.wait_job(id).await.unwrap().state, "done");
    engine.disconnect_server(BASE).await.unwrap();
    assert!(engine.connections().is_empty());
}
