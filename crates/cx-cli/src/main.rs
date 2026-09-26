//! `cx`: Cross Explore without a window.
//!
//! `cx serve` runs a headless peer (for a NAS or server) that the desktop app
//! can pair with; the other commands are a small client for scripts and for
//! checking a setup end to end. Client commands use their own identity from
//! `--state-dir`; pairing it once with a server makes it trusted there.

use clap::{Parser, Subcommand};
use cx_core::provider::list_all;
use cx_core::{CxError, Location, MemoryCredentials, Provider, Result, Vfs, WriteMode};
use cx_local::LocalProvider;
use cx_peer::{PeerConfig, PeerEvent, PeerService, Share};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

#[derive(Parser)]
#[command(name = "cx", version, about = "Cross Explore peer tools")]
struct Cli {
    /// Where the device key, paired devices, shares and audit log live.
    #[arg(long, global = true, env = "CX_STATE_DIR")]
    state_dir: Option<PathBuf>,
    /// Name shown to other devices (default: host name).
    #[arg(long, global = true)]
    name: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Serve folders to paired devices until interrupted.
    Serve {
        /// A folder to share: Name=/path, or Name=/path:ro for read-only.
        #[arg(long = "share", value_name = "NAME=PATH[:ro]")]
        shares: Vec<String>,
        #[arg(long, default_value_t = 47470)]
        port: u16,
        /// Listen on this address only (default: all interfaces).
        #[arg(long)]
        bind: Option<std::net::IpAddr>,
        /// Listen only on this machine's Tailscale address.
        #[arg(long)]
        tailscale_only: bool,
        /// Accept devices of the same Tailscale user without pairing.
        #[arg(long)]
        tailnet_auto_trust: bool,
    },
    /// Ask the running `cx serve` (same --state-dir) for a new pairing code.
    PairCode {
        #[arg(long, default_value_t = 47470)]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
    /// Pair with a device showing a code: cx pair nas.local 123456
    Pair { host: String, code: String },
    /// List a folder: cx ls peer://nas.local/Share/dir
    Ls { uri: String },
    /// Download a file: cx get peer://nas/Share/file ./local
    Get { uri: String, dest: PathBuf },
    /// Upload a file: cx put ./file peer://nas/Share/dir/
    Put {
        src: PathBuf,
        uri: String,
        /// Replace an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Print this device's identity.
    Id,
    /// List paired devices.
    Devices,
}

fn parse_share(spec: &str) -> Result<Share> {
    let (name, path) = spec.split_once('=').ok_or_else(|| CxError::InvalidLocation(format!("--share {spec}: expected Name=/path[:ro]")))?;
    let (path, read_only) = match path.strip_suffix(":ro") {
        Some(p) => (p, true),
        None => (path.strip_suffix(":rw").unwrap_or(path), false),
    };
    Ok(Share { name: name.trim().to_string(), path: PathBuf::from(path), read_only })
}

fn state_dir(cli: &Cli) -> PathBuf {
    cli.state_dir.clone().unwrap_or_else(PeerConfig::default_state_dir)
}

/// A client-only service: no listening port of its own.
async fn client(cli: &Cli) -> Result<PeerService> {
    let mut cfg = PeerConfig::new(state_dir(cli));
    cfg.name = cli.name.clone();
    cfg.listen = false;
    cfg.port = 0;
    PeerService::start(cfg, cx_peer::events::ignore_events()).await
}

fn vfs(svc: &PeerService) -> Arc<Vfs> {
    let vfs = Vfs::new(Arc::new(LocalProvider), Arc::new(MemoryCredentials::default()));
    vfs.register(svc.connector());
    vfs
}

fn describe(e: &PeerEvent) -> String {
    match e {
        PeerEvent::RemoteAccess(a) => format!(
            "{} {} {} {}{}",
            if a.name.is_empty() { &a.device_id } else { &a.name },
            a.op,
            a.path.as_deref().unwrap_or(""),
            if a.ok { "ok" } else { "failed" },
            a.error.as_ref().map(|e| format!(": {e}")).unwrap_or_default()
        ),
        PeerEvent::PairingCompleted { device } => format!("paired with {} ({})", device.name, device.device_id),
        PeerEvent::PeerConnected { name, addr, .. } => format!("{name} connected from {addr}"),
        PeerEvent::PeerDisconnected { name, reason, .. } => format!("{name} disconnected: {reason}"),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

async fn serve(cli: &Cli, shares: &[String], port: u16, bind: Option<std::net::IpAddr>, tailscale_only: bool, tailnet_auto_trust: bool) -> Result<()> {
    let mut cfg = PeerConfig::new(state_dir(cli));
    cfg.name = cli.name.clone();
    cfg.port = port;
    cfg.bind = bind;
    cfg.tailscale_only = tailscale_only;
    cfg.tailnet_auto_trust = tailnet_auto_trust;
    if !shares.is_empty() {
        cfg.shares = Some(shares.iter().map(|s| parse_share(s)).collect::<Result<_>>()?);
    }
    let svc = PeerService::start(cfg, Arc::new(|e| println!("event: {}", describe(&e)))).await?;
    // Remember the shares so a later `cx serve` without --share reuses them.
    svc.set_shares(svc.shares())?;
    let id = svc.identity();
    println!("Cross Explore peer \"{}\" ({})", id.name, id.device_id);
    println!("fingerprint: {}", id.fingerprint);
    println!("listening on {}", svc.local_addr()?);
    for s in svc.shares() {
        println!("share {} -> {} ({})", s.name, s.path.display(), if s.read_only { "read-only" } else { "read-write" });
    }
    if svc.shares().is_empty() {
        println!("warning: nothing shared yet (use --share Name=/path)");
    }
    let code = svc.start_pairing();
    println!("pairing code: {} (valid 2 minutes; `cx pair-code` shows a new one)", code.code);
    tokio::signal::ctrl_c().await.map_err(|e| CxError::io("waiting for Ctrl-C", e))?;
    svc.stop().await;
    Ok(())
}

async fn ls(cli: &Cli, uri: &str) -> Result<()> {
    let svc = client(cli).await?;
    let vfs = vfs(&svc);
    let loc = Location::parse(uri)?;
    let p = vfs.provider(&loc).await?;
    let mut rows = list_all(p.as_ref(), &loc).await?;
    rows.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    for e in rows {
        let kind = if e.is_dir { 'd' } else { '-' };
        let ro = if e.readonly { "ro" } else { "rw" };
        println!("{kind}{ro} {:>14} {}{}", if e.is_dir { String::new() } else { e.size.to_string() }, e.name, if e.is_dir { "/" } else { "" });
    }
    Ok(())
}

async fn get(cli: &Cli, uri: &str, dest: &Path) -> Result<()> {
    let svc = client(cli).await?;
    let vfs = vfs(&svc);
    let loc = Location::parse(uri)?;
    let p = vfs.provider(&loc).await?;
    let meta = p.stat(&loc).await?;
    if meta.is_dir {
        return Err(CxError::Unsupported("downloading folders".into()));
    }
    let dest = if dest.is_dir() { dest.join(loc.name()) } else { dest.to_path_buf() };
    let mut r = p.open_read(&loc, 0).await?;
    let mut f = tokio::fs::File::create(&dest).await.map_err(|e| CxError::from_io(e, dest.display()))?;
    let t = std::time::Instant::now();
    let n = tokio::io::copy(&mut r, &mut f).await.map_err(|e| CxError::io(uri, e))?;
    f.flush().await.map_err(|e| CxError::from_io(e, dest.display()))?;
    if let Some(ms) = meta.modified {
        let _ = LocalProvider.set_modified(&Location::local(&dest), ms).await;
    }
    eprintln!("{} -> {} ({n} bytes, {:.1?})", uri, dest.display(), t.elapsed());
    Ok(())
}

async fn put(cli: &Cli, src: &Path, uri: &str, force: bool) -> Result<()> {
    let svc = client(cli).await?;
    let vfs = vfs(&svc);
    let mut loc = Location::parse(uri)?;
    let p = vfs.provider(&loc).await?;
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).ok_or_else(|| CxError::InvalidName(src.display().to_string()))?;
    if uri.ends_with('/') || p.stat(&loc).await.map(|e| e.is_dir).unwrap_or(false) {
        loc = loc.join(&name);
    }
    let meta = std::fs::metadata(src).map_err(|e| CxError::from_io(e, src.display()))?;
    let mut f = tokio::fs::File::open(src).await.map_err(|e| CxError::from_io(e, src.display()))?;
    let mode = if force { WriteMode::Truncate } else { WriteMode::CreateNew };
    let t = std::time::Instant::now();
    let mut w = p.open_write(&loc, mode).await?;
    let n = tokio::io::copy(&mut f, &mut w).await.map_err(|e| CxError::io(loc.uri(), e))?;
    w.shutdown().await.map_err(|e| CxError::io(loc.uri(), e))?;
    if let Some(ms) = cx_core::Entry::from_metadata(name, src, &meta).modified {
        let _ = p.set_modified(&loc, ms).await;
    }
    eprintln!("{} -> {} ({n} bytes, {:.1?})", src.display(), loc, t.elapsed());
    Ok(())
}

async fn run(cli: Cli) -> Result<()> {
    match &cli.cmd {
        Cmd::Serve { shares, port, bind, tailscale_only, tailnet_auto_trust } => serve(&cli, shares, *port, *bind, *tailscale_only, *tailnet_auto_trust).await,
        Cmd::PairCode { port, host } => {
            let svc = client(&cli).await?;
            let code = svc.request_pairing_code(&format!("{host}:{port}")).await?;
            println!("{}", code.code);
            Ok(())
        }
        Cmd::Pair { host, code } => {
            let svc = client(&cli).await?;
            let dev = svc.pair(host, code).await?;
            println!("paired with \"{}\" ({}), fingerprint {}", dev.name, dev.device_id, dev.fingerprint);
            Ok(())
        }
        Cmd::Ls { uri } => ls(&cli, uri).await,
        Cmd::Get { uri, dest } => get(&cli, uri, dest).await,
        Cmd::Put { src, uri, force } => put(&cli, src, uri, *force).await,
        Cmd::Id => {
            let svc = client(&cli).await?;
            println!("{}", serde_json::to_string_pretty(&svc.identity()).unwrap_or_default());
            Ok(())
        }
        Cmd::Devices => {
            let svc = client(&cli).await?;
            for d in svc.trusted_devices() {
                println!("{}  {}  {}  {}", d.device_id, d.fingerprint, d.name, d.last_addrs.join(","));
            }
            Ok(())
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cx: {e}");
            if let CxError::HostKeyUnknown { fingerprint, .. } = &e {
                eprintln!("cx: device fingerprint {fingerprint}; pair first with `cx pair <host> <code>`");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_share_specs() {
        assert_eq!(parse_share("Media=/srv/media:ro").unwrap(), Share { name: "Media".into(), path: "/srv/media".into(), read_only: true });
        assert_eq!(parse_share("Docs=/home/me/docs").unwrap(), Share { name: "Docs".into(), path: "/home/me/docs".into(), read_only: false });
        assert_eq!(parse_share(r"W=C:\data:rw").unwrap().path, PathBuf::from(r"C:\data"));
        assert!(parse_share("nope").is_err());
    }
}
