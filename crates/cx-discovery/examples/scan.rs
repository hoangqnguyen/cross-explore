//! Run discovery on the real network for a few seconds and print what it
//! found: `cargo run -p cx-discovery --example scan [seconds] [--json] [--advertise]`.
//! `--advertise` also announces a dummy `_crossx` peer, which should then
//! show up merged into this machine's device.

use cx_discovery::{Discovery, DiscoveryConfig, DiscoveryEvent};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let secs: u64 = args.iter().find_map(|a| a.parse().ok()).unwrap_or(10);
    let json = args.iter().any(|a| a == "--json");

    let start = Instant::now();
    let (updates, losses) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (u, l) = (updates.clone(), losses.clone());
    let discovery = Discovery::start(DiscoveryConfig::default(), move |event| {
        let t = start.elapsed().as_secs_f32();
        match &event {
            DiscoveryEvent::DeviceUpdated(d) => {
                u.fetch_add(1, Ordering::Relaxed);
                let services: Vec<&str> = d.services.iter().map(|s| s.uri.as_str()).collect();
                eprintln!("[{t:5.2}s] updated {:<28} {:<24} {:?} {services:?}", d.id, d.name, d.kind);
            }
            DiscoveryEvent::DeviceLost { id } => {
                l.fetch_add(1, Ordering::Relaxed);
                eprintln!("[{t:5.2}s] lost    {id}");
            }
        }
    });
    if args.iter().any(|a| a == "--advertise") {
        // Announce a fake peer so the `_crossx` path can be seen end to end.
        let ad = cx_discovery::Advertisement { name: "cx-scan example".into(), port: 47470, device_id: "cx-scan-example".into(), txt: vec![("v".into(), "1".into())] };
        if let Err(e) = discovery.advertise(ad) {
            eprintln!("advertise failed: {e}");
        }
    }
    std::thread::sleep(Duration::from_secs(secs));

    let devices = discovery.devices();
    if json {
        println!("{}", serde_json::to_string_pretty(&devices).unwrap());
        return;
    }
    println!("\n=== {} devices after {secs}s ({} update events, {} lost) ===", devices.len(), updates.load(Ordering::Relaxed), losses.load(Ordering::Relaxed));
    for d in &devices {
        let tailnet = d.tailnet.as_ref().map(|t| format!(" tailnet[{}{} os={} owner={}]", if t.online { "online" } else { "offline" }, if t.is_self { ", self" } else { "" }, t.os, t.owner.as_deref().unwrap_or("-"))).unwrap_or_default();
        println!("\n{} — {:?}{}{}", d.name, d.kind, d.model.as_deref().map(|m| format!(" ({m})")).unwrap_or_default(), tailnet);
        println!("  id: {}   host: {}", d.id, d.hostname.as_deref().unwrap_or("-"));
        println!("  addresses: {}", d.addresses.iter().map(|a| a.to_string()).collect::<Vec<_>>().join(", "));
        println!("  sources: {:?}", d.sources);
        for s in &d.services {
            println!("  service: {:<40} {} (via {:?})", s.uri, s.label, s.source);
        }
        for s in &d.shares {
            println!("  share:   {:<40} {}", s.uri, s.name);
        }
    }
    println!("\n=== suggestions ===");
    for s in discovery.suggestions() {
        println!("  {:>3}  {:<40} {} — {}", s.score, s.uri, s.title, s.subtitle);
    }
}
