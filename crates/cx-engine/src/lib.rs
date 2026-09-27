//! The Cross Explore engine, without any GUI.
//!
//! Everything the desktop app's backend keeps alive, wired together once so
//! the Tauri app and the terminal UI run the very same code:
//!
//! * a [`Vfs`] with every connector (local, SMB, SFTP with known_hosts
//!   trust, FTP/FTPS with certificate trust, WebDAV over http and https,
//!   S3, and peer once [`Engine::start_peer`] ran) and the archive provider;
//! * a keychain-backed [`cx_core::CredentialStore`]
//!   ([`credentials::KeychainCredentials`]);
//! * the [`TransferManager`] (jobs interrupted by a quit come back paused),
//!   plus compress/extract tasks, folded into [`jobs::JobView`]s;
//! * the [`Thumbnailer`], the [`tags::Tags`] store (Finder tags on macOS),
//!   nearby-device discovery and the peer service;
//! * folder listings, watches (live or polled against the listing just
//!   shown, see [`files::RecentListings`]), folder sizes, search and places.
//!
//! The API uses plain callbacks and [`events::EngineEvent`]s instead of any
//! front-end types. Build it inside a tokio runtime: the transfer manager
//! and background tasks spawn onto the runtime current at construction.
//!
//! # From the desktop app's commands
//!
//! | Tauri command | Engine |
//! |---|---|
//! | `list_dir(uri, channel)` | [`Engine::list_dir`]`(uri, \|e\| channel.send(e).is_ok())` ([`ListEvent`] serializes the same) |
//! | `watch_dir` / `unwatch_dir` | [`Engine::watch_dir`] (a [`cx_core::WatchSink`]) / [`Engine::unwatch_dir`]; [`WatchInfo`] serializes as `{id, mode: "live"\|"polling"}` |
//! | `stat_entry`, `create_folder`, `rename_entry`, `trash_entries`, `free_space` | [`Engine::stat`], [`Engine::create_folder`], [`Engine::rename`], [`Engine::trash`], [`Engine::free_space`] |
//! | `dir_size(uri, channel)` | [`Engine::dir_size`]`(uri, \|p\| channel.send(p).is_ok())` |
//! | `preview_text` | [`Engine::preview_text`] |
//! | `subscribe(channel)` | `engine.events.set(Arc::new(move \|e\| { channel.send(serde_json::to_value(e)) }))` |
//! | `transfer_submit(req)` | [`Engine::submit`] ([`SubmitRequest`] deserializes the UI's `{kind, sources, dest, conflict, verify}`) |
//! | `transfer_pause/resume/cancel/resolve/list/clear` | [`Engine::pause`], [`Engine::resume`], [`Engine::cancel`], [`Engine::resolve`], [`Engine::job_list`], [`Engine::clear_finished`] |
//! | `undo`, `compare_dirs` | [`Engine::undo`], [`Engine::compare_dirs`] (and [`Engine::sync_dirs`] for the plan) |
//! | `search_start(root, query, channel)`, `cancel_task` | [`Engine::search_start`] ([`SearchRequest`], [`SearchEvent`]), [`Engine::cancel_task`] |
//! | `connect_server`, `disconnect_server`, `connections`, `trust_host_key` | [`Engine::connect_server`], [`Engine::disconnect_server`], [`Engine::connections`], [`Engine::trust_host_key`] |
//! | `peer_*`, `discovery_devices`, `discovery_refresh` | [`Engine::peer_status`], [`Engine::peer_set_enabled`], [`Engine::peer_set_shares`], [`Engine::peer_set_auto_trust`], [`Engine::peer_pair_code`], [`Engine::peer_pair`], [`Engine::peer_forget`], [`Engine::peer_send`], [`Engine::peer_respond`], [`Engine::devices`], [`Engine::refresh_discovery`] |
//! | `tags_get/set/find` | `engine.tags.get_many`, `engine.tags.set`, [`Engine::tags_find`] |
//! | `open_entry`, `reveal_entry`, `open_terminal` | [`Engine::local_copy`] then the Tauri opener (or [`Engine::open_entry`]); [`system::reveal_entry`]; [`system::open_terminal`] |
//! | `os_clipboard_set/get`, `full_disk_access`, `open_full_disk_access_settings` | [`system::os_clipboard_set`], [`system::os_clipboard_get`], [`system::full_disk_access`], [`system::open_full_disk_access_settings`] |
//! | `term_*` | `engine.terminals` ([`cx_term::Terminals`]) |
//! | `places` | [`Engine::places`] ([`places::set_translucent`] for the window material flag) |
//! | `build_state` / setup | [`Engine::new`]`(`[`EngineConfig`]`::new(app_data_dir, app_cache_dir))` then [`Engine::start_background`] |
//! | protocols (`cxfile://`, `cxthumb://`) | `engine.vfs` for bytes, [`Engine::thumbnail`] |
//!
//! ```no_run
//! # async fn demo() -> cx_core::Result<()> {
//! let engine = cx_engine::Engine::new(cx_engine::EngineConfig::standard()?)?;
//! engine.events.set(std::sync::Arc::new(|e| println!("{}", serde_json::to_string(&e).unwrap())));
//! engine.start_background();
//! engine.list_dir("~", |e| { println!("{e:?}"); true }).await?;
//! # Ok(()) }
//! ```

pub mod credentials;
pub mod events;
pub mod files;
pub mod jobs;
pub mod net;
pub mod peer;
pub mod places;
pub mod search;
pub mod system;
pub mod tags;
pub mod tasks;
pub mod transfers;

pub use cx_core::poll::PollConfig;
pub use events::{EngineEvent, EventSink};
pub use files::{ListEvent, SizeProgress, TextPreview, WatchInfo, WatchMode};
pub use jobs::{JobConflict, JobView};
pub use peer::{PeerPrefs, PeerStatus, PEER_PORT};
pub use search::{SearchEvent, SearchHitView, SearchRequest};
pub use transfers::SubmitRequest;

use cx_core::{CredentialStore, CxError, Result, Vfs};
use cx_discovery::Discovery;
use cx_peer::PeerService;
use cx_thumbs::Thumbnailer;
use cx_transfer::TransferManager;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The desktop app's bundle identifier; its data folders are shared, so
/// trusted host keys, tags, peer identity and pairings are the same in the
/// GUI and the terminal UI.
pub const APP_ID: &str = "dev.crossexplore.explorer";

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Durable state: known_hosts, FTPS trust, tags, peer identity, prefs.
    pub data_dir: PathBuf,
    /// Archive extraction cache, thumbnails, files downloaded for opening.
    pub cache_dir: PathBuf,
    /// Where the transfer manager persists unfinished jobs. Give each front
    /// end its own, so two running at once don't resume each other's jobs.
    pub transfers_dir: PathBuf,
    /// Remember credentials in the OS keychain (off: memory only, for tests).
    pub keychain: bool,
    /// Finder tags for local files on macOS (off: tags.json only).
    pub finder_tags: bool,
    /// Home folder override (phones; tests).
    pub home: Option<PathBuf>,
    pub thumb_cache_bytes: u64,
    /// Polling interval for folders whose provider can't push changes.
    pub poll: PollConfig,
    /// Port the peer service listens on while sharing is on.
    pub peer_port: u16,
}

impl EngineConfig {
    pub fn new(data_dir: impl Into<PathBuf>, cache_dir: impl Into<PathBuf>) -> EngineConfig {
        let data_dir = data_dir.into();
        EngineConfig {
            transfers_dir: data_dir.join("transfers"),
            data_dir,
            cache_dir: cache_dir.into(),
            keychain: true,
            finder_tags: true,
            home: None,
            thumb_cache_bytes: 512 << 20,
            poll: PollConfig::default(),
            peer_port: PEER_PORT,
        }
    }

    /// The desktop app's folders (Tauri's `app_data_dir` / `app_cache_dir`).
    pub fn standard() -> Result<EngineConfig> {
        let data = dirs::data_dir().ok_or_else(|| CxError::Io("no data directory".into()))?.join(APP_ID);
        let cache = dirs::cache_dir().ok_or_else(|| CxError::Io("no cache directory".into()))?.join(APP_ID);
        Ok(EngineConfig::new(data, cache))
    }

    /// Everything under one folder, nothing in the keychain or Finder tags:
    /// for tests and portable setups.
    pub fn isolated(root: impl Into<PathBuf>) -> EngineConfig {
        let root = root.into();
        let mut c = EngineConfig::new(root.join("data"), root.join("cache"));
        c.keychain = false;
        c.finder_tags = false;
        c.peer_port = 0;
        c
    }
}

/// Everything the backend keeps alive for the app's lifetime.
pub struct Engine {
    pub vfs: Arc<Vfs>,
    /// Subscribe with `events.set(...)`.
    pub events: Arc<events::Emitter>,
    pub jobs: Arc<jobs::Jobs>,
    pub transfers: Arc<TransferManager>,
    pub thumbs: Thumbnailer,
    pub tags: tags::Tags,
    /// Embedded terminal sessions (the desktop app's terminal panel).
    pub terminals: cx_term::Terminals,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub config: EngineConfig,
    pub(crate) rt: tokio::runtime::Handle,
    pub(crate) tasks: tasks::Tasks,
    pub(crate) recent: files::RecentListings,
    pub(crate) watches: files::Watches,
    pub(crate) discovery: Mutex<Option<Discovery>>,
    pub(crate) peer: tokio::sync::RwLock<Option<Arc<PeerService>>>,
    pub(crate) peer_prefs: Mutex<PeerPrefs>,
    pub(crate) offer_jobs: Mutex<HashMap<String, u64>>,
}

impl Engine {
    /// Build the engine. Must run inside a tokio runtime (the transfer
    /// manager spawns onto it). Creates the data and cache folders.
    pub fn new(config: EngineConfig) -> Result<Arc<Engine>> {
        let rt = tokio::runtime::Handle::try_current().map_err(|_| CxError::Io("the engine must be created inside a tokio runtime".into()))?;
        let io = |e: std::io::Error, p: &PathBuf| CxError::from_io(e, p.display());
        for d in [&config.data_dir, &config.cache_dir] {
            std::fs::create_dir_all(d).map_err(|e| io(e, d))?;
        }
        if let Some(home) = &config.home {
            cx_core::location::set_home(home.clone());
        }
        #[cfg(target_os = "ios")]
        if let Some(home) = dirs::home_dir() {
            let _ = std::fs::create_dir_all(home.join("Downloads"));
        }
        #[cfg(target_os = "android")]
        {
            // No Unix home on Android: the app's own storage is "home", with
            // a Documents folder people can fill from the network.
            let home = config.data_dir.join("files");
            std::fs::create_dir_all(home.join("Documents")).map_err(|e| io(e, &home))?;
            std::fs::create_dir_all(home.join("Downloads")).map_err(|e| io(e, &home))?;
            cx_core::location::set_home(home);
        }

        let creds: Arc<dyn CredentialStore> = if config.keychain { Arc::new(credentials::KeychainCredentials::default()) } else { Arc::new(credentials::KeychainCredentials::session_only()) };
        let vfs = Vfs::new(Arc::new(cx_local::LocalProvider), creds);
        net::register_connectors(&vfs, &config.data_dir);
        cx_archive::ArchiveProvider::install(&vfs, config.cache_dir.join("archives"));

        let events = Arc::new(events::Emitter::default());
        let jobs = jobs::Jobs::new(events.clone());
        let transfers = {
            let jobs = jobs.clone();
            TransferManager::new(vfs.clone(), config.transfers_dir.clone(), move |e| jobs.on_transfer(e))
        };
        // Jobs interrupted by a quit come back paused, ready to resume.
        for job in transfers.restore_pending() {
            jobs.on_transfer(cx_transfer::TransferEvent::JobAdded { job });
        }
        let thumbs = Thumbnailer::new(config.cache_dir.join("thumbs"), config.thumb_cache_bytes)?;
        let tags_path = config.data_dir.join("tags.json");
        let tags = if config.finder_tags { tags::Tags::new(tags_path) } else { tags::Tags::json_only(tags_path) };
        Ok(Arc::new(Engine {
            peer_prefs: Mutex::new(Engine::load_peer_prefs(&config.data_dir)),
            vfs,
            events,
            jobs,
            transfers,
            thumbs,
            tags,
            terminals: cx_term::Terminals::new(),
            data_dir: config.data_dir.clone(),
            cache_dir: config.cache_dir.clone(),
            config,
            rt,
            tasks: tasks::Tasks::default(),
            recent: files::RecentListings::default(),
            watches: files::Watches::default(),
            discovery: Mutex::new(None),
            peer: tokio::sync::RwLock::new(None),
            offer_jobs: Mutex::new(HashMap::new()),
        }))
    }

    /// Start discovery, then the peer service (in the background; a peer
    /// failure is reported on stderr and leaves the rest working).
    pub fn start_background(self: &Arc<Self>) {
        self.start_discovery();
        let engine = self.clone();
        self.rt.spawn(async move {
            if let Err(e) = engine.start_peer().await {
                eprintln!("peer mode unavailable: {e}");
            }
        });
    }

    /// Stop discovery and the peer service (for a clean exit).
    pub async fn shutdown(&self) {
        if let Some(d) = self.discovery.lock().unwrap().take() {
            d.stop();
        }
        if let Some(p) = self.peer.write().await.take() {
            p.stop().await;
        }
    }

    /// Home, standard folders and volumes.
    pub fn places(&self) -> places::Places {
        places::places()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cx_core::{Change, Location};
    use std::time::Duration;

    async fn engine() -> (tempfile::TempDir, Arc<Engine>) {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = EngineConfig::isolated(dir.path().join("state"));
        cfg.poll = PollConfig { min: Duration::from_millis(50), max: Duration::from_millis(200) };
        let e = Engine::new(cfg).unwrap();
        std::fs::create_dir_all(dir.path().join("files")).unwrap();
        (dir, e)
    }

    fn uri(p: &std::path::Path) -> String {
        Location::local(p).uri()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lists_in_batches_with_meta_first() {
        let (dir, e) = engine().await;
        let files = dir.path().join("files");
        for i in 0..50 {
            std::fs::write(files.join(format!("f{i}.txt")), b"x").unwrap();
        }
        let mut events = Vec::new();
        let n = e.list_dir(&uri(&files), |ev| {
            events.push(ev);
            true
        })
        .await
        .unwrap();
        assert_eq!(n, 50);
        assert!(matches!(events[0], ListEvent::Meta { .. }));
        assert!(matches!(events.last(), Some(ListEvent::Done { total: 50, .. })));
        let listed: usize = events.iter().map(|e| if let ListEvent::Batch { entries } = e { entries.len() } else { 0 }).sum();
        assert_eq!(listed, 50);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn file_ops_undo_and_sizes() {
        let (dir, e) = engine().await;
        let files = dir.path().join("files");
        let d = uri(&files);
        let made = e.create_folder(&d, None).await.unwrap();
        assert!(made.is_dir);
        std::fs::write(files.join(&made.name).join("a.bin"), vec![0u8; 1000]).unwrap();
        let renamed = e.rename(&d, &made.name, "Stuff").await.unwrap();
        assert_eq!(renamed.name, "Stuff");
        let size = e.dir_size(&uri(&files.join("Stuff")), |_| true).await.unwrap();
        assert_eq!(size, 1000);
        e.undo(cx_transfer::UndoOp::rename(&Location::local(&files), &made.name, "Stuff")).await.unwrap();
        assert!(files.join(&made.name).is_dir());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn copy_job_and_compress_extract_report_jobs() {
        let (dir, e) = engine().await;
        let seen = Arc::new(Mutex::new(Vec::<JobView>::new()));
        let s = seen.clone();
        e.events.set(Arc::new(move |ev| {
            if let EngineEvent::Job { job } = ev {
                s.lock().unwrap().push(job);
            }
        }));
        let files = dir.path().join("files");
        std::fs::create_dir_all(files.join("src")).unwrap();
        std::fs::create_dir_all(files.join("dst")).unwrap();
        std::fs::write(files.join("src/a.txt"), b"hello").unwrap();
        let id = e.submit(SubmitRequest::new("copy", vec![uri(&files.join("src/a.txt"))], Some(uri(&files.join("dst"))))).unwrap();
        let job = e.wait_job(id).await.unwrap();
        assert_eq!(job.state, "done");
        assert_eq!(std::fs::read(files.join("dst/a.txt")).unwrap(), b"hello");
        assert!(matches!(job.undo, Some(cx_transfer::UndoOp::Copy { .. })));

        let zip = uri(&files.join("out.zip"));
        let id = e.submit(SubmitRequest::new("compress", vec![uri(&files.join("src"))], Some(zip.clone()))).unwrap();
        assert!(tasks::is_task(id));
        assert_eq!(e.wait_job(id).await.unwrap().state, "done");
        std::fs::create_dir_all(files.join("x")).unwrap();
        let id = e.submit(SubmitRequest::new("extract", vec![zip.clone()], Some(uri(&files.join("x"))))).unwrap();
        assert_eq!(e.wait_job(id).await.unwrap().state, "done");
        assert!(files.join("x/out/src/a.txt").exists() || files.join("x/src/a.txt").exists());
        // Browse the zip as a folder through the archive provider.
        let n = e.list_dir(&format!("archive://{zip}!/"), |_| true).await.unwrap();
        assert!(n >= 1);
        assert!(!seen.lock().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn local_watch_is_live() {
        let (dir, e) = engine().await;
        let files = dir.path().join("files");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<Change>>();
        let info = e.watch_dir_with(&uri(&files), move |c| {
            let _ = tx.send(c);
        })
        .await
        .unwrap();
        assert_eq!(info.mode, WatchMode::Live);
        tokio::time::sleep(Duration::from_millis(200)).await;
        std::fs::write(files.join("new.txt"), b"x").unwrap();
        let got = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let batch = rx.recv().await.unwrap();
                if batch.iter().any(|c| matches!(c, Change::Upsert { entry } if entry.name == "new.txt")) {
                    return true;
                }
            }
        })
        .await;
        assert!(got.unwrap_or(false));
        e.unwatch_dir(info.id);
        assert!(e.watches.is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn search_by_name_and_content() {
        let (dir, e) = engine().await;
        let files = dir.path().join("files");
        std::fs::create_dir_all(files.join("deep/er")).unwrap();
        std::fs::write(files.join("deep/er/needle.md"), b"the quick fox").unwrap();
        std::fs::write(files.join("hay.txt"), b"nothing").unwrap();
        for (req, want) in [
            (SearchRequest { text: "needle".into(), ..Default::default() }, "needle.md"),
            (SearchRequest { content: Some("quick".into()), ..Default::default() }, "needle.md"),
        ] {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            e.search_start(&uri(&files), req, move |ev| {
                let _ = tx.send(ev);
            })
            .unwrap();
            let mut names = Vec::new();
            while let Some(ev) = rx.recv().await {
                match ev {
                    SearchEvent::Hits { hits } => names.extend(hits.into_iter().map(|h| h.entry.name)),
                    SearchEvent::Done { .. } => break,
                }
            }
            assert_eq!(names, vec![want.to_string()]);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compare_and_sync() {
        let (dir, e) = engine().await;
        let files = dir.path().join("files");
        std::fs::create_dir_all(files.join("l")).unwrap();
        std::fs::create_dir_all(files.join("r")).unwrap();
        std::fs::write(files.join("l/only.txt"), b"1").unwrap();
        let (l, r) = (uri(&files.join("l")), uri(&files.join("r")));
        let diff = e.compare_dirs(&l, &r, false).await.unwrap();
        assert_eq!(diff.len(), 1);
        let ids = e.sync_dirs(&l, &r, &diff, cx_transfer::SyncDirection::LeftToRight).unwrap();
        for id in ids {
            e.wait_job(id).await;
        }
        assert!(files.join("r/only.txt").exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unknown_servers_fail_cleanly() {
        let (_dir, e) = engine().await;
        assert!(e.trust_host_key("smb://nas/share", "ssh-ed25519", "SHA256:x").is_err());
        assert!(e.connections().is_empty());
        assert!(e.connect_server("file:///tmp", None, false).await.is_err());
    }
}
