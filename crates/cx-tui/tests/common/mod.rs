//! Test harness: a real engine on temporary folders, the app on top, and a
//! pump that feeds background results back until a condition holds.

#![allow(dead_code)]

use cx_engine::{Engine, EngineConfig, PollConfig};
use cx_tui::msg::Msg;
use cx_tui::settings::Settings;
use cx_tui::App;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

/// One home folder per test binary (the home override is process-wide),
/// so crumbs and snapshots don't depend on the machine.
pub fn home() -> PathBuf {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let h = std::env::temp_dir().join(format!("cx-tui-{}", env!("CARGO_CRATE_NAME")));
        std::fs::create_dir_all(&h).unwrap();
        // Canonical, so paths match what the OS reports (/private/var on macOS).
        let h = h.canonicalize().unwrap();
        // Dates in snapshots shouldn't depend on the machine's time zone.
        std::env::set_var("TZ", "UTC");
        cx_core::location::set_home(h.clone());
        h
    })
    .clone()
}

/// A fresh folder `name` under the test home.
pub fn folder(name: &str) -> PathBuf {
    let p = home().join(name);
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

pub fn uri(p: &Path) -> String {
    cx_core::Location::local(p).uri()
}

/// Write a file with a fixed modification time (2021-03-04 12:00 UTC).
pub fn write(p: &Path, data: &[u8]) {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(p, data).unwrap();
    let f = std::fs::File::options().write(true).open(p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_614_859_200)).unwrap();
}

pub struct Harness {
    pub state: tempfile::TempDir,
    pub app: App,
    pub rx: UnboundedReceiver<Msg>,
}

impl Harness {
    pub async fn new() -> Harness {
        Harness::with_settings(Settings::default()).await
    }

    pub async fn with_settings(mut settings: Settings) -> Harness {
        settings.fda_tip_shown = true;
        home();
        let state = tempfile::tempdir().unwrap();
        let mut cfg = EngineConfig::isolated(state.path());
        cfg.poll = PollConfig { min: Duration::from_millis(100), max: Duration::from_millis(400) };
        let engine = Engine::new(cfg).unwrap();
        let (app, rx) = App::new(engine, settings, None);
        Harness { state, app, rx }
    }

    /// Handle messages until `cond` holds (or panic after 10 s).
    pub async fn until(&mut self, what: &str, cond: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            self.app.prepare();
            if cond(&self.app) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                panic!("timed out waiting for: {what}\ntoasts: {:?}\njobs: {:?}\ndialogs: {}", self.app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>(), self.app.jobs.iter().map(|j| (j.id, &j.kind, &j.state, &j.sources, &j.dest, j.conflict.as_ref().map(|c| (&c.source_uri, &c.dest_uri)))).collect::<Vec<_>>(), self.app.dialogs.len());
            }
            if let Ok(Some(m)) = tokio::time::timeout(left.min(Duration::from_millis(50)), self.rx.recv()).await {
                self.app.handle_msg(m);
            }
        }
    }

    /// Wait until every folder on screen is listed and watched live (the
    /// header's "● live"), so snapshots don't depend on timing.
    pub async fn wait_live(&mut self) {
        self.until("live watches", |a| {
            a.visible_panes().iter().all(|&p| {
                let t = a.panes[p].tab();
                !t.is_folder() || t.folders().all(|f| f.is_ready() && !f.is_listing() && f.watch.is_some())
            })
        })
        .await;
    }

    /// Wait until the active tab finished listing `n` rows.
    pub async fn listed(&mut self, n: usize) {
        self.until(&format!("{n} rows listed"), move |a| a.tab().folder.is_ready() && a.tab().rows().len() == n).await;
    }

    /// Handle whatever arrives for `d`.
    pub async fn settle(&mut self, d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end {
            if let Ok(Some(m)) = tokio::time::timeout(Duration::from_millis(20), self.rx.recv()).await {
                self.app.handle_msg(m);
            }
        }
        self.app.prepare();
    }

    /// Open `uri` in the active tab and wait for the listing.
    pub async fn open(&mut self, uri: &str) {
        self.app.navigate(uri);
        let target = cx_core::Location::parse(uri).map(|l| l.uri()).unwrap_or_else(|_| uri.to_string());
        self.until("listing", |a| a.tab().folder.is_ready() && (a.tab().dir_uri() == target || a.tab().uri() == uri)).await;
    }

    pub fn key(&mut self, code: KeyCode) {
        self.app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    pub fn key_mod(&mut self, code: KeyCode, m: KeyModifiers) {
        self.app.handle_key(KeyEvent::new(code, m));
    }

    pub fn typ(&mut self, text: &str) {
        for c in text.chars() {
            self.key(KeyCode::Char(c));
        }
    }

    /// Names on screen in the active tab, in order.
    pub fn names(&self) -> Vec<String> {
        let t = self.app.tab();
        t.rows().iter().map(|r| format!("{}{}", "  ".repeat(r.depth as usize), t.item(r).name())).collect()
    }

    pub fn select(&mut self, name: &str) {
        let t = self.app.tab_mut();
        let i = t.rows().iter().position(|r| t.item(r).name() == name).unwrap_or_else(|| panic!("no row {name}"));
        t.select_only(i);
    }

    pub fn screen(&mut self, w: u16, h: u16) -> String {
        self.app.prepare();
        self.app.free.clear();
        cx_tui::ui::render_to_string(&self.app, w, h)
    }
}

/// Compare with `tests/snapshots/<name>.txt`; write it when missing or
/// when `UPDATE_SNAPSHOTS=1`.
pub fn snapshot(name: &str, text: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.txt"));
    let update = std::env::var("UPDATE_SNAPSHOTS").is_ok_and(|v| v == "1");
    match std::fs::read_to_string(&path) {
        Ok(expected) if !update => {
            if expected != text {
                let got = dir.join(format!("{name}.new.txt"));
                std::fs::write(&got, text).unwrap();
                panic!("snapshot {name} differs; see {} (UPDATE_SNAPSHOTS=1 accepts)\n--- got ---\n{text}", got.display());
            }
        }
        _ => std::fs::write(&path, text).unwrap(),
    }
}
