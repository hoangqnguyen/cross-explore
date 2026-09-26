//! Opening an SSH connection: host key verification, authentication and the
//! SFTP channels.
//!
//! Each connection carries two SFTP channels. The high-level `SftpSession`
//! owns file handles (its `File` type pipelines reads and writes, which is
//! what makes transfers fast). The raw session serves everything else: it
//! lets listings stream `READDIR` replies as they arrive instead of waiting
//! for the whole folder, and it can send extension requests (`copy-data`,
//! `statvfs`) that the high-level API does not expose. Two channels also
//! mean a big listing never queues behind a transfer's packets.

use crate::known_hosts::{self, HostKeyStatus};
use cx_core::{CxError, Endpoint, Result, Secret};
use russh::client::{self, AuthResult, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::MethodKind;
use russh_sftp::client::{RawSftpSession, SftpSession};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Per-request SFTP timeout. Generous: a server deleting a huge file or
/// flushing a big write can legitimately take a while to answer.
const REQUEST_TIMEOUT_SECS: u64 = 60;
/// Agents can hold many keys; servers drop the connection after
/// `MaxAuthTries` (6 by default) failures, so only offer the first few.
const MAX_AGENT_KEYS: usize = 5;
const DEFAULT_KEYS: [&str; 3] = ["id_ed25519", "id_ecdsa", "id_rsa"];

/// Everything needed to (re)open a connection.
#[derive(Clone)]
pub(crate) struct ConnectParams {
    pub endpoint: Endpoint,
    pub creds: Option<cx_core::Credentials>,
    /// App store first, then `~/.ssh/known_hosts` (if any).
    pub known_hosts: Vec<PathBuf>,
    /// Where default identities (`id_ed25519`, …) are looked up.
    pub ssh_dir: Option<PathBuf>,
    pub use_agent: bool,
}

impl ConnectParams {
    /// Credentials' user (from the sign-in dialog) wins, then the URI's,
    /// then the local account name, like `ssh host` does.
    pub fn user(&self) -> String {
        self.creds
            .as_ref()
            .map(|c| c.user.clone())
            .filter(|u| !u.is_empty())
            .or_else(|| self.endpoint.user.clone())
            .unwrap_or_else(os_user)
    }
}

fn os_user() -> String {
    std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "root".into())
}

pub(crate) struct Session {
    ssh: Handle<Client>,
    pub sftp: SftpSession,
    pub raw: Arc<RawSftpSession>,
    /// Extensions the server advertised in its SFTP `VERSION` reply.
    pub extensions: HashMap<String, String>,
}

impl Session {
    pub fn is_closed(&self) -> bool {
        self.ssh.is_closed()
    }

    pub fn has_extension(&self, name: &str) -> bool {
        self.extensions.contains_key(name)
    }

    pub async fn disconnect(&self) {
        let _ = self.ssh.disconnect(russh::Disconnect::ByApplication, "", "en").await;
    }
}

/// russh calls this during key exchange. Rejecting the key aborts the
/// handshake with a generic error, so the verdict is parked in `verdict` for
/// `open` to turn into a proper `HostKeyUnknown`.
struct Client {
    host: String,
    port: u16,
    uri: String,
    files: Vec<PathBuf>,
    verdict: Arc<Mutex<Option<CxError>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> std::result::Result<bool, Self::Error> {
        let key = key.public_key();
        let files: Vec<&Path> = self.files.iter().map(PathBuf::as_path).collect();
        let status = known_hosts::check(&files, &self.host, self.port, &key);
        if status == HostKeyStatus::Trusted {
            return Ok(true);
        }
        *self.verdict.lock().unwrap() = Some(CxError::HostKeyUnknown {
            uri: self.uri.clone(),
            host: known_hosts::host_pattern(&self.host, self.port),
            key_type: known_hosts::key_type(&key),
            fingerprint: known_hosts::fingerprint(&key),
            changed: status == HostKeyStatus::Changed,
        });
        Ok(false)
    }
}

pub(crate) async fn open(p: &ConnectParams) -> Result<Session> {
    let ep = &p.endpoint;
    let port = ep.port_or_default();
    let verdict = Arc::new(Mutex::new(None));
    let handler = Client { host: ep.host.clone(), port, uri: ep.uri(), files: p.known_hosts.clone(), verdict: verdict.clone() };

    let cfg = client::Config {
        // Keepalives notice a dead link (sleeping laptop, dropped VPN) so
        // the next operation reconnects instead of hanging.
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        inactivity_timeout: None,
        nodelay: true,
        // Bigger windows keep more bytes in flight on high-latency links.
        window_size: 16 * 1024 * 1024,
        ..Default::default()
    };

    let host = ep.host.clone();
    let connecting = client::connect(Arc::new(cfg), (host.as_str(), port), handler);
    let mut ssh = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => return Err(CxError::Connection(format!("{}: timed out", ep.uri()))),
        Ok(Err(e)) => {
            if let Some(v) = verdict.lock().unwrap().take() {
                return Err(v);
            }
            return Err(CxError::Connection(format!("{}: {e}", ep.uri())));
        }
        Ok(Ok(h)) => h,
    };

    authenticate(&mut ssh, p).await?;

    let sftp_cfg = || russh_sftp::client::Config {
        request_timeout_secs: REQUEST_TIMEOUT_SECS,
        max_concurrent_reads: 32,
        max_concurrent_writes: 32,
        max_write_packet_len: 64 * 1024,
        ..Default::default()
    };
    let sftp = SftpSession::new_with_config(sftp_channel(&ssh).await?, sftp_cfg()).await.map_err(conn_err)?;
    let raw = RawSftpSession::new_with_config(sftp_channel(&ssh).await?, sftp_cfg());
    let version = raw.init().await.map_err(conn_err)?;
    Ok(Session { ssh, sftp, raw: Arc::new(raw), extensions: version.extensions })
}

async fn sftp_channel(ssh: &Handle<Client>) -> Result<russh::ChannelStream<client::Msg>> {
    let ch = ssh.channel_open_session().await.map_err(conn_err)?;
    ch.request_subsystem(true, "sftp").await.map_err(conn_err)?;
    Ok(ch.into_stream())
}

fn conn_err(e: impl std::fmt::Display) -> CxError {
    CxError::Connection(e.to_string())
}

/// Try credentials in the order the user expects: whatever they typed, else
/// the ssh-agent, else the default key files — the same things `ssh` tries.
async fn authenticate(ssh: &mut Handle<Client>, p: &ConnectParams) -> Result<()> {
    let user = p.user();
    let denied = |reason: &str| CxError::AuthRequired { uri: p.endpoint.uri(), user: Some(user.clone()), reason: reason.to_string() };
    let ssh_err = |e: russh::Error| CxError::Connection(format!("{}: {e}", p.endpoint.uri()));

    match p.creds.as_ref().map(|c| &c.secret) {
        Some(Secret::Password { password }) => {
            let res = ssh.authenticate_password(&user, password).await.map_err(ssh_err)?;
            let remaining = match res {
                AuthResult::Success => return Ok(()),
                AuthResult::Failure { remaining_methods, .. } => remaining_methods,
            };
            // Many servers (macOS, PAM setups) only take passwords through
            // keyboard-interactive.
            if remaining.contains(&MethodKind::KeyboardInteractive) && keyboard_interactive(ssh, &user, password).await.map_err(ssh_err)? {
                return Ok(());
            }
            if !remaining.contains(&MethodKind::Password) && !remaining.contains(&MethodKind::KeyboardInteractive) {
                return Err(denied("The server does not accept passwords; use a key"));
            }
            return Err(denied("Wrong password"));
        }
        Some(Secret::Key { path, passphrase }) => {
            let key = match russh::keys::load_secret_key(expand_tilde(path), passphrase.as_deref()) {
                Ok(k) => k,
                Err(russh::keys::Error::KeyIsEncrypted) => return Err(denied("The key is encrypted: enter its passphrase")),
                Err(_) if passphrase.is_some() => return Err(denied("Wrong key passphrase")),
                Err(e) => return Err(denied(&format!("Cannot read key {path}: {e}"))),
            };
            if publickey(ssh, &user, key).await.map_err(ssh_err)? {
                return Ok(());
            }
            return Err(denied("Key rejected by the server"));
        }
        Some(Secret::None) | None => {}
    }

    if p.use_agent && agent_auth(ssh, &user).await {
        return Ok(());
    }
    if let Some(dir) = &p.ssh_dir {
        for name in DEFAULT_KEYS {
            // Encrypted default keys are skipped: we can't prompt here, and the
            // sign-in dialog can pass the key with its passphrase explicitly.
            let Ok(key) = russh::keys::load_secret_key(dir.join(name), None) else { continue };
            if publickey(ssh, &user, key).await.map_err(ssh_err)? {
                return Ok(());
            }
        }
    }
    Err(denied("Sign-in required"))
}

async fn publickey(ssh: &mut Handle<Client>, user: &str, key: PrivateKey) -> std::result::Result<bool, russh::Error> {
    let hash = ssh.best_supported_rsa_hash().await?.flatten();
    let res = ssh.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await?;
    Ok(res.success())
}

async fn keyboard_interactive(ssh: &mut Handle<Client>, user: &str, password: &str) -> std::result::Result<bool, russh::Error> {
    let mut res = ssh.authenticate_keyboard_interactive_start(user, None).await?;
    // Answer every prompt with the password; bail out if the server keeps
    // asking (e.g. a second factor we can't answer).
    for _ in 0..4 {
        match res {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.to_string()).collect();
                res = ssh.authenticate_keyboard_interactive_respond(answers).await?;
            }
        }
    }
    Ok(false)
}

#[cfg(unix)]
async fn agent_auth(ssh: &mut Handle<Client>, user: &str) -> bool {
    use russh::keys::agent::client::AgentClient;
    if std::env::var_os("SSH_AUTH_SOCK").is_none() {
        return false;
    }
    let Ok(agent) = AgentClient::connect_env().await else { return false };
    agent_try(ssh, user, agent.dynamic()).await
}

#[cfg(windows)]
async fn agent_auth(ssh: &mut Handle<Client>, user: &str) -> bool {
    use russh::keys::agent::client::AgentClient;
    let agent = match AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
        Ok(a) => a.dynamic(),
        Err(_) => match AgentClient::connect_pageant().await {
            Ok(a) => a.dynamic(),
            Err(_) => return false,
        },
    };
    agent_try(ssh, user, agent).await
}

async fn agent_try(
    ssh: &mut Handle<Client>,
    user: &str,
    mut agent: russh::keys::agent::client::AgentClient<Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin + 'static>>,
) -> bool {
    use russh::keys::agent::AgentIdentity;
    let Ok(ids) = agent.request_identities().await else { return false };
    let hash = ssh.best_supported_rsa_hash().await.ok().flatten().flatten();
    for id in ids.into_iter().take(MAX_AGENT_KEYS) {
        let AgentIdentity::PublicKey { key, .. } = id else { continue };
        match ssh.authenticate_publickey_with(user, key, hash, &mut agent).await {
            Ok(AuthResult::Success) => return true,
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
    false
}

fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|h| h.join(rest)).unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}
