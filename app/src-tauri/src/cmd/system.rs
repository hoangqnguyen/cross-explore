//! Handing things to the OS: default apps, the file manager, terminals.

use super::AppState;
use cx_core::{CxError, Location, Result, Scheme, WriteMode};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

fn spawn(cmd: &mut Command) -> Result<()> {
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| CxError::Io(format!("couldn't start {:?}: {e}", cmd.get_program())))
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

/// Open with a specific app (not the default). Remote files are downloaded
/// to a cache first, same as [`open_entry`].
#[tauri::command]
pub async fn open_entry_with(
    app_handle: AppHandle,
    uri: String,
    with: String,
    app: AppState<'_>,
) -> Result<()> {
    let loc = Location::parse(&uri)?;
    let path = match loc.local_path() {
        Some(p) => p.to_path_buf(),
        None => download(&app, &loc).await?,
    };
    app_handle
        .opener()
        .open_path(path.to_string_lossy(), Some(with.as_str()))
        .map_err(|e| CxError::Io(format!("cannot open {} with {with}: {e}", path.display())))
}

/// Apps that can open a file of this extension, for the "Open With" menu.
/// Best effort and OS-specific: there's no single portable API for it.
/// Async and off the main thread: it reads app bundles from disk.
#[tauri::command]
pub async fn apps_for_extension(ext: String) -> Vec<AppInfo> {
    tauri::async_runtime::spawn_blocking(move || find_apps(ext))
        .await
        .unwrap_or_default()
}

fn find_apps(ext: String) -> Vec<AppInfo> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    #[cfg(target_os = "macos")]
    return macos_apps::for_extension(&ext);
    #[cfg(all(target_os = "linux", not(target_os = "android")))]
    return linux_apps::for_extension(&ext);
    #[cfg(not(any(
        target_os = "macos",
        all(target_os = "linux", not(target_os = "android"))
    )))]
    {
        let _ = ext;
        Vec::new()
    }
}

#[derive(Serialize, Clone)]
pub struct AppInfo {
    name: String,
    /// What [`open_entry_with`] is given back as `with`: an app name on
    /// macOS, an executable path on Linux.
    id: String,
}

/// Windows has its own "Open With" picker; use it instead of enumerating
/// apps ourselves, since it already knows file associations better than we
/// could by scanning the registry.
#[tauri::command]
pub fn open_with_dialog(uri: String) -> Result<()> {
    let loc = Location::parse(&uri)?;
    let path = loc
        .local_path()
        .ok_or_else(|| CxError::Unsupported("Open With for a remote file".into()))?;
    #[cfg(windows)]
    return spawn(
        Command::new("rundll32").arg("shell32.dll,OpenAs_RunDLL").arg(path),
    );
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(CxError::Unsupported("the Open With dialog on this OS".into()))
    }
}

#[cfg(target_os = "macos")]
mod macos_apps {
    use super::AppInfo;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    /// Where `.app` bundles actually live. Not recursive beyond one extra
    /// level (e.g. `/Applications/Utilities`), since apps don't nest apps.
    fn roots() -> Vec<PathBuf> {
        let mut v = vec![
            PathBuf::from("/Applications"),
            PathBuf::from("/System/Applications"),
            PathBuf::from("/System/Applications/Utilities"),
        ];
        if let Some(home) = dirs::home_dir() {
            v.push(home.join("Applications"));
        }
        v
    }

    fn bundles_in(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "app") {
                out.push(p);
            } else if p.is_dir() {
                // One extra level only (e.g. a vendor subfolder), see `roots`.
                if let Ok(inner) = std::fs::read_dir(&p) {
                    for e in inner.flatten() {
                        let p = e.path();
                        if p.extension().is_some_and(|x| x == "app") {
                            out.push(p);
                        }
                    }
                }
            }
        }
    }

    /// `CFBundleName`/`CFBundleDisplayName`, falling back to the bundle's
    /// own file name, and the extensions it declares handling (classic
    /// `CFBundleDocumentTypes`; most apps still list these even when they
    /// also declare UTIs, which plist alone can't resolve to extensions).
    fn read_bundle(path: &Path) -> Option<(String, Vec<String>)> {
        let info = plist::Value::from_file(path.join("Contents/Info.plist")).ok()?;
        let dict = info.as_dictionary()?;
        let name = dict
            .get("CFBundleDisplayName")
            .or_else(|| dict.get("CFBundleName"))
            .and_then(|v| v.as_string())
            .map(str::to_owned)
            .unwrap_or_else(|| path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
        let mut exts = Vec::new();
        if let Some(types) = dict.get("CFBundleDocumentTypes").and_then(|v| v.as_array()) {
            for t in types {
                let Some(list) = t.as_dictionary().and_then(|d| d.get("CFBundleTypeExtensions")).and_then(|v| v.as_array()) else {
                    continue;
                };
                for x in list {
                    if let Some(s) = x.as_string() {
                        exts.push(s.to_ascii_lowercase());
                    }
                }
            }
        }
        Some((name, exts))
    }

    /// Every installed app with the extensions it opens. Reading hundreds of
    /// Info.plists takes a while, so the list is reused for a few minutes
    /// (long enough for a burst of "Open With" menus, short enough to pick up
    /// newly installed apps).
    fn index() -> Arc<Vec<(AppInfo, Vec<String>)>> {
        const FRESH: Duration = Duration::from_secs(300);
        type Index = Arc<Vec<(AppInfo, Vec<String>)>>;
        static CACHE: Mutex<Option<(Instant, Index)>> = Mutex::new(None);
        if let Some((at, list)) = &*CACHE.lock().unwrap() {
            if at.elapsed() < FRESH {
                return list.clone();
            }
        }
        let mut bundles = Vec::new();
        for r in roots() {
            bundles_in(&r, &mut bundles);
        }
        let list: Index = Arc::new(
            bundles
                .iter()
                .filter_map(|path| {
                    let (name, exts) = read_bundle(path)?;
                    Some((AppInfo { name, id: path.to_string_lossy().into_owned() }, exts))
                })
                .collect(),
        );
        *CACHE.lock().unwrap() = Some((Instant::now(), list.clone()));
        list
    }

    pub fn for_extension(ext: &str) -> Vec<AppInfo> {
        let mut matched = Vec::new();
        let mut all = Vec::new();
        for (info, exts) in index().iter() {
            let info = info.clone();
            if exts.iter().any(|e| e == ext || e == "*") {
                matched.push(info);
            } else {
                all.push(info);
            }
        }
        let mut list = if matched.is_empty() { all } else { matched };
        list.sort_by_key(|a| a.name.to_ascii_lowercase());
        list.dedup_by(|a, b| a.name == b.name);
        list
    }
}

#[cfg(all(target_os = "linux", not(target_os = "android")))]
mod linux_apps {
    use super::AppInfo;
    use std::path::PathBuf;
    use std::process::Command;

    fn desktop_dirs() -> Vec<PathBuf> {
        let mut v = vec![PathBuf::from("/usr/share/applications"), PathBuf::from("/usr/local/share/applications")];
        if let Some(home) = dirs::home_dir() {
            v.push(home.join(".local/share/applications"));
        }
        v
    }

    /// The `Exec=` line, minus the `%f`/`%u`/etc. field codes `open` doesn't
    /// understand (it runs the program directly with the path as the only
    /// argument, same as any other app here).
    fn exec_program(exec: &str) -> Option<String> {
        exec.split_whitespace().find(|t| !t.starts_with('%')).map(str::to_owned)
    }

    fn parse_desktop(path: &std::path::Path) -> Option<(String, String, Vec<String>)> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut name = None;
        let mut exec = None;
        let mut mimes = Vec::new();
        let mut no_display = false;
        let mut in_entry = false;
        for line in text.lines() {
            let line = line.trim();
            if line == "[Desktop Entry]" {
                in_entry = true;
            } else if line.starts_with('[') {
                in_entry = false;
            } else if in_entry {
                if let Some(v) = line.strip_prefix("Name=") {
                    name.get_or_insert_with(|| v.to_string());
                } else if let Some(v) = line.strip_prefix("Exec=") {
                    exec = exec_program(v);
                } else if let Some(v) = line.strip_prefix("MimeType=") {
                    mimes.extend(v.split(';').filter(|s| !s.is_empty()).map(str::to_owned));
                } else if line == "NoDisplay=true" || line == "Terminal=true" {
                    no_display = true;
                }
            }
        }
        if no_display {
            return None;
        }
        Some((name?, exec?, mimes))
    }

    pub fn for_extension(ext: &str) -> Vec<AppInfo> {
        // There's no portable ext→mimetype table; ask the desktop for it,
        // the same way the file manager would.
        let mime = Command::new("xdg-mime")
            .arg("query")
            .arg("filetype")
            .arg(format!("x.{ext}"))
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        let mut matched = Vec::new();
        let mut all = Vec::new();
        for dir in desktop_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let path = e.path();
                if path.extension().is_none_or(|x| x != "desktop") {
                    continue;
                }
                let Some((name, exec, mimes)) = parse_desktop(&path) else {
                    continue;
                };
                let info = AppInfo { name, id: exec };
                if mime.as_deref().is_some_and(|m| mimes.iter().any(|x| x == m)) {
                    matched.push(info);
                } else {
                    all.push(info);
                }
            }
        }
        let mut list = if matched.is_empty() { all } else { matched };
        list.sort_by_key(|a| a.name.to_ascii_lowercase());
        list.dedup_by(|a, b| a.name == b.name);
        list
    }
}

/// A local copy of a remote file, cached by URI + size + mtime. Concurrent
/// requests for the same file share one download, and the file only appears
/// under its real name once complete (so no app ever sees half of it).
async fn download(app: &AppState<'_>, loc: &Location) -> Result<PathBuf> {
    let provider = app.vfs.provider(loc).await?;
    let entry = provider.stat(loc).await?;
    if entry.is_dir {
        return Err(CxError::Unsupported(
            "opening remote folders in another app".into(),
        ));
    }
    let key = blake3::hash(format!("{}|{}|{:?}", loc.uri(), entry.size, entry.modified).as_bytes())
        .to_hex()
        .to_string();
    let dir = app.cache_dir.join("open").join(&key[..16]);
    let dest = dir.join(&entry.name);

    static INFLIGHT: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let lock = INFLIGHT
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .entry(key.clone())
        .or_default()
        .clone();
    let _guard = lock.lock().await;

    if tokio::fs::metadata(&dest)
        .await
        .map(|m| m.len() == entry.size)
        .unwrap_or(false)
    {
        return Ok(dest);
    }
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| CxError::from_io(e, dir.display()))?;
    let part = dir.join(format!(".{}.part", entry.name));
    // 1 MB buffers: the default 8 KB means tiny network reads and a
    // blocking-pool hop per 8 KB written.
    const BUF: usize = 1024 * 1024;
    let src = provider.open_read(loc, 0).await?;
    let dst = app
        .vfs
        .local()
        .open_write(&cx_core::Location::local(&part), WriteMode::Truncate)
        .await?;
    let mut src = tokio::io::BufReader::with_capacity(BUF, src);
    let mut dst = tokio::io::BufWriter::with_capacity(BUF, dst);
    tokio::io::copy_buf(&mut src, &mut dst)
        .await
        .map_err(|e| CxError::io("download failed", e))?;
    tokio::io::AsyncWriteExt::shutdown(&mut dst)
        .await
        .map_err(|e| CxError::io("download failed", e))?;
    tokio::fs::rename(&part, &dest)
        .await
        .map_err(|e| CxError::from_io(e, dest.display()))?;
    Ok(dest)
}

/// Local copies of remote files, for dragging them into other apps as real
/// files (mail attachments, chat, editors). Local files come back as-is.
#[tauri::command]
pub async fn stage_for_drag(uris: Vec<String>, app: AppState<'_>) -> Result<Vec<String>> {
    let mut out = Vec::with_capacity(uris.len());
    for uri in uris {
        let loc = Location::parse(&uri)?;
        let path = match loc.local_path() {
            Some(p) => p.to_path_buf(),
            None => download(&app, &loc).await?,
        };
        out.push(path.to_string_lossy().into_owned());
    }
    Ok(out)
}

/// Show the item selected in Finder / Explorer / the Linux file manager.
#[tauri::command]
pub fn reveal_entry(uri: String) -> Result<()> {
    let loc = Location::parse(&uri)?;
    let path = loc.local_path().ok_or_else(|| {
        CxError::Unsupported("showing remote items in the system file manager".into())
    })?;
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
        .args([
            "--session",
            "--print-reply",
            "--dest=org.freedesktop.FileManager1",
            "/org/freedesktop/FileManager1",
            "org.freedesktop.FileManager1.ShowItems",
        ])
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
            // No user means OpenSSH would assume the local account. Callers
            // ask first (and remember a successful login); don't guess here.
            let user = endpoint
                .user
                .as_deref()
                .filter(|u| !u.is_empty())
                .ok_or_else(|| CxError::AuthRequired {
                    uri: uri.clone(),
                    user: None,
                    reason: "SSH needs a user name. The local account is not assumed.".into(),
                })?;
            if !cx_term::acceptable_ssh_token(user)
                || !cx_term::acceptable_ssh_token(&endpoint.host)
            {
                return Err(CxError::InvalidLocation(format!(
                    "bad ssh target: {user}@{}",
                    endpoint.host
                )));
            }
            let quoted = path.replace('\'', "'\\''");
            let target = cx_term::user_at_host(user, &endpoint.host);
            let cmd = format!(
                "ssh -t -p {} {target} \"cd '{}' && exec \\$SHELL -l\"",
                endpoint.port_or_default(),
                quoted
            );
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
    spawn(Command::new("wt").arg("-d").arg(path)).or_else(|_| {
        spawn(
            Command::new("cmd")
                .args(["/C", "start", "cmd", "/K", "cd", "/d"])
                .arg(path),
        )
    })
}

#[cfg(windows)]
fn ssh_terminal(cmd: &str) -> Result<()> {
    spawn(Command::new("wt").args(["cmd", "/K", cmd]))
        .or_else(|_| spawn(Command::new("cmd").args(["/C", "start", "cmd", "/K", cmd])))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn local_terminal(path: &Path) -> Result<()> {
    for (bin, flag) in [
        ("x-terminal-emulator", "--working-directory"),
        ("gnome-terminal", "--working-directory"),
        ("konsole", "--workdir"),
        ("xfce4-terminal", "--working-directory"),
    ] {
        if spawn(Command::new(bin).arg(format!("{flag}={}", path.display()))).is_ok() {
            return Ok(());
        }
    }
    Err(CxError::Unsupported("no terminal emulator found".into()))
}

#[cfg(not(any(target_os = "macos", windows)))]
fn ssh_terminal(cmd: &str) -> Result<()> {
    for bin in [
        "x-terminal-emulator",
        "gnome-terminal",
        "konsole",
        "xfce4-terminal",
    ] {
        if spawn(Command::new(bin).args(["-e", "sh", "-c", cmd])).is_ok() {
            return Ok(());
        }
    }
    Err(CxError::Unsupported("no terminal emulator found".into()))
}

/// Put local files on the system clipboard, so Finder / Explorer can paste them.
#[tauri::command]
pub fn os_clipboard_set(uris: Vec<String>) -> Result<()> {
    let paths: Vec<String> = uris
        .iter()
        .filter_map(|u| {
            Location::parse(u)
                .ok()?
                .local_path()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .collect();
    if paths.is_empty() {
        return Ok(());
    }
    os_clipboard::set(paths)
}

/// Files another app copied (as URIs), if any.
#[tauri::command]
pub fn os_clipboard_get() -> Vec<String> {
    os_clipboard::get()
        .into_iter()
        .map(|p| Location::local(p).uri())
        .collect()
}

#[cfg(any(
    target_os = "macos",
    windows,
    all(target_os = "linux", not(target_os = "android"))
))]
mod os_clipboard {
    use clipboard_rs::{Clipboard, ClipboardContext};
    use cx_core::{CxError, Result};

    pub fn set(paths: Vec<String>) -> Result<()> {
        let ctx = ClipboardContext::new()
            .map_err(|e| CxError::Io(format!("clipboard unavailable: {e}")))?;
        ctx.set_files(paths)
            .map_err(|e| CxError::Io(format!("couldn't copy to the clipboard: {e}")))
    }

    pub fn get() -> Vec<String> {
        ClipboardContext::new()
            .ok()
            .and_then(|c| c.get_files().ok())
            .unwrap_or_default()
            .into_iter()
            .map(|f| f.strip_prefix("file://").map(str::to_owned).unwrap_or(f))
            .collect()
    }
}

#[cfg(not(any(
    target_os = "macos",
    windows,
    all(target_os = "linux", not(target_os = "android"))
)))]
mod os_clipboard {
    use cx_core::Result;

    pub fn set(_paths: Vec<String>) -> Result<()> {
        Ok(())
    }

    pub fn get() -> Vec<String> {
        Vec::new()
    }
}

/// Image shown under the cursor when dragging files out to other apps.
#[tauri::command]
pub fn drag_icon(app: AppState<'_>) -> Result<String> {
    let path = app.cache_dir.join("drag-icon.png");
    if !path.exists() {
        std::fs::write(&path, include_bytes!("../../icons/64x64.png"))
            .map_err(|e| CxError::from_io(e, path.display()))?;
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Whether the app may read every folder without macOS asking first (Full
/// Disk Access). Always true elsewhere. Checked by reading a folder that
/// only Full Disk Access unlocks.
#[tauri::command]
pub fn full_disk_access() -> bool {
    #[cfg(target_os = "macos")]
    {
        let Some(home) = dirs::home_dir() else {
            return true;
        };
        ["Library/Safari", "Library/Mail", "Library/Messages"]
            .iter()
            .map(|p| home.join(p))
            .filter(|p| p.exists())
            .any(|p| std::fs::read_dir(p).is_ok())
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// Open System Settings at Privacy & Security → Full Disk Access.
#[tauri::command]
pub fn open_full_disk_access_settings() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        spawn(
            Command::new("open")
                .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"),
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}
