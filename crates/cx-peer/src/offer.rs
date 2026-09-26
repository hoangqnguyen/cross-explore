//! "Send to device": AirDrop-style offers.
//!
//! The sender opens a stream with an `Offer` listing names and sizes. The
//! receiver shows an `IncomingOffer` event and answers `Pending`; the user
//! then accepts (choosing a folder) or declines. On accept the sender
//! streams every file's bytes back to back on the same stream (sizes were
//! announced, so no extra framing is needed), and the receiver confirms
//! with a final `Ok`. Files never overwrite: name clashes get "name (2).ext".

use crate::client::PeerClient;
use crate::events::{Direction, OfferFileInfo, OfferState, PeerEvent, PeerRef};
use crate::fsutil::keep_both;
use crate::protocol::{expect_frame, write_frame, OfferFile, Request, Response};
use crate::server::Remote;
use crate::service::Inner;
use cx_core::{validate_name, CxError, Result};
use quinn::{RecvStream, SendStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::oneshot;

/// How long an offer waits for the user before it lapses.
const DECISION_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_FILES: usize = 10_000;
const PROGRESS_EVERY: Duration = Duration::from_millis(100);
const CHUNK: usize = 1 << 20;

pub(crate) struct PendingOffer {
    decide: oneshot::Sender<Option<PathBuf>>,
}

/// Accept (`Some(folder)`) or decline (`None`) an incoming offer.
pub(crate) fn decide(inner: &Inner, offer_id: &str, dest: Option<PathBuf>) -> Result<()> {
    if let Some(d) = &dest {
        if !d.is_dir() {
            return Err(CxError::NotFound(format!("folder {}", d.display())));
        }
    }
    let pending = inner.offers.lock().unwrap().remove(offer_id).ok_or_else(|| CxError::NotFound(format!("offer {offer_id}")))?;
    pending.decide.send(dest).map_err(|_| CxError::Cancelled)
}

/// Emits `OfferProgress` events, throttled while bytes flow.
struct Progress<'a> {
    inner: &'a Inner,
    offer_id: String,
    direction: Direction,
    peer: PeerRef,
    total: u64,
    bytes: u64,
    last: Instant,
}

impl Progress<'_> {
    fn emit(&mut self, state: OfferState, file: Option<String>, saved: Vec<String>, error: Option<String>) {
        self.last = Instant::now();
        self.inner.emit(PeerEvent::OfferProgress {
            offer_id: self.offer_id.clone(),
            direction: self.direction,
            peer: self.peer.clone(),
            state,
            bytes: self.bytes,
            total: self.total,
            file,
            saved,
            error,
        });
    }

    fn advance(&mut self, n: u64, file: &str) {
        self.bytes += n;
        if self.last.elapsed() >= PROGRESS_EVERY {
            self.emit(OfferState::Transferring, Some(file.to_string()), Vec::new(), None);
        }
    }
}

fn random_id() -> String {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut b).expect("system random");
    crate::identity::hex(&b)
}

/// Sender side. Returns once the receiver has the offer.
pub(crate) async fn send(inner: &Arc<Inner>, client: Arc<PeerClient>, paths: Vec<PathBuf>) -> Result<String> {
    if paths.is_empty() {
        return Err(CxError::InvalidLocation("nothing to send".into()));
    }
    let mut files = Vec::new();
    for p in &paths {
        let meta = tokio::fs::metadata(p).await.map_err(|e| CxError::from_io(e, p.display()))?;
        if !meta.is_file() {
            return Err(CxError::Unsupported(format!("sending folders ({})", p.display())));
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| CxError::InvalidName(p.display().to_string()))?;
        files.push(OfferFile { name, size: meta.len() });
    }
    let total = files.iter().map(|f| f.size).sum();
    let offer_id = random_id();
    let (send, recv, resp) = client.call(Request::Offer { offer_id: offer_id.clone(), files: files.clone(), total }).await?;
    if resp != Response::Pending {
        return Err(CxError::Connection("unexpected reply to offer".into()));
    }
    let (device_id, name) = client.device().unwrap_or_default();
    let peer = PeerRef { device_id, name };
    let inner = inner.clone();
    let id = offer_id.clone();
    let mut pending = Progress { inner: &inner, offer_id: id.clone(), direction: Direction::Outgoing, peer: peer.clone(), total, bytes: 0, last: Instant::now() };
    pending.emit(OfferState::Pending, None, Vec::new(), None);
    tokio::spawn(async move {
        let mut progress = Progress { inner: &inner, offer_id: id, direction: Direction::Outgoing, peer, total, bytes: 0, last: Instant::now() };
        match stream_files(&mut progress, &paths, &files, send, recv).await {
            Ok(true) => progress.emit(OfferState::Completed, None, Vec::new(), None),
            Ok(false) => progress.emit(OfferState::Declined, None, Vec::new(), None),
            Err(CxError::Cancelled) => progress.emit(OfferState::Cancelled, None, Vec::new(), None),
            Err(e) => progress.emit(OfferState::Failed, None, Vec::new(), Some(e.to_string())),
        }
    });
    Ok(offer_id)
}

/// `Ok(false)` when declined.
async fn stream_files(progress: &mut Progress<'_>, paths: &[PathBuf], files: &[OfferFile], mut send: SendStream, mut recv: RecvStream) -> Result<bool> {
    match expect_frame(&mut recv).await? {
        Response::Accepted => {}
        Response::Declined => return Ok(false),
        Response::Err(e) => return Err(e.into()),
        _ => return Err(CxError::Connection("unexpected reply to offer".into())),
    }
    progress.emit(OfferState::Accepted, None, Vec::new(), None);
    let mut buf = vec![0u8; CHUNK];
    for (path, f) in paths.iter().zip(files) {
        let file = tokio::fs::File::open(path).await.map_err(|e| CxError::from_io(e, path.display()))?;
        let mut file = file.take(f.size);
        let mut left = f.size;
        while left > 0 {
            let n = file.read(&mut buf).await.map_err(|e| CxError::io(path.display(), e))?;
            if n == 0 {
                let _ = send.reset(1u32.into());
                return Err(CxError::Io(format!("{} changed while sending", path.display())));
            }
            send.write_all(&buf[..n]).await.map_err(|e| CxError::Connection(e.to_string()))?;
            left -= n as u64;
            progress.advance(n as u64, &f.name);
        }
    }
    let _ = send.finish();
    match expect_frame(&mut recv).await? {
        Response::Ok => Ok(true),
        Response::Err(e) => Err(e.into()),
        _ => Err(CxError::Connection("unexpected reply to offer".into())),
    }
}

fn check_offer(offer_id: &str, files: &[OfferFile], total: u64) -> Result<()> {
    if offer_id.is_empty() || offer_id.len() > 64 || !offer_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(CxError::InvalidName("offer id".into()));
    }
    if files.is_empty() || files.len() > MAX_FILES {
        return Err(CxError::InvalidLocation("bad file count".into()));
    }
    for f in files {
        validate_name(&f.name)?;
    }
    if files.iter().try_fold(0u64, |a, f| a.checked_add(f.size)) != Some(total) {
        return Err(CxError::InvalidLocation("sizes do not add up".into()));
    }
    Ok(())
}

/// Receiver side, on the server's stream for an `Offer` request.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn receive(
    inner: &Arc<Inner>,
    remote: &Remote,
    _conn: &quinn::Connection,
    offer_id: String,
    files: Vec<OfferFile>,
    total: u64,
    send: &mut SendStream,
    recv: &mut RecvStream,
) -> Result<()> {
    if let Err(e) = check_offer(&offer_id, &files, total) {
        write_frame(send, &Response::err(e.clone())).await?;
        return Err(e);
    }
    let (tx, rx) = oneshot::channel();
    let fresh = {
        let mut offers = inner.offers.lock().unwrap();
        let fresh = !offers.contains_key(&offer_id);
        if fresh {
            offers.insert(offer_id.clone(), PendingOffer { decide: tx });
        }
        fresh
    };
    if !fresh {
        let e = CxError::AlreadyExists(format!("offer {offer_id}"));
        write_frame(send, &Response::err(e.clone())).await?;
        return Err(e);
    }
    let from = PeerRef { device_id: remote.device_id.clone(), name: remote.name() };
    inner.emit(PeerEvent::IncomingOffer { offer_id: offer_id.clone(), from: from.clone(), files: files.iter().map(OfferFileInfo::from).collect(), total });
    write_frame(send, &Response::Pending).await?;

    let mut progress = Progress { inner, offer_id: offer_id.clone(), direction: Direction::Incoming, peer: from, total, bytes: 0, last: Instant::now() };
    // The sender sends nothing until we answer, so any activity on its side
    // of the stream (FIN, reset, connection loss) means it gave up.
    let mut probe = [0u8; 1];
    let decision = tokio::select! {
        d = rx => d.ok(),
        _ = recv.read(&mut probe) => None,
        _ = tokio::time::sleep(DECISION_TIMEOUT) => None,
    };
    let dest = match decision {
        Some(Some(dest)) => dest,
        Some(None) => {
            write_frame(send, &Response::Declined).await?;
            progress.emit(OfferState::Declined, None, Vec::new(), None);
            return Ok(());
        }
        None => {
            inner.offers.lock().unwrap().remove(&offer_id);
            let _ = write_frame(send, &Response::err(CxError::Cancelled)).await;
            progress.emit(OfferState::Cancelled, None, Vec::new(), None);
            return Err(CxError::Cancelled);
        }
    };
    write_frame(send, &Response::Accepted).await?;
    progress.emit(OfferState::Accepted, None, Vec::new(), None);
    match receive_files(&mut progress, &dest, &files, recv).await {
        Ok(saved) => {
            write_frame(send, &Response::Ok).await?;
            progress.emit(OfferState::Completed, None, saved, None);
            Ok(())
        }
        Err(e) => {
            let _ = recv.stop(1u32.into());
            let _ = write_frame(send, &Response::err(e.clone())).await;
            progress.emit(OfferState::Failed, None, Vec::new(), Some(e.to_string()));
            Err(e)
        }
    }
}

async fn receive_files(progress: &mut Progress<'_>, dest: &Path, files: &[OfferFile], recv: &mut RecvStream) -> Result<Vec<String>> {
    let mut saved = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    for f in files {
        let (path, file) = keep_both(dest, &f.name, |p| std::fs::OpenOptions::new().write(true).create_new(true).open(p)).map_err(|e| CxError::from_io(e, dest.display()))?;
        let shown = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let result: Result<()> = async {
            let mut out = tokio::io::BufWriter::with_capacity(CHUNK, tokio::fs::File::from_std(file));
            let mut left = f.size;
            while left > 0 {
                let want = buf.len().min(left as usize);
                let n = match recv.read(&mut buf[..want]).await.map_err(|e| CxError::Connection(e.to_string()))? {
                    Some(n) => n,
                    None => return Err(CxError::Connection("sender stopped early".into())),
                };
                out.write_all(&buf[..n]).await.map_err(|e| CxError::from_io(e, &shown))?;
                left -= n as u64;
                progress.advance(n as u64, &shown);
            }
            out.flush().await.map_err(|e| CxError::from_io(e, &shown))?;
            Ok(())
        }
        .await;
        if let Err(e) = result {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
        saved.push(path.to_string_lossy().into_owned());
    }
    Ok(saved)
}
