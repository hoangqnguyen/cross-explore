use crate::{CxError, Result, ShellCommand, TermEvent};
use cx_core::Location;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

#[cfg(not(any(target_os = "ios", target_os = "android")))]
use crate::session::Session;

/// Phones can't spawn processes, so there is never a session; the type
/// exists only so the API (and the app's command handlers) compile.
#[cfg(any(target_os = "ios", target_os = "android"))]
enum Session {}

#[cfg(any(target_os = "ios", target_os = "android"))]
#[allow(clippy::unused_self)]
impl Session {
    fn write(&self, _: &[u8]) -> Result<()> {
        match *self {}
    }
    fn resize(&self, _: u16, _: u16) -> Result<()> {
        match *self {}
    }
    fn kill(&self) {
        match *self {}
    }
    fn cwd(&self) -> Option<PathBuf> {
        match *self {}
    }
}

type Sessions = Mutex<HashMap<u64, Arc<Session>>>;

/// All terminal sessions of the app. Cheap to share (`Send + Sync`); keep one
/// in the app state. Dropping it closes every session.
pub struct Terminals {
    sessions: Arc<Sessions>,
    next_id: AtomicU64,
}

impl Default for Terminals {
    fn default() -> Self {
        Terminals::new()
    }
}

impl Terminals {
    pub fn new() -> Terminals {
        Terminals { sessions: Arc::new(Mutex::new(HashMap::new())), next_id: AtomicU64::new(1) }
    }

    /// Open a terminal "in" `loc` (see [`ShellCommand::for_location`]): the
    /// login shell in a local folder, or `ssh` into an SFTP server's folder.
    ///
    /// `on_event` is called from a background thread with coalesced output
    /// and, last, exactly one [`TermEvent::Exit`] — also after [`close`].
    /// Returns the session id used by the other methods.
    ///
    /// [`close`]: Terminals::close
    pub fn open(&self, loc: &Location, cols: u16, rows: u16, on_event: impl Fn(TermEvent) + Send + Sync + 'static) -> Result<u64> {
        let cmd = ShellCommand::for_location(loc)?;
        self.spawn(&cmd, cols, rows, on_event)
    }

    /// Run an arbitrary command in a new PTY (what [`open`](Terminals::open)
    /// does after picking the command).
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    pub fn spawn(&self, cmd: &ShellCommand, cols: u16, rows: u16, on_event: impl Fn(TermEvent) + Send + Sync + 'static) -> Result<u64> {
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let weak: std::sync::Weak<Sessions> = Arc::downgrade(&self.sessions);
        let on_exit = Box::new(move || {
            if let Some(sessions) = weak.upgrade() {
                // Take it out under the lock, drop (closing the PTY) outside it.
                let session = lock(&sessions).remove(&id);
                drop(session);
            }
        });
        let session = Arc::new(Session::spawn(cmd, cols, rows, Box::new(on_event), on_exit)?);
        lock(&self.sessions).insert(id, session.clone());
        // A child that died before the insert had nothing to remove; the pump
        // runs `on_exit` only after the exited flag is set, so one of the two
        // sides always sees the other.
        if session.has_exited() {
            lock(&self.sessions).remove(&id);
        }
        Ok(id)
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub fn spawn(&self, _cmd: &ShellCommand, _cols: u16, _rows: u16, _on_event: impl Fn(TermEvent) + Send + Sync + 'static) -> Result<u64> {
        let _ = &self.next_id;
        Err(CxError::Unsupported("terminals are not available on this platform".into()))
    }

    /// Send keystrokes / pasted text (raw bytes, as xterm.js `onData` gives them).
    pub fn write(&self, id: u64, data: &[u8]) -> Result<()> {
        self.get(id)?.write(data)
    }

    /// Tell the PTY (and so the program, via SIGWINCH) the new grid size.
    pub fn resize(&self, id: u64, cols: u16, rows: u16) -> Result<()> {
        self.get(id)?.resize(cols, rows)
    }

    /// Kill the shell and everything it started, and free the PTY. The
    /// session's listener still receives its final `Exit` event. Unknown or
    /// already-exited ids are ignored.
    pub fn close(&self, id: u64) {
        let session = lock(&self.sessions).remove(&id);
        if let Some(s) = session {
            s.kill();
        }
    }

    /// The shell's current directory, so the file list can follow `cd`.
    /// Best effort: `None` for ssh sessions, on Windows, or if the session is
    /// gone. Reports the shell's own directory, not that of a program running
    /// in the foreground.
    pub fn cwd(&self, id: u64) -> Option<PathBuf> {
        self.get(id).ok()?.cwd()
    }

    /// Ids of the sessions that are still running.
    pub fn ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = lock(&self.sessions).keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    fn get(&self, id: u64) -> Result<Arc<Session>> {
        lock(&self.sessions).get(&id).cloned().ok_or_else(|| CxError::NotFound(format!("terminal session {id}")))
    }
}

impl Drop for Terminals {
    fn drop(&mut self) {
        let all: Vec<_> = lock(&self.sessions).drain().map(|(_, s)| s).collect();
        for s in all {
            s.kill();
        }
    }
}

fn lock(sessions: &Sessions) -> std::sync::MutexGuard<'_, HashMap<u64, Arc<Session>>> {
    sessions.lock().unwrap_or_else(|e| e.into_inner())
}
