//! Render tests: key screens drawn into ratatui's TestBackend and compared
//! with text snapshots in `tests/snapshots` (set UPDATE_SNAPSHOTS=1 to
//! accept changes).

mod common;

use common::*;
use cx_tui::commands::Action;
use cx_tui::dialog::*;
use cx_tui::input::TextInput;
use cx_tui::settings::{Keymap, Settings};
use ratatui::crossterm::event::KeyCode;

async fn sample(h: &mut Harness, name: &str) -> std::path::PathBuf {
    let dir = folder(name);
    std::fs::create_dir_all(dir.join("Documents")).unwrap();
    std::fs::create_dir_all(dir.join("Photos")).unwrap();
    write(&dir.join("Documents/report.pdf"), &[0u8; 120_000]);
    write(&dir.join("notes.md"), b"# Notes\n\nSome `code` here.\n");
    write(&dir.join("main.rs"), b"fn main() {\n    println!(\"hi\");\n}\n");
    write(&dir.join("archive.zip"), b"PK");
    write(&dir.join("photo.jpg"), &[0u8; 2_500_000]);
    for d in ["Documents", "Photos"] {
        let f = std::fs::File::open(dir.join(d)).unwrap();
        let _ = f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_614_859_200));
    }
    h.app.start(&[uri(&dir)]);
    h.until("listing", |a| a.tab().folder.is_ready() && a.tab().rows().len() == 6).await;
    h.wait_live().await;
    dir
}

#[tokio::test(flavor = "multi_thread")]
async fn details_view() {
    let mut h = Harness::new().await;
    sample(&mut h, "details").await;
    h.key(KeyCode::Down);
    h.key(KeyCode::Right); // expands Photos (empty) in place
    h.wait_live().await;
    let s = h.screen(100, 16);
    println!("{s}");
    assert!(s.contains("Name ▲"));
    assert!(s.contains("Documents"));
    assert!(s.contains("report.pdf") || s.contains("▸ Documents"));
    assert!(s.contains("2.5 MB"));
    assert!(s.contains("2021-03-04"));
    assert!(s.contains("6 items") || s.contains("items"));
    snapshot("details", &s);
}

#[tokio::test(flavor = "multi_thread")]
async fn outline_and_brief() {
    let mut h = Harness::new().await;
    sample(&mut h, "outline").await;
    // Cursor on Documents; → expands it in place.
    h.key(KeyCode::Right);
    h.until("expanded", |a| a.tab().rows().len() == 7).await;
    h.wait_live().await;
    let s = h.screen(90, 14);
    println!("{s}");
    assert!(s.contains("▾ Documents"));
    assert!(s.contains("    report.pdf") || s.contains("report.pdf"));
    snapshot("outline", &s);
    h.app.run(Action::ViewBrief);
    let s = h.screen(90, 10);
    println!("{s}");
    assert!(!s.contains("Modified"));
    snapshot("brief", &s);
}

#[tokio::test(flavor = "multi_thread")]
async fn dual_pane_commander() {
    let settings = Settings { keymap: Keymap::Commander, ..Default::default() };
    let mut h = Harness::with_settings(settings).await;
    let dir = sample(&mut h, "dual").await;
    h.app.run(Action::ToggleDual);
    h.app.navigate_in(1, &uri(&dir.join("Documents")), None);
    h.until("right pane", |a| a.panes[1].tab().folder.is_ready() && a.panes[1].tab().rows().len() == 1).await;
    h.app.run(Action::NewTab);
    h.wait_live().await;
    let s = h.screen(120, 14);
    println!("{s}");
    assert!(s.contains("report.pdf"));
    assert!(s.contains("1Help"), "Commander F-key bar");
    assert!(s.contains("Commander"));
    snapshot("dual", &s);
}

#[tokio::test(flavor = "multi_thread")]
async fn dialogs() {
    let mut h = Harness::new().await;
    let dir = sample(&mut h, "dialogs").await;
    // Destination picker.
    h.select("notes.md");
    h.app.places = None; // machine-specific drives stay out of snapshots
    h.app.settings.bookmarks.push(cx_tui::settings::Bookmark { name: "Docs".into(), uri: uri(&dir.join("Documents")) });
    h.app.run(Action::CopyTo);
    let s = h.screen(100, 24);
    println!("{s}");
    assert!(s.contains("Copy “notes.md” to…"));
    assert!(s.contains("Favorites"));
    assert!(s.contains("Docs"));
    snapshot("dest_picker", &s);
    h.app.dialogs.clear();

    // Connect.
    h.app.run(Action::Connect);
    if let Some(Dialog::Connect(f)) = h.app.dialogs.last_mut() {
        f.host = TextInput::new("sftp://pi@nas.local:2222/home/pi");
        f.absorb_uri();
    }
    let s = h.screen(100, 26);
    println!("{s}");
    assert!(s.contains("Connect to server"));
    assert!(s.contains("SFTP (SSH)"));
    assert!(s.contains("sftp://pi@nas.local:2222/home/pi"));
    snapshot("connect", &s);
    h.app.dialogs.clear();

    // Conflict.
    let e = |name: &str, size, ms| cx_core::Entry { name: name.into(), kind: cx_core::EntryKind::File, is_dir: false, size, modified: Some(ms), created: None, hidden: false, readonly: false };
    h.app.dialogs.push(Dialog::Conflict(ConflictDlg {
        job: 1,
        conflict: cx_engine::JobConflict { id: 1, source: e("notes.md", 2048, 1_700_000_000_000), source_uri: uri(&dir.join("notes.md")), dest: e("notes.md", 1024, 1_600_000_000_000), dest_uri: uri(&dir.join("Documents/notes.md")) },
        apply_all: true,
        cursor: 2,
    }));
    let s = h.screen(100, 22);
    println!("{s}");
    assert!(s.contains("already exists"));
    assert!(s.contains("Keep both"));
    assert!(s.contains("newer"));
    assert!(s.contains("[✓] Apply to all"));
    h.app.dialogs.clear();

    // Help.
    h.app.run(Action::Help);
    let s = h.screen(110, 30);
    println!("{s}");
    assert!(s.contains("Keyboard shortcuts"));
    assert!(s.contains("Alt+F1"));
    let all: Vec<String> = cx_tui::ui::dialogs::help_lines(&h.app, &cx_tui::ui::theme::Theme::dark(), 100).iter().map(|l| l.to_string()).collect();
    assert!(all.iter().any(|l| l.contains("Copy to…") && l.contains("Alt+C")));
    assert!(all.iter().any(|l| l.contains("Command palette") && l.contains("Ctrl+P")));
    snapshot("help", &s);
    h.app.dialogs.clear();

    // Palette ranks commands fuzzily.
    h.app.run(Action::Palette);
    h.typ("cpyto");
    let s = h.screen(100, 24);
    println!("{s}");
    assert!(s.contains("Command palette"));
    assert!(s.lines().any(|l| l.contains("Copy to…")));
}

#[tokio::test(flavor = "multi_thread")]
async fn transfers_panel_and_preview() {
    let mut h = Harness::new().await;
    let dir = sample(&mut h, "panels").await;
    let dest = folder("panels-dest");
    let job = cx_engine::JobView {
        id: 42,
        kind: "copy".into(),
        state: "running".into(),
        sources: vec![uri(&dir.join("photo.jpg"))],
        dest: Some(uri(&dest)),
        bytes_done: 1_250_000,
        bytes_total: 2_500_000,
        files_done: 0,
        files_total: 1,
        current: Some("photo.jpg".into()),
        speed: 5_000_000.0,
        eta: Some(1.0),
        errors: vec![],
        conflict: None,
        undo: None,
        started_at: 0,
    };
    let mut done = job.clone();
    done.id = 41;
    done.state = "done".into();
    done.bytes_done = done.bytes_total;
    h.app.jobs = vec![job, done];
    h.app.run(Action::Transfers);
    h.select("main.rs");
    h.app.run(Action::TogglePreview);
    h.until("preview", |a| a.preview.as_ref().is_some_and(|p| matches!(p.content, cx_tui::preview::Content::Text { .. }))).await;
    // Birth times depend on the file system; keep them out of the snapshot.
    if let Some(p) = h.app.preview.as_mut() {
        p.entry.created = None;
    }
    h.wait_live().await;
    let s = h.screen(120, 24);
    println!("{s}");
    assert!(s.contains("Transfers"));
    assert!(s.contains("50%"));
    assert!(s.contains("5.0 MB/s"));
    assert!(s.contains("println!"));
    snapshot("transfers_preview", &s);
}

/// Every screen, every dialog, at sizes from tiny to huge: drawing must
/// never panic or hang (long names in narrow panes once looped forever).
#[tokio::test(flavor = "multi_thread")]
async fn renders_at_any_size() {
    let settings = Settings { dual: true, preview_pane: true, ..Default::default() };
    let mut h = Harness::with_settings(settings).await;
    let dir = sample(&mut h, "a-rather-long-folder-name-for-narrow-panes").await;
    write(&dir.join("an extremely long file name that will never fit in a narrow terminal pane.txt"), b"x");
    h.app.transfers_open = true;
    h.typ("n");
    let dialogs: Vec<Action> = vec![Action::Help, Action::Connect, Action::CopyTo, Action::MultiRename, Action::Search, Action::Settings, Action::Palette, Action::PlacesLeft, Action::SortMenu, Action::Tags, Action::Pair, Action::PeerSettings];
    let sizes = [(1u16, 1u16), (3, 2), (10, 5), (20, 8), (33, 12), (47, 15), (80, 24), (132, 43), (250, 80)];
    let mut screens = 0;
    for step in 0..=dialogs.len() {
        h.app.dialogs.clear();
        if step > 0 {
            h.app.run(dialogs[step - 1]);
        }
        for (w, ht) in sizes {
            h.app.prepare();
            // A hang fails loudly instead of leaving the test run stuck.
            let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let d = done.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(20));
                if !d.load(std::sync::atomic::Ordering::SeqCst) {
                    eprintln!("render at {w}x{ht} (step {step}) hung");
                    std::process::exit(101);
                }
            });
            let s = cx_tui::ui::render_to_string(&h.app, w, ht);
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            assert_eq!(s.lines().count(), ht as usize);
            screens += 1;
        }
    }
    h.app.quicklook = true;
    for (w, ht) in sizes {
        cx_tui::ui::render_to_string(&h.app, w, ht);
    }
    assert!(screens > 100);
}
