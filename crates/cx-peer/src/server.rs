//! Serving shares to other devices.
//!
//! A connection's identity is its TLS key. Every request is authorized
//! against the trust store *at the time of the request*, so removing a
//! device takes effect immediately. Unpaired devices may only say hello and
//! pair. All file access goes through [`crate::shares::resolve`] and is
//! written to the audit log.

use crate::events::{AuditRecord, Direction, PeerEvent};
use crate::fsutil::now_ms;
use crate::identity::{device_id, PublicKey};
use crate::pairing::{wrong_code, Confirm, Pake};
use crate::protocol::{expect_frame, read_frame, write_frame, Hello, Request, Response, ShareInfo, WireChange, PROTOCOL_VERSION};
use crate::service::Inner;
use crate::shares::{self, mask_entry, Resolved, ShareRoot, Target};
use crate::trust::TrustedDevice;
use cx_core::{validate_name, CxError, Location, Provider, Result, TrashedItem};
use cx_local::LocalProvider;
use quinn::{RecvStream, SendStream};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const COPY_BUF: usize = 1 << 20;

/// The device on the other end of an incoming connection.
pub(crate) struct Remote {
    pub key: PublicKey,
    pub device_id: String,
    pub addr: SocketAddr,
    pub name: Mutex<String>,
    pub os: Mutex<String>,
    /// Connected with this device's own key (a local admin tool).
    pub is_self: bool,
    /// Verified as the same Tailscale user (only counts while auto-trust is on).
    pub tailnet: bool,
}

impl Remote {
    pub fn name(&self) -> String {
        self.name.lock().unwrap().clone()
    }
}

/// What an authorized device may see.
pub(crate) struct Access {
    pub shares: Vec<ShareRoot>,
}

pub(crate) fn access(inner: &Inner, remote: &Remote) -> Option<Access> {
    if remote.is_self {
        return Some(Access { shares: inner.shares() });
    }
    if let Some(dev) = inner.trust.by_key(&remote.key) {
        return Some(Access { shares: inner.shares().into_iter().filter(|s| dev.may_use_share(&s.share.name)).collect() });
    }
    if remote.tailnet && inner.tailnet_auto_trust() {
        return Some(Access { shares: inner.shares() });
    }
    None
}

pub(crate) async fn accept_loop(inner: Arc<Inner>) {
    while let Some(incoming) = inner.endpoint.accept().await {
        tokio::spawn(handle_conn(inner.clone(), incoming));
    }
}

async fn handle_conn(inner: Arc<Inner>, incoming: quinn::Incoming) {
    let addr = incoming.remote_address();
    let Ok(Ok(conn)) = tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming).await else { return };
    let Some(key) = crate::tls::peer_key(&conn) else {
        conn.close(2u32.into(), b"bad identity");
        return;
    };
    let trusted = inner.trust.by_key(&key);
    let is_self = key == *inner.identity.public_key();
    let tailnet = trusted.is_none() && !is_self && inner.tailnet_auto_trust() && inner.tailnet.same_user(addr.ip()).await;
    let remote = Arc::new(Remote {
        device_id: device_id(&key),
        name: Mutex::new(trusted.as_ref().map(|d| d.name.clone()).unwrap_or_default()),
        os: Mutex::new(String::new()),
        key,
        addr,
        is_self,
        tailnet,
    });
    let id = conn.stable_id();
    inner.incoming.lock().unwrap().insert(id, (remote.device_id.clone(), conn.clone()));
    let announced = !is_self && access(&inner, &remote).is_some();
    if announced {
        inner.trust.note_addr(&remote.device_id, addr.to_string());
        inner.emit(PeerEvent::PeerConnected { device_id: remote.device_id.clone(), name: remote.name(), addr: addr.to_string(), direction: Direction::Incoming });
    }
    let reason = loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                tokio::spawn(handle_stream(inner.clone(), remote.clone(), conn.clone(), send, recv));
            }
            Err(e) => break e.to_string(),
        }
    };
    inner.incoming.lock().unwrap().remove(&id);
    if announced {
        inner.emit(PeerEvent::PeerDisconnected { device_id: remote.device_id.clone(), name: remote.name(), reason, direction: Direction::Incoming });
    }
}

fn not_paired() -> CxError {
    CxError::AuthRequired { uri: String::new(), user: None, reason: "this device is not paired; pair with a code first".into() }
}

async fn handle_stream(inner: Arc<Inner>, remote: Arc<Remote>, conn: quinn::Connection, mut send: SendStream, mut recv: RecvStream) {
    let req: Request = match tokio::time::timeout(REQUEST_TIMEOUT, read_frame(&mut recv)).await {
        Ok(Ok(Some(r))) => r,
        _ => return,
    };
    let op = req.op();
    let path = req.path();
    let result = match req {
        Request::Hello(h) => hello(&inner, &remote, h, &mut send).await,
        Request::PairStart { spake, name, os } => pair(&inner, &remote, spake, name, os, &mut send, &mut recv).await,
        Request::PairFinish { .. } => Err(CxError::Connection("unexpected PairFinish".into())),
        Request::NewPairingCode if remote.is_self => {
            let c = inner.pairing.start();
            write_frame(&mut send, &Response::PairingCode { code: c.code, expires_at: c.expires_at }).await
        }
        req => match access(&inner, &remote) {
            None => {
                let _ = write_frame(&mut send, &Response::err(not_paired())).await;
                Err(not_paired())
            }
            Some(acc) => serve(&inner, &remote, &acc, req, &conn, &mut send, &mut recv).await,
        },
    };
    let _ = send.finish();
    if op != "hello" && !remote.is_self {
        let rec = AuditRecord {
            time: now_ms(),
            device_id: remote.device_id.clone(),
            name: remote.name(),
            addr: remote.addr.to_string(),
            op: op.to_string(),
            path,
            ok: result.is_ok(),
            error: result.err().map(|e| e.to_string()),
        };
        inner.audit.append(&rec);
        inner.emit(PeerEvent::RemoteAccess(rec));
    }
}

fn clean_label(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(64).collect()
}

fn own_hello(inner: &Inner) -> Hello {
    Hello { version: PROTOCOL_VERSION, device_id: inner.identity.device_id().to_string(), name: inner.identity.name.clone(), os: std::env::consts::OS.to_string() }
}

async fn hello(inner: &Inner, remote: &Remote, h: Hello, send: &mut SendStream) -> Result<()> {
    // The device id is derived from the TLS key; a mismatching claim is ignored.
    if h.device_id == remote.device_id {
        *remote.name.lock().unwrap() = clean_label(&h.name);
        *remote.os.lock().unwrap() = clean_label(&h.os);
    }
    let trusted = access(inner, remote).is_some();
    write_frame(send, &Response::Hello { hello: own_hello(inner), trusted }).await
}

async fn pair(inner: &Inner, remote: &Remote, spake: Vec<u8>, name: String, os: String, send: &mut SendStream, recv: &mut RecvStream) -> Result<()> {
    let code = match inner.pairing.attempt() {
        Ok(c) => c,
        Err(e) => {
            write_frame(send, &Response::err(e.clone())).await?;
            return Err(e);
        }
    };
    let (pake, msg) = Pake::server(&code, &remote.key, inner.identity.public_key());
    let confirm = match pake.finish(&spake) {
        Ok(c) => c,
        Err(e) => {
            write_frame(send, &Response::err(e.clone())).await?;
            return Err(e);
        }
    };
    write_frame(send, &Response::PairChallenge { spake: msg, confirm: confirm.server_proof() }).await?;
    let got = match tokio::time::timeout(REQUEST_TIMEOUT, expect_frame::<_, Request>(recv)).await {
        Ok(Ok(Request::PairFinish { confirm })) => confirm,
        // The client gives up without finishing when our proof didn't match
        // its code: a wrong code.
        _ => return Err(wrong_code()),
    };
    if !Confirm::check(confirm.client_proof(), got) {
        write_frame(send, &Response::err(wrong_code())).await?;
        return Err(wrong_code());
    }
    inner.pairing.consume(&code);
    let name = clean_label(&name);
    *remote.name.lock().unwrap() = name.clone();
    let mut dev = TrustedDevice::new(&remote.key, name, clean_label(&os));
    dev.last_addrs = vec![remote.addr.to_string()];
    let dev = inner.trust.insert(dev)?;
    write_frame(send, &Response::Hello { hello: own_hello(inner), trusted: true }).await?;
    inner.emit(PeerEvent::PairingCompleted { device: dev });
    Ok(())
}

fn resolve_path(acc: &Access, path: &str) -> Result<Resolved> {
    match shares::resolve(&acc.shares, path)? {
        Target::Path(r) => Ok(r),
        Target::Root => Err(CxError::PermissionDenied("/ lists the shared folders; it holds no files".into())),
    }
}

fn loc(r: &Resolved) -> Location {
    Location::local(&r.path)
}

/// Run a request and write its response(s). Returns the outcome for the audit log.
#[allow(clippy::too_many_arguments)]
async fn serve(inner: &Arc<Inner>, remote: &Arc<Remote>, acc: &Access, req: Request, conn: &quinn::Connection, send: &mut SendStream, recv: &mut RecvStream) -> Result<()> {
    // Streaming requests write their own responses.
    let simple = match req {
        Request::List { path } => return list(acc, &path, send).await,
        Request::ReadRange { path, offset } => return read_range(acc, &path, offset, send).await,
        Request::Write { path, mode } => return write(acc, &path, mode.into(), send, recv).await,
        Request::Watch { path } => return watch(acc, &path, send).await,
        Request::Offer { offer_id, files, total } => {
            return crate::offer::receive(inner, remote, conn, offer_id, files, total, send, recv).await;
        }
        req => simple(acc, req).await,
    };
    match simple {
        Ok(resp) => write_frame(send, &resp).await,
        Err(e) => {
            write_frame(send, &Response::err(e.clone())).await?;
            Err(e)
        }
    }
}

async fn simple(acc: &Access, req: Request) -> Result<Response> {
    let p = LocalProvider;
    match req {
        Request::ListShares => Ok(Response::Shares(acc.shares.iter().map(|s| ShareInfo { name: s.share.name.clone(), read_only: s.share.read_only }).collect())),
        Request::Stat { path } => match shares::resolve(&acc.shares, &path)? {
            Target::Root => Ok(Response::Entry(shares::root_entry())),
            Target::Path(r) if r.is_share_root() => Ok(Response::Entry(shares::share_entry(&r.share))),
            Target::Path(r) => {
                let e = p.stat(&loc(&r)).await.map_err(|e| r.scrub(e))?;
                let dir = r.path.parent().unwrap_or(&r.share.root);
                Ok(Response::Entry(mask_entry(dir, &r.share.root, r.share.share.read_only, e)))
            }
        },
        Request::CreateDir { dir, name } => {
            let r = resolve_path(acc, &dir)?;
            r.require_writable()?;
            Ok(Response::Entry(p.create_dir(&loc(&r), name.as_deref()).await.map_err(|e| r.scrub(e))?))
        }
        Request::Move { src, dst } => {
            let (s, d) = (resolve_path(acc, &src)?, resolve_path(acc, &dst)?);
            s.require_mutable_entry()?;
            d.require_mutable_entry()?;
            p.move_to(&loc(&s), &loc(&d)).await.map_err(|e| s.scrub(d.scrub(e)))?;
            Ok(Response::Ok)
        }
        Request::Remove { path } => {
            let r = resolve_path(acc, &path)?;
            r.require_mutable_entry()?;
            p.remove(&loc(&r)).await.map_err(|e| r.scrub(e))?;
            Ok(Response::Ok)
        }
        Request::Trash { dir, names } => {
            let d = resolve_path(acc, &dir)?;
            let mut items = Vec::new();
            for n in &names {
                validate_name(n)?;
                let r = resolve_path(acc, &cx_core::location::join_posix(&d.peer_path, n))?;
                r.require_mutable_entry()?;
                p.trash(&loc(&d), std::slice::from_ref(n)).await.map_err(|e| r.scrub(e))?;
                // Where it went on this machine is none of the peer's business.
                items.push(TrashedItem { original: r.peer_path.clone(), trashed: None });
            }
            Ok(Response::Trashed(items))
        }
        Request::SetModified { path, ms } => {
            let r = resolve_path(acc, &path)?;
            r.require_writable()?;
            p.set_modified(&loc(&r), ms).await.map_err(|e| r.scrub(e))?;
            Ok(Response::Ok)
        }
        Request::CopyWithin { src, dst } => {
            let (s, d) = (resolve_path(acc, &src)?, resolve_path(acc, &dst)?);
            d.require_mutable_entry()?;
            let (from, to, root) = (s.path.clone(), d.path.clone(), s.share.root.clone());
            tokio::task::spawn_blocking(move || copy_tree(&from, &to, &root))
                .await
                .map_err(|e| CxError::Io(e.to_string()))?
                .map_err(|e| s.scrub(d.scrub(e)))?;
            Ok(Response::Copied(true))
        }
        Request::FreeSpace { path } => {
            let r = resolve_path(acc, &path)?;
            let s = p.free_space(&loc(&r)).await.map_err(|e| r.scrub(e))?;
            Ok(Response::Space(s.map(|s| (s.free, s.total))))
        }
        Request::Hash { path } => {
            let r = resolve_path(acc, &path)?;
            let file = r.path.clone();
            let h = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
                let mut hasher = blake3::Hasher::new();
                hasher.update_reader(std::fs::File::open(&file)?)?;
                Ok(hasher.finalize().to_hex().to_string())
            })
            .await
            .map_err(|e| CxError::Io(e.to_string()))?
            .map_err(|e| r.scrub(CxError::from_io(e, r.path.display())))?;
            Ok(Response::Hash(h))
        }
        other => Err(CxError::Unsupported(format!("request {}", other.op()))),
    }
}

/// Recursive copy that refuses to overwrite and never follows symlinks out
/// of the share (links are recreated only if they stay inside).
fn copy_tree(src: &Path, dst: &Path, root: &Path) -> Result<()> {
    fn io(p: &Path) -> impl Fn(std::io::Error) -> CxError + '_ {
        move |e| CxError::from_io(e, p.display())
    }
    if std::fs::symlink_metadata(dst).is_ok() {
        return Err(CxError::AlreadyExists(dst.display().to_string()));
    }
    let meta = std::fs::symlink_metadata(src).map_err(io(src))?;
    if meta.is_dir() {
        if dst.starts_with(src) {
            return Err(CxError::InvalidLocation("cannot copy a folder into itself".into()));
        }
        std::fs::create_dir(dst).map_err(io(dst))?;
        for de in std::fs::read_dir(src).map_err(io(src))? {
            let de = de.map_err(io(src))?;
            copy_tree(&de.path(), &dst.join(de.file_name()), root)?;
        }
        Ok(())
    } else if meta.file_type().is_symlink() {
        match std::fs::canonicalize(src) {
            Ok(t) if t.starts_with(root) => copy_tree(&t, dst, root),
            _ => Ok(()), // skip links that leave the share
        }
    } else {
        if reflink_copy::reflink(src, dst).is_err() {
            std::fs::copy(src, dst).map_err(io(src))?;
        }
        Ok(())
    }
}

async fn list(acc: &Access, path: &str, send: &mut SendStream) -> Result<()> {
    let r = match shares::resolve(&acc.shares, path) {
        Ok(Target::Root) => {
            let entries: Vec<_> = acc.shares.iter().map(shares::share_entry).collect();
            let total = entries.len() as u64;
            write_frame(send, &Response::Entries(entries)).await?;
            return write_frame(send, &Response::ListEnd { total }).await;
        }
        Ok(Target::Path(r)) => r,
        Err(e) => {
            write_frame(send, &Response::err(e.clone())).await?;
            return Err(e);
        }
    };
    let (tx, mut rx) = mpsc::channel(4);
    let dir = loc(&r);
    let lister = tokio::spawn(async move { LocalProvider.list(&dir, tx).await });
    let (root, ro) = (r.share.root.clone(), r.share.share.read_only);
    while let Some(batch) = rx.recv().await {
        let batch: Vec<_> = batch.into_iter().map(|e| mask_entry(&r.path, &root, ro, e)).collect();
        write_frame(send, &Response::Entries(batch)).await?;
    }
    match lister.await.map_err(|e| CxError::Io(e.to_string()))? {
        Ok(total) => write_frame(send, &Response::ListEnd { total: total as u64 }).await,
        Err(e) => {
            let e = r.scrub(e);
            write_frame(send, &Response::err(e.clone())).await?;
            Err(e)
        }
    }
}

async fn read_range(acc: &Access, path: &str, offset: u64, send: &mut SendStream) -> Result<()> {
    let opened = async {
        let r = resolve_path(acc, path)?;
        let mut f = tokio::fs::File::open(&r.path).await.map_err(|e| r.scrub(CxError::from_io(e, r.path.display())))?;
        let meta = f.metadata().await.map_err(|e| CxError::io(path, e))?;
        if meta.is_dir() {
            return Err(CxError::InvalidLocation(format!("{path} is a folder")));
        }
        if offset > 0 {
            f.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| CxError::io(path, e))?;
        }
        Ok((f, meta.len().saturating_sub(offset)))
    }
    .await;
    let (f, size) = match opened {
        Ok(v) => v,
        Err(e) => {
            write_frame(send, &Response::err(e.clone())).await?;
            return Err(e);
        }
    };
    write_frame(send, &Response::Ready { size: Some(size) }).await?;
    let mut reader = tokio::io::BufReader::with_capacity(COPY_BUF, f);
    match tokio::io::copy_buf(&mut reader, send).await {
        Ok(_) => Ok(()),
        // The reader closed early (seeked, cancelled): not an error.
        Err(e) if e.kind() == std::io::ErrorKind::NotConnected || e.to_string().contains("stopped") => Ok(()),
        Err(e) => Err(CxError::io(path, e)),
    }
}

async fn write(acc: &Access, path: &str, mode: cx_core::WriteMode, send: &mut SendStream, recv: &mut RecvStream) -> Result<()> {
    let opened = async {
        let r = resolve_path(acc, path)?;
        r.require_mutable_entry()?;
        let f = LocalProvider.open_write(&loc(&r), mode).await.map_err(|e| r.scrub(e))?;
        Ok::<_, CxError>(f)
    }
    .await;
    let f = match opened {
        Ok(f) => f,
        Err(e) => {
            write_frame(send, &Response::err(e.clone())).await?;
            return Err(e);
        }
    };
    write_frame(send, &Response::Ready { size: None }).await?;
    let mut out = tokio::io::BufWriter::with_capacity(COPY_BUF, f);
    let mut buf = vec![0u8; 256 << 10];
    let copied: Result<()> = async {
        loop {
            // quinn's own `read`: `None` is the end of the stream.
            let Some(n) = recv.read(&mut buf).await.map_err(|e| CxError::Connection(e.to_string()))? else { break };
            out.write_all(&buf[..n]).await.map_err(|e| CxError::io(path, e))?;
        }
        out.flush().await.map_err(|e| CxError::io(path, e))?;
        out.shutdown().await.map_err(|e| CxError::io(path, e))
    }
    .await;
    match copied {
        Ok(()) => write_frame(send, &Response::Ok).await,
        Err(e) => {
            let _ = recv.stop(1u32.into());
            let _ = write_frame(send, &Response::err(e.clone())).await;
            Err(e)
        }
    }
}

async fn watch(acc: &Access, path: &str, send: &mut SendStream) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Vec<cx_core::Change>>();
    let target = match shares::resolve(&acc.shares, path) {
        Ok(t) => t,
        Err(e) => {
            write_frame(send, &Response::err(e.clone())).await?;
            return Err(e);
        }
    };
    // "/" (the share list) only changes when the owner edits shares; keep
    // the stream open without events so clients treat it like any folder.
    let (_watch, mask) = match &target {
        Target::Root => (None, None),
        Target::Path(r) => match cx_local::watch_dir(&r.path, move |c| {
            let _ = tx.send(c);
        }) {
            Ok(w) => (Some(w), Some((r.path.clone(), r.share.root.clone(), r.share.share.read_only))),
            Err(e) => {
                let e = r.scrub(CxError::io(format!("cannot watch {}", r.path.display()), e));
                write_frame(send, &Response::err(e.clone())).await?;
                return Err(e);
            }
        },
    };
    write_frame(send, &Response::Ok).await?;
    // Resolves when the client drops its end of the stream (unwatch).
    let stopped = send.stopped();
    tokio::pin!(stopped);
    loop {
        tokio::select! {
            changes = rx.recv() => {
                let Some(changes) = changes else { break };
                let wire: Vec<WireChange> = changes
                    .into_iter()
                    .map(|c| match (c, &mask) {
                        (cx_core::Change::Upsert { entry }, Some((dir, root, ro))) => WireChange::Upsert(mask_entry(dir, root, *ro, entry)),
                        (c, _) => c.into(),
                    })
                    .collect();
                if write_frame(send, &Response::Changes(wire)).await.is_err() {
                    break;
                }
            }
            _ = &mut stopped => break,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PeerConfig, PeerService, Share};

    /// A client that skips the hello and speaks the protocol directly still
    /// gets nothing without pairing.
    #[tokio::test]
    async fn unpaired_requests_are_refused_by_the_server() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("share")).unwrap();
        let start = |state: &str, shares: Vec<Share>| {
            let mut cfg = PeerConfig::new(tmp.path().join(state));
            cfg.bind = Some("127.0.0.1".parse().unwrap());
            cfg.port = 0;
            cfg.shares = Some(shares);
            PeerService::start(cfg, crate::events::ignore_events())
        };
        let server = start("server", vec![Share { name: "S".into(), path: tmp.path().join("share"), read_only: false }]).await.unwrap();
        let stranger = start("stranger", vec![]).await.unwrap();
        let conn = crate::client::dial(stranger.inner(), &[server.local_addr().unwrap()]).await.unwrap();
        for req in [Request::ListShares, Request::List { path: "/S".into() }, Request::Write { path: "/S/x".into(), mode: crate::protocol::WireWriteMode::CreateNew }, Request::NewPairingCode] {
            let (mut send, mut recv) = conn.open_bi().await.unwrap();
            write_frame(&mut send, &req).await.unwrap();
            let _ = send.finish();
            let resp: Response = expect_frame(&mut recv).await.unwrap();
            assert!(matches!(resp, Response::Err(crate::protocol::WireError::AuthRequired { .. })), "{req:?}: {resp:?}");
        }
        assert!(!tmp.path().join("share/x").exists());
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        write_frame(&mut send, &Request::Hello(own_hello(stranger.inner()))).await.unwrap();
        let resp: Response = expect_frame(&mut recv).await.unwrap();
        assert!(matches!(resp, Response::Hello { trusted: false, .. }), "{resp:?}");
    }

    #[test]
    fn copy_tree_copies_and_refuses_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f.txt"), b"hi").unwrap();
        copy_tree(&root.join("a"), &root.join("c"), &root).unwrap();
        assert_eq!(std::fs::read(root.join("c/b/f.txt")).unwrap(), b"hi");
        assert!(matches!(copy_tree(&root.join("a"), &root.join("c"), &root), Err(CxError::AlreadyExists(_))));
        assert!(copy_tree(&root.join("a"), &root.join("a/b/x"), &root).is_err());
    }
}
