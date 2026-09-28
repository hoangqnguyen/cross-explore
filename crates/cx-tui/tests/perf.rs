//! 100 000-row folders: streaming in, sorting, filtering, selecting and
//! patching must stay interactive. Bounds are loose enough for debug builds
//! on a slow CI machine; release builds are 10–30× faster (run with
//! `cargo test --release -p cx-tui --test perf -- --nocapture` for numbers).

use cx_core::{Change, Entry, EntryKind};
use cx_tui::folder::Folder;
use cx_tui::settings::ViewMode;
use cx_tui::sort::{SortKey, SortSpec};
use cx_tui::tab::{Source, Tab};
use std::time::{Duration, Instant};

const N: usize = 100_000;

fn entry(i: usize) -> Entry {
    // A scrambled order, some folders, varied sizes and dates.
    let k = (i * 7919) % N;
    let is_dir = k.is_multiple_of(50);
    Entry {
        name: if is_dir { format!("Folder {k}") } else { format!("file_{k}.{}", ["txt", "jpg", "rs", "pdf"][k % 4]) },
        kind: if is_dir { EntryKind::Dir } else { EntryKind::File },
        is_dir,
        size: (k as u64 * 131) % 10_000_000,
        modified: Some(1_600_000_000_000 + (k as i64 * 977) % 100_000_000),
        created: None,
        hidden: k.is_multiple_of(97),
        readonly: false,
        executable: false,
    }
}

fn timed<T>(label: &str, limit: Duration, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    let e = t.elapsed();
    println!("{label:<40} {:>8.1} ms", e.as_secs_f64() * 1e3);
    assert!(e < limit, "{label} took {e:?} (limit {limit:?})");
    r
}

#[test]
fn hundred_thousand_rows() {
    let slow = Duration::from_secs(if cfg!(debug_assertions) { 6 } else { 1 });
    let quick = Duration::from_millis(if cfg!(debug_assertions) { 1500 } else { 150 });

    let mut folder = Folder::new(1, "file:///big", SortSpec::default());
    timed("stream 100 batches, drawn every 10", slow, || {
        for b in 0..100 {
            folder.add_batch((b * 1000..(b + 1) * 1000).map(entry).collect());
            if b % 10 == 9 {
                folder.settle(); // what a frame does
            }
        }
        folder.finish_load(0.0);
    });
    assert_eq!(folder.items.len(), N);
    assert!(folder.items.windows(2).all(|w| cx_tui::sort::compare(folder.sort, &w[0], &w[1]).is_le()), "stays sorted");
    let dirs = folder.items.iter().take_while(|i| i.entry.is_dir).count();
    assert_eq!(dirs, N / 50, "folders first");

    let mut tab = Tab::new(1, folder, Source::Folder, ViewMode::Details);
    timed("rows (hidden filtered out)", quick, || tab.refresh_rows(false));
    let visible = tab.rows().len();
    assert!(visible < N && visible > N - 2000);
    timed("rows unchanged (cached)", Duration::from_millis(50), || tab.refresh_rows(false));

    tab.filter = "_123".into();
    timed("type-to-filter \"_123\"", quick, || tab.refresh_rows(false));
    assert!(tab.rows().iter().all(|r| tab.item(r).lname.contains("_123")));
    assert!(!tab.rows().is_empty());
    tab.filter.clear();
    tab.refresh_rows(true);
    assert_eq!(tab.rows().len(), N);

    timed("sort by size, descending", slow, || tab.folder.set_sort(SortSpec { key: SortKey::Size, desc: true }));
    tab.refresh_rows(true);
    let files: Vec<u64> = tab.folder.items.iter().filter(|i| !i.entry.is_dir).map(|i| i.entry.size).collect();
    assert!(files.windows(2).all(|w| w[0] >= w[1]));
    timed("sort by type", slow, || tab.folder.set_sort(SortSpec { key: SortKey::Type, desc: false }));
    timed("sort by name", slow, || tab.folder.set_sort(SortSpec::default()));

    timed("select all", quick, || tab.select_all());
    assert_eq!(tab.selected_rows().len(), N);
    timed("invert selection", quick, || tab.invert_selection());
    assert!(tab.selected_rows().is_empty());
    timed("select *.jpg", slow, || tab.select_pattern("*.jpg", true));

    tab.move_to(50_000, false);
    let key = tab.cursor_item().unwrap().name().to_string();
    let changes: Vec<Change> = (0..100).map(|i| Change::Upsert { entry: Entry { name: format!("new_{i}.txt"), ..entry(1) } }).chain((0..50).map(|i| Change::Remove { name: entry(i * 3 + 1).name })).collect();
    timed("apply 150 watch patches", quick, || tab.folder.apply(changes));
    tab.refresh_rows(true);
    assert_eq!(tab.rows().len(), N + 100 - 50);
    assert_eq!(tab.cursor_item().unwrap().name(), key, "cursor stays on its row");

    let t = Instant::now();
    tab.scroll_into_view(40);
    assert!(t.elapsed() < Duration::from_millis(5));
}
