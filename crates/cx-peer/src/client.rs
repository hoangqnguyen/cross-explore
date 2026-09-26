//! Talking to another device: the connection (with transparent reconnect),
//! pairing, and [`PeerProvider`], which serves `peer://` locations.
//!
//! Before any request the server's key must be acceptable: pinned in the
//! trust store (and, when the host was a device id, be exactly that
//! device), our own key, or a same-user tailnet node when auto-trust is on.
//! Otherwise connecting fails with [`CxError::HostKeyUnknown`] and the UI
//! offers to pair.

use crate::events::{Direction, PeerEvent};
use crate::identity::{device_id, fingerprint, looks_like_device_id, PublicKey};
use crate::net::SERVER_NAME;
use crate::pairing::{wrong_code, Confirm, Pake};
use crate::protocol::{conn_err, expect_frame, read_frame, write_frame, Hello, Request, Response, ShareInfo, PROTOCOL_VERSION};
use crate::service::Inner;
use crate::trust::TrustedDevice;
use async_trait::async_trait;
use cx_core::{Capabilities, Change, CxError, Entry, Location, Provider, ReadStream, Result, Scheme, Space, TrashedItem, WatchGuard, WatchSink, WriteMode, WriteStream};
use quinn::{RecvStream, SendStream};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::AsyncWrite;
use tokio::sync::mpsc;

const DIAL_TIMEOUT: Duration = Duration::from_secs(6);

/// Where to dial and whom we expect to find there.
async fn resolve(inner: &Inner, host: &str, port: u16) -> Result<(Vec<SocketAddr>, Option<String>)> {
    if looks_like_device_id(host) {
        let mut addrs = inner.directory.get(host);
        if let Some(dev) = inner.trust.get(host) {
            addrs.extend(dev.last_addrs.iter().filter_map(|a| a.parse::<SocketAddr>().ok()));
        }
        addrs.dedup();
        if !addrs.is_empty() {
            return Ok((addrs, Some(host.to_string())));
        }
    }
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await.map_err(|e| CxError::Connection(format!("{host}: {e}")))?.collect();
    if addrs.is_empty() {
        return Err(CxError::Connection(format!("{host}: no address")));
    }
    Ok((addrs, None))
}

pub(crate) async fn dial(inner: &Inner, addrs: &[SocketAddr]) -> Result<quinn::Connection> {
    let mut last = CxError::Connection("no address to connect to".into());
    for addr in addrs {
        let connecting = match inner.endpoint.connect_with(inner.client_config.clone(), *addr, SERVER_NAME) {
            Ok(c) => c,
            Err(e) => {
                last = CxError::Connection(format!("{addr}: {e}"));
                continue;
            }
        };
        match tokio::time::timeout(DIAL_TIMEOUT, connecting).await {
            Ok(Ok(conn)) => return Ok(conn),
            Ok(Err(e)) => last = CxError::Connection(format!("{addr}: {e}")),
            Err(_) => last = CxError::Connection(format!("{addr}: timed out")),
        }
    }
    Err(last)
}

fn server_key(conn: &quinn::Connection) -> Result<PublicKey> {
    crate::tls::peer_key(conn).ok_or_else(|| CxError::Connection("peer presented no usable key".into()))
}

fn peer_uri(host: &str, port: u16) -> String {
    cx_core::Endpoint { scheme: Scheme::Peer, user: None, host: host.to_string(), port: (port != Scheme::Peer.default_port()).then_some(port) }.uri()
}

/// Is the server behind `conn` one we may talk to?
async fn check_server(inner: &Inner, conn: &quinn::Connection, key: &PublicKey, host: &str, port: u16, expected: Option<&str>) -> Result<()> {
    let id = device_id(key);
    let unknown = |changed: bool| CxError::HostKeyUnknown {
        uri: peer_uri(host, port),
        host: host.to_string(),
        key_type: "ed25519".into(),
        fingerprint: fingerprint(key),
        changed,
    };
    if expected.is_some_and(|e| e != id) {
        return Err(unknown(true));
    }
    if inner.trust.by_key(key).is_some() || key == inner.identity.public_key() {
        return Ok(());
    }
    if inner.tailnet_auto_trust() && inner.tailnet.same_user(conn.remote_address().ip()).await {
        return Ok(());
    }
    Err(unknown(inner.trust.get(&id).is_some()))
}

fn own_hello(inner: &Inner) -> Hello {
    Hello { version: PROTOCOL_VERSION, device_id: inner.identity.device_id().to_string(), name: inner.identity.name.clone(), os: std::env::consts::OS.to_string() }
}

#[derive(Clone)]
pub(crate) struct Live {
    pub conn: quinn::Connection,
}

/// One logical connection to a device, re-established on demand.
pub struct PeerClient {
    inner: Weak<Inner>,
    host: String,
    port: u16,
    live: tokio::sync::Mutex<Option<Live>>,
    last_device: std::sync::Mutex<Option<(String, String)>>,
}

impl PeerClient {
    pub(crate) fn new(inner: Weak<Inner>, host: String, port: u16) -> PeerClient {
        PeerClient { inner, host, port, live: tokio::sync::Mutex::new(None), last_device: std::sync::Mutex::new(None) }
    }

    fn inner(&self) -> Result<Arc<Inner>> {
        self.inner.upgrade().filter(|i| !i.stopped.load(Ordering::SeqCst)).ok_or_else(|| CxError::Connection("peer service is stopped".into()))
    }

    /// Device id of the peer, once connected at least once.
    pub fn device_id(&self) -> Option<String> {
        self.last_device.lock().unwrap().as_ref().map(|d| d.0.clone())
    }

    pub(crate) fn device(&self) -> Option<(String, String)> {
        self.last_device.lock().unwrap().clone()
    }

    /// The live connection, reconnecting if it dropped.
    pub(crate) async fn connection(&self) -> Result<Live> {
        let mut g = self.live.lock().await;
        if let Some(l) = g.as_ref() {
            if l.conn.close_reason().is_none() {
                return Ok(l.clone());
            }
        }
        *g = None;
        let inner = self.inner()?;
        let (addrs, expected) = resolve(&inner, &self.host, self.port).await?;
        let conn = dial(&inner, &addrs).await?;
        let key = server_key(&conn)?;
        if let Err(e) = check_server(&inner, &conn, &key, &self.host, self.port, expected.as_deref()).await {
            conn.close(3u32.into(), b"untrusted");
            return Err(e);
        }
        let id = device_id(&key);
        let (mut send, mut recv) = conn.open_bi().await.map_err(conn_err)?;
        write_frame(&mut send, &Request::Hello(own_hello(&inner))).await?;
        let _ = send.finish();
        let (hello, trusted) = match expect_frame(&mut recv).await? {
            Response::Hello { hello, trusted } => (hello, trusted),
            Response::Err(e) => return Err(e.into()),
            _ => return Err(CxError::Connection("unexpected reply to hello".into())),
        };
        if hello.device_id != id {
            conn.close(3u32.into(), b"bad hello");
            return Err(CxError::Connection("peer claimed another device id".into()));
        }
        if !trusted {
            conn.close(0u32.into(), b"not paired");
            return Err(CxError::AuthRequired {
                uri: peer_uri(&self.host, self.port),
                user: None,
                reason: format!("{} has not paired with this device", hello.name),
            });
        }
        if inner.trust.get(&id).is_some() {
            inner.trust.note_addr(&id, conn.remote_address().to_string());
            let _ = inner.trust.update(&id, |d| {
                d.name = hello.name.clone();
                d.os = hello.os.clone();
            });
        }
        *self.last_device.lock().unwrap() = Some((id.clone(), hello.name.clone()));
        let live = Live { conn: conn.clone() };
        if key != *inner.identity.public_key() {
            inner.emit(PeerEvent::PeerConnected { device_id: id.clone(), name: hello.name.clone(), addr: conn.remote_address().to_string(), direction: Direction::Outgoing });
            let weak = self.inner.clone();
            tokio::spawn(async move {
                let reason = conn.closed().await.to_string();
                if let Some(inner) = weak.upgrade() {
                    inner.emit(PeerEvent::PeerDisconnected { device_id: id, name: hello.name, reason, direction: Direction::Outgoing });
                }
            });
        }
        *g = Some(live.clone());
        Ok(live)
    }

    async fn forget(&self, conn: &quinn::Connection) {
        let mut g = self.live.lock().await;
        if g.as_ref().is_some_and(|l| l.conn.stable_id() == conn.stable_id()) {
            *g = None;
        }
    }

    /// Open a stream and send `req`, reconnecting once if the cached
    /// connection turns out to be dead.
    pub(crate) async fn open(&self, req: &Request) -> Result<(SendStream, RecvStream)> {
        let mut last = None;
        for _ in 0..2 {
            let live = self.connection().await?;
            match live.conn.open_bi().await {
                Ok((mut send, recv)) => match write_frame(&mut send, req).await {
                    Ok(()) => return Ok((send, recv)),
                    Err(e) => last = Some(e),
                },
                Err(e) => last = Some(conn_err(e)),
            }
            self.forget(&live.conn).await;
        }
        Err(last.unwrap_or_else(|| CxError::Connection("connection failed".into())))
    }

    /// Send `req` and read the first response (errors become `Err`).
    /// Read-only requests are retried once on a fresh connection when the
    /// old one dies before answering.
    pub(crate) async fn call(&self, req: Request) -> Result<(SendStream, RecvStream, Response)> {
        let idempotent = matches!(
            req,
            Request::List { .. } | Request::Stat { .. } | Request::ReadRange { .. } | Request::Watch { .. } | Request::FreeSpace { .. } | Request::Hash { .. } | Request::ListShares
        );
        let attempts = if idempotent { 2 } else { 1 };
        let mut last = None;
        for _ in 0..attempts {
            let (send, mut recv) = self.open(&req).await?;
            match expect_frame::<_, Response>(&mut recv).await {
                Ok(Response::Err(e)) => return Err(e.into()),
                Ok(resp) => return Ok((send, recv, resp)),
                Err(e) => {
                    if let Some(l) = self.live.lock().await.as_ref().filter(|l| l.conn.close_reason().is_some()).cloned() {
                        self.forget(&l.conn).await;
                    }
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| CxError::Connection("no response".into())))
    }

    async fn simple(&self, req: Request) -> Result<Response> {
        let (mut send, _recv, resp) = self.call(req).await?;
        let _ = send.finish();
        Ok(resp)
    }
}

fn unexpected(what: &str) -> CxError {
    CxError::Connection(format!("unexpected reply to {what}"))
}

/// Pair with the device at `addr` using the code it shows.
pub(crate) async fn pair(inner: &Arc<Inner>, addr: &str, code: &str) -> Result<TrustedDevice> {
    let code: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if code.len() != 6 {
        return Err(CxError::InvalidName("a pairing code has 6 digits".into()));
    }
    let (host, port) = crate::connector::split_host_port(addr);
    let (addrs, _) = resolve(inner, &host, port).await?;
    let conn = dial(inner, &addrs).await?;
    let key = server_key(&conn)?;
    let result = async {
        let (mut send, mut recv) = conn.open_bi().await.map_err(conn_err)?;
        let (pake, msg) = Pake::client(&code, inner.identity.public_key(), &key);
        write_frame(&mut send, &Request::PairStart { spake: msg, name: inner.identity.name.clone(), os: std::env::consts::OS.to_string() }).await?;
        let (their, proof) = match expect_frame(&mut recv).await? {
            Response::PairChallenge { spake, confirm } => (spake, confirm),
            Response::Err(e) => return Err(e.into()),
            _ => return Err(unexpected("pairing")),
        };
        let confirm = pake.finish(&their)?;
        if !Confirm::check(confirm.server_proof(), proof) {
            let _ = send.reset(1u32.into());
            return Err(wrong_code());
        }
        write_frame(&mut send, &Request::PairFinish { confirm: confirm.client_proof() }).await?;
        let _ = send.finish();
        let hello = match expect_frame(&mut recv).await? {
            Response::Hello { hello, trusted: true } => hello,
            Response::Err(e) => return Err(e.into()),
            _ => return Err(unexpected("pairing")),
        };
        if hello.device_id != device_id(&key) {
            return Err(CxError::Connection("peer claimed another device id".into()));
        }
        let mut dev = TrustedDevice::new(&key, hello.name, hello.os);
        dev.last_addrs = vec![conn.remote_address().to_string()];
        inner.trust.insert(dev)
    }
    .await;
    conn.close(0u32.into(), b"paired");
    let dev = result?;
    inner.emit(PeerEvent::PairingCompleted { device: dev.clone() });
    Ok(dev)
}

pub(crate) async fn request_pairing_code(inner: &Arc<Inner>, addr: &str) -> Result<crate::PairingCode> {
    let (host, port) = crate::connector::split_host_port(addr);
    let (addrs, _) = resolve(inner, &host, port).await?;
    let conn = dial(inner, &addrs).await?;
    let result = async {
        if server_key(&conn)? != *inner.identity.public_key() {
            return Err(CxError::PermissionDenied(format!("{addr} is a different device (not this state directory's service)")));
        }
        let (mut send, mut recv) = conn.open_bi().await.map_err(conn_err)?;
        write_frame(&mut send, &Request::NewPairingCode).await?;
        let _ = send.finish();
        match expect_frame(&mut recv).await? {
            Response::PairingCode { code, expires_at } => Ok(crate::PairingCode { code, expires_at }),
            Response::Err(e) => Err(e.into()),
            _ => Err(unexpected("pairing code")),
        }
    }
    .await;
    conn.close(0u32.into(), b"done");
    result
}

/// `peer://` locations on one device.
pub struct PeerProvider {
    client: Arc<PeerClient>,
}

impl PeerProvider {
    pub(crate) fn new(client: Arc<PeerClient>) -> PeerProvider {
        PeerProvider { client }
    }

    /// (device id, name) of the device behind this provider.
    pub fn device(&self) -> Option<(String, String)> {
        self.client.device()
    }

    pub async fn list_shares(&self) -> Result<Vec<ShareInfo>> {
        match self.client.simple(Request::ListShares).await? {
            Response::Shares(s) => Ok(s),
            _ => Err(unexpected("list shares")),
        }
    }

    /// BLAKE3 of a file, computed on the peer (verifies transfers without
    /// moving the bytes twice).
    pub async fn hash(&self, loc: &Location) -> Result<String> {
        match self.client.simple(Request::Hash { path: path(loc)? }).await? {
            Response::Hash(h) => Ok(h),
            _ => Err(unexpected("hash")),
        }
    }
}

fn path(loc: &Location) -> Result<String> {
    match loc {
        Location::Remote { endpoint, path } if endpoint.scheme == Scheme::Peer => Ok(path.clone()),
        _ => Err(CxError::InvalidLocation(loc.uri())),
    }
}

fn expect_ok(resp: Response, what: &str) -> Result<()> {
    match resp {
        Response::Ok => Ok(()),
        _ => Err(unexpected(what)),
    }
}

#[async_trait]
impl Provider for PeerProvider {
    fn scheme(&self) -> &'static str {
        "peer"
    }

    fn capabilities(&self) -> Capabilities {
        // Read-only shares refuse writes per request (and mark entries readonly).
        Capabilities { live_watch: true, polling: false, server_copy: true, trash: true, posix: false, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let (mut send, mut recv, mut resp) = self.client.call(Request::List { path: path(dir)? }).await?;
        let _ = send.finish();
        let mut sent = 0;
        loop {
            match resp {
                Response::Entries(batch) => {
                    sent += batch.len();
                    if sink.send(batch).await.is_err() {
                        // Receiver gone: cancelled. Dropping `recv` tells the peer to stop.
                        return Ok(sent);
                    }
                }
                Response::ListEnd { .. } => return Ok(sent),
                Response::Err(e) => return Err(e.into()),
                _ => return Err(unexpected("list")),
            }
            resp = expect_frame(&mut recv).await?;
        }
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        match self.client.simple(Request::Stat { path: path(loc)? }).await? {
            Response::Entry(mut e) => {
                // The peer names a share root after the share; the root after nothing.
                if e.name.is_empty() {
                    e.name = loc.name();
                }
                Ok(e)
            }
            _ => Err(unexpected("stat")),
        }
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        match self.client.simple(Request::CreateDir { dir: path(dir)?, name: name.map(str::to_owned) }).await? {
            Response::Entry(e) => Ok(e),
            _ => Err(unexpected("create folder")),
        }
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        expect_ok(self.client.simple(Request::Move { src: path(src)?, dst: path(dst)? }).await?, "move")
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        expect_ok(self.client.simple(Request::Remove { path: path(loc)? }).await?, "remove")
    }

    async fn trash(&self, dir: &Location, names: &[String]) -> Result<Vec<TrashedItem>> {
        let base = dir.clone();
        match self.client.simple(Request::Trash { dir: path(dir)?, names: names.to_vec() }).await? {
            // The peer reports peer paths; turn them into full URIs.
            Response::Trashed(items) => Ok(items
                .into_iter()
                .map(|t| TrashedItem { original: base.join(t.original.rsplit('/').next().unwrap_or("")).uri(), trashed: None })
                .collect()),
            _ => Err(unexpected("trash")),
        }
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let (mut send, recv, resp) = self.client.call(Request::ReadRange { path: path(loc)?, offset }).await?;
        let _ = send.finish();
        match resp {
            Response::Ready { .. } => Ok(Box::pin(recv)),
            _ => Err(unexpected("read")),
        }
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let (send, recv, resp) = self.client.call(Request::Write { path: path(loc)?, mode: mode.into() }).await?;
        match resp {
            Response::Ready { .. } => Ok(Box::pin(PeerWriter { send, recv: Some(recv), finishing: None, done: false })),
            _ => Err(unexpected("write")),
        }
    }

    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        expect_ok(self.client.simple(Request::SetModified { path: path(loc)?, ms }).await?, "set modified")
    }

    async fn copy_within(&self, src: &Location, dst: &Location) -> Result<bool> {
        match self.client.simple(Request::CopyWithin { src: path(src)?, dst: path(dst)? }).await? {
            Response::Copied(b) => Ok(b),
            _ => Err(unexpected("copy")),
        }
    }

    async fn watch(&self, dir: &Location, sink: WatchSink) -> Result<Option<WatchGuard>> {
        let p = path(dir)?;
        let (send, recv, _) = self.client.call(Request::Watch { path: p.clone() }).await?;
        let client = self.client.clone();
        let task = tokio::spawn(watch_loop(client, p, sink, send, recv));
        Ok(Some(WatchGuard::new(AbortOnDrop(task))))
    }

    async fn free_space(&self, loc: &Location) -> Result<Option<Space>> {
        match self.client.simple(Request::FreeSpace { path: path(loc)? }).await? {
            Response::Space(s) => Ok(s.map(|(free, total)| Space { free, total })),
            _ => Err(unexpected("free space")),
        }
    }
}

/// Forward pushed changes. If the connection drops, re-subscribe (with
/// backoff) and send `Reset` so the UI re-lists whatever it missed.
async fn watch_loop(client: Arc<PeerClient>, path: String, sink: WatchSink, send: SendStream, recv: RecvStream) {
    let mut stream = Some((send, recv));
    loop {
        if let Some((_send, mut recv)) = stream.take() {
            while let Ok(Some(resp)) = read_frame::<_, Response>(&mut recv).await {
                if let Response::Changes(c) = resp {
                    sink(c.into_iter().map(Change::from).collect());
                }
            }
        }
        let mut delay = Duration::from_millis(200);
        loop {
            tokio::time::sleep(delay).await;
            match client.call(Request::Watch { path: path.clone() }).await {
                Ok((s, r, _)) => {
                    sink(vec![Change::Reset]);
                    stream = Some((s, r));
                    break;
                }
                // The folder is gone (or no longer shared with us): the
                // re-list after Reset shows the error.
                Err(CxError::NotFound(_) | CxError::PermissionDenied(_) | CxError::AuthRequired { .. }) => {
                    sink(vec![Change::Reset]);
                    return;
                }
                Err(_) => delay = (delay * 2).min(Duration::from_secs(5)),
            }
        }
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

type Finishing = Pin<Box<dyn Future<Output = io::Result<()>> + Send>>;

/// Upload stream. `shutdown()` sends FIN and waits for the peer to confirm
/// the file was written; dropping it before that aborts the upload.
struct PeerWriter {
    send: SendStream,
    recv: Option<RecvStream>,
    finishing: Option<Finishing>,
    done: bool,
}

impl AsyncWrite for PeerWriter {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.send).poll_write(cx, buf).map_err(|e| io::Error::new(io::ErrorKind::BrokenPipe, e))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.done {
            return Poll::Ready(Ok(()));
        }
        if self.finishing.is_none() {
            let _ = self.send.finish();
            let Some(mut recv) = self.recv.take() else { return Poll::Ready(Ok(())) };
            self.finishing = Some(Box::pin(async move {
                match expect_frame::<_, Response>(&mut recv).await {
                    Ok(Response::Ok) => Ok(()),
                    Ok(Response::Err(e)) => Err(CxError::from(e).into()),
                    Ok(_) => Err(io::Error::other("unexpected reply to write")),
                    Err(e) => Err(e.into()),
                }
            }));
        }
        let res = std::task::ready!(self.finishing.as_mut().expect("set above").as_mut().poll(cx));
        self.done = true;
        Poll::Ready(res)
    }
}

impl Drop for PeerWriter {
    fn drop(&mut self) {
        if self.finishing.is_none() && !self.done {
            // Not shut down: abort instead of letting the peer believe the
            // (truncated) file is complete.
            let _ = self.send.reset(1u32.into());
        }
    }
}
