//! Handing things to the OS: default apps, the file manager, terminals.

use super::AppState;
use cx_core::{CxError, Location, Result, Scheme, WriteMode};
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

fn spawn(cmd: &mut Command) -> Result<()> {
    cmd.spawn().map(|_| ()).map_err(|e| CxError::Io(format!("couldn't start {:?}: {e}", cmd.get_program())))
}

/// Open with the default app. Remote files are downloaded to a cache first.
#[tauri::command]
pub async fn open_entry(app_handle: AppHandle, uri: String, app: AppState<'_>) -> Result<()> {
    let loc = Location::parse(&uri)?;
    let path = match loc.local_path() {
        Some(p) => p.to_path_buf(),
        None => download(&app, &loc).await?,
    };
    app_handle
        .opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| CxError::Io(format!("cannot open {}: {e}", path.display())))
}

async fn download(app: &AppState<'_>, loc: &Location) -> Result<PathBuf> {
    let provider = app.vfs.provider(loc).await?;
    let entry = provider.stat(loc).await?;
    if entry.is_dir {
        return Err(CxError::Unsupported("opening remote folders in another app".into()));
    }
    let key = blake3::hash(format!("{}|{}|{:?}", loc.uri(), entry.size, entry.modified).as_bytes()).to_hex();
    let dir = app.cache_dir.join("open").join(&key[..16]);
    let dest = dir.join(&entry.name);
    if tokio::fs::metadata(&dest).await.map(|m| m.len() == entry.size).unwrap_or(false) {
        return Ok(dest);
    }
    tokio::fs::create_dir_all(&dir).await.map_err(|e| CxError::from_io(e, dir.display()))?;
    let mut src = provider.open_read(loc, 0).await?;
    let local = cx_core::Location::local(&dest);
    let mut dst = app.vfs.local().open_write(&local, WriteMode::Truncate).await?;
    tokio::io::copy(&mut src, &mut dst).await.map_err(|e| CxError::io("download failed", e))?;
    tokio::io::AsyncWriteExt::shutdown(&mut dst).await.map_err(|e| CxError::io("download failed", e))?;
    Ok(dest)
}

/// Show the item selected in Finder / Explorer / the Linux file manager.
#[tauri::command]
pub fn reveal_entry(uri: String) -> Result<()> {
    let loc = Location::parse(&uri)?;
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
        .status();
    if matches!(dbus, Ok(s) if s.success()) {
        return Ok(());
    }
    spawn(Command::new("xdg-open").arg(path.parent().unwrap_or(path)))
}

/// Open a terminal here. For SFTP folders, an SSH session in that folder.
#[tauri::command]
pub fn open_terminal(uri: String) -> Result<()> {
    let loc = Location::parse(&uri)?;
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
