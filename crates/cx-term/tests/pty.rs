//! Real PTY sessions against the user's login shell.
#![cfg(not(any(target_os = "ios", target_os = "android")))]

use cx_core::{CxError, Location};
use cx_term::{TermEvent, Terminals};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Shells with heavy rc files can take a while to show a prompt.
const TIMEOUT: Duration = Duration::from_secs(20);
const NL: &str = if cfg!(windows) { "\r\n" } else { "\n" };

#[derive(Default)]
struct Log {
    out: Vec<u8>,
    exit: Option<Option<i32>>,
    events_after_exit: usize,
}

#[derive(Clone, Default)]
struct Probe(Arc<Mutex<Log>>);

impl Probe {
    fn listener(&self) -> impl Fn(TermEvent) + Send + Sync + 'static {
        let log = self.0.clone();
        move |ev| {
            let mut log = log.lock().unwrap();
            if log.exit.is_some() {
                log.events_after_exit += 1;
            }
            match ev {
                TermEvent::Output(b) => log.out.extend(b),
                TermEvent::Exit { code } => log.exit = Some(code),
                TermEvent::Authenticated { .. } => {}
            }
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap().out).into_owned()
    }

    fn wait_until(&self, what: &str, f: impl Fn(&Log) -> bool) {
        let start = Instant::now();
        while start.elapsed() < TIMEOUT {
            if f(&self.0.lock().unwrap()) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "timed out waiting for {what}; output so far:\n{}",
            self.text()
        );
    }

    fn wait_output(&self, needle: &str) {
        self.wait_until(needle, |l| String::from_utf8_lossy(&l.out).contains(needle));
    }

    fn wait_exit(&self) -> Option<i32> {
        self.wait_until("exit", |l| l.exit.is_some());
        thread::sleep(Duration::from_millis(50));
        let log = self.0.lock().unwrap();
        assert_eq!(log.events_after_exit, 0, "Exit must be the last event");
        log.exit.unwrap()
    }
}

fn temp_dir() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    // /var → /private/var on macOS: compare against what the OS reports.
    let dir = tmp.path().canonicalize().unwrap();
    (tmp, dir)
}

fn open(terms: &Terminals, dir: &Path, probe: &Probe) -> u64 {
    terms
        .open(
            &Location::Local(dir.to_path_buf()),
            200,
            50,
            probe.listener(),
        )
        .unwrap()
}

#[test]
fn shell_runs_in_folder() {
    let (_tmp, dir) = temp_dir();
    let terms = Terminals::new();
    let probe = Probe::default();
    let id = open(&terms, &dir, &probe);

    terms.write(id, format!("pwd{NL}").as_bytes()).unwrap();
    let echo = if cfg!(windows) {
        "echo \"cx-$(1+1)\""
    } else {
        "echo cx-$((1+1))"
    };
    terms.write(id, format!("{echo}{NL}").as_bytes()).unwrap();

    probe.wait_output("cx-2");
    let name = dir.file_name().unwrap().to_str().unwrap();
    probe.wait_output(name);
    #[cfg(unix)]
    probe.wait_output(dir.to_str().unwrap());
    terms.close(id);
    probe.wait_exit();
}

#[test]
fn resize() {
    let (_tmp, dir) = temp_dir();
    let terms = Terminals::new();
    let probe = Probe::default();
    let id = open(&terms, &dir, &probe);
    terms.resize(id, 101, 37).unwrap();
    #[cfg(unix)]
    {
        terms
            .write(id, format!("stty size{NL}").as_bytes())
            .unwrap();
        probe.wait_output("37 101");
    }
    assert!(terms.resize(id + 1000, 80, 24).is_err());
    terms.close(id);
    probe.wait_exit();
}

#[test]
fn close_kills_child_and_frees_session() {
    let (_tmp, dir) = temp_dir();
    let terms = Terminals::new();
    let probe = Probe::default();
    let id = open(&terms, &dir, &probe);
    // A job that ignores nothing but would outlive a shell-only kill.
    #[cfg(unix)]
    terms
        .write(id, format!("sleep 1000 &{NL}echo started{NL}").as_bytes())
        .unwrap();
    #[cfg(unix)]
    probe.wait_output("started");
    assert_eq!(terms.ids(), vec![id]);

    terms.close(id);
    probe.wait_exit();
    assert!(terms.ids().is_empty());
    assert!(matches!(terms.write(id, b"x"), Err(CxError::NotFound(_))));
    assert_eq!(terms.cwd(id), None);
    terms.close(id); // idempotent
}

#[test]
fn exit_reports_code() {
    let (_tmp, dir) = temp_dir();
    let terms = Terminals::new();
    let probe = Probe::default();
    let id = open(&terms, &dir, &probe);
    terms.write(id, format!("exit 3{NL}").as_bytes()).unwrap();
    assert_eq!(probe.wait_exit(), Some(3));
    // The session removes itself once the shell is gone.
    let start = Instant::now();
    while !terms.ids().is_empty() && start.elapsed() < TIMEOUT {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(terms.ids().is_empty());
}

#[test]
fn dropping_terminals_closes_sessions() {
    let (_tmp, dir) = temp_dir();
    let terms = Terminals::new();
    let (a, b) = (Probe::default(), Probe::default());
    open(&terms, &dir, &a);
    open(&terms, &dir, &b);
    drop(terms);
    a.wait_exit();
    b.wait_exit();
}

#[test]
fn missing_folder_is_not_found() {
    let (_tmp, dir) = temp_dir();
    let terms = Terminals::new();
    let err = terms
        .open(&Location::Local(dir.join("nope")), 80, 24, |_| {})
        .unwrap_err();
    assert!(matches!(err, CxError::NotFound(_)), "{err:?}");
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn cwd_follows_cd() {
    let (_tmp, dir) = temp_dir();
    let sub = dir.join("it's a dir");
    std::fs::create_dir(&sub).unwrap();
    let terms = Terminals::new();
    let probe = Probe::default();
    let id = open(&terms, &dir, &probe);
    assert_eq!(terms.cwd(id), Some(dir.clone()));

    terms
        .write(
            id,
            format!("cd {}{NL}", cx_term::posix_quote("it's a dir")).as_bytes(),
        )
        .unwrap();
    let start = Instant::now();
    while terms.cwd(id).as_deref() != Some(sub.as_path()) {
        assert!(
            start.elapsed() < TIMEOUT,
            "cwd stayed {:?}; output:\n{}",
            terms.cwd(id),
            probe.text()
        );
        thread::sleep(Duration::from_millis(20));
    }
    terms.close(id);
    probe.wait_exit();
}
