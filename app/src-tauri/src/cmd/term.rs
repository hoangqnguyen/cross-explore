//! Embedded terminal sessions (a PTY per panel), streamed to xterm.js.
//!
//! SFTP folders open the system `ssh`. The UI asks for the remote user first
//! (OpenSSH would otherwise assume the local account, which is often wrong)
//! and passes it in the URI. A successful login is remembered. A password
//! login emits `authenticated` so the UI can offer to install an SSH key on
//! the same connection.

use crate::cmd::AppState;
use cx_core::{CxError, Location, Result, Scheme};
use cx_term::{
    control_socket, install_public_key, next_ssh_id, prepare_ssh, watch_auth_log, SshUsers,
    TermEvent, Terminals,
};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use tauri::ipc::Channel;
use tauri::State;

fn forward(on_event: Channel<serde_json::Value>) -> impl Fn(TermEvent) + Send + Sync + 'static {
    move |e| emit(&on_event, &e)
}

fn emit(on_event: &Channel<serde_json::Value>, event: &TermEvent) {
    if let Ok(v) = serde_json::to_value(event) {
        let _ = on_event.send(v);
    }
}

#[tauri::command]
pub fn term_open(
    uri: String,
    cols: u16,
    rows: u16,
    on_event: Channel<serde_json::Value>,
    terms: State<'_, Terminals>,
    app: AppState<'_>,
) -> Result<u64> {
    let loc = Location::parse(&uri)?;
    let Location::Remote { endpoint, path } = &loc else {
        return terms.open(&loc, cols, rows, forward(on_event));
    };
    if endpoint.scheme != Scheme::Sftp {
        return terms.open(&loc, cols, rows, forward(on_event));
    }
    open_ssh(endpoint, path, &uri, cols, rows, on_event, &*terms, &**app)
}

#[cfg(any(target_os = "ios", target_os = "android"))]
fn open_ssh(
    endpoint: &cx_core::Endpoint,
    path: &str,
    uri: &str,
    cols: u16,
    rows: u16,
    on_event: Channel<serde_json::Value>,
    terms: &Terminals,
    _app: &crate::state::App,
) -> Result<u64> {
    let _ = (endpoint, path, uri);
    terms.open(&Location::parse(uri)?, cols, rows, forward(on_event))
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn open_ssh(
    endpoint: &cx_core::Endpoint,
    path: &str,
    uri: &str,
    cols: u16,
    rows: u16,
    on_event: Channel<serde_json::Value>,
    terms: &Terminals,
    app: &crate::state::App,
) -> Result<u64> {
    let user = endpoint
        .user
        .clone()
        .filter(|u| !u.is_empty())
        .ok_or_else(|| CxError::AuthRequired {
            uri: uri.to_string(),
            user: None,
            reason: "SSH needs a user name. The local account is not assumed.".into(),
        })?;

    let tag = next_ssh_id();
    let log_file = app
        .cache_dir
        .join("ssh-logs")
        .join(format!("ssh-{tag}.log"));
    let control = control_socket(tag);
    let launch = prepare_ssh(
        endpoint,
        path,
        &user,
        log_file.clone(),
        control.clone(),
        None,
    )?;

    let stop = Arc::new(AtomicBool::new(false));
    let exit_code = Arc::new(AtomicI32::new(-1));
    let stop_for_events = Arc::clone(&stop);
    let code_for_events = Arc::clone(&exit_code);
    // One emitter shared by the PTY listener and the auth watcher. `Channel`
    // is cloned into both; sends are synchronized inside Tauri.
    let emit_session = on_event.clone();
    let emit_auth = on_event;

    let session = terms.spawn_ssh(&launch, cols, rows, move |e| {
        if let TermEvent::Exit { code } = &e {
            code_for_events.store(code.unwrap_or(-2), Ordering::SeqCst);
            stop_for_events.store(true, Ordering::SeqCst);
        }
        emit(&emit_session, &e);
    })?;

    let data_dir = app.data_dir.clone();
    let host = launch.host.clone();
    let port = launch.port;
    let user = launch.user.clone();
    let can_copy = launch.control_path.is_some();
    watch_auth_log(log_file, stop, move |method| {
        let users = SshUsers::new(&data_dir);
        match method {
            Some(method) => {
                users.set(&host, port, &user);
                emit(
                    &emit_auth,
                    &TermEvent::Authenticated {
                        method: method.as_str().to_string(),
                        copy_id: can_copy && method.needs_key(),
                    },
                );
            }
            None if exit_code.load(Ordering::SeqCst) == 255 => {
                users.forget(&host, port);
            }
            None => {}
        }
    });
    Ok(session)
}

/// The user name remembered after a successful SSH login to `host`, if any.
#[tauri::command]
pub fn ssh_saved_user(host: String, port: u16, app: AppState<'_>) -> Option<String> {
    SshUsers::new(&app.data_dir).get(&host, port)
}

/// Install the default public key on the server this session signed in to.
/// `password` is only used when the session has no control socket (Windows).
#[tauri::command]
pub fn ssh_copy_id(
    id: u64,
    password: Option<String>,
    terms: State<'_, Terminals>,
) -> Result<String> {
    let info = terms
        .ssh_info(id)
        .ok_or_else(|| CxError::NotFound("that terminal session has ended".into()))?;
    install_public_key(
        &info.user,
        &info.host,
        info.port,
        info.control_path.as_deref(),
        password.as_deref(),
    )
}

#[tauri::command]
pub fn term_write(id: u64, data: String, terms: State<'_, Terminals>) -> Result<()> {
    terms.write(id, data.as_bytes())
}

#[tauri::command]
pub fn term_resize(id: u64, cols: u16, rows: u16, terms: State<'_, Terminals>) -> Result<()> {
    terms.resize(id, cols, rows)
}

#[tauri::command]
pub fn term_close(id: u64, terms: State<'_, Terminals>) {
    terms.close(id);
}

/// The shell's current folder, so the file view can follow `cd`.
#[tauri::command]
pub fn term_cwd(id: u64, terms: State<'_, Terminals>) -> Option<String> {
    terms.cwd(id).map(|p| Location::local(p).uri())
}
