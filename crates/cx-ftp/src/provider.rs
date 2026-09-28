use crate::listing::{format_mdtm, parse_list, parse_mlsx};
use crate::pool::{self, Lease, Params, Pool};
use async_trait::async_trait;
use cx_core::location::join_posix;
use cx_core::provider::list_all;
use cx_core::{validate_name, Capabilities, CxError, Entry, EntryKind, Location, Provider, ReadStream, Result, WriteMode, WriteStream};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};
use suppaftp::tokio::{AsyncRustlsStream, TransferStream};
use suppaftp::{FtpError, Status};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, BufReader, ReadBuf};
use tokio::sync::mpsc;

const FIRST_BATCH: usize = 128;
const NEXT_BATCH: usize = 1024;
/// Control connections per server: enough for a listing next to a transfer
/// in each direction, few enough not to hit per-client server limits.
pub const POOL_SIZE: usize = 3;

/// A connected FTP or FTPS server.
pub struct FtpProvider {
    pool: Arc<Pool>,
    scheme: &'static str,
    home: String,
}

impl FtpProvider {
    pub(crate) async fn connect(params: Params) -> Result<FtpProvider> {
        // Open one connection now so bad credentials or an untrusted
        // certificate surface at connect time, where the UI expects them.
        let mut first = pool::open(&params).await?;
        // The login folder: "/" on chrooted servers, the account's home
        // (e.g. /home/me) elsewhere. Asked now, before any CWD.
        let home = first.ftp.pwd().await.ok().filter(|p| p.starts_with('/')).unwrap_or_else(|| "/".into());
        let scheme = params.endpoint.scheme.as_str();
        let pool = Pool::new(params, POOL_SIZE);
        pool.seed(first);
        Ok(FtpProvider { pool, scheme, home })
    }

    /// The folder the server puts the user in after login, as a POSIX path.
    /// Handy as the initial folder.
    pub fn home(&self) -> &str {
        &self.home
    }
}

type Transfer = TransferStream<AsyncRustlsStream>;

/// How long a data command may take to get its connection going.
const DATA_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What to verify before a data command (see `start_transfer`).
#[derive(Clone, Copy)]
enum Check<'a> {
    Cwd(&'a str),
    Size(&'a str),
}
type Finish = Pin<Box<dyn Future<Output = std::result::Result<(), FtpError>> + Send>>;

fn path_of(loc: &Location) -> Result<String> {
    match loc {
        Location::Remote { path, .. } => Ok(path.clone()),
        _ => Err(CxError::InvalidLocation(loc.uri())),
    }
}

/// FTP replies → our errors. The protocol only has coarse codes, so the
/// server's message text decides between "missing", "exists" and "denied".
/// Anything that means the connection is unusable becomes `Connection`,
/// which makes the caller retry on a fresh connection.
fn map_err(e: FtpError, path: &str) -> CxError {
    match e {
        FtpError::UnexpectedResponse(r) => {
            let msg = String::from_utf8_lossy(&r.body).trim().to_string();
            let lower = msg.to_ascii_lowercase();
            match r.status {
                Status::NotAvailable | Status::CannotOpenDataConnection => CxError::Connection(format!("{path}: {msg}")),
                Status::FileUnavailable | Status::RequestFileActionIgnored | Status::BadFilename | Status::ActionAborted => {
                    // "File exists", "already exists" — but not "can't check for file existence".
                    if lower.contains("exists") {
                        CxError::AlreadyExists(path.to_string())
                    } else if lower.contains("permission") || lower.contains("denied") || lower.contains("not allowed") {
                        CxError::PermissionDenied(path.to_string())
                    } else if r.status == Status::FileUnavailable || lower.contains("no such") || lower.contains("not found") {
                        CxError::NotFound(path.to_string())
                    } else {
                        CxError::Io(format!("{path}: {msg}"))
                    }
                }
                Status::NotLoggedIn => CxError::PermissionDenied(path.to_string()),
                Status::ExceededStorage => CxError::Io(format!("{path}: server storage full ({msg})")),
                _ => CxError::Io(format!("{path}: {msg}")),
            }
        }
        FtpError::ConnectionError(e) => CxError::Connection(format!("{path}: {e}")),
        FtpError::BadResponse => CxError::Connection(format!("{path}: bad server response")),
        FtpError::SecureError(e) => CxError::Connection(format!("{path}: TLS: {e}")),
        other => CxError::Io(format!("{path}: {other}")),
    }
}

/// Run `$body` with a pooled connection (`$ftp: &mut Ftp`). If the
/// connection turns out to be dead (servers drop idle ones), the command is
/// retried once on a fresh connection.
macro_rules! with_conn {
    ($self:expr, |$ftp:ident, $feat:ident| $body:expr) => {{
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut lease = $self.pool.lease().await?;
            #[allow(unused_variables)]
            let $feat = lease.features();
            let res = {
                let $ftp = lease.ftp();
                $body
            };
            match res {
                Err(CxError::Connection(_)) if attempt == 1 => {
                    lease.discard();
                    continue;
                }
                Err(e @ CxError::Connection(_)) => {
                    lease.discard();
                    break Err(e);
                }
                other => break other,
            }
        }
    }};
}

impl FtpProvider {
    /// Start a data command, retrying once on a dead connection.
    ///
    /// `check` runs first on the control connection. It matters for FTPS:
    /// suppaftp opens the data connection and starts TLS on it *before*
    /// reading the server's reply, so when the server refuses the command
    /// (missing file, no permission) nobody answers the handshake and the
    /// call would hang. Checking the target first turns the common failures
    /// into clean errors, and the timeout bounds the rest.
    async fn start_transfer(&self, cmd: String, path: &str, check: Check<'_>, rest: u64) -> Result<(Lease, Transfer)> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let mut lease = self.pool.lease().await?;
            let ftp = lease.ftp();
            let run = async {
                match check {
                    Check::Cwd(dir) => ftp.cwd(dir).await?,
                    Check::Size(file) => ftp.size(file).await.map(|_| ())?,
                }
                if rest > 0 {
                    ftp.resume_transfer(rest as usize).await?;
                }
                ftp.custom_data_command(cmd.clone(), &[Status::AboutToSend, Status::AlreadyOpen]).await.map(|(_, t)| t)
            };
            let res = match tokio::time::timeout(DATA_TIMEOUT, run).await {
                Ok(r) => r.map_err(|e| map_err(e, path)),
                Err(_) => {
                    lease.discard();
                    return Err(CxError::Io(format!("{path}: the server did not open the data connection")));
                }
            };
            match res {
                Ok(t) => return Ok((lease, t)),
                Err(CxError::Connection(_)) if attempt == 1 => lease.discard(),
                Err(e) => {
                    if matches!(e, CxError::Connection(_)) {
                        lease.discard();
                    }
                    return Err(e);
                }
            }
        }
    }

    /// Open the listing's data stream: `MLSD` when supported, else `LIST -a`
    /// (with a plain `LIST` fallback). Changing into the folder first turns
    /// a missing folder into a clean `NotFound`.
    async fn open_listing(&self, path: &str) -> Result<(Lease, Transfer, bool)> {
        let mlsd = self.pool.lease().await?.features().has("MLSD");
        if mlsd {
            let (l, t) = self.start_transfer(format!("MLSD {path}"), path, Check::Cwd(path), 0).await?;
            return Ok((l, t, true));
        }
        match self.start_transfer("LIST -a".into(), path, Check::Cwd(path), 0).await {
            Ok((l, t)) => Ok((l, t, false)),
            Err(CxError::Io(_)) => self.start_transfer("LIST".into(), path, Check::Cwd(path), 0).await.map(|(l, t)| (l, t, false)),
            Err(e) => Err(e),
        }
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        match self.stat_path(path, String::new()).await {
            Ok(_) => Ok(true),
            Err(CxError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// FTP can't tell whether a symlink points at a folder; trying to
    /// change into it can.
    async fn resolve_links(&self, dir: &str, entries: &mut [Entry]) -> Result<()> {
        for e in entries.iter_mut().filter(|e| e.kind == EntryKind::Symlink) {
            let target = join_posix(dir, &e.name);
            let t = target.as_str();
            e.is_dir = with_conn!(self, |ftp, feat| Ok::<_, CxError>(ftp.cwd(t).await.is_ok()))?;
        }
        Ok(())
    }

    async fn stat_path(&self, path: &str, name: String) -> Result<Entry> {
        if path == "/" {
            return Ok(Entry { name, kind: EntryKind::Dir, is_dir: true, size: 0, modified: None, created: None, hidden: false, readonly: false, executable: false });
        }
        let mlst = self.pool.lease().await?.features().has("MLST");
        if mlst {
            let line = with_conn!(self, |ftp, feat| ftp.mlst(Some(path)).await.map_err(|e| map_err(e, path)))?;
            let mut e = parse_mlsx(&line).ok_or_else(|| CxError::Io(format!("{path}: unreadable MLST reply")))?.entry;
            e.hidden = name.starts_with('.');
            e.name = name;
            if e.kind == EntryKind::Symlink {
                let parent = cx_core::location::normalize_posix(&format!("{path}/.."));
                let mut one = [Entry { name: path.rsplit('/').next().unwrap_or("").to_string(), ..e.clone() }];
                self.resolve_links(&parent, &mut one).await?;
                e.is_dir = one[0].is_dir;
            }
            return Ok(e);
        }
        // No MLST: find the entry in its parent's listing.
        let (parent, base) = path.rsplit_once('/').unwrap_or(("", path));
        let parent = if parent.is_empty() { "/" } else { parent };
        let (tx, mut rx) = mpsc::channel(8);
        let listing = self.list_path(parent, tx);
        let find = async {
            let mut found = None;
            while let Some(batch) = rx.recv().await {
                if found.is_none() {
                    found = batch.into_iter().find(|e| e.name == base);
                }
            }
            found
        };
        let (res, found) = tokio::join!(listing, find);
        match res {
            Ok(_) | Err(CxError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
        let mut e = found.ok_or_else(|| CxError::NotFound(path.to_string()))?;
        e.name = name;
        Ok(e)
    }

    async fn list_path(&self, path: &str, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let (mut lease, transfer, mlsd) = self.open_listing(path).await?;
        let mut reader = BufReader::new(transfer);
        let mut line = Vec::new();
        let mut batch = Vec::new();
        let mut links = Vec::new();
        let mut limit = FIRST_BATCH;
        let mut total = 0;
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line).await {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    lease.discard();
                    return Err(CxError::Connection(format!("{path}: {e}")));
                }
            }
            let text = String::from_utf8_lossy(&line);
            let entry = if mlsd { parse_mlsx(&text).filter(|m| !m.is_self_or_parent).map(|m| m.entry) } else { parse_list(&text) };
            let Some(entry) = entry else { continue };
            // Links are resolved after the transfer, when the connection is free.
            if entry.kind == EntryKind::Symlink {
                links.push(entry);
                continue;
            }
            batch.push(entry);
            if batch.len() >= limit {
                total += batch.len();
                if sink.send(std::mem::take(&mut batch)).await.is_err() {
                    lease.discard(); // abandoned mid-transfer
                    return Ok(total);
                }
                limit = NEXT_BATCH;
            }
        }
        reader.into_inner().finish().await.map_err(|e| map_err(e, path))?;
        drop(lease);
        self.resolve_links(path, &mut links).await?;
        batch.append(&mut links);
        total += batch.len();
        if !batch.is_empty() {
            let _ = sink.send(batch).await;
        }
        Ok(total)
    }

    async fn remove_tree(&self, dir: &Location) -> Result<()> {
        let path = path_of(dir)?;
        for e in list_all(self, dir).await? {
            let child = dir.join(&e.name);
            if e.kind == EntryKind::Dir {
                Box::pin(self.remove_tree(&child)).await?;
            } else {
                let p = path_of(&child)?;
                let p = p.as_str();
                with_conn!(self, |ftp, feat| ftp.rm(p).await.map_err(|e| map_err(e, p)))?;
            }
        }
        let p = path.as_str();
        with_conn!(self, |ftp, feat| ftp.rmdir(p).await.map_err(|e| map_err(e, p)))
    }
}

#[async_trait]
impl Provider for FtpProvider {
    fn scheme(&self) -> &'static str {
        self.scheme
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { live_watch: false, polling: true, server_copy: false, trash: false, posix: false, writable: true }
    }

    async fn list(&self, dir: &Location, sink: mpsc::Sender<Vec<Entry>>) -> Result<usize> {
        let path = path_of(dir)?;
        self.list_path(&path, sink).await
    }

    async fn stat(&self, loc: &Location) -> Result<Entry> {
        let path = path_of(loc)?;
        self.stat_path(&path, loc.name()).await
    }

    async fn create_dir(&self, dir: &Location, name: Option<&str>) -> Result<Entry> {
        let base = path_of(dir)?;
        let auto = name.is_none();
        let names: Box<dyn Iterator<Item = String> + Send> = match name {
            Some(n) => {
                validate_name(n)?;
                Box::new(std::iter::once(n.to_string()))
            }
            None => Box::new((1..10_000).map(|n| if n == 1 { "New folder".to_string() } else { format!("New folder ({n})") })),
        };
        for name in names {
            let path = join_posix(&base, &name);
            let p = path.as_str();
            match with_conn!(self, |ftp, feat| ftp.mkdir(p).await.map_err(|e| map_err(e, p))) {
                Ok(()) => return self.stat_path(&path, name).await,
                // Servers word "exists" differently (or not at all), so look.
                Err(e) => {
                    if !self.exists(&path).await.unwrap_or(false) {
                        return Err(e);
                    }
                    if !auto {
                        return Err(CxError::AlreadyExists(path));
                    }
                }
            }
        }
        Err(CxError::AlreadyExists(join_posix(&base, "New folder")))
    }

    async fn move_to(&self, src: &Location, dst: &Location) -> Result<()> {
        let (from, to) = (path_of(src)?, path_of(dst)?);
        let case_only = src.parent() == dst.parent() && src.name().to_lowercase() == dst.name().to_lowercase() && from != to;
        if !case_only && self.exists(&to).await? {
            return Err(CxError::AlreadyExists(to));
        }
        let (f, t) = (from.as_str(), to.as_str());
        with_conn!(self, |ftp, feat| ftp.rename(f, t).await.map_err(|e| map_err(e, f)))
    }

    async fn remove(&self, loc: &Location) -> Result<()> {
        let path = path_of(loc)?;
        let e = self.stat_path(&path, loc.name()).await?;
        if e.kind == EntryKind::Dir {
            return self.remove_tree(loc).await;
        }
        let p = path.as_str();
        with_conn!(self, |ftp, feat| ftp.rm(p).await.map_err(|e| map_err(e, p)))
    }

    async fn open_read(&self, loc: &Location, offset: u64) -> Result<ReadStream> {
        let path = path_of(loc)?;
        let (lease, transfer) = self.start_transfer(format!("RETR {path}"), &path, Check::Size(&path), offset).await?;
        Ok(Box::pin(FtpStream { transfer: Some(transfer), finish: None, lease: Some(lease) }))
    }

    async fn open_write(&self, loc: &Location, mode: WriteMode) -> Result<WriteStream> {
        let path = path_of(loc)?;
        // FTP has no exclusive create; check first (racy, but the best the
        // protocol offers).
        if mode == WriteMode::CreateNew && self.exists(&path).await? {
            return Err(CxError::AlreadyExists(path));
        }
        let verb = if mode == WriteMode::Append { "APPE" } else { "STOR" };
        let parent = cx_core::location::normalize_posix(&format!("{path}/.."));
        let (lease, transfer) = self.start_transfer(format!("{verb} {path}"), &path, Check::Cwd(&parent), 0).await?;
        Ok(Box::pin(FtpStream { transfer: Some(transfer), finish: None, lease: Some(lease) }))
    }

    /// `MFMT` where advertised, else the `MDTM <time> <path>` (vsftpd) and
    /// `SITE UTIME` (ProFTPD/Pure-FTPd) variants. Best effort.
    async fn set_modified(&self, loc: &Location, ms: i64) -> Result<()> {
        let path = path_of(loc)?;
        let ts = format_mdtm(ms);
        let p = path.as_str();
        let ts = ts.as_str();
        let _ = with_conn!(self, |ftp, feat| {
            let mut cmds = Vec::new();
            if feat.has("MFMT") {
                cmds.push((format!("MFMT {ts} {p}"), Status::File));
            }
            cmds.push((format!("MDTM {ts} {p}"), Status::File));
            cmds.push((format!("SITE UTIME {ts} {p}"), Status::CommandOk));
            let mut res = Err(CxError::Unsupported("set modification time".into()));
            for (cmd, ok) in cmds {
                match ftp.custom_command(cmd, &[ok]).await {
                    Ok(_) => {
                        res = Ok(());
                        break;
                    }
                    Err(e @ (FtpError::ConnectionError(_) | FtpError::BadResponse)) => {
                        res = Err(map_err(e, p));
                        break;
                    }
                    Err(_) => {}
                }
            }
            res
        });
        Ok(())
    }
}

/// A running `RETR`/`STOR`/`APPE`. The transfer's completion reply is read
/// when the download hits EOF or the upload is shut down; only then does
/// the connection go back to the pool. Dropped half-way, the connection is
/// discarded (its control channel would still expect the abort reply).
struct FtpStream {
    transfer: Option<Transfer>,
    finish: Option<Finish>,
    lease: Option<Lease>,
}

impl FtpStream {
    fn poll_finish(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            if let Some(f) = self.finish.as_mut() {
                let res = ready!(f.as_mut().poll(cx));
                self.finish = None;
                let lease = self.lease.take();
                return Poll::Ready(match res {
                    Ok(()) => Ok(()), // lease returns to the pool on drop
                    Err(e) => {
                        if let Some(mut l) = lease {
                            l.discard();
                        }
                        Err(io::Error::other(e.to_string()))
                    }
                });
            }
            let Some(t) = self.transfer.take() else { return Poll::Ready(Ok(())) };
            self.finish = Some(Box::pin(t.finish()));
        }
    }

    fn fail(&mut self, e: io::Error) -> Poll<io::Result<()>> {
        if let Some(l) = self.lease.as_mut() {
            l.discard();
        }
        Poll::Ready(Err(e))
    }
}

impl Drop for FtpStream {
    fn drop(&mut self) {
        if self.transfer.is_some() || self.finish.is_some() {
            if let Some(l) = self.lease.as_mut() {
                l.discard();
            }
        }
    }
}

impl AsyncRead for FtpStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if let Some(t) = this.transfer.as_mut() {
            let before = buf.filled().len();
            if let Err(e) = ready!(Pin::new(t).poll_read(cx, buf)) {
                return this.fail(e);
            }
            if buf.filled().len() > before || buf.remaining() == 0 {
                return Poll::Ready(Ok(()));
            }
        }
        // EOF: collect the server's "transfer complete" before reporting it,
        // so a truncated download is an error, not a short file.
        this.poll_finish(cx)
    }
}

impl AsyncWrite for FtpStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let Some(t) = this.transfer.as_mut() else { return Poll::Ready(Err(io::Error::other("write after shutdown"))) };
        match ready!(Pin::new(t).poll_write(cx, buf)) {
            Ok(n) => Poll::Ready(Ok(n)),
            Err(e) => this.fail(e).map(|r| r.map(|_| 0)),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        match this.transfer.as_mut() {
            Some(t) => Pin::new(t).poll_flush(cx),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().poll_finish(cx)
    }
}
