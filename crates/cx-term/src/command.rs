//! What to run for a location: pure argv building, no processes spawned, so
//! the quoting rules can be unit-tested without a shell or a server.

use cx_core::{CxError, Endpoint, Location, Result, Scheme};
use std::path::PathBuf;

/// A program to start in a PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCommand {
    /// Program and arguments. Empty means "the user's login shell": on Unix
    /// `$SHELL` (falling back to the passwd entry, then `/bin/sh`) started
    /// with a `-name` argv\[0\], the convention every shell understands as
    /// "login shell" — unlike `-l`, which not all shells accept.
    pub argv: Vec<String>,
    /// Working directory. `None` starts in the home folder.
    pub cwd: Option<PathBuf>,
    /// True when the shell runs on this machine. Only then does the process's
    /// working directory mean anything to the file list (see
    /// [`Terminals::cwd`](crate::Terminals::cwd)).
    pub local: bool,
}

impl ShellCommand {
    /// The command that opens a terminal "in" `loc`:
    ///
    /// * local folder → the login shell in that folder;
    /// * a folder inside a local archive → the login shell next to the archive
    ///   (a shell can't `cd` into a zip);
    /// * `sftp://` → the system `ssh` client, see [`ShellCommand::ssh`].
    ///
    /// Other remote schemes have no shell to talk to and are `Unsupported`.
    pub fn for_location(loc: &Location) -> Result<ShellCommand> {
        match loc {
            Location::Local(dir) => Ok(ShellCommand::login_shell(dir.clone())),
            Location::Archive { container, .. } => match container.as_ref() {
                Location::Local(file) => {
                    let dir = file
                        .parent()
                        .ok_or_else(|| CxError::InvalidLocation(file.display().to_string()))?;
                    Ok(ShellCommand::login_shell(dir.to_path_buf()))
                }
                _ => Err(CxError::Unsupported(
                    "a terminal can't be opened inside a remote archive".into(),
                )),
            },
            Location::Remote { endpoint, path } if endpoint.scheme == Scheme::Sftp => {
                ShellCommand::ssh(endpoint, path)
            }
            Location::Remote { endpoint, .. } => Err(CxError::Unsupported(format!(
                "a terminal can't be opened on {} locations",
                endpoint.scheme.label()
            ))),
        }
    }

    /// The user's interactive shell in `dir`.
    ///
    /// On Windows there is no `$SHELL` and `COMSPEC` is cmd.exe, which few
    /// people want as a terminal: prefer PowerShell 7 (`pwsh`), then Windows
    /// PowerShell (always present), then `COMSPEC`.
    pub fn login_shell(dir: PathBuf) -> ShellCommand {
        ShellCommand {
            argv: default_shell_argv(),
            cwd: Some(dir),
            local: true,
        }
    }

    /// `ssh -t [-p port] -- [user@]host "cd '<path>'; exec $SHELL -l"`.
    ///
    /// The system client is used on purpose: it brings the user's
    /// `~/.ssh/config`, agent, known_hosts and interactive password/2FA
    /// prompts (which happen right in the terminal) for free.
    ///
    /// When `endpoint` has no user, none is sent and OpenSSH assumes the
    /// local account. The embedded terminal does not use that path: it asks
    /// for a user and calls [`ssh_with`](ShellCommand::ssh_with).
    ///
    /// The remote command is parsed by the remote login shell, so the path is
    /// single-quoted POSIX style (also valid in fish and csh). `;` rather than
    /// `&&` so a folder that vanished still leaves the user in a shell (with
    /// cd's error on screen) instead of a session that dies instantly. `--`
    /// and the leading-dash check keep a hostile host name from being read
    /// as an ssh option.
    pub fn ssh(endpoint: &Endpoint, path: &str) -> Result<ShellCommand> {
        ShellCommand::ssh_with(endpoint, path, None, &[])
    }

    /// [`ssh`](ShellCommand::ssh), with `user` forced onto the destination
    /// (when set) and extra client arguments inserted before `--`.
    pub fn ssh_with(
        endpoint: &Endpoint,
        path: &str,
        user: Option<&str>,
        options: &[String],
    ) -> Result<ShellCommand> {
        if !acceptable_ssh_token(&endpoint.host) {
            return Err(CxError::InvalidLocation(format!(
                "bad ssh host: {:?}",
                endpoint.host
            )));
        }
        let user = user.or(endpoint.user.as_deref());
        if let Some(user) = user {
            if !acceptable_ssh_token(user) {
                return Err(CxError::InvalidLocation(format!("bad ssh user: {user:?}")));
            }
        }
        // IPv6 needs brackets once a user is present: `pi@::1` is not a host.
        let dest = match user {
            Some(user) => user_at_host(user, &endpoint.host),
            None => endpoint.host.clone(),
        };

        let mut argv = vec!["ssh".to_string(), "-t".to_string()];
        argv.extend(options.iter().cloned());
        if let Some(port) = endpoint.port {
            argv.push("-p".into());
            argv.push(port.to_string());
        }
        argv.push("--".into());
        argv.push(dest);
        let path = if path.is_empty() { "/" } else { path };
        argv.push(format!("cd {}; exec $SHELL -l", posix_quote(path)));
        Ok(ShellCommand {
            argv,
            cwd: None,
            local: false,
        })
    }
}

/// `user@host`, with an IPv6 host in brackets (`user@[::1]`).
pub fn user_at_host(user: &str, host: &str) -> String {
    if host.contains(':') {
        format!("{user}@[{host}]")
    } else {
        format!("{user}@{host}")
    }
}

/// A host or user safe to place on an `ssh` command line: not empty, not an
/// option (`-o…`), and no whitespace or `@` that would change how ssh splits
/// `user@host`.
pub fn acceptable_ssh_token(s: &str) -> bool {
    !(s.is_empty()
        || s.starts_with('-')
        || s.chars()
            .any(|c| c == '@' || c.is_whitespace() || c.is_control()))
}

/// Quote `s` as one word for a POSIX shell: wrap in single quotes (inside
/// which nothing is special) and spell each `'` as `'\''`.
pub fn posix_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

#[cfg(not(windows))]
fn default_shell_argv() -> Vec<String> {
    Vec::new()
}

#[cfg(windows)]
fn default_shell_argv() -> Vec<String> {
    let on_path = |exe: &str| {
        std::env::var_os("PATH")
            .is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
    };
    let prog = if on_path("pwsh.exe") {
        "pwsh.exe".to_string()
    } else if on_path("powershell.exe") {
        "powershell.exe".to_string()
    } else {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
    };
    let mut argv = vec![prog.clone()];
    if prog.ends_with("pwsh.exe") || prog.ends_with("powershell.exe") {
        argv.push("-NoLogo".into());
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sftp(uri: &str) -> Vec<String> {
        ShellCommand::for_location(&Location::parse(uri).unwrap())
            .unwrap()
            .argv
    }

    #[test]
    fn quoting() {
        assert_eq!(posix_quote("/a b"), "'/a b'");
        assert_eq!(posix_quote("it's"), "'it'\\''s'");
        assert_eq!(posix_quote("$HOME `x` \"y\""), "'$HOME `x` \"y\"'");
    }

    #[test]
    fn ssh_plain() {
        assert_eq!(
            sftp("sftp://nas/home/pi"),
            ["ssh", "-t", "--", "nas", "cd '/home/pi'; exec $SHELL -l"]
        );
    }

    #[test]
    fn ssh_user_port_and_tricky_path() {
        let argv = sftp("sftp://pi@nas:2222/srv/it's%20a%20\"dir\"%20$(rm%20-rf)");
        assert_eq!(
            argv,
            [
                "ssh",
                "-t",
                "-p",
                "2222",
                "--",
                "pi@nas",
                "cd '/srv/it'\\''s a \"dir\" $(rm -rf)'; exec $SHELL -l"
            ]
        );
    }

    #[test]
    fn ssh_ipv6_and_root() {
        assert_eq!(
            sftp("sftp://[::1]/"),
            ["ssh", "-t", "--", "::1", "cd '/'; exec $SHELL -l"]
        );
        assert_eq!(
            sftp("sftp://pi@[::1]/home"),
            ["ssh", "-t", "--", "pi@[::1]", "cd '/home'; exec $SHELL -l"]
        );
    }

    #[test]
    fn ssh_rejects_option_injection() {
        let ep = Endpoint {
            scheme: Scheme::Sftp,
            user: None,
            host: "-oProxyCommand=x".into(),
            port: None,
        };
        assert!(ShellCommand::ssh(&ep, "/").is_err());
        let ep = Endpoint {
            scheme: Scheme::Sftp,
            user: Some("-x".into()),
            host: "h".into(),
            port: None,
        };
        assert!(ShellCommand::ssh(&ep, "/").is_err());
    }

    #[test]
    fn other_locations() {
        let tmp = std::env::temp_dir();
        let cmd = ShellCommand::for_location(&Location::Local(tmp.clone())).unwrap();
        assert!(cmd.local);
        assert_eq!(cmd.cwd, Some(tmp.clone()));

        let zip = Location::Archive {
            container: Box::new(Location::Local(tmp.join("x.zip"))),
            inner: "/docs".into(),
        };
        assert_eq!(ShellCommand::for_location(&zip).unwrap().cwd, Some(tmp));

        let smb = Location::parse("smb://host/share").unwrap();
        assert!(matches!(
            ShellCommand::for_location(&smb),
            Err(CxError::Unsupported(_))
        ));
    }
}
