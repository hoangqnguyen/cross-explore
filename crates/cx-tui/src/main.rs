//! `cx-tui`: Cross Explore in the terminal.

use cx_engine::{Engine, EngineConfig};
use cx_tui::settings::Settings;
use cx_tui::App;
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
cx-tui — Cross Explore in the terminal

Usage: cx-tui [OPTIONS] [LOCATION [LOCATION]]

Locations are paths or URIs (sftp://, smb://, ftp://, ftps://, dav://,
davs://, s3://, peer://, archive://). Two locations open side by side.

Options:
  --config FILE     settings file (default: the config folder's cross-explore/tui.json)
  --commander       use Total Commander keys (saved)
  --no-discovery    don't look for nearby devices
  --no-peer         don't start peer mode
  --no-mouse        don't capture the mouse
  -h, --help        show this help
  -V, --version     show the version

Press F1 inside for every shortcut, Ctrl+P for the command palette.";

struct Args {
    locations: Vec<String>,
    config: Option<PathBuf>,
    commander: bool,
    discovery: bool,
    peer: bool,
    mouse: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        locations: vec![],
        config: None,
        commander: false,
        discovery: true,
        peer: true,
        mouse: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.into()),
            "-V" | "--version" => return Err(format!("cx-tui {}", env!("CARGO_PKG_VERSION"))),
            "--config" => a.config = Some(it.next().ok_or("--config needs a file")?.into()),
            "--commander" => a.commander = true,
            "--no-discovery" => a.discovery = false,
            "--no-peer" => a.peer = false,
            "--no-mouse" => a.mouse = false,
            s if s.starts_with("--") => return Err(format!("unknown option {s}\n\n{USAGE}")),
            s => {
                // Relative paths are relative to where we were started.
                let loc = if s.contains("://")
                    || s.starts_with('~')
                    || std::path::Path::new(s).is_absolute()
                {
                    s.to_string()
                } else {
                    std::env::current_dir()
                        .map(|d| d.join(s).display().to_string())
                        .unwrap_or_else(|_| s.to_string())
                };
                a.locations.push(loc);
            }
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            println!("{msg}");
            return ExitCode::SUCCESS;
        }
    };
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("cx-tui: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = rt.block_on(async move {
        let mut cfg = EngineConfig::standard()?;
        // Our own unfinished-jobs file: the desktop app may run at the same time.
        cfg.transfers_dir = cfg.data_dir.join("transfers-tui");
        let engine = Engine::new(cfg)?;
        if args.discovery {
            engine.start_discovery();
        }
        if args.peer {
            let e = engine.clone();
            tokio::spawn(async move {
                let _ = e.start_peer().await;
            });
        }
        let path = args.config.clone().or_else(Settings::default_path);
        let mut settings = path.as_deref().map(Settings::load).unwrap_or_default();
        if args.commander {
            settings.keymap = cx_tui::settings::Keymap::Commander;
        }
        if !args.mouse {
            settings.mouse = false;
        }
        let (mut app, rx) = App::new(engine.clone(), settings, path);
        app.start(&args.locations);
        let r = cx_tui::term::run(&mut app, rx).await;
        app.save_settings();
        engine.shutdown().await;
        r.map_err(|e| cx_core::CxError::Io(e.to_string()))
    });
    rt.shutdown_timeout(std::time::Duration::from_millis(300));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cx-tui: {e}");
            ExitCode::FAILURE
        }
    }
}
