//! Two peer services on 127.0.0.1 with separate state dirs talking over
//! real QUIC.

use cx_core::provider::list_all;
use cx_core::{Change, CxError, Endpoint, Location, Provider, Scheme, WriteMode};
use cx_peer::{OfferState, PeerConfig, PeerEvent, PeerProvider, PeerService, Share};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

struct Node {
    svc: PeerService,
    events: mpsc::UnboundedReceiver<PeerEvent>,
    state: tempfile::TempDir,
}

impl Node {
    fn addr(&self) -> String {
        format!("127.0.0.1:{}", self.svc.local_addr().unwrap().port())
    }

    fn port(&self) -> u16 {
        self.svc.local_addr().unwrap().port()
    }

    /// Wait for the first event matching `pred`, skipping others.
    async fn wait_for(&mut self, what: &str, pred: impl Fn(&PeerEvent) -> bool) -> PeerEvent {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(e)) if pred(&e) => return e,
                Ok(Some(_)) => {}
                _ => panic!("timed out waiting for {what}"),
            }
        }
    }
}

fn config(state: &Path, port: u16, shares: Vec<Share>) -> PeerConfig {
    let mut cfg = PeerConfig::new(state);
    cfg.bind = Some("127.0.0.1".parse().unwrap());
    cfg.port = port;
    cfg.shares = Some(shares);
    cfg
}

async fn start(cfg: PeerConfig) -> (PeerService, mpsc::UnboundedReceiver<PeerEvent>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let svc = PeerService::start(cfg, Arc::new(move |e| {
        let _ = tx.send(e);
    }))
    .await
    .unwrap();
    (svc, rx)
}

async fn node(name: &str, shares: Vec<Share>) -> Node {
    let state = tempfile::tempdir().unwrap();
    let mut cfg = config(state.path(), 0, shares);
    cfg.name = Some(name.into());
    let (svc, events) = start(cfg).await;
    Node { svc, events, state }
}

struct Setup {
    server: Node,
    client: Node,
    /// Real folder behind the "Docs" share (canonical).
    docs: PathBuf,
    /// Real folder behind the read-only "Media" share.
    media: PathBuf,
    _files: tempfile::TempDir,
}

impl Setup {
    fn loc(&self, path: &str) -> Location {
        Location::Remote { endpoint: self.endpoint(), path: path.to_string() }
    }

    fn endpoint(&self) -> Endpoint {
        Endpoint { scheme: Scheme::Peer, user: None, host: "127.0.0.1".into(), port: Some(self.server.port()) }
    }

    async fn provider(&self) -> Arc<PeerProvider> {
        self.client.svc.provider("127.0.0.1", Some(self.server.port())).await.unwrap()
    }
}

async fn paired() -> Setup {
    let files = tempfile::tempdir().unwrap();
    let root = files.path().canonicalize().unwrap();
    let (docs, media) = (root.join("docs"), root.join("media"));
    std::fs::create_dir_all(docs.join("sub")).unwrap();
    std::fs::create_dir_all(&media).unwrap();
    std::fs::write(docs.join("hello.txt"), b"hello world").unwrap();
    std::fs::write(media.join("song.mp3"), b"la la la").unwrap();
    std::fs::write(root.join("secret.txt"), b"top secret").unwrap();
    let shares = vec![
        Share { name: "Docs".into(), path: docs.clone(), read_only: false },
        Share { name: "Media".into(), path: media.clone(), read_only: true },
    ];
    let server = node("server", shares).await;
    let client = node("client", vec![]).await;
    let code = server.svc.start_pairing();
    client.svc.pair(&server.addr(), &code.code).await.unwrap();
    Setup { server, client, docs, media, _files: files }
}

#[tokio::test]
async fn pairing_with_right_wrong_and_expired_codes() {
    let mut server = node("server", vec![]).await;
    let client = node("client", vec![]).await;

    // No code shown yet.
    assert!(matches!(client.svc.pair(&server.addr(), "123456").await, Err(CxError::AuthRequired { .. })));

    let code = server.svc.start_pairing();
    let wrong = if code.code == "000000" { "000001" } else { "000000" };
    let err = client.svc.pair(&server.addr(), wrong).await.unwrap_err();
    assert!(matches!(err, CxError::AuthRequired { ref reason, .. } if reason.contains("wrong")), "{err:?}");
    assert!(server.svc.trusted_devices().is_empty());

    // The right code still works after a wrong guess, and only once.
    let dev = client.svc.pair(&server.addr(), &code.code).await.unwrap();
    assert_eq!(dev.device_id, server.svc.identity().device_id);
    assert_eq!(dev.name, "server");
    let on_server = server.svc.trusted_devices();
    assert_eq!(on_server.len(), 1);
    assert_eq!(on_server[0].device_id, client.svc.identity().device_id);
    assert_eq!(on_server[0].public_key, client.svc.identity().public_key);
    server.wait_for("pairing event", |e| matches!(e, PeerEvent::PairingCompleted { .. })).await;
    assert!(client.svc.pair(&server.addr(), &code.code).await.is_err());

    // Expired codes are refused.
    let state = tempfile::tempdir().unwrap();
    let mut cfg = config(state.path(), 0, vec![]);
    cfg.pairing_code_ttl = Duration::from_millis(50);
    let (short, _ev) = start(cfg).await;
    let code = short.start_pairing();
    tokio::time::sleep(Duration::from_millis(120)).await;
    let addr = format!("127.0.0.1:{}", short.local_addr().unwrap().port());
    assert!(matches!(client.svc.pair(&addr, &code.code).await, Err(CxError::AuthRequired { .. })));
}

#[tokio::test]
async fn untrusted_devices_are_rejected() {
    let s = paired().await;
    // A device the server never paired with: it does not know the server's
    // key either, so connecting reports an unknown host key.
    let stranger = node("stranger", vec![]).await;
    let err = stranger.svc.provider("127.0.0.1", Some(s.server.port())).await.err().unwrap();
    assert!(matches!(err, CxError::HostKeyUnknown { changed: false, .. }), "{err:?}");

    // A device whose pairing was revoked on the server is refused at once,
    // even though it still trusts the server.
    let p = s.provider().await;
    list_all(p.as_ref(), &s.loc("/Docs")).await.unwrap();
    assert!(s.server.svc.remove_trusted(&s.client.svc.identity().device_id).unwrap());
    tokio::time::sleep(Duration::from_millis(100)).await;
    let err = list_all(p.as_ref(), &s.loc("/Docs")).await.unwrap_err();
    assert!(matches!(err, CxError::AuthRequired { .. }), "{err:?}");
    let err = s.client.svc.provider("127.0.0.1", Some(s.server.port())).await.err().unwrap();
    assert!(matches!(err, CxError::AuthRequired { .. }), "{err:?}");
}

#[tokio::test]
async fn shares_and_file_operations() {
    let s = paired().await;
    let p = s.provider().await;
    assert!(p.capabilities().live_watch);

    let mut root: Vec<String> = list_all(p.as_ref(), &s.loc("/")).await.unwrap().into_iter().map(|e| e.name).collect();
    root.sort();
    assert_eq!(root, ["Docs", "Media"]);
    let shares = p.list_shares().await.unwrap();
    assert!(shares.iter().any(|s| s.name == "Media" && s.read_only));
    assert_eq!(p.stat(&s.loc("/Docs")).await.unwrap().name, "Docs");

    let docs = list_all(p.as_ref(), &s.loc("/Docs")).await.unwrap();
    let hello = docs.iter().find(|e| e.name == "hello.txt").unwrap();
    assert_eq!(hello.size, 11);
    assert!(docs.iter().any(|e| e.name == "sub" && e.is_dir));

    let st = p.stat(&s.loc("/Docs/hello.txt")).await.unwrap();
    assert_eq!(st.size, 11);
    assert!(matches!(p.stat(&s.loc("/Docs/nope")).await, Err(CxError::NotFound(_))));

    let d = p.create_dir(&s.loc("/Docs"), None).await.unwrap();
    assert_eq!(d.name, "New folder");
    assert!(s.docs.join("New folder").is_dir());
    let named = p.create_dir(&s.loc("/Docs"), Some("Reports")).await.unwrap();
    assert!(named.is_dir);

    let renamed = p.rename(&s.loc("/Docs"), "hello.txt", "greeting.txt").await.unwrap();
    assert_eq!(renamed.name, "greeting.txt");
    assert!(s.docs.join("greeting.txt").exists() && !s.docs.join("hello.txt").exists());
    assert!(matches!(p.rename(&s.loc("/Docs"), "greeting.txt", "Reports").await, Err(CxError::AlreadyExists(_))));
    p.move_to(&s.loc("/Docs/greeting.txt"), &s.loc("/Docs/sub/greeting.txt")).await.unwrap();
    assert!(s.docs.join("sub/greeting.txt").exists());

    assert!(p.copy_within(&s.loc("/Docs/sub"), &s.loc("/Docs/sub copy")).await.unwrap());
    assert_eq!(std::fs::read(s.docs.join("sub copy/greeting.txt")).unwrap(), b"hello world");
    assert_eq!(p.hash(&s.loc("/Docs/sub/greeting.txt")).await.unwrap(), blake3::hash(b"hello world").to_hex().to_string());

    p.set_modified(&s.loc("/Docs/sub/greeting.txt"), 1_000_000_000_000).await.unwrap();
    assert_eq!(p.stat(&s.loc("/Docs/sub/greeting.txt")).await.unwrap().modified, Some(1_000_000_000_000));

    p.remove(&s.loc("/Docs/sub copy")).await.unwrap();
    assert!(!s.docs.join("sub copy").exists());
    assert!(p.free_space(&s.loc("/Docs")).await.unwrap().is_some_and(|sp| sp.total > 0));

    // Share roots and "/" itself can't be removed or renamed.
    assert!(matches!(p.remove(&s.loc("/Docs")).await, Err(CxError::PermissionDenied(_))));
    assert!(p.remove(&s.loc("/")).await.is_err());

    // The server audited all of it.
    let log = std::fs::read_to_string(s.server.svc.audit_log_path()).unwrap();
    assert!(log.lines().any(|l| l.contains("\"op\":\"remove\"") && l.contains("/Docs/sub copy")));
}

#[tokio::test]
async fn traversal_and_symlink_escapes_are_refused() {
    let s = paired().await;
    let p = s.provider().await;
    // Location::parse would normalize "..", so send raw paths like a hostile client.
    for path in ["/Docs/../../etc", "/Docs/../secret.txt", "/Docs/sub/../../secret.txt", "/../Docs"] {
        let e = p.stat(&s.loc(path)).await.unwrap_err();
        assert!(matches!(e, CxError::PermissionDenied(_)), "{path}: {e:?}");
        assert!(p.open_read(&s.loc(path), 0).await.is_err(), "{path}");
    }
    #[cfg(unix)]
    {
        let outside = s.docs.parent().unwrap();
        std::os::unix::fs::symlink(outside, s.docs.join("escape")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret.txt"), s.docs.join("secret-link")).unwrap();
        for path in ["/Docs/escape", "/Docs/escape/secret.txt", "/Docs/secret-link"] {
            let e = p.stat(&s.loc(path)).await.unwrap_err();
            assert!(matches!(e, CxError::PermissionDenied(_)), "{path}: {e:?}");
            assert!(p.open_read(&s.loc(path), 0).await.is_err(), "{path}");
        }
        assert!(p.open_write(&s.loc("/Docs/escape/new.txt"), WriteMode::CreateNew).await.is_err());
        assert!(!outside.join("new.txt").exists());
        let e = list_all(p.as_ref(), &s.loc("/Docs/escape")).await.unwrap_err();
        assert!(matches!(e, CxError::PermissionDenied(_)), "{e:?}");
        // Listed, but without details about the outside target.
        let rows = list_all(p.as_ref(), &s.loc("/Docs")).await.unwrap();
        let link = rows.iter().find(|e| e.name == "secret-link").unwrap();
        assert_eq!(link.size, 0);
    }
    // Error messages never reveal where the share lives.
    let e = p.stat(&s.loc("/Docs/missing.txt")).await.unwrap_err();
    assert!(!e.to_string().contains(s.docs.to_str().unwrap()), "{e}");
}

#[tokio::test]
async fn read_only_share_refuses_writes() {
    let s = paired().await;
    let p = s.provider().await;
    let rows = list_all(p.as_ref(), &s.loc("/Media")).await.unwrap();
    assert!(rows.iter().all(|e| e.readonly));
    assert!(matches!(p.open_write(&s.loc("/Media/new.mp3"), WriteMode::CreateNew).await, Err(CxError::PermissionDenied(_))));
    assert!(matches!(p.create_dir(&s.loc("/Media"), None).await, Err(CxError::PermissionDenied(_))));
    assert!(matches!(p.remove(&s.loc("/Media/song.mp3")).await, Err(CxError::PermissionDenied(_))));
    assert!(matches!(p.rename(&s.loc("/Media"), "song.mp3", "x.mp3").await, Err(CxError::PermissionDenied(_))));
    assert!(matches!(p.move_to(&s.loc("/Docs/hello.txt"), &s.loc("/Media/hello.txt")).await, Err(CxError::PermissionDenied(_))));
    assert!(s.media.join("song.mp3").exists() && s.docs.join("hello.txt").exists());
    // Reading is fine.
    let mut r = p.open_read(&s.loc("/Media/song.mp3"), 3).await.unwrap();
    let mut out = String::new();
    r.read_to_string(&mut out).await.unwrap();
    assert_eq!(out, "la la");
}

// Both peers share this process: give them real threads, as two machines would have.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_write_read_round_trip_and_offsets() {
    const SIZE: usize = 50 << 20;
    let s = paired().await;
    let p = s.provider().await;
    let mut data = vec![0u8; SIZE];
    blake3::Hasher::new().update(b"seed").finalize_xof().fill(&mut data);

    let loc = s.loc("/Docs/big.bin");
    let t = Instant::now();
    let mut w = p.open_write(&loc, WriteMode::CreateNew).await.unwrap();
    for chunk in data.chunks(1 << 20) {
        w.write_all(chunk).await.unwrap();
    }
    w.shutdown().await.unwrap();
    let up = t.elapsed();
    assert_eq!(std::fs::metadata(s.docs.join("big.bin")).unwrap().len(), SIZE as u64);

    let t = Instant::now();
    let mut r = p.open_read(&loc, 0).await.unwrap();
    let mut back = Vec::with_capacity(SIZE);
    r.read_to_end(&mut back).await.unwrap();
    let down = t.elapsed();
    assert!(back == data, "downloaded bytes differ");
    let mbps = |d: Duration| SIZE as f64 / (1 << 20) as f64 / d.as_secs_f64();
    eprintln!("50 MiB over QUIC loopback: upload {:.0} MiB/s ({up:.2?}), download {:.0} MiB/s ({down:.2?})", mbps(up), mbps(down));

    // Offset reads.
    let off = 12_345_678;
    let mut r = p.open_read(&loc, off as u64).await.unwrap();
    let mut first = vec![0u8; 1000];
    r.read_exact(&mut first).await.unwrap();
    assert_eq!(first, data[off..off + 1000]);
    drop(r); // stops the rest of the stream

    assert_eq!(p.hash(&loc).await.unwrap(), blake3::hash(&data).to_hex().to_string());
    // CreateNew refuses to overwrite; Append continues.
    assert!(matches!(p.open_write(&loc, WriteMode::CreateNew).await, Err(CxError::AlreadyExists(_))));
    let mut w = p.open_write(&s.loc("/Docs/log.txt"), WriteMode::Append).await.unwrap();
    w.write_all(b"one ").await.unwrap();
    w.shutdown().await.unwrap();
    let mut w = p.open_write(&s.loc("/Docs/log.txt"), WriteMode::Append).await.unwrap();
    w.write_all(b"two").await.unwrap();
    w.shutdown().await.unwrap();
    assert_eq!(std::fs::read(s.docs.join("log.txt")).unwrap(), b"one two");
}

#[tokio::test]
async fn watch_pushes_server_side_changes() {
    let s = paired().await;
    let p = s.provider().await;
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<Change>>();
    let guard = p
        .watch(&s.loc("/Docs"), Arc::new(move |c| {
            let _ = tx.send(c);
        }))
        .await
        .unwrap()
        .expect("peer watch is live");
    tokio::time::sleep(Duration::from_millis(150)).await;

    let t = Instant::now();
    std::fs::write(s.docs.join("fresh.txt"), b"new").unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    'outer: loop {
        let batch = tokio::time::timeout_at(deadline, rx.recv()).await.expect("no change pushed").unwrap();
        for c in batch {
            if matches!(&c, Change::Upsert { entry } if entry.name == "fresh.txt") {
                break 'outer;
            }
        }
    }
    let latency = t.elapsed();
    eprintln!("watch latency: {latency:.2?}");
    assert!(latency < Duration::from_secs(1), "{latency:?}");

    std::fs::remove_file(s.docs.join("fresh.txt")).unwrap();
    let batch = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
    assert!(batch.iter().any(|c| matches!(c, Change::Remove { name } if name == "fresh.txt")), "{batch:?}");
    drop(guard);
}

async fn wait_offer(node: &mut Node, state: OfferState) -> PeerEvent {
    node.wait_for("offer progress", |e| matches!(e, PeerEvent::OfferProgress { state: st, .. } if *st == state)).await
}

#[tokio::test]
async fn send_offer_accept_and_decline() {
    let mut s = paired().await;
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("photo.jpg");
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&file, &data).unwrap();
    let dest = tempfile::tempdir().unwrap();
    std::fs::write(dest.path().join("photo.jpg"), b"already here").unwrap();

    let id = s.client.svc.send_offer(&s.server.addr(), vec![file.clone()]).await.unwrap();
    let ev = s.server.wait_for("incoming offer", |e| matches!(e, PeerEvent::IncomingOffer { .. })).await;
    let PeerEvent::IncomingOffer { offer_id, from, files, total } = ev else { unreachable!() };
    assert_eq!(offer_id, id);
    assert_eq!(from.name, "client");
    assert_eq!(files[0].name, "photo.jpg");
    assert_eq!(total, data.len() as u64);
    let json = serde_json::to_value(PeerEvent::IncomingOffer { offer_id: id.clone(), from, files, total }).unwrap();
    assert_eq!(json["type"], "incomingOffer");

    s.server.svc.accept_offer(&id, dest.path().to_path_buf()).unwrap();
    let done = wait_offer(&mut s.server, OfferState::Completed).await;
    let PeerEvent::OfferProgress { saved, bytes, .. } = done else { unreachable!() };
    assert_eq!(bytes, data.len() as u64);
    let saved = PathBuf::from(&saved[0]);
    assert_eq!(saved.file_name().unwrap(), "photo (2).jpg");
    assert_eq!(std::fs::read(&saved).unwrap(), data);
    assert_eq!(std::fs::read(dest.path().join("photo.jpg")).unwrap(), b"already here");
    wait_offer(&mut s.client, OfferState::Completed).await;

    // Declined offers transfer nothing.
    let id = s.client.svc.send_offer(&s.server.addr(), vec![file]).await.unwrap();
    s.server.wait_for("incoming offer", |e| matches!(e, PeerEvent::IncomingOffer { offer_id, .. } if *offer_id == id)).await;
    s.server.svc.decline_offer(&id).unwrap();
    wait_offer(&mut s.client, OfferState::Declined).await;
    assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 2);
    assert!(s.server.svc.accept_offer(&id, dest.path().to_path_buf()).is_err());
}

#[tokio::test]
async fn reconnects_after_server_restart() {
    let mut s = paired().await;
    let p = s.provider().await;
    list_all(p.as_ref(), &s.loc("/Docs")).await.unwrap();
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<Change>>();
    let _guard = p
        .watch(&s.loc("/Docs"), Arc::new(move |c| {
            let _ = tx.send(c);
        }))
        .await
        .unwrap();

    let port = s.server.port();
    let shares = s.server.svc.shares();
    s.server.svc.stop().await;
    assert!(list_all(p.as_ref(), &s.loc("/Docs")).await.is_err(), "server is down");

    // Same identity (state dir) and port: the old provider just works again.
    let (svc, events) = start(config(s.server.state.path(), port, shares)).await;
    s.server.svc = svc;
    s.server.events = events;
    let rows = list_all(p.as_ref(), &s.loc("/Docs")).await.unwrap();
    assert!(rows.iter().any(|e| e.name == "hello.txt"));

    // The watch re-subscribed and asked for a re-list.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let batch = tokio::time::timeout_at(deadline, rx.recv()).await.expect("watch did not resume").unwrap();
        if batch.contains(&Change::Reset) {
            break;
        }
    }
    std::fs::write(s.docs.join("after-restart.txt"), b"x").unwrap();
    loop {
        let batch = tokio::time::timeout_at(deadline, rx.recv()).await.expect("no change after restart").unwrap();
        if batch.iter().any(|c| matches!(c, Change::Upsert { entry } if entry.name == "after-restart.txt")) {
            break;
        }
    }
}
