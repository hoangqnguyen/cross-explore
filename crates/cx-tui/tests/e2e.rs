//! End to end: the app driven by keys and commands against real folders,
//! checking the file system after every step.

mod common;

use common::*;
use cx_tui::commands::Action;
use cx_tui::dialog::Dialog;
use cx_tui::settings::Settings;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use std::path::Path;
use std::time::Duration;

fn settings() -> Settings {
    // Never touch the developer's real clipboard from tests.
    Settings {
        os_clipboard: false,
        ..Default::default()
    }
}

async fn wait_fs(h: &mut Harness, what: &str, cond: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}; toasts: {:?}",
            h.app.toasts.iter().map(|t| &t.text).collect::<Vec<_>>()
        );
        h.settle(Duration::from_millis(30)).await;
    }
    h.settle(Duration::from_millis(50)).await;
}

fn set_prompt(h: &mut Harness, text: &str) {
    match h.app.dialogs.last_mut() {
        Some(Dialog::Prompt(p)) => p.input.set(text),
        other => panic!(
            "expected a prompt, got {}",
            if other.is_some() {
                "another dialog"
            } else {
                "nothing"
            }
        ),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn create_rename_undo() {
    let mut h = Harness::with_settings(settings()).await;
    let dir = folder("e2e-create");
    write(&dir.join("a.txt"), b"a");
    h.app.start(&[uri(&dir)]);
    h.listed(1).await;

    // F7: new folder (prompt), named Alpha.
    h.key(KeyCode::F(7));
    set_prompt(&mut h, "Alpha");
    h.key(KeyCode::Enter);
    wait_fs(&mut h, "Alpha", || dir.join("Alpha").is_dir()).await;
    h.until("row", |a| a.tab().rows().len() == 2).await;
    assert_eq!(
        h.app.tab().cursor_item().unwrap().name(),
        "Alpha",
        "new folder is selected"
    );

    // F2: rename a.txt → b.txt.
    h.select("a.txt");
    h.key(KeyCode::F(2));
    set_prompt(&mut h, "b.txt");
    h.key(KeyCode::Enter);
    wait_fs(&mut h, "rename", || {
        dir.join("b.txt").exists() && !dir.join("a.txt").exists()
    })
    .await;
    h.until("row renamed", |a| {
        a.tab()
            .rows()
            .iter()
            .any(|r| a.tab().item(r).name() == "b.txt")
    })
    .await;

    // Ctrl+Z twice: rename back, then remove the new folder.
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    wait_fs(&mut h, "undo rename", || dir.join("a.txt").exists()).await;
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    wait_fs(&mut h, "undo new folder", || !dir.join("Alpha").exists()).await;
    h.until("rows follow the file system", |a| a.tab().rows().len() == 1)
        .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_move_to_other_pane_and_many_destinations() {
    let mut h = Harness::with_settings(settings()).await;
    let src = folder("e2e-copy");
    let d1 = folder("e2e-copy-d1");
    let d2 = folder("e2e-copy-d2");
    let d3 = folder("e2e-copy-d3");
    write(&src.join("one.txt"), b"1");
    write(&src.join("two.txt"), b"22");
    write(&src.join("sub/inner.txt"), b"x");
    h.app.start(&[uri(&src), uri(&d1)]);
    h.until("both panes", |a| {
        a.dual() && a.panes[0].tab().rows().len() == 3 && a.panes[1].tab().folder.is_ready()
    })
    .await;

    // Commander F5 with confirmation.
    h.app.settings.keymap = cx_tui::settings::Keymap::Commander;
    h.select("one.txt");
    h.key(KeyCode::F(5));
    assert!(matches!(h.app.dialogs.last(), Some(Dialog::Confirm(_))));
    h.key(KeyCode::Enter);
    wait_fs(&mut h, "copy to other pane", || d1.join("one.txt").exists()).await;
    h.until("other pane shows it", |a| {
        a.panes[1].tab().rows().len() == 1
    })
    .await;
    assert!(src.join("one.txt").exists());

    // Undo the copy (trashes the copy).
    h.until("undo entry", |a| !a.undo.is_empty()).await;
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    wait_fs(&mut h, "undo copy", || !d1.join("one.txt").exists()).await;

    // Copy to… several destinations at once: tick two, type a third.
    h.app.settings.bookmarks.push(cx_tui::settings::Bookmark {
        name: "D2".into(),
        uri: uri(&d2),
    });
    h.select("sub");
    h.app.run(Action::CopyTo);
    let Some(Dialog::DestPicker(p)) = h.app.dialogs.last() else {
        panic!("picker")
    };
    let vis = p.visible();
    let other = vis
        .iter()
        .position(|&i| p.items[i].uri == uri(&d1))
        .expect("other pane offered");
    let fav = vis
        .iter()
        .position(|&i| p.items[i].uri == uri(&d2))
        .expect("favorite offered");
    for pos in [other, fav] {
        if let Some(Dialog::DestPicker(p)) = h.app.dialogs.last_mut() {
            p.cursor = pos;
        }
        h.key(KeyCode::Char(' '));
    }
    h.typ(&d3.display().to_string());
    h.key(KeyCode::Enter);
    wait_fs(&mut h, "copies in three places", || {
        d1.join("sub/inner.txt").exists()
            && d2.join("sub/inner.txt").exists()
            && d3.join("sub/inner.txt").exists()
    })
    .await;
    assert!(
        h.app.settings.recent_destinations.len() >= 3,
        "destinations remembered"
    );

    // Move to… one destination, then undo moves it back.
    h.select("two.txt");
    h.app.run(Action::MoveTo);
    h.typ(&d2.display().to_string());
    h.key(KeyCode::Enter);
    wait_fs(&mut h, "moved", || {
        d2.join("two.txt").exists() && !src.join("two.txt").exists()
    })
    .await;
    h.until("undo entry for move", |a| {
        a.undo.iter().any(|u| u.label.starts_with("Move"))
    })
    .await;
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    wait_fs(&mut h, "moved back", || {
        src.join("two.txt").exists() && !d2.join("two.txt").exists()
    })
    .await;

    // F6 moves to the other pane.
    h.app.settings.confirm_transfer = false;
    h.select("one.txt");
    h.key(KeyCode::F(6));
    wait_fs(&mut h, "F6", || {
        d1.join("one.txt").exists() && !src.join("one.txt").exists()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clipboard_duplicate_trash_delete() {
    let mut h = Harness::with_settings(settings()).await;
    let a = folder("e2e-clip-a");
    let b = folder("e2e-clip-b");
    write(&a.join("doc.txt"), b"doc");
    write(&a.join("gone.txt"), b"x");
    write(&a.join("perm.txt"), b"y");
    h.app.start(&[uri(&a)]);
    h.listed(3).await;

    // Ctrl+C, go elsewhere, Ctrl+V.
    h.select("doc.txt");
    h.key_mod(KeyCode::Char('c'), KeyModifiers::CONTROL);
    h.open(&uri(&b)).await;
    h.key_mod(KeyCode::Char('v'), KeyModifiers::CONTROL);
    wait_fs(&mut h, "paste", || b.join("doc.txt").exists()).await;
    h.until("pasted row (live watch)", |ap| ap.tab().rows().len() == 1)
        .await;
    assert!(
        ap_fresh(&h, "doc.txt"),
        "new rows are highlighted: token {} watch {:?}, fresh {:?}",
        h.app.tab().folder.token,
        h.app.tab().folder.watch,
        h.app.tab().folder.fresh
    );

    // Duplicate keeps both.
    h.select("doc.txt");
    h.app.run(Action::Duplicate);
    wait_fs(&mut h, "duplicate", || {
        std::fs::read_dir(&b).unwrap().count() == 2
    })
    .await;

    // Back, trash, undo restores.
    h.key_mod(KeyCode::Left, KeyModifiers::ALT);
    h.until("back", |ap| {
        ap.tab().dir_uri() == uri(&a) && ap.tab().folder.is_ready() && ap.tab().rows().len() == 3
    })
    .await;
    h.select("gone.txt");
    h.key(KeyCode::Delete);
    wait_fs(&mut h, "trashed", || !a.join("gone.txt").exists()).await;
    assert!(h
        .app
        .tab()
        .rows()
        .iter()
        .all(|r| h.app.tab().item(r).name() != "gone.txt"));
    h.until("trash undo entry", |ap| {
        ap.undo.iter().any(|u| u.label.contains("Trash"))
    })
    .await;
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    wait_fs(&mut h, "restored from trash", || {
        a.join("gone.txt").exists()
    })
    .await;

    // Shift+Delete asks, then deletes for good.
    h.select("perm.txt");
    h.key_mod(KeyCode::Delete, KeyModifiers::SHIFT);
    match h.app.dialogs.last_mut() {
        Some(Dialog::Confirm(c)) => {
            assert!(
                c.danger && c.on_cancel,
                "dangerous confirmations default to Cancel"
            );
            c.on_cancel = false;
        }
        _ => panic!("expected a confirmation"),
    }
    h.key(KeyCode::Enter);
    wait_fs(&mut h, "deleted", || !a.join("perm.txt").exists()).await;
}

fn ap_fresh(h: &Harness, name: &str) -> bool {
    h.app.tab().folder.is_fresh(name)
}

#[tokio::test(flavor = "multi_thread")]
async fn multi_rename_and_undo_batch() {
    let mut h = Harness::with_settings(settings()).await;
    let dir = folder("e2e-mrename");
    for n in ["x.JPG", "y.JPG", "z.JPG"] {
        write(&dir.join(n), n.as_bytes());
    }
    h.app.start(&[uri(&dir)]);
    h.listed(3).await;
    h.app.run(Action::SelectAll);
    h.app.run(Action::MultiRename);
    match h.app.dialogs.last_mut() {
        Some(Dialog::MultiRename(m)) => {
            m.name_mask.set("img_[C]");
            m.case = cx_tui::rename::CaseMode::Lower;
            let plan = m.plan();
            assert_eq!(
                plan.iter().map(|p| p.to.as_str()).collect::<Vec<_>>(),
                vec!["img_01.jpg", "img_02.jpg", "img_03.jpg"]
            );
        }
        _ => panic!("multi-rename dialog"),
    }
    h.key(KeyCode::Enter);
    wait_fs(&mut h, "renamed", || {
        dir.join("img_03.jpg").exists() && !dir.join("x.JPG").exists()
    })
    .await;
    h.until("one undo entry", |a| {
        a.undo.last().is_some_and(|u| u.ops.len() == 3)
    })
    .await;
    h.key_mod(KeyCode::Char('z'), KeyModifiers::CONTROL);
    wait_fs(&mut h, "names back", || {
        ["x.JPG", "y.JPG", "z.JPG"]
            .iter()
            .all(|n| dir.join(n).exists())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn search_compress_extract_outline() {
    let mut h = Harness::with_settings(settings()).await;
    let dir = folder("e2e-search");
    write(&dir.join("proj/src/needle.rs"), b"fn haystack() {}\n");
    write(&dir.join("proj/readme.md"), b"find the magic word here\n");
    write(&dir.join("other.txt"), b"nothing");
    h.app.start(&[uri(&dir)]);
    h.listed(2).await;

    // Alt+F7: name search.
    h.key_mod(KeyCode::F(7), KeyModifiers::ALT);
    h.typ("needle");
    h.key(KeyCode::Enter);
    h.until("search results", |a| {
        matches!(
            &a.tab().source,
            cx_tui::tab::Source::Search { task: None, .. }
        ) && a.tab().folder.is_ready()
    })
    .await;
    assert_eq!(h.names(), vec!["needle.rs"]);
    // Content search via the dialog's second field.
    h.key_mod(KeyCode::F(7), KeyModifiers::ALT);
    h.key(KeyCode::Tab);
    h.typ("magic");
    h.key(KeyCode::Enter);
    h.until("content results", |a| {
        matches!(
            &a.tab().source,
            cx_tui::tab::Source::Search {
                task: None,
                content: true,
                ..
            }
        ) && a.tab().folder.is_ready()
    })
    .await;
    assert_eq!(h.names(), vec!["readme.md"]);
    let detail = h
        .app
        .tab()
        .cursor_item()
        .and_then(|i| i.detail.clone())
        .unwrap_or_default();
    assert!(
        detail.contains(":1:") && detail.contains("magic"),
        "snippet with line number: {detail}"
    );

    // Back to the folder; compress "proj" to a zip.
    h.key_mod(KeyCode::Left, KeyModifiers::ALT);
    h.key_mod(KeyCode::Left, KeyModifiers::ALT);
    h.until("folder again", |a| {
        a.tab().is_folder() && a.tab().folder.is_ready() && a.tab().rows().len() == 2
    })
    .await;
    h.select("proj");
    h.app.run(Action::Compress);
    set_prompt(&mut h, "pack");
    h.key(KeyCode::Enter);
    wait_fs(&mut h, "zip", || dir.join("pack.zip").exists()).await;
    h.until("zip job done", |a| {
        a.jobs
            .iter()
            .any(|j| j.kind == "compress" && j.state == "done")
    })
    .await;

    // Browse into the zip like a folder.
    h.until("zip row", |a| {
        a.tab()
            .rows()
            .iter()
            .any(|r| a.tab().item(r).name() == "pack.zip")
    })
    .await;
    h.select("pack.zip");
    h.key(KeyCode::Enter);
    h.until("inside the archive", |a| {
        a.tab().dir_uri().starts_with("archive://") && a.tab().folder.is_ready()
    })
    .await;
    assert_eq!(h.names(), vec!["proj"]);
    h.key(KeyCode::Backspace);
    h.until("out again", |a| {
        a.tab().dir_uri() == uri(&dir) && a.tab().folder.is_ready()
    })
    .await;

    // Extract it into a new folder.
    let out = folder("e2e-search-out");
    std::fs::copy(dir.join("pack.zip"), out.join("pack.zip")).unwrap();
    h.open(&uri(&out)).await;
    h.select("pack.zip");
    h.app.run(Action::Extract);
    wait_fs(&mut h, "extracted", || walk_has(&out, "needle.rs")).await;

    // Outline: → expands in place, the watch shows new files, ← collapses.
    h.open(&uri(&dir)).await;
    h.select("proj");
    h.key(KeyCode::Right);
    h.until("expanded", |a| a.tab().rows().len() == 5).await;
    assert_eq!(
        h.names(),
        vec!["proj", "  src", "  readme.md", "other.txt", "pack.zip"]
    );
    write(&dir.join("proj/late.txt"), b"late");
    h.until("live inside the outline", |a| {
        a.tab()
            .rows()
            .iter()
            .any(|r| a.tab().item(r).name() == "late.txt")
    })
    .await;
    h.select("src");
    h.key_mod(KeyCode::Right, KeyModifiers::SHIFT); // expand recursively
    h.until("recursive", |a| {
        a.tab()
            .rows()
            .iter()
            .any(|r| a.tab().item(r).name() == "needle.rs")
    })
    .await;
    h.select("proj");
    h.key(KeyCode::Left);
    h.until("collapsed", |a| a.tab().rows().len() == 3).await;
}

fn walk_has(dir: &Path, name: &str) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten()
        .any(|e| e.file_name() == name || (e.path().is_dir() && walk_has(&e.path(), name)))
}

#[tokio::test(flavor = "multi_thread")]
async fn navigation_history_filter_tabs_bookmarks() {
    let mut h = Harness::with_settings(settings()).await;
    let dir = folder("e2e-nav");
    std::fs::create_dir_all(dir.join("inner/deeper")).unwrap();
    write(&dir.join("apple.txt"), b"");
    write(&dir.join("banana.txt"), b"");
    h.app.start(&[uri(&dir)]);
    h.listed(3).await;

    // Type to filter; Esc clears.
    h.typ("ban");
    h.app.prepare();
    assert_eq!(h.names(), vec!["banana.txt"]);
    h.key(KeyCode::Esc);
    h.app.prepare();
    assert_eq!(h.names().len(), 3);

    // Enter a folder, Backspace goes up and re-selects it.
    h.select("inner");
    h.key(KeyCode::Enter);
    h.until("inner", |a| {
        a.tab().dir_uri() == uri(&dir.join("inner")) && a.tab().folder.is_ready()
    })
    .await;
    h.key(KeyCode::Backspace);
    h.until("up", |a| {
        a.tab().dir_uri() == uri(&dir) && a.tab().folder.is_ready()
    })
    .await;
    assert_eq!(h.app.tab().cursor_item().unwrap().name(), "inner");
    // Alt+← / Alt+→ walk the history.
    h.key_mod(KeyCode::Left, KeyModifiers::ALT);
    h.until("back to inner", |a| {
        a.tab().dir_uri() == uri(&dir.join("inner"))
    })
    .await;
    h.key_mod(KeyCode::Right, KeyModifiers::ALT);
    h.until("forward", |a| a.tab().dir_uri() == uri(&dir)).await;

    // Go to (Ctrl+L) with a path.
    h.key_mod(KeyCode::Char('l'), KeyModifiers::CONTROL);
    set_prompt(&mut h, &dir.join("inner/deeper").display().to_string());
    h.key(KeyCode::Enter);
    h.until("go to", |a| {
        a.tab().dir_uri() == uri(&dir.join("inner/deeper"))
    })
    .await;

    // Tabs: Ctrl+T, Alt+1, Ctrl+W.
    h.key_mod(KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert_eq!(h.app.pane().tabs.len(), 2);
    h.key_mod(KeyCode::Char('1'), KeyModifiers::ALT);
    assert_eq!(h.app.pane().active, 0);
    h.key_mod(KeyCode::Char('w'), KeyModifiers::CONTROL);
    assert_eq!(h.app.pane().tabs.len(), 1);
    h.app.run(Action::ReopenTab);
    assert_eq!(h.app.pane().tabs.len(), 2);

    // Ctrl+B bookmarks, the hotlist lists it and navigates.
    h.open(&uri(&dir)).await;
    h.key_mod(KeyCode::Char('b'), KeyModifiers::CONTROL);
    assert!(h.app.settings.is_bookmarked(&uri(&dir)));
    h.open(&uri(&dir.join("inner"))).await;
    h.key_mod(KeyCode::Char('d'), KeyModifiers::CONTROL);
    assert!(matches!(h.app.dialogs.last(), Some(Dialog::Menu(_))));
    h.key(KeyCode::Enter);
    h.until("hotlist jump", |a| a.tab().dir_uri() == uri(&dir))
        .await;

    // Session round trip keeps tabs.
    let s = h.app.session();
    assert!(s.panes[0].tabs.len() >= 2);

    // Save a workspace, change tabs, reopen it.
    h.app.run(Action::SaveWorkspace);
    set_prompt(&mut h, "Nav");
    h.key(KeyCode::Enter);
    let saved: usize = h.app.settings.workspaces[0].session.panes[0].tabs.len();
    h.key_mod(KeyCode::Char('t'), KeyModifiers::CONTROL);
    h.key_mod(KeyCode::Char('t'), KeyModifiers::CONTROL);
    assert_eq!(h.app.panes[0].tabs.len(), saved + 2);
    h.app
        .menu_action(cx_tui::dialog::MenuAction::Workspace("Nav".into()));
    assert_eq!(h.app.panes[0].tabs.len(), saved);

    // A search hit: "Show in enclosing folder" opens its folder with it selected.
    h.open(&cx_tui::app::search_uri(&uri(&dir), "deeper", false, false))
        .await;
    h.until("hit", |a| a.tab().rows().len() == 1).await;
    h.app.run(Action::ShowInFolder);
    h.until("enclosing folder", |a| {
        a.tab().dir_uri() == uri(&dir.join("inner")) && a.tab().folder.is_ready()
    })
    .await;
    assert_eq!(h.app.tab().cursor_item().unwrap().name(), "deeper");

    // Home page opens with our favorite on it.
    h.app.run(Action::HomePage);
    h.app.prepare();
    assert!(h.names().iter().any(|n| n == "e2e-nav"));
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_and_sync_panes() {
    let mut h = Harness::with_settings(settings()).await;
    let l = folder("e2e-cmp-l");
    let r = folder("e2e-cmp-r");
    write(&l.join("only-left.txt"), b"l");
    write(&l.join("same.txt"), b"s");
    write(&r.join("same.txt"), b"s");
    write(&r.join("only-right.txt"), b"r");
    h.app.start(&[uri(&l), uri(&r)]);
    h.until("panes", |a| a.panes[1].tab().folder.is_ready())
        .await;
    h.app.run(Action::CompareDirs);
    h.until("diff", |a| {
        matches!(a.tab().source, cx_tui::tab::Source::Compare { .. }) && a.tab().folder.is_ready()
    })
    .await;
    let mut names = h.names();
    names.sort();
    assert_eq!(names, vec!["only-left.txt", "only-right.txt"]);
    // Both ways.
    h.app.menu_action(cx_tui::dialog::MenuAction::Sync(
        cx_transfer::SyncDirection::Both,
    ));
    wait_fs(&mut h, "synced", || {
        r.join("only-left.txt").exists() && l.join("only-right.txt").exists()
    })
    .await;

    // Text diff of two files.
    write(&l.join("a.txt"), b"one\ntwo\nthree\n");
    write(&r.join("a.txt"), b"one\n2\nthree\nfour\n");
    h.app
        .show_diff(uri(&l.join("a.txt")), uri(&r.join("a.txt")));
    h.until("diff dialog", |a| {
        matches!(a.dialogs.last(), Some(Dialog::Diff(_)))
    })
    .await;
    let Some(Dialog::Diff(d)) = h.app.dialogs.last() else {
        unreachable!()
    };
    assert_eq!((d.added, d.removed), (2, 1));
}

/// Going to an SFTP folder: host-key review, then sign-in, then the
/// listing (polled). Needs `CX_TEST_SFTP=1` and docker/sftp running.
#[tokio::test(flavor = "multi_thread")]
async fn sftp_host_key_and_sign_in_prompts() {
    if std::env::var("CX_TEST_SFTP")
        .map(|v| v != "1")
        .unwrap_or(true)
    {
        eprintln!("skipped: set CX_TEST_SFTP=1 with docker/sftp running");
        return;
    }
    let mut h = Harness::with_settings(settings()).await;
    let local = folder("e2e-sftp");
    h.app.start(&[uri(&local)]);
    h.listed(0).await;
    // Ctrl+L, type the URI.
    h.key_mod(KeyCode::Char('l'), KeyModifiers::CONTROL);
    set_prompt(&mut h, "sftp://cx@127.0.0.1:2222/upload");
    h.key(KeyCode::Enter);
    h.until("host key prompt", |a| {
        matches!(a.dialogs.last(), Some(Dialog::HostKey(_)))
    })
    .await;
    let s = h.screen(100, 24);
    assert!(s.contains("Fingerprint") && s.contains("SHA256:"), "{s}");
    h.key(KeyCode::Enter); // Trust and connect
    h.until("sign-in prompt", |a| {
        matches!(a.dialogs.last(), Some(Dialog::SignIn(_)))
    })
    .await;
    if let Some(Dialog::SignIn(s)) = h.app.dialogs.last() {
        assert_eq!(s.user.text, "cx", "user taken from the URI");
        assert_eq!(s.focus, 1, "password field focused");
    }
    h.typ("cxpass");
    h.key(KeyCode::Enter);
    h.until("listing over SFTP", |a| {
        a.dialogs.is_empty() && a.tab().folder.is_ready()
    })
    .await;
    h.wait_live().await;
    assert_eq!(
        h.app.tab().folder.watch.map(|w| w.1),
        Some(cx_engine::WatchMode::Polling)
    );
    // Make a folder there and see it (our own change shows right away).
    h.key(KeyCode::F(7));
    let name = format!("tui-{}", std::process::id());
    set_prompt(&mut h, &name);
    h.key(KeyCode::Enter);
    h.until("remote folder row", |a| {
        a.tab()
            .rows()
            .iter()
            .any(|r| a.tab().item(r).name() == name)
    })
    .await;
    h.select(&name);
    h.app.settings.confirm_permanent_delete = false;
    h.app.run(Action::Trash); // SFTP has no trash: asks to delete permanently
    assert!(matches!(h.app.dialogs.last(), Some(Dialog::Confirm(c)) if c.danger));
    if let Some(Dialog::Confirm(c)) = h.app.dialogs.last_mut() {
        c.on_cancel = false;
    }
    h.key(KeyCode::Enter);
    h.until("remote folder gone", |a| {
        a.tab()
            .rows()
            .iter()
            .all(|r| a.tab().item(r).name() != name)
    })
    .await;
}
