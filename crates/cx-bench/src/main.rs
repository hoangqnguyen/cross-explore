//! Measures the backend half of the performance budgets from the plan:
//! how fast folders list and how fast changes reach the UI.
//!
//!     cargo run -p cx-bench --release -- [--check] [sizes...]
//!
//! With `--check` the process exits non-zero when a budget is missed, so CI
//! can gate on it. Fixture folders live in the system temp dir and are reused.

use cx_core::{Location, Provider};
use cx_local::LocalProvider;
use cx_local::{watch_dir, Change};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Backend share of the "first rows on screen < 50 ms" budget.
const FIRST_BATCH_BUDGET: Duration = Duration::from_millis(10);
/// Backend share of "10k items fully listed < 300 ms", scaled per 10k items.
const FULL_LIST_BUDGET_PER_10K: Duration = Duration::from_millis(150);
/// Backend share of "local change on screen < 150 ms".
const WATCH_P95_BUDGET: Duration = Duration::from_millis(120);

struct Outcome {
    failed: bool,
}

impl Outcome {
    fn report(&mut self, label: &str, value: Duration, budget: Duration) {
        let ok = value <= budget;
        self.failed |= !ok;
        println!(
            "  {} {label:<28} {:>9.2} ms   (budget {} ms)",
            if ok { "✓" } else { "✗" },
            value.as_secs_f64() * 1e3,
            budget.as_millis()
        );
    }
}

fn fixture(count: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cx-bench-{count}"));
    let ready = std::fs::read_dir(&dir).map(|rd| rd.count() == count).unwrap_or(false);
    if !ready {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..count {
            // 1% folders, the rest files of a few sizes and extensions.
            let p = dir.join(match i % 100 {
                0 => format!("Folder {i}"),
                n if n % 3 == 0 => format!("photo_{i}.jpg"),
                n if n % 3 == 1 => format!("notes {i}.md"),
                _ => format!("report-{i}.pdf"),
            });
            if i % 100 == 0 {
                std::fs::create_dir(&p).unwrap();
            } else {
                std::fs::write(&p, vec![0u8; i % 7 * 100]).unwrap();
            }
        }
    }
    dir
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn percentile(mut v: Vec<Duration>, p: f64) -> Duration {
    v.sort();
    v[((v.len() - 1) as f64 * p).round() as usize]
}

async fn bench_list(dir: &Path, count: usize, out: &mut Outcome) {
    let loc = Location::local(dir);
    let mut firsts = Vec::new();
    let mut totals = Vec::new();
    let mut bytes = 0;
    for _ in 0..7 {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let started = Instant::now();
        let loc2 = loc.clone();
        let task = tokio::spawn(async move { LocalProvider.list(&loc2, tx).await });
        let mut first = None;
        let mut all = Vec::with_capacity(count);
        while let Some(batch) = rx.recv().await {
            first.get_or_insert_with(|| started.elapsed());
            all.extend(batch);
        }
        let n = task.await.unwrap().unwrap();
        totals.push(started.elapsed());
        firsts.push(first.unwrap());
        assert_eq!(n, count);
        bytes = serde_json::to_vec(&all).unwrap().len();
    }
    let scale = (count as f64 / 10_000.0).max(1.0);
    out.report("first batch", median(firsts), FIRST_BATCH_BUDGET);
    out.report("full listing", median(totals), FULL_LIST_BUDGET_PER_10K.mul_f64(scale));
    println!("    IPC payload                  {:>9.2} MB", bytes as f64 / 1e6);
}

fn bench_watch(out: &mut Outcome) {
    let tmp = std::env::temp_dir().join("cx-bench-watch");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let dir = tmp.canonicalize().unwrap();
    let (tx, rx) = mpsc::channel();
    let _w = watch_dir(&dir, move |changes| {
        let seen = Instant::now();
        for c in changes {
            if let Change::Upsert { entry } = c {
                let _ = tx.send((entry.name, seen));
            }
        }
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let mut lat = Vec::new();
    for i in 0..40 {
        let name = format!("file-{i}.txt");
        let t0 = Instant::now();
        std::fs::write(dir.join(&name), b"x").unwrap();
        loop {
            let (got, at) = rx.recv_timeout(Duration::from_secs(3)).expect("watcher event");
            if got == name {
                lat.push(at - t0);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    out.report("change → patch p50", percentile(lat.clone(), 0.5), WATCH_P95_BUDGET);
    out.report("change → patch p95", percentile(lat, 0.95), WATCH_P95_BUDGET);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let check = args.iter().any(|a| a == "--check");
    let mut sizes: Vec<usize> = args.iter().filter_map(|a| a.parse().ok()).collect();
    if sizes.is_empty() {
        sizes = vec![10_000, 100_000];
    }
    if cfg!(debug_assertions) {
        println!("note: debug build; run with --release for meaningful numbers");
    }
    let mut out = Outcome { failed: false };
    for n in sizes {
        print!("preparing {n} files… ");
        let dir = fixture(n);
        println!("{}", dir.display());
        bench_list(&dir, n, &mut out).await;
    }
    println!("live updates");
    bench_watch(&mut out);
    if check && out.failed {
        std::process::exit(1);
    }
}
