//! One PTY session: the child process plus three threads.
//!
//! * **reader** — blocking reads from the PTY master, forwarded raw;
//! * **waiter** — blocks in `wait()` for the child's exit status;
//! * **pump** — coalesces output and calls the listener.
//!
//! Coalescing matters because every listener call is an IPC message to the
//! webview: `ls -R` or a build log produces thousands of tiny reads a second.
//! The pump holds bytes for at most [`FLUSH_AFTER`] after the first unsent
//! byte, or until [`FLUSH_BYTES`] pile up — imperceptible when typing, and
//! it cuts message counts by orders of magnitude under load.

use crate::{cwd, CxError, Result, ShellCommand, TermEvent};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const FLUSH_AFTER: Duration = Duration::from_millis(8);
const FLUSH_BYTES: usize = 32 * 1024;
/// After the child exits, how long to keep draining output. On Unix EOF
/// follows immediately unless a background job still holds the terminal;
/// ConPTY only reaches EOF once the pseudo console is closed.
const DRAIN_AFTER_EXIT: Duration = Duration::from_millis(500);
/// How long a closed session gets to exit on SIGHUP before SIGKILL.
#[cfg(unix)]
const KILL_GRACE: Duration = Duration::from_secs(2);

pub(crate) type Listener = Box<dyn Fn(TermEvent) + Send + Sync>;

enum Msg {
    Data(Vec<u8>),
    Eof,
    Exited(Option<i32>),
}

pub(crate) struct Session {
    // Dropping the master closes the PTY (and, on Windows, the ConPTY and
    // every process attached to it).
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
    local: bool,
    exited: Arc<AtomicBool>,
}

impl Session {
    /// Start `cmd`. `on_exit` runs on the pump thread once the child has
    /// exited (before the final `Exit` event), so the owner can drop the
    /// session and with it the PTY.
    pub fn spawn(
        cmd: &ShellCommand,
        cols: u16,
        rows: u16,
        listener: Listener,
        on_exit: Box<dyn FnOnce() + Send>,
    ) -> Result<Session> {
        if let Some(dir) = &cmd.cwd {
            // portable-pty silently falls back to $HOME for a bad cwd; a
            // terminal opened "here" must not quietly open elsewhere.
            if !dir.is_dir() {
                return Err(CxError::NotFound(dir.display().to_string()));
            }
        }
        let pty = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|e| CxError::io("open pty", e))?;

        let mut builder = if cmd.argv.is_empty() {
            CommandBuilder::new_default_prog()
        } else {
            CommandBuilder::from_argv(cmd.argv.iter().map(Into::into).collect())
        };
        if let Some(dir) = &cmd.cwd {
            builder.cwd(dir);
            // Shells trust $PWD when it names their cwd, so `pwd` and the
            // prompt keep the path the user browsed to (symlinks and all)
            // instead of the resolved one; the inherited $PWD is our own.
            builder.env("PWD", dir);
        }
        builder.env("TERM", "xterm-256color");
        builder.env("COLORTERM", "truecolor");
        // GUI apps on macOS start without a locale; shells then fall back to
        // ASCII and mangle every non-English file name.
        if cfg!(unix)
            && ["LC_ALL", "LC_CTYPE", "LANG"]
                .iter()
                .all(|k| std::env::var_os(k).is_none())
        {
            builder.env("LANG", "en_US.UTF-8");
        }

        let program = cmd.argv.first().map(String::as_str).unwrap_or("shell");
        let mut child = pty
            .slave
            .spawn_command(builder)
            .map_err(|e| CxError::io(format!("start {program}"), e))?;
        // The child has its own copies of the slave side; ours must go or the
        // reader never sees EOF.
        drop(pty.slave);

        let pid = child.process_id();
        let killer = child.clone_killer();
        let mut reader = pty
            .master
            .try_clone_reader()
            .map_err(|e| CxError::io("pty reader", e))?;
        let writer = pty
            .master
            .take_writer()
            .map_err(|e| CxError::io("pty writer", e))?;
        let exited = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Msg>();

        let tx_read = tx.clone();
        thread::Builder::new()
            .name("cx-term-read".into())
            .spawn(move || {
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if tx_read.send(Msg::Data(buf[..n].to_vec())).is_err() {
                                return;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        // EIO is how Linux reports "all slave fds closed".
                        Err(_) => break,
                    }
                }
                let _ = tx_read.send(Msg::Eof);
            })
            .map_err(|e| CxError::io("spawn thread", e))?;

        let exited_flag = exited.clone();
        thread::Builder::new()
            .name("cx-term-wait".into())
            .spawn(move || {
                let code = match child.wait() {
                    Ok(status) if status.signal().is_none() => Some(status.exit_code() as i32),
                    _ => None,
                };
                exited_flag.store(true, Ordering::SeqCst);
                let _ = tx.send(Msg::Exited(code));
            })
            .map_err(|e| CxError::io("spawn thread", e))?;

        thread::Builder::new()
            .name("cx-term-pump".into())
            .spawn(move || pump(rx, listener, on_exit))
            .map_err(|e| CxError::io("spawn thread", e))?;

        Ok(Session {
            master: Mutex::new(pty.master),
            writer: Mutex::new(writer),
            killer: Mutex::new(killer),
            pid,
            local: cmd.local,
            exited,
        })
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    pub fn write(&self, data: &[u8]) -> Result<()> {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(data)
            .and_then(|_| w.flush())
            .map_err(|e| CxError::io("write to terminal", e))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let master = self.master.lock().unwrap_or_else(|e| e.into_inner());
        master
            .resize(size(cols, rows))
            .map_err(|e| CxError::io("resize terminal", e))
    }

    pub fn cwd(&self) -> Option<PathBuf> {
        if !self.local || self.has_exited() {
            return None;
        }
        cwd::process_cwd(self.pid?)
    }

    /// Hang up the whole process group, as closing a terminal window does,
    /// so jobs started from the shell go too. The shell is a session leader
    /// (portable-pty calls `setsid`), hence its pid is the group id. Anything
    /// that ignores SIGHUP gets SIGKILL after a grace period.
    #[cfg(unix)]
    pub fn kill(&self) {
        let Some(pid) = self.pid else {
            let _ = self.killer.lock().unwrap_or_else(|e| e.into_inner()).kill();
            return;
        };
        let pgid = pid as libc::pid_t;
        // SAFETY: plain syscall; a stale group just yields ESRCH.
        unsafe { libc::killpg(pgid, libc::SIGHUP) };
        let exited = self.exited.clone();
        let _ = thread::Builder::new()
            .name("cx-term-reap".into())
            .spawn(move || {
                let until = Instant::now() + KILL_GRACE;
                while Instant::now() < until {
                    if exited.load(Ordering::SeqCst) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(25));
                }
                // SAFETY: as above.
                unsafe { libc::killpg(pgid, libc::SIGKILL) };
            });
    }

    /// Terminating the shell and then closing the pseudo console (when the
    /// session is dropped) ends every process attached to it.
    #[cfg(not(unix))]
    pub fn kill(&self) {
        let _ = self.killer.lock().unwrap_or_else(|e| e.into_inner()).kill();
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn pump(rx: mpsc::Receiver<Msg>, listener: Listener, on_exit: Box<dyn FnOnce() + Send>) {
    let mut buf: Vec<u8> = Vec::new();
    let mut flush_at: Option<Instant> = None;
    let mut give_up_at: Option<Instant> = None;
    let mut eof = false;
    let mut exit: Option<Option<i32>> = None;
    let mut on_exit = Some(on_exit);

    let flush = |buf: &mut Vec<u8>, flush_at: &mut Option<Instant>| {
        *flush_at = None;
        if !buf.is_empty() {
            listener(TermEvent::Output(std::mem::take(buf)));
        }
    };

    while !(eof && exit.is_some()) {
        let deadline = match (flush_at, give_up_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let msg = match deadline {
            Some(t) => rx.recv_timeout(t.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match msg {
            Ok(Msg::Data(d)) => {
                if buf.is_empty() {
                    flush_at = Some(Instant::now() + FLUSH_AFTER);
                    buf = d;
                } else {
                    buf.extend_from_slice(&d);
                }
                if buf.len() >= FLUSH_BYTES {
                    flush(&mut buf, &mut flush_at);
                }
            }
            Ok(Msg::Eof) => eof = true,
            Ok(Msg::Exited(code)) => {
                exit = Some(code);
                give_up_at = Some(Instant::now() + DRAIN_AFTER_EXIT);
                // Drops the master: required for ConPTY to deliver EOF.
                if let Some(f) = on_exit.take() {
                    f();
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                if flush_at.is_some_and(|t| t <= now) {
                    flush(&mut buf, &mut flush_at);
                }
                if give_up_at.is_some_and(|t| t <= now) {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    flush(&mut buf, &mut flush_at);
    if let Some(f) = on_exit.take() {
        f();
    }
    listener(TermEvent::Exit {
        code: exit.flatten(),
    });
}
