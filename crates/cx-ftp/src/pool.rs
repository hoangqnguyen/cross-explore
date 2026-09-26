//! A few FTP control connections per server.
//!
//! FTP runs one command at a time per connection, and a transfer occupies
//! its connection until it finishes. With a small pool a folder can be
//! listed while a download is running, and uploads can run side by side.

use crate::tls;
use cx_core::{Credentials, CxError, Endpoint, Result, Scheme, Secret};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use suppaftp::tokio::{AsyncRustlsConnector, AsyncRustlsStream, ImplAsyncFtpStream};
use suppaftp::types::FileType;
use suppaftp::{FtpError, Status};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) type Ftp = ImplAsyncFtpStream<AsyncRustlsStream>;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Servers drop idle control connections (often after 5–15 minutes); check
/// one with NOOP before reusing it after this long.
const IDLE_CHECK: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(crate) struct Params {
    pub endpoint: Endpoint,
    pub creds: Option<Credentials>,
    pub trust_store: PathBuf,
}

/// What the server said it supports in `FEAT`.
#[derive(Debug, Default, Clone)]
pub(crate) struct Features(HashSet<String>);

impl Features {
    pub fn has(&self, name: &str) -> bool {
        self.0.contains(name)
    }
}

pub(crate) struct Conn {
    pub ftp: Ftp,
    pub features: Arc<Features>,
}

pub(crate) struct Pool {
    params: Params,
    idle: Mutex<Vec<(Conn, Instant)>>,
    permits: Arc<Semaphore>,
}

/// A connection borrowed from the pool. Goes back when dropped, unless
/// something went wrong with it (see [`Lease::discard`]).
pub(crate) struct Lease {
    conn: Option<Conn>,
    pool: Arc<Pool>,
    healthy: bool,
    _permit: OwnedSemaphorePermit,
}

impl Lease {
    pub fn ftp(&mut self) -> &mut Ftp {
        &mut self.conn.as_mut().expect("lease holds a connection").ftp
    }

    pub fn features(&self) -> Arc<Features> {
        self.conn.as_ref().expect("lease holds a connection").features.clone()
    }

    /// Don't reuse this connection (it broke, or a transfer was abandoned
    /// half-way and the control channel may be out of sync).
    pub fn discard(&mut self) {
        self.healthy = false;
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let (true, Some(conn)) = (self.healthy, self.conn.take()) {
            self.pool.idle.lock().unwrap().push((conn, Instant::now()));
        }
    }
}

impl Pool {
    pub fn new(params: Params, size: usize) -> Arc<Pool> {
        Arc::new(Pool { params, idle: Mutex::new(Vec::new()), permits: Arc::new(Semaphore::new(size.max(1))) })
    }

    /// Hand in a connection opened while connecting, so it gets reused.
    pub fn seed(&self, conn: Conn) {
        self.idle.lock().unwrap().push((conn, Instant::now()));
    }

    pub async fn lease(self: &Arc<Self>) -> Result<Lease> {
        let permit = self.permits.clone().acquire_owned().await.map_err(|_| CxError::Cancelled)?;
        loop {
            let idle = self.idle.lock().unwrap().pop();
            let Some((mut conn, since)) = idle else { break };
            if since.elapsed() < IDLE_CHECK || conn.ftp.noop().await.is_ok() {
                return Ok(Lease { conn: Some(conn), pool: self.clone(), healthy: true, _permit: permit });
            }
        }
        let conn = open(&self.params).await?;
        Ok(Lease { conn: Some(conn), pool: self.clone(), healthy: true, _permit: permit })
    }
}

/// Open and sign in one control connection.
pub(crate) async fn open(p: &Params) -> Result<Conn> {
    let ep = &p.endpoint;
    let port = ep.port_or_default();
    let uri = ep.uri();
    let conn_err = |e: FtpError| CxError::Connection(format!("{uri}: {e}"));

    let mut ftp = match tokio::time::timeout(CONNECT_TIMEOUT, Ftp::connect((ep.host.as_str(), port))).await {
        Err(_) => return Err(CxError::Connection(format!("{uri}: timed out"))),
        Ok(r) => r.map_err(conn_err)?,
    };
    // Servers behind NAT often announce a private address in their PASV
    // reply; connecting to the control connection's address instead works
    // everywhere (FileZilla does the same).
    ftp.set_passive_nat_workaround(true);

    if ep.scheme == Scheme::Ftps {
        let refused = Arc::new(Mutex::new(None));
        let cfg = tls::client_config(&p.trust_store, &ep.host, port, refused.clone())?;
        let connector = AsyncRustlsConnector::from(suppaftp::tokio_rustls::TlsConnector::from(Arc::new(cfg)));
        ftp = match ftp.into_secure(connector, &ep.host).await {
            Ok(f) => f,
            Err(e) => {
                if let Some(r) = refused.lock().unwrap().take() {
                    return Err(CxError::HostKeyUnknown {
                        uri,
                        host: tls::host_pattern(&ep.host, port),
                        key_type: tls::KEY_TYPE.into(),
                        fingerprint: r.fingerprint,
                        changed: r.changed,
                    });
                }
                return Err(conn_err(e));
            }
        };
    }

    login(&mut ftp, p).await?;

    let features = match ftp.feat().await {
        Ok(f) => Features(f.into_keys().map(|k| k.to_ascii_uppercase()).collect()),
        Err(_) => Features::default(),
    };
    if features.has("UTF8") {
        let _ = ftp.custom_command("OPTS UTF8 ON", &[Status::CommandOk, Status::CommandNotImplemented]).await;
    }
    ftp.transfer_type(FileType::Binary).await.map_err(conn_err)?;
    Ok(Conn { ftp, features: Arc::new(features) })
}

/// Explicit credentials, else an anonymous attempt, else ask.
async fn login(ftp: &mut Ftp, p: &Params) -> Result<()> {
    let ep = &p.endpoint;
    let (user, pass, explicit) = match &p.creds {
        Some(Credentials { user, secret: Secret::Password { password } }) => {
            let user = if user.is_empty() { ep.user.clone().unwrap_or_default() } else { user.clone() };
            (user, password.clone(), true)
        }
        // A named account without a password can't sign in; only try
        // anonymous when no account was named.
        _ => match ep.user.as_deref() {
            Some(u) if !u.eq_ignore_ascii_case("anonymous") && !u.eq_ignore_ascii_case("ftp") => {
                return Err(CxError::AuthRequired { uri: ep.uri(), user: Some(u.to_string()), reason: "Password required".into() });
            }
            _ => ("anonymous".to_string(), "anonymous@".to_string(), false),
        },
    };
    match ftp.login(user.as_str(), pass.as_str()).await {
        Ok(()) => Ok(()),
        Err(FtpError::UnexpectedResponse(r)) if matches!(r.status, Status::NotLoggedIn | Status::InvalidCredentials | Status::LoginNeedAccount) => {
            let reason = if explicit { "Wrong password" } else { "Sign-in required" };
            Err(CxError::AuthRequired { uri: ep.uri(), user: explicit.then_some(user), reason: reason.into() })
        }
        Err(e) => Err(CxError::Connection(format!("{}: {e}", ep.uri()))),
    }
}
