//! The terminal loop: raw mode and the alternate screen, an input thread,
//! and one `select!` over input, background messages and a redraw tick.
//! Messages are drained in bursts before drawing, so a folder streaming in
//! thousands of batches redraws at most once per burst.
//!
//! Shells and editors run with the TUI suspended: the input thread is
//! parked first (so it can't steal their keystrokes), the terminal is
//! restored, the program runs in the foreground, then everything comes back.

use crate::app::{App, External};
use crate::msg::Msg;
use crate::ui;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::Terminal;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

struct InputThread {
    paused: Arc<AtomicBool>,
    idle: Arc<AtomicBool>,
}

impl InputThread {
    fn start() -> (InputThread, UnboundedReceiver<Event>) {
        let (tx, rx) = unbounded_channel();
        let paused = Arc::new(AtomicBool::new(false));
        let idle = Arc::new(AtomicBool::new(false));
        let (p, i) = (paused.clone(), idle.clone());
        std::thread::Builder::new()
            .name("cx-tui-input".into())
            .spawn(move || loop {
                if p.load(Ordering::SeqCst) {
                    i.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(15));
                    continue;
                }
                i.store(false, Ordering::SeqCst);
                match event::poll(Duration::from_millis(40)) {
                    Ok(true) => {
                        // Paused while polling: leave the input for the child.
                        if p.load(Ordering::SeqCst) {
                            continue;
                        }
                        match event::read() {
                            Ok(e) => {
                                if tx.send(e).is_err() {
                                    return;
                                }
                            }
                            // A sequence crossterm can't parse: skip it, keep reading.
                            Err(_) => std::thread::sleep(Duration::from_millis(5)),
                        }
                    }
                    Ok(false) => {
                        if tx.is_closed() {
                            return;
                        }
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(20)),
                }
            })
            .expect("input thread");
        (InputThread { paused, idle }, rx)
    }

    fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
        // The thread notices within one poll interval; don't hang if it's gone.
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while !self.idle.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }
}

struct Screen {
    mouse: bool,
    enhanced: bool,
}

impl Screen {
    fn enter(mouse: bool) -> io::Result<Screen> {
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableBracketedPaste)?;
        if mouse {
            execute!(out, EnableMouseCapture)?;
        }
        // Kitty keyboard protocol where available: Ctrl+Shift+X, Ctrl+Tab,
        // Ctrl+Enter and friends become distinguishable.
        let enhanced = terminal::supports_keyboard_enhancement().unwrap_or(false);
        if enhanced {
            execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
        }
        Ok(Screen { mouse, enhanced })
    }

    fn leave(&self) -> io::Result<()> {
        let mut out = io::stdout();
        if self.enhanced {
            let _ = execute!(out, PopKeyboardEnhancementFlags);
        }
        if self.mouse {
            let _ = execute!(out, DisableMouseCapture);
        }
        execute!(out, DisableBracketedPaste, LeaveAlternateScreen, ratatui::crossterm::cursor::Show)?;
        terminal::disable_raw_mode()
    }
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let mut out = io::stdout();
        let _ = execute!(out, PopKeyboardEnhancementFlags, DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen, ratatui::crossterm::cursor::Show);
        let _ = terminal::disable_raw_mode();
        prev(info);
    }));
}

/// Run `ext` in the foreground terminal. Returns an error message if it
/// couldn't start.
fn run_external(ext: &External) -> Result<(), String> {
    let mut cmd = match ext {
        External::Shell { argv, cwd } => {
            let mut c = if argv.is_empty() {
                #[cfg(windows)]
                let sh = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
                #[cfg(not(windows))]
                let sh = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
                std::process::Command::new(sh)
            } else {
                let mut c = std::process::Command::new(&argv[0]);
                c.args(&argv[1..]);
                c
            };
            if let Some(d) = cwd {
                c.current_dir(d);
            }
            println!("\r\nCross Explore: shell in {} — type `exit` to come back.\r\n", cwd.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| "the server".into()));
            c
        }
        External::Edit { path, .. } => {
            let editor = std::env::var("VISUAL").or_else(|_| std::env::var("EDITOR")).ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| if cfg!(windows) { "notepad".into() } else { "vi".into() });
            let mut parts = editor.split_whitespace();
            let mut c = std::process::Command::new(parts.next().unwrap_or("vi"));
            c.args(parts);
            c.arg(path);
            c
        }
    };
    cmd.status().map(|_| ()).map_err(|e| format!("couldn't start {:?}: {e}", cmd.get_program()))
}

/// Run the UI until the user quits.
pub async fn run(app: &mut App, mut rx: UnboundedReceiver<Msg>) -> io::Result<()> {
    install_panic_hook();
    let mut screen = Screen::enter(app.settings.mouse)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut term = Terminal::new(backend)?;
    term.clear()?;
    let (input, mut events) = InputThread::start();
    let mut animating = app.prepare();
    term.draw(|f| ui::render(f, app))?;
    loop {
        let tick = if animating { Duration::from_millis(80) } else { Duration::from_millis(500) };
        tokio::select! {
            Some(ev) = events.recv() => {
                handle_event(app, &term, ev)?;
                // Key repeat: handle queued keys before drawing.
                while let Ok(ev) = events.try_recv() {
                    handle_event(app, &term, ev)?;
                }
            }
            Some(msg) = rx.recv() => {
                app.handle_msg(msg);
                let mut n = 0;
                while let Ok(msg) = rx.try_recv() {
                    app.handle_msg(msg);
                    n += 1;
                    if n > 2000 { break; }
                }
            }
            _ = tokio::time::sleep(tick) => {}
        }
        if app.quit {
            break;
        }
        if let Some(text) = app.osc52.take() {
            let mut out = io::stdout();
            let _ = out.write_all(crate::util::osc52(&text).as_bytes());
            let _ = out.flush();
        }
        if let Some(ext) = app.external.take() {
            input.pause();
            screen.leave()?;
            let result = run_external(&ext);
            screen = Screen::enter(app.settings.mouse)?;
            term.clear()?;
            input.resume();
            match result {
                Err(e) => app.error(e),
                Ok(()) => {
                    if let External::Edit { path, upload } = ext {
                        app.edited(path, upload);
                    }
                    app.reload();
                }
            }
        }
        animating = app.prepare();
        term.draw(|f| ui::render(f, app))?;
    }
    input.pause();
    screen.leave()?;
    Ok(())
}

fn handle_event(app: &mut App, term: &Terminal<CrosstermBackend<io::Stdout>>, ev: Event) -> io::Result<()> {
    match ev {
        Event::Key(k) => app.handle_key(k),
        Event::Mouse(m) => {
            let size = term.size()?;
            let layout = ui::layout::compute(ratatui::layout::Rect::new(0, 0, size.width, size.height), app);
            app.handle_mouse(m, &layout);
        }
        Event::Paste(text) => app.paste_text(&text),
        _ => {}
    }
    Ok(())
}
