//! SSH sessions for SFTP folders: an explicit remote user, a debug log so we
//! can see *how* authentication succeeded, and (on Unix) a control socket so
//! a password login can install a public key without asking again.

use crate::command::{acceptable_ssh_token, user_at_host, ShellCommand};
use crate::{CxError, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

static NEXT: AtomicU64 = AtomicU64::new(1);

/// A fresh id for log files and control sockets.
pub fn next_ssh_id() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// How OpenSSH authenticated. Parsed from `ssh -v` debug output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMethod {
    PublicKey,
    Password,
    KeyboardInteractive,
    Other,
}

impl AuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthMethod::PublicKey => "publickey",
            AuthMethod::Password => "password",
            AuthMethod::KeyboardInteractive => "keyboard-interactive",
            AuthMethod::Other => "other",
        }
    }

    pub fn parse(name: &str) -> AuthMethod {
        match name.trim() {
            "publickey" => AuthMethod::PublicKey,
            "password" => AuthMethod::Password,
            "keyboard-interactive" => AuthMethod::KeyboardInteractive,
            _ => AuthMethod::Other,
        }
    }

    /// The user typed a secret. A key is not what let them in, so offering
    /// to install one is useful.
    pub fn needs_key(self) -> bool {
        matches!(self, AuthMethod::Password | AuthMethod::KeyboardInteractive)
    }
}

/// The last successful method in an `ssh -v` log, if authentication finished.
pub fn parse_auth_method(log: &str) -> Option<AuthMethod> {
    let mut found = None;
    for line in log.lines() {
        if let Some(rest) = line.split("Authentication succeeded (").nth(1) {
            if let Some(name) = rest.split(')').next() {
                found = Some(AuthMethod::parse(name));
            }
        } else if let Some(i) = line.find(" using \"") {
            if let Some(name) = line[i + " using \"".len()..].split('"').next() {
                found = Some(AuthMethod::parse(name));
            }
        }
    }
    found
}

/// What to run, plus the files that record the login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshSession {
    pub command: ShellCommand,
    pub user: String,
    pub host: String,
    pub port: u16,
    pub log_file: PathBuf,
    /// Unix domain socket for a multiplexed master. `None` on Windows, where
    /// OpenSSH has no ControlMaster, and when the path would be too long.
    pub control_path: Option<PathBuf>,
    /// `Some(secs)` keeps the master alive after the foreground `ssh` exits
    /// (the terminal UI suspends into ssh, then asks about the key). The
    /// embedded terminal uses `None`: the session itself is the master.
    pub control_persist_secs: Option<u64>,
}

/// Build `ssh -t` for `user` on `endpoint`. `user` is always sent; OpenSSH
/// would otherwise assume the local account.
pub fn prepare_ssh(
    endpoint: &cx_core::Endpoint,
    path: &str,
    user: &str,
    log_file: PathBuf,
    control_path: Option<PathBuf>,
    control_persist_secs: Option<u64>,
) -> Result<SshSession> {
    if !acceptable_ssh_token(user) {
        return Err(CxError::InvalidLocation(format!("bad ssh user: {user:?}")));
    }
    if let Some(dir) = log_file.parent() {
        fs::create_dir_all(dir).map_err(|e| CxError::from_io(e, dir.display()))?;
    }
    let port = endpoint.port_or_default();
    let mut options = vec![
        "-v".to_string(),
        "-E".to_string(),
        log_file.display().to_string(),
    ];
    if let Some(cp) = &control_path {
        if let Some(dir) = cp.parent() {
            fs::create_dir_all(dir).map_err(|e| CxError::from_io(e, dir.display()))?;
        }
        options.push("-o".into());
        options.push("ControlMaster=yes".into());
        options.push("-o".into());
        options.push(format!("ControlPath={}", cp.display()));
        options.push("-o".into());
        match control_persist_secs {
            Some(secs) => options.push(format!("ControlPersist={secs}")),
            None => options.push("ControlPersist=no".into()),
        }
    }
    // Port is in `options` so it isn't added twice. Clearing it on the copy
    // keeps [`ShellCommand::ssh_with`] from appending its own `-p`.
    let mut ep = endpoint.clone();
    ep.port = None;
    options.push("-p".into());
    options.push(port.to_string());
    let command = ShellCommand::ssh_with(&ep, path, Some(user), &options)?;
    Ok(SshSession {
        command,
        user: user.to_string(),
        host: endpoint.host.clone(),
        port,
        log_file,
        control_path,
        control_persist_secs,
    })
}

/// A private directory for control sockets. `None` when multiplexing isn't
/// available or the path would exceed the Unix socket limit.
pub fn control_socket(id: u64) -> Option<PathBuf> {
    #[cfg(not(unix))]
    {
        let _ = id;
        None
    }
    #[cfg(unix)]
    {
        let home = dirs::home_dir()?;
        let ssh_dir = home.join(".ssh");
        let created = !ssh_dir.exists();
        fs::create_dir_all(&ssh_dir).ok()?;
        use std::os::unix::fs::PermissionsExt;
        if created {
            let _ = fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700));
        }
        let dir = ssh_dir.join("cx-cm");
        fs::create_dir_all(&dir).ok()?;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        sweep_old(&dir);
        let path = dir.join(format!("s{id}"));
        // macOS rejects socket paths at 104 bytes, including the trailing NUL.
        if path.as_os_str().len() >= 100 {
            return None;
        }
        Some(path)
    }
}

#[cfg(unix)]
fn sweep_old(dir: &Path) {
    let Some(old) = SystemTime::now().checked_sub(Duration::from_secs(24 * 3600)) else {
        return;
    };
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .is_some_and(|t| t < old);
        if stale {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Poll `log` until authentication succeeds or `stop` is set, then call
/// `on_done` once. The log file is removed afterwards.
pub fn watch_auth_log(
    log: PathBuf,
    stop: Arc<AtomicBool>,
    on_done: impl FnOnce(Option<AuthMethod>) + Send + 'static,
) {
    let _ = thread::Builder::new()
        .name("cx-ssh-auth".into())
        .spawn(move || {
            let method = loop {
                let found = fs::read_to_string(&log)
                    .ok()
                    .and_then(|t| parse_auth_method(&t));
                if found.is_some() {
                    break found;
                }
                if stop.load(Ordering::SeqCst) {
                    break fs::read_to_string(&log)
                        .ok()
                        .and_then(|t| parse_auth_method(&t));
                }
                thread::sleep(Duration::from_millis(40));
            };
            let _ = fs::remove_file(&log);
            on_done(method);
        });
}

const REMOTE_INSTALL: &str = "umask 077; mkdir -p \"$HOME/.ssh\"; chmod 700 \"$HOME/.ssh\"; key=$(head -n 1); test -n \"$key\" || exit 1; touch \"$HOME/.ssh/authorized_keys\"; grep -qxF \"$key\" \"$HOME/.ssh/authorized_keys\" || printf '%s\\n' \"$key\" >> \"$HOME/.ssh/authorized_keys\"; chmod 600 \"$HOME/.ssh/authorized_keys\"";

/// Append the default public key to `user@host:~/.ssh/authorized_keys`.
///
/// Uses the live control socket when there is one, so a password isn't needed
/// again. Otherwise `password` is handed to `ssh` through `SSH_ASKPASS` and
/// never written to disk. Creates `~/.ssh/id_ed25519` when the user has no key.
pub fn install_public_key(
    user: &str,
    host: &str,
    port: u16,
    control_path: Option<&Path>,
    password: Option<&str>,
) -> Result<String> {
    if !acceptable_ssh_token(user) || !acceptable_ssh_token(host) {
        return Err(CxError::InvalidLocation(format!(
            "bad ssh target: {user}@{host}"
        )));
    }
    let pub_path = ensure_pubkey()?;
    let key_text =
        fs::read_to_string(&pub_path).map_err(|e| CxError::from_io(e, pub_path.display()))?;
    let key = key_text.lines().next().unwrap_or("").trim();
    if !(key.starts_with("ssh-") || key.starts_with("ecdsa-") || key.starts_with("sk-")) {
        return Err(CxError::Io(format!(
            "{} is not an OpenSSH public key",
            pub_path.display()
        )));
    }

    let mut cmd = Command::new("ssh");
    cmd.arg("-o").arg("ConnectTimeout=15");
    cmd.arg("-o").arg("NumberOfPasswordPrompts=1");
    if let Some(cp) = control_path {
        if !wait_for_socket(cp) {
            return Err(CxError::Io(
                "the SSH session ended before the key could be copied".into(),
            ));
        }
        cmd.arg("-o").arg("BatchMode=yes");
        cmd.arg("-o").arg("ControlMaster=no");
        cmd.arg("-o").arg(format!("ControlPath={}", cp.display()));
    } else if password.is_some() {
        cmd.arg("-o")
            .arg("PreferredAuthentications=password,keyboard-interactive");
        cmd.arg("-o").arg("NumberOfPasswordPrompts=1");
    } else {
        return Err(CxError::Io(
            "this SSH session can't install a key without the password".into(),
        ));
    }
    cmd.arg("-p").arg(port.to_string());
    cmd.arg("--");
    cmd.arg(user_at_host(user, host));
    cmd.arg(REMOTE_INSTALL);
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let askpass = if control_path.is_none() {
        Some(AskPass::new(password.unwrap())?)
    } else {
        None
    };
    if let Some(ask) = &askpass {
        ask.apply(&mut cmd);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| CxError::io("start ssh to copy the key", e))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(key.as_bytes())
            .and_then(|_| stdin.write_all(b"\n"))
            .map_err(|e| CxError::io("send the public key", e))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| CxError::io("copy the SSH key", e))?;
    drop(askpass);
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let err = err.trim();
        let detail = if err.is_empty() {
            format!("ssh exited with {}", output.status)
        } else {
            err.to_string()
        };
        return Err(CxError::Io(format!("could not copy the key: {detail}")));
    }
    Ok(format!(
        "Copied {} to {user}@{host}. Later SSH sessions can use the key.",
        pub_path.display()
    ))
}

fn wait_for_socket(path: &Path) -> bool {
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(30));
    }
    path.exists()
}

fn ensure_pubkey() -> Result<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| CxError::Io("no home directory for an SSH key".into()))?;
    let ssh_dir = home.join(".ssh");
    for name in ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"] {
        let path = ssh_dir.join(name);
        if path.is_file() {
            return Ok(path);
        }
    }
    let created = !ssh_dir.exists();
    fs::create_dir_all(&ssh_dir).map_err(|e| CxError::from_io(e, ssh_dir.display()))?;
    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700));
    }
    let private = ssh_dir.join("id_ed25519");
    let status = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-q", "-C", "cross-explore", "-f"])
        .arg(&private)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()
        .map_err(|e| CxError::io("start ssh-keygen", e))?;
    if !status.success() {
        return Err(CxError::Io(
            "couldn't create an SSH key (~/.ssh/id_ed25519)".into(),
        ));
    }
    Ok(ssh_dir.join("id_ed25519.pub"))
}

/// A short-lived askpass program. The password stays in the child environment.
struct AskPass {
    program: PathBuf,
    password: String,
    _dir: tempfile::TempDir,
}

impl AskPass {
    fn new(password: &str) -> Result<AskPass> {
        let dir = tempfile::tempdir().map_err(|e| CxError::io("create askpass dir", e))?;
        #[cfg(unix)]
        let program = {
            let path = dir.path().join("askpass");
            fs::write(&path, "#!/bin/sh\nprintf '%s\\n' \"$CX_SSH_PASS\"\n")
                .map_err(|e| CxError::from_io(e, path.display()))?;
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
                .map_err(|e| CxError::from_io(e, path.display()))?;
            path
        };
        #[cfg(not(unix))]
        let program = {
            let path = dir.path().join("askpass.cmd");
            fs::write(&path, "@echo off\r\npowershell -NoProfile -Command \"[Console]::Out.WriteLine($env:CX_SSH_PASS)\"\r\n").map_err(|e| CxError::from_io(e, path.display()))?;
            path
        };
        Ok(AskPass {
            program,
            password: password.to_string(),
            _dir: dir,
        })
    }

    fn apply(&self, cmd: &mut Command) {
        cmd.env("CX_SSH_PASS", &self.password);
        cmd.env("SSH_ASKPASS", &self.program);
        cmd.env("SSH_ASKPASS_REQUIRE", "force");
        if std::env::var_os("DISPLAY").is_none() {
            cmd.env("DISPLAY", ":0");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cx_core::{Endpoint, Scheme};

    fn ep() -> Endpoint {
        Endpoint {
            scheme: Scheme::Sftp,
            user: None,
            host: "nas".into(),
            port: Some(2222),
        }
    }

    #[test]
    fn parse_methods() {
        assert_eq!(
            parse_auth_method("debug1: Authentications that can continue: publickey,password\n"),
            None
        );
        assert_eq!(
            parse_auth_method("debug1: Authentication succeeded (publickey).\n"),
            Some(AuthMethod::PublicKey)
        );
        let both = "debug1: Authentication succeeded (publickey).\ndebug1: Authentication succeeded (password).\n";
        assert_eq!(parse_auth_method(both), Some(AuthMethod::Password));
        assert!(AuthMethod::Password.needs_key());
        assert!(!AuthMethod::PublicKey.needs_key());
        let alt = "Authenticated to nas ([10.0.0.2]:22) using \"keyboard-interactive\".\n";
        assert_eq!(
            parse_auth_method(alt),
            Some(AuthMethod::KeyboardInteractive)
        );
    }

    #[test]
    fn argv_forces_the_user_and_records_auth() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("ssh.log");
        let session = prepare_ssh(&ep(), "/srv/it's", "pi", log.clone(), None, None).unwrap();
        assert_eq!(session.user, "pi");
        assert_eq!(session.port, 2222);
        assert!(session.control_path.is_none());
        let argv = session.command.argv;
        assert_eq!(argv[0], "ssh");
        assert!(argv.iter().any(|a| a == "-v"));
        let e = argv.iter().position(|a| a == "-E").unwrap();
        assert_eq!(argv[e + 1], log.display().to_string());
        assert!(argv.windows(2).any(|w| w[0] == "-p" && w[1] == "2222"));
        assert!(argv.iter().any(|a| a == "pi@nas"));
        assert!(argv.last().unwrap().contains("cd '/srv/it'\\''s'"));
        assert!(!session.command.local);
    }

    #[test]
    fn rejects_a_user_that_could_be_an_option() {
        let dir = tempfile::tempdir().unwrap();
        let err = prepare_ssh(
            &ep(),
            "/",
            "-oProxyCommand=x",
            dir.path().join("l"),
            None,
            None,
        );
        assert!(err.is_err());
    }
}
