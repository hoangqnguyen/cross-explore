mod common;

use common::*;
use cx_core::{Location, Provider};
use cx_local::LocalProvider;
use cx_transfer::*;
use std::fs;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn local_copy_keeps_tree_contents_and_times() {
    let env = env();
    let src = env.root().join("src");
    let big = bytes(3 * 1024 * 1024 + 17, 1);
    write(&src.join("a.txt"), b"alpha");
    write(&src.join("sub/b.bin"), &big);
    write(&src.join("sub/deep/c.txt"), b"c");
    fs::create_dir_all(src.join("empty")).unwrap();
    set_mtime(&src.join("a.txt"), 1_600_000_000_123);
    set_mtime(&src.join("sub/b.bin"), 1_500_000_000_000);
    set_mtime(&src.join("sub"), 1_400_000_000_000);
    let dst = env.root().join("dst");
    fs::create_dir(&dst).unwrap();

    let id = env.mgr.submit(JobRequest::copy(vec![uri(&src)], uri(&dst)));
    let snap = env.wait(id).await;
    assert_done(&snap);
    assert_eq!(fs::read(dst.join("src/a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(dst.join("src/sub/b.bin")).unwrap(), big);
    assert_eq!(fs::read(dst.join("src/sub/deep/c.txt")).unwrap(), b"c");
    assert!(dst.join("src/empty").is_dir());
    assert_eq!(mtime(&dst.join("src/a.txt")).await, Some(1_600_000_000_123));
    assert_eq!(mtime(&dst.join("src/sub/b.bin")).await, Some(1_500_000_000_000));
    assert_eq!(mtime(&dst.join("src/sub")).await, Some(1_400_000_000_000));
    assert!(!has_part_files(&dst));
    assert_eq!(snap.progress.files_total, 3);
    assert_eq!(snap.progress.files_done, 3);
    assert_eq!(snap.progress.bytes_done, snap.progress.bytes_total);
    assert_eq!(snap.progress.bytes_total, big.len() as u64 + 6);
    assert!(src.join("a.txt").exists(), "copy keeps the source");

    let undo = env.finished_undo(id).unwrap();
    assert_eq!(undo, UndoOp::Copy { created: vec![uri(&dst.join("src"))] });
    env.mgr.undo(undo).await.unwrap();
    assert!(!dst.join("src").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn events_are_camel_case_and_ordered() {
    let env = env();
    write(&env.root().join("a.txt"), b"x");
    fs::create_dir(env.root().join("d")).unwrap();
    let id = env.mgr.submit(JobRequest::copy(vec![uri(&env.root().join("a.txt"))], uri(&env.root().join("d"))));
    env.wait(id).await;
    let events = env.events.lock().unwrap().clone();
    assert!(matches!(events.first(), Some(TransferEvent::JobAdded { .. })));
    assert!(matches!(events.last(), Some(TransferEvent::Finished { state: JobState::Done, .. })));
    let states: Vec<JobState> = events
        .iter()
        .filter_map(|e| match e {
            TransferEvent::StateChanged { state, .. } => Some(*state),
            _ => None,
        })
        .collect();
    assert_eq!(states, [JobState::Scanning, JobState::Running, JobState::Done]);
    let progress = events.iter().rev().find(|e| matches!(e, TransferEvent::Progress { .. })).unwrap();
    let json = serde_json::to_value(progress).unwrap();
    assert_eq!(json["type"], "progress");
    assert_eq!(json["progress"]["bytesDone"], 1);
    assert_eq!(json["progress"]["filesTotal"], 1);
    let fin = serde_json::to_value(events.last().unwrap()).unwrap();
    assert_eq!(fin["type"], "finished");
    assert_eq!(fin["state"], "done");
    assert_eq!(fin["undo"]["type"], "copy");
}

#[tokio::test(flavor = "multi_thread")]
async fn same_volume_move_is_a_rename_and_undoes() {
    let env = env();
    let src = env.root().join("folder");
    write(&src.join("x/y.txt"), b"y");
    let dst = env.root().join("target");
    fs::create_dir(&dst).unwrap();
    let ino_before = std::fs::metadata(src.join("x/y.txt")).unwrap();
    let id = env.mgr.submit(JobRequest::move_to(vec![uri(&src)], uri(&dst)));
    assert_done(&env.wait(id).await);
    assert!(!src.exists());
    assert_eq!(fs::read(dst.join("folder/x/y.txt")).unwrap(), b"y");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let after = fs::metadata(dst.join("folder/x/y.txt")).unwrap();
        assert_eq!(ino_before.ino(), after.ino(), "a rename keeps the inode");
    }
    let _ = ino_before;
    let undo = env.finished_undo(id).unwrap();
    assert!(matches!(&undo, UndoOp::Move { items } if items.len() == 1));
    env.mgr.undo(undo).await.unwrap();
    assert_eq!(fs::read(src.join("x/y.txt")).unwrap(), b"y");
    assert!(!dst.join("folder").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn move_merges_into_existing_folder() {
    let env = env();
    write(&env.root().join("a/docs/new.txt"), b"new");
    write(&env.root().join("b/docs/old.txt"), b"old");
    let snap = env.run(JobRequest::move_to(vec![uri(&env.root().join("a/docs"))], uri(&env.root().join("b")))).await;
    assert_done(&snap);
    assert_eq!(fs::read(env.root().join("b/docs/new.txt")).unwrap(), b"new");
    assert_eq!(fs::read(env.root().join("b/docs/old.txt")).unwrap(), b"old");
    assert!(!env.root().join("a/docs").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn local_to_remote_and_back() {
    let env = env();
    let src = env.root().join("photos");
    let img = bytes(2 * 1024 * 1024 + 5, 7);
    write(&src.join("img.raw"), &img);
    write(&src.join("sub/note.txt"), b"note");
    set_mtime(&src.join("img.raw"), 1_234_567_890_000);
    env.remote.mkdir_all("/home/pi");

    let up = env.run(JobRequest::copy(vec![uri(&src)], ruri("/home/pi")).with_verify(true)).await;
    assert_done(&up);
    assert_eq!(env.remote.read("/home/pi/photos/img.raw").unwrap(), img);
    assert_eq!(env.remote.read("/home/pi/photos/sub/note.txt").unwrap(), b"note");
    assert_eq!(env.remote.modified("/home/pi/photos/img.raw"), Some(1_234_567_890_000));
    assert!(!env.remote.paths().iter().any(|p| p.ends_with(".cxpart")));

    let back = env.root().join("back");
    fs::create_dir(&back).unwrap();
    let down = env.run(JobRequest::copy(vec![ruri("/home/pi/photos")], uri(&back))).await;
    assert_done(&down);
    assert_eq!(fs::read(back.join("photos/img.raw")).unwrap(), img);
    assert_eq!(mtime(&back.join("photos/img.raw")).await, Some(1_234_567_890_000));
}

#[tokio::test(flavor = "multi_thread")]
async fn cross_provider_move_deletes_source_and_undoes() {
    let env = env();
    let src = env.root().join("move-me");
    write(&src.join("f.txt"), b"f");
    write(&src.join("d/g.txt"), b"g");
    env.remote.mkdir_all("/dst");
    let id = env.mgr.submit(JobRequest::move_to(vec![uri(&src)], ruri("/dst")));
    assert_done(&env.wait(id).await);
    assert!(!src.exists());
    assert_eq!(env.remote.read("/dst/move-me/d/g.txt").unwrap(), b"g");

    env.mgr.undo(env.finished_undo(id).unwrap()).await.unwrap();
    assert_eq!(fs::read(src.join("d/g.txt")).unwrap(), b"g");
    assert!(!env.remote.exists("/dst/move-me"));
}

#[tokio::test(flavor = "multi_thread")]
async fn conflicts_ask_then_keep_both_and_replace_for_all() {
    let mut env = env();
    let (src, dst) = (env.root().join("src"), env.root().join("dst"));
    for n in ["a", "b", "c", "d"] {
        write(&src.join(format!("{n}.txt")), format!("new {n}").as_bytes());
    }
    for n in ["a", "b", "c"] {
        write(&dst.join(format!("{n}.txt")), b"old");
    }
    let sources: Vec<String> = ["a", "b", "c", "d"].iter().map(|n| uri(&src.join(format!("{n}.txt")))).collect();
    let id = env.mgr.submit(JobRequest::copy(sources, uri(&dst)));

    let mut answers = vec![(Resolution::KeepBoth, false), (Resolution::Replace, true)].into_iter();
    let mut asked = Vec::new();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), env.rx.recv()).await.unwrap().unwrap();
        match ev {
            TransferEvent::Conflict { id: jid, conflict } => {
                assert_eq!(env.mgr.job(id).unwrap().state, JobState::WaitingForConflict);
                assert_eq!(conflict.dest.size, 3);
                asked.push(conflict.dest.name.clone());
                let (r, all) = answers.next().expect("asked too often");
                env.mgr.resolve(jid, conflict.conflict_id, r, all);
            }
            TransferEvent::Finished { .. } => break,
            _ => {}
        }
    }
    assert_eq!(asked, ["a.txt", "b.txt"]);
    let snap = env.wait(id).await;
    assert_done(&snap);
    assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"old");
    assert_eq!(fs::read(dst.join("a (2).txt")).unwrap(), b"new a");
    assert_eq!(fs::read(dst.join("b.txt")).unwrap(), b"new b");
    assert_eq!(fs::read(dst.join("c.txt")).unwrap(), b"new c");
    assert_eq!(fs::read(dst.join("d.txt")).unwrap(), b"new d");
    // Replaced files are not "created": undo leaves them.
    let UndoOp::Copy { created } = env.finished_undo(id).unwrap() else { panic!() };
    let mut names: Vec<String> = created.iter().map(|u| Location::parse(u).unwrap().name()).collect();
    names.sort();
    assert_eq!(names, ["a (2).txt", "d.txt"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_skip_for_all_and_folder_merge() {
    let mut env = env();
    let (src, dst) = (env.root().join("src"), env.root().join("dst"));
    write(&src.join("proj/x.txt"), b"new x");
    write(&src.join("proj/y.txt"), b"new y");
    write(&src.join("proj/z.txt"), b"new z");
    write(&dst.join("proj/x.txt"), b"old x");
    write(&dst.join("proj/y.txt"), b"old y");
    let id = env.mgr.submit(JobRequest::copy(vec![uri(&src.join("proj"))], uri(&dst)));
    let mut conflicts = 0;
    loop {
        match tokio::time::timeout(Duration::from_secs(10), env.rx.recv()).await.unwrap().unwrap() {
            TransferEvent::Conflict { id: jid, conflict } => {
                conflicts += 1;
                env.mgr.resolve(jid, conflict.conflict_id, Resolution::Skip, true);
            }
            TransferEvent::Finished { .. } => break,
            _ => {}
        }
    }
    assert_eq!(conflicts, 1, "apply to all answers the rest");
    assert_done(&env.wait(id).await);
    assert_eq!(fs::read(dst.join("proj/x.txt")).unwrap(), b"old x");
    assert_eq!(fs::read(dst.join("proj/y.txt")).unwrap(), b"old y");
    assert_eq!(fs::read(dst.join("proj/z.txt")).unwrap(), b"new z");
}

#[tokio::test(flavor = "multi_thread")]
async fn policies_replace_if_newer_and_keep_both() {
    let env = env();
    let (src, dst) = (env.root().join("src"), env.root().join("dst"));
    write(&src.join("newer.txt"), b"src newer");
    write(&src.join("older.txt"), b"src older");
    write(&dst.join("newer.txt"), b"dst");
    write(&dst.join("older.txt"), b"dst");
    set_mtime(&src.join("newer.txt"), 2_000_000_000_000);
    set_mtime(&dst.join("newer.txt"), 1_000_000_000_000);
    set_mtime(&src.join("older.txt"), 1_000_000_000_000);
    set_mtime(&dst.join("older.txt"), 2_000_000_000_000);
    let sources = vec![uri(&src.join("newer.txt")), uri(&src.join("older.txt"))];
    assert_done(&env.run(JobRequest::copy(sources.clone(), uri(&dst)).with_conflict(ConflictPolicy::ReplaceIfNewer)).await);
    assert_eq!(fs::read(dst.join("newer.txt")).unwrap(), b"src newer");
    assert_eq!(fs::read(dst.join("older.txt")).unwrap(), b"dst");

    assert_done(&env.run(JobRequest::copy(sources, uri(&dst)).with_conflict(ConflictPolicy::KeepBoth)).await);
    assert_eq!(fs::read(dst.join("older (2).txt")).unwrap(), b"src older");
    assert_eq!(fs::read(dst.join("newer (2).txt")).unwrap(), b"src newer");
}

#[tokio::test(flavor = "multi_thread")]
async fn copy_into_itself_is_refused() {
    let env = env();
    let dir = env.root().join("dir");
    write(&dir.join("sub/f.txt"), b"f");
    let snap = env.run(JobRequest::copy(vec![uri(&dir)], uri(&dir.join("sub")))).await;
    assert_eq!(snap.state, JobState::Failed);
    assert!(snap.error.as_deref().unwrap().contains("into itself"), "{:?}", snap.error);
    assert!(!dir.join("sub/dir").exists());

    env.remote.mkdir_all("/r/inner");
    let snap = env.run(JobRequest::move_to(vec![ruri("/r")], ruri("/r/inner"))).await;
    assert_eq!(snap.state, JobState::Failed);
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicating_in_place_uses_copy_names() {
    let env = env();
    let dir = env.root().join("here");
    write(&dir.join("report.pdf"), b"pdf");
    write(&dir.join("folder/x"), b"x");
    let file = vec![uri(&dir.join("report.pdf"))];
    assert_done(&env.run(JobRequest::copy(file.clone(), uri(&dir))).await);
    assert_done(&env.run(JobRequest::copy(file, uri(&dir))).await);
    assert_done(&env.run(JobRequest::copy(vec![uri(&dir.join("folder"))], uri(&dir))).await);
    assert_eq!(fs::read(dir.join("report - Copy.pdf")).unwrap(), b"pdf");
    assert_eq!(fs::read(dir.join("report - Copy (2).pdf")).unwrap(), b"pdf");
    assert_eq!(fs::read(dir.join("folder - Copy/x")).unwrap(), b"x");
    // Moving onto itself does nothing.
    assert_done(&env.run(JobRequest::move_to(vec![uri(&dir.join("report.pdf"))], uri(&dir))).await);
    assert!(dir.join("report.pdf").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_mid_transfer_leaves_no_partial_file() {
    let env = env();
    env.remote.put("/big.bin", bytes(2 * 1024 * 1024, 3));
    env.remote.set_chunk_size(16 * 1024);
    env.remote.set_chunk_delay(Duration::from_millis(2));
    let dst = env.root().join("dl");
    fs::create_dir(&dst).unwrap();
    let id = env.mgr.submit(JobRequest::copy(vec![ruri("/big.bin")], uri(&dst)));
    env.wait_bytes(id, 100_000).await;
    env.mgr.cancel(id);
    let snap = env.wait(id).await;
    assert_eq!(snap.state, JobState::Cancelled);
    assert_eq!(fs::read_dir(&dst).unwrap().count(), 0, "no .cxpart and no final file");
}

#[tokio::test(flavor = "multi_thread")]
async fn resume_after_dropped_connection_gives_identical_bytes() {
    let env = env();
    let data = bytes(1024 * 1024 + 333, 9);
    env.remote.put("/f.bin", data.clone());
    env.remote.set_chunk_size(32 * 1024);
    env.remote.fail_reads_after(300_000, 2);
    let dst = env.root().join("dl");
    fs::create_dir(&dst).unwrap();
    let snap = env.run(JobRequest::copy(vec![ruri("/f.bin")], uri(&dst)).with_verify(true)).await;
    assert_done(&snap);
    assert_eq!(fs::read(dst.join("f.bin")).unwrap(), data);
    // 1 original + 2 resumed reads + 1 verification read.
    assert_eq!(env.remote.reads_opened(), 4);
    assert_eq!(snap.progress.bytes_done, data.len() as u64);

    // Uploads resume too (Append on the .cxpart).
    let up = bytes(700_000, 4);
    write(&env.root().join("up.bin"), &up);
    env.remote.mkdir_all("/in");
    env.remote.fail_writes_after(250_000, 1);
    assert_done(&env.run(JobRequest::copy(vec![uri(&env.root().join("up.bin"))], ruri("/in"))).await);
    assert_eq!(env.remote.read("/in/up.bin").unwrap(), up);
}

#[tokio::test(flavor = "multi_thread")]
async fn gives_up_after_retries_and_reports_the_file() {
    let env = env();
    env.remote.put("/bad.bin", bytes(200_000, 1));
    env.remote.put("/good.txt", b"ok".to_vec());
    env.remote.set_chunk_size(10_000);
    env.remote.fail_reads_after(50_000, 10);
    let dst = env.root().join("dl");
    fs::create_dir(&dst).unwrap();
    let snap = env.run(JobRequest::copy(vec![ruri("/bad.bin"), ruri("/good.txt")], uri(&dst))).await;
    assert_eq!(snap.state, JobState::Done);
    assert_eq!(snap.errors.len(), 1);
    assert!(snap.errors[0].uri.ends_with("/bad.bin"));
    assert_eq!(fs::read(dst.join("good.txt")).unwrap(), b"ok");
    assert!(!dst.join("bad.bin").exists());
    assert!(!has_part_files(&dst));
}

#[tokio::test(flavor = "multi_thread")]
async fn pause_and_resume() {
    let env = env();
    let data = bytes(1024 * 1024, 5);
    env.remote.put("/slow.bin", data.clone());
    env.remote.set_chunk_size(16 * 1024);
    env.remote.set_chunk_delay(Duration::from_millis(2));
    let dst = env.root().join("dl");
    fs::create_dir(&dst).unwrap();
    let id = env.mgr.submit(JobRequest::copy(vec![ruri("/slow.bin")], uri(&dst)));
    env.wait_bytes(id, 50_000).await;
    env.mgr.pause(id);
    tokio::time::sleep(Duration::from_millis(30)).await;
    let paused = env.mgr.job(id).unwrap();
    assert_eq!(paused.state, JobState::Paused);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let later = env.mgr.job(id).unwrap();
    assert_eq!(paused.progress.bytes_done, later.progress.bytes_done, "no bytes move while paused");
    assert!(later.progress.bytes_done < data.len() as u64);
    env.mgr.resume(id);
    assert_done(&env.wait(id).await);
    assert_eq!(fs::read(dst.join("slow.bin")).unwrap(), data);
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_and_rename_undo_and_new_folder_undo() {
    let env = env();
    let dir = env.root().join("d");
    write(&dir.join("x/y.txt"), b"y");
    write(&dir.join("z.txt"), b"z");
    let snap = env.run(JobRequest::delete(vec![uri(&dir.join("x")), uri(&dir.join("missing"))])).await;
    assert_eq!(snap.state, JobState::Done);
    assert_eq!(snap.errors.len(), 1, "missing item reported");
    assert!(!dir.join("x").exists());
    assert!(snap.undo.is_none(), "permanent delete has no undo");

    let loc = Location::local(&dir);
    LocalProvider.rename(&loc, "z.txt", "renamed.txt").await.unwrap();
    env.mgr.undo(UndoOp::rename(&loc, "z.txt", "renamed.txt")).await.unwrap();
    assert_eq!(fs::read(dir.join("z.txt")).unwrap(), b"z");

    let nf = LocalProvider.create_dir(&loc, None).await.unwrap();
    let nf_loc = loc.join(&nf.name);
    write(&dir.join(&nf.name).join("keep"), b"k");
    assert!(env.mgr.undo(UndoOp::new_folder(&nf_loc)).await.is_err(), "non-empty folder stays");
    fs::remove_file(dir.join(&nf.name).join("keep")).unwrap();
    env.mgr.undo(UndoOp::new_folder(&nf_loc)).await.unwrap();
    assert!(!dir.join(&nf.name).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn trash_without_support_reports_per_item() {
    let env = env();
    env.remote.put("/t.txt", b"t".to_vec());
    let snap = env.run(JobRequest::trash(vec![ruri("/t.txt")])).await;
    assert_eq!(snap.errors.len(), 1);
    assert!(env.remote.exists("/t.txt"));
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_to_remote_same_endpoint_move_and_copy() {
    let env = env();
    env.remote.put("/a/f.txt", b"f".to_vec());
    env.remote.mkdir_all("/b");
    assert_done(&env.run(JobRequest::copy(vec![ruri("/a")], ruri("/b"))).await);
    assert_eq!(env.remote.read("/b/a/f.txt").unwrap(), b"f");
    env.remote.mkdir_all("/c");
    assert_done(&env.run(JobRequest::move_to(vec![ruri("/a")], ruri("/c"))).await);
    assert!(!env.remote.exists("/a"));
    assert_eq!(env.remote.read("/c/a/f.txt").unwrap(), b"f");
}

#[tokio::test(flavor = "multi_thread")]
async fn many_small_files_are_batched_and_all_arrive() {
    let env = env();
    let src = env.root().join("many");
    for i in 0..150 {
        write(&src.join(format!("d{}/f{i}.txt", i % 7)), format!("{i}").as_bytes());
    }
    env.remote.mkdir_all("/m");
    let snap = env.run(JobRequest::copy(vec![uri(&src)], ruri("/m"))).await;
    assert_done(&snap);
    assert_eq!(snap.progress.files_done, 150);
    for i in 0..150 {
        assert_eq!(env.remote.read(&format!("/m/many/d{}/f{i}.txt", i % 7)).unwrap(), format!("{i}").as_bytes());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unfinished_jobs_are_restored_and_resume_partial_files() {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    let remote = cx_testkit::MemProvider::with_scheme("sftp");
    let small = b"small".to_vec();
    let big = bytes(1024 * 1024, 11);
    remote.put("/src/a-small.txt", small.clone());
    remote.put("/src/b-big.bin", big.clone());
    remote.set_chunk_size(16 * 1024);
    remote.set_chunk_delay(Duration::from_millis(2));
    let dst = tmp.path().join("dst");
    fs::create_dir(&dst).unwrap();
    let dst_path = dst.clone();

    let first = env_with(tmp, state.clone(), remote.clone());
    let id = first.mgr.submit(JobRequest::copy(vec![ruri("/src")], uri(&dst)));
    first.wait_bytes(id, 300_000).await;
    first.mgr.pause(id);
    tokio::time::sleep(Duration::from_millis(100)).await; // let the record be saved
    assert!(has_part_files(&dst_path), "partial file is on disk");

    // A second app instance finds the unfinished job.
    let vfs = cx_testkit::mem_vfs(std::sync::Arc::new(LocalProvider), remote.clone());
    let second = TransferManager::with_config(vfs, state.clone(), config(), |_| {});
    let pending = second.restore_pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, id);
    assert_eq!(pending[0].state, JobState::Paused);
    let reads_before = remote.reads_opened();
    remote.set_chunk_delay(Duration::ZERO);
    second.resume(id);
    let snap = tokio::time::timeout(Duration::from_secs(20), second.wait(id)).await.unwrap().unwrap();
    assert_done(&snap);
    assert_eq!(fs::read(dst_path.join("src/b-big.bin")).unwrap(), big);
    assert_eq!(fs::read(dst_path.join("src/a-small.txt")).unwrap(), small);
    assert_eq!(remote.reads_opened() - reads_before, 1, "only the unfinished file is read again");
    assert!(!has_part_files(&dst_path));
    assert!(second.restore_pending().is_empty());
    assert!(fs::read_dir(&state).unwrap().next().is_none(), "state file removed when done");
    drop(first);
}

#[tokio::test(flavor = "multi_thread")]
async fn compare_and_sync() {
    let env = env();
    let (l, r) = (env.root().join("left"), env.root().join("right"));
    write(&l.join("same.txt"), b"same");
    write(&r.join("same.txt"), b"same");
    write(&l.join("newer.txt"), b"left is newer");
    write(&r.join("newer.txt"), b"right");
    write(&l.join("older.txt"), b"left");
    write(&r.join("older.txt"), b"right is newer");
    write(&l.join("sub/only-left.txt"), b"L");
    write(&r.join("sub/only-right.txt"), b"R");
    write(&l.join("content.txt"), b"abcd");
    write(&r.join("content.txt"), b"abce");
    write(&l.join("lonely/inner.txt"), b"i");
    let t = 1_700_000_000_000;
    for p in ["same.txt", "content.txt"] {
        set_mtime(&l.join(p), t);
        set_mtime(&r.join(p), t);
    }
    set_mtime(&l.join("newer.txt"), t + 60_000);
    set_mtime(&r.join("newer.txt"), t);
    set_mtime(&l.join("older.txt"), t);
    set_mtime(&r.join("older.txt"), t + 60_000);

    let diff = env.mgr.compare(&uri(&l), &uri(&r), CompareOptions::default()).await.unwrap();
    let kinds: Vec<(&str, DiffKind)> = diff.iter().map(|d| (d.rel_path.as_str(), d.kind)).collect();
    assert_eq!(
        kinds,
        [
            ("content.txt", DiffKind::Same),
            ("lonely", DiffKind::LeftOnly),
            ("newer.txt", DiffKind::NewerLeft),
            ("older.txt", DiffKind::NewerRight),
            ("same.txt", DiffKind::Same),
            ("sub/only-left.txt", DiffKind::LeftOnly),
            ("sub/only-right.txt", DiffKind::RightOnly),
        ]
    );
    let by_content = env.mgr.compare(&uri(&l), &uri(&r), CompareOptions { recursive: false, by: CompareBy::Content }).await.unwrap();
    let get = |p: &str| by_content.iter().find(|d| d.rel_path == p).unwrap().kind;
    assert_eq!(get("content.txt"), DiffKind::Different);
    assert_eq!(get("sub"), DiffKind::Same);
    assert!(!by_content.iter().any(|d| d.rel_path.contains('/')));

    let plan = sync_plan(&uri(&l), &uri(&r), &diff, SyncDirection::Both).unwrap();
    assert_eq!(plan.len(), 4, "{plan:#?}"); // both roots and both "sub" folders
    for req in plan {
        assert_done(&env.run(req).await);
    }
    let after = env.mgr.compare(&uri(&l), &uri(&r), CompareOptions::default()).await.unwrap();
    assert!(after.iter().all(|d| d.kind == DiffKind::Same), "{after:#?}");
    assert_eq!(fs::read(r.join("newer.txt")).unwrap(), b"left is newer");
    assert_eq!(fs::read(l.join("older.txt")).unwrap(), b"right is newer");
    assert_eq!(fs::read(r.join("lonely/inner.txt")).unwrap(), b"i");
}
