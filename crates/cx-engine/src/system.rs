//! Handing things to the OS: default apps, the file manager, terminal
//! windows, the system clipboard.
//!
//! Every spawned program gets null stdio, so it can't scribble over a
//! terminal UI that is running in the same terminal.

use crate::Engine;
use cx_core::{CxError, Location, Result, Scheme, WriteMode};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn spawn(cmd: &mut Command) -> Result<()> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    cmd.spawn().map(|_| ()).map_err(|e| CxError::Io(format!("couldn't start {:?}: {e}", cmd.get_program())))
}

/// Open a local file or folder with its default app.
pub fn open_path(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    return spawn(Command::new("open").arg(path));
    #[cfg(windows)]
    return spawn(Command::new("cmd").args(["/C", "start", ""]).arg(path));
    #[cfg(not(any(target_os = "macos", windows)))]
    return spawn(Command::new("xdg-open").arg(path));
}

/// Open a local file with a specific program (`$EDITOR`-style), detached.
pub fn open_with(program: &str, path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    return spawn(Command::new("open").args(["-a", program]).arg(path));
    #[cfg(not(target_os = "macos"))]
    return spawn(Command::new(program).arg(path));
}

impl Engine {
    /// A local path for `uri`: itself when local, otherwise a copy
    /// downloaded to the cache (reused while size and date match).
    pub async fn local_copy(&self, uri: &str) -> Result<PathBuf> {
        let loc = Location::parse(uri)?;
        if let Some(p) = loc.local_path() {
            return Ok(p.to_path_buf());
        }
        let provider = self.vfs.provider(&loc).await?;
        let entry = provider.stat(&loc).await?;
        if entry.is_dir {
            return Err(CxError::Unsupported("opening remote folders in another app".into()));
        }
        let key = blake3::hash(format!("{}|{}|{:?}", loc.uri(), entry.size, entry.modified).as_bytes()).to_hex();
        let dir = self.cache_dir.join("open").join(&key[..16]);
        let dest = dir.join(&entry.name);
        if tokio::fs::metadata(&dest).await.map(|m| m.len() == entry.size).unwrap_or(false) {
            return Ok(dest);
        }
        tokio::fs::create_dir_all(&dir).await.map_err(|e| CxError::from_io(e, dir.display()))?;
        let mut src = provider.open_read(&loc, 0).await?;
        let local = Location::local(&dest);
        let mut dst = self.vfs.local().open_write(&local, WriteMode::Truncate).await?;
        tokio::io::copy(&mut src, &mut dst).await.map_err(|e| CxError::io("download failed", e))?;
        tokio::io::AsyncWriteExt::shutdown(&mut dst).await.map_err(|e| CxError::io("download failed", e))?;
        Ok(dest)
    }

    /// Open with the default app. Remote files are downloaded first.
    pub async fn open_entry(&self, uri: &str) -> Result<()> {
        let path = self.local_copy(uri).await?;
        open_path(&path)
    }
}

/// Show the item selected in Finder / Explorer / the Linux file manager.
pub fn reveal_entry(uri: &str) -> Result<()> {
    let loc = Location::parse(uri)?;
    let path = loc.local_path().ok_or_else(|| CxError::Unsupported("showing remote items in the system file manager".into()))?;
    reveal(path)
}

#[cfg(target_os = "macos")]
fn reveal(path: &Path) -> Result<()> {
    spawn(Command::new("open").arg("-R").arg(path))
}

#[cfg(windows)]
fn reveal(path: &Path) -> Result<()> {
    spawn(Command::new("explorer").arg(format!("/select,{}", path.display())))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn reveal(path: &Path) -> Result<()> {
    // The FileManager1 D-Bus interface selects the item; fall back to opening the folder.
    let uri = Location::local(path).uri();
    let dbus = Command::new("dbus-send")
        .args(["--session", "--print-reply", "--dest=org.freedesktop.FileManager1", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems"])
        .arg(format!("array:string:{uri}"))
        .arg("string:")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if matches!(dbus, Ok(s) if s.success()) {
        return Ok(());
    }
    spawn(Command::new("xdg-open").arg(path.parent().unwrap_or(path)))
}

/// Open a terminal *window* here. For SFTP folders, an SSH session in that
/// folder. (A terminal UI runs the shell in place instead; see
/// [`cx_term::ShellCommand`].)
pub fn open_terminal(uri: &str) -> Result<()> {
    let loc = Location::parse(uri)?;
    match &loc {
        Location::Local(p) => local_terminal(p),
        Location::Remote { endpoint, path } if endpoint.scheme == Scheme::Sftp => {
            let target = match &endpoint.user {
                Some(u) => format!("{u}@{}", endpoint.host),
                None => endpoint.host.clone(),
            };
            let quoted = path.replace('\'', "'\\''");
            let cmd = format!("ssh -t -p {} {} \"cd '{}' && exec \\$SHELL -l\"", endpoint.port_or_default(), target, quoted);
            ssh_terminal(&cmd)
        }
        _ => Err(CxError::Unsupported("a terminal for this location".into())),
    }
}

#[cfg(target_os = "macos")]
fn local_terminal(path: &Path) -> Result<()> {
    spawn(Command::new("open").args(["-a", "Terminal"]).arg(path))
}

#[cfg(target_os = "macos")]
fn ssh_terminal(cmd: &str) -> Result<()> {
    let script = format!("tell application \"Terminal\" to do script \"{}\"\ntell application \"Terminal\" to activate", cmd.replace('\\', "\\\\").replace('"', "\\\""));
    spawn(Command::new("osascript").arg("-e").arg(script))
}

#[cfg(windows)]
fn local_terminal(path: &Path) -> Result<()> {
    spawn(Command::new("wt").arg("-d").arg(path)).or_else(|_| spawn(Command::new("cmd").args(["/C", "start", "cmd", "/K", "cd", "/d"]).arg(path)))
}

#[cfg(windows)]
fn ssh_terminal(cmd: &str) -> Result<()> {
    spawn(Command::new("wt").args(["cmd", "/K", cmd])).or_else(|_| spawn(Command::new("cmd").args(["/C", "start", "cmd", "/K", cmd])))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn local_terminal(path: &Path) -> Result<()> {
    for (bin, flag) in [("x-terminal-emulator", "--working-directory"), ("gnome-terminal", "--working-directory"), ("konsole", "--workdir"), ("xfce4-terminal", "--working-directory")] {
        if spawn(Command::new(bin).arg(format!("{flag}={}", path.display()))).is_ok() {
            return Ok(());
        }
    }
    Err(CxError::Unsupported("no terminal emulator found".into()))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn ssh_terminal(cmd: &str) -> Result<()> {
    for bin in ["x-terminal-emulator", "gnome-terminal", "konsole", "xfce4-terminal"] {
        if spawn(Command::new(bin).args(["-e", "sh", "-c", cmd])).is_ok() {
            return Ok(());
        }
    }
    Err(CxError::Unsupported("no terminal emulator found".into()))
}

/// Put local files on the system clipboard, so Finder / Explorer can paste them.
pub fn os_clipboard_set(uris: &[String]) -> Result<()> {
    let paths: Vec<String> = uris.iter().filter_map(|u| Location::parse(u).ok()?.local_path().map(|p| p.to_string_lossy().into_owned())).collect();
    if paths.is_empty() {
        return Ok(());
    }
    os_clipboard::set(paths)
}

/// Files another app copied (as URIs), if any.
pub fn os_clipboard_get() -> Vec<String> {
    os_clipboard::get().into_iter().map(|p| Location::local(p).uri()).collect()
}

#[cfg(any(target_os = "macos", windows, all(target_os = "linux", not(target_os = "android"))))]
mod os_clipboard {
    use clipboard_rs::{Clipboard, ClipboardContext};
    use cx_core::{CxError, Result};

    pub fn set(paths: Vec<String>) -> Result<()> {
        let ctx = ClipboardContext::new().map_err(|e| CxError::Io(format!("clipboard unavailable: {e}")))?;
        ctx.set_files(paths).map_err(|e| CxError::Io(format!("couldn't copy to the clipboard: {e}")))
    }

    pub fn get() -> Vec<String> {
        ClipboardContext::new().ok().and_then(|c| c.get_files().ok()).unwrap_or_default().into_iter().map(|f| f.strip_prefix("file://").map(str::to_owned).unwrap_or(f)).collect()
    }
}

#[cfg(not(any(target_os = "macos", windows, all(target_os = "linux", not(target_os = "android")))))]
mod os_clipboard {
    use cx_core::Result;

    pub fn set(_paths: Vec<String>) -> Result<()> {
        Ok(())
    }

    pub fn get() -> Vec<String> {
        Vec::new()
    }
}

/// Whether the app may read every folder without macOS asking first (Full
/// Disk Access). Always true elsewhere. Checked by reading a folder that
/// only Full Disk Access unlocks.
pub fn full_disk_access() -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(home) = dirs::home_dir() else { return true };
        ["Library/Safari", "Library/Mail", "Library/Messages"].iter().map(|p| home.join(p)).filter(|p| p.exists()).any(|p| std::fs::read_dir(p).is_ok())
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Open System Settings at Privacy & Security → Full Disk Access.
pub fn open_full_disk_access_settings() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        spawn(Command::new("open").arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}
