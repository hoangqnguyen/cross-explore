//! Thousands of pseudo-random keys and clicks in both keymaps, with every
//! dialog reachable: nothing may panic, and the screen always draws.
//! (Deterministic: the same seed gives the same run.)

mod common;

use common::*;
use cx_tui::settings::{Keymap, Settings};
use cx_tui::ui::layout;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::time::Duration;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }

    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[(self.next() % v.len() as u64) as usize]
    }
}

fn key(rng: &mut Lcg) -> Option<KeyEvent> {
    let codes = [
        KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right, KeyCode::Home, KeyCode::End, KeyCode::PageUp, KeyCode::PageDown,
        KeyCode::Tab, KeyCode::BackTab, KeyCode::Enter, KeyCode::Esc, KeyCode::Backspace, KeyCode::Insert,
        KeyCode::Char(' '), KeyCode::Char('a'), KeyCode::Char('e'), KeyCode::Char('x'), KeyCode::Char('.'), KeyCode::Char('+'),
        KeyCode::Char('-'), KeyCode::Char('*'), KeyCode::Char('/'), KeyCode::Char('1'), KeyCode::Char('c'), KeyCode::Char('k'),
        KeyCode::Char('l'), KeyCode::Char('p'), KeyCode::Char('t'), KeyCode::Char('w'), KeyCode::Char('m'), KeyCode::Char('z'),
        KeyCode::Char('d'), KeyCode::Char('b'), KeyCode::Char('j'), KeyCode::Char('v'), KeyCode::Char('g'),
        KeyCode::F(1), KeyCode::F(2), KeyCode::F(3), KeyCode::F(4), KeyCode::F(5), KeyCode::F(6), KeyCode::F(7), KeyCode::F(9),
    ];
    let mods = [KeyModifiers::NONE, KeyModifiers::NONE, KeyModifiers::NONE, KeyModifiers::SHIFT, KeyModifiers::CONTROL, KeyModifiers::ALT];
    let code = rng.pick(&codes);
    let m = rng.pick(&mods);
    // Never quit, never touch the real Trash, never suspend for a shell.
    match (code, m) {
        (KeyCode::Char('q'), KeyModifiers::CONTROL) | (KeyCode::Char('o'), KeyModifiers::CONTROL) => None,
        (KeyCode::F(4), _) => None,
        _ => Some(KeyEvent::new(code, m)),
    }
}

async fn run(keymap: Keymap, seed: u64) {
    let settings = Settings { keymap, os_clipboard: false, confirm_permanent_delete: true, ..Default::default() };
    let mut h = Harness::with_settings(settings).await;
    let dir = folder(&format!("fuzz-{seed}"));
    for i in 0..30 {
        write(&dir.join(format!("file {i}.txt")), format!("line {i}\n").as_bytes());
    }
    std::fs::create_dir_all(dir.join("sub/deeper")).unwrap();
    write(&dir.join("sub/x.md"), b"# x");
    h.app.start(&[cx_core::Location::local(&dir).uri()]);
    h.listed(31).await;
    let mut rng = Lcg(seed);
    let area = Rect::new(0, 0, 100, 30);
    for step in 0..2500 {
        if rng.next().is_multiple_of(12) {
            let l = layout::compute(area, &h.app);
            let kind = match rng.next() % 4 {
                0 => MouseEventKind::ScrollDown,
                1 => MouseEventKind::ScrollUp,
                2 => MouseEventKind::Down(MouseButton::Right),
                _ => MouseEventKind::Down(MouseButton::Left),
            };
            let ev = MouseEvent { kind, column: (rng.next() % 100) as u16, row: (rng.next() % 30) as u16, modifiers: KeyModifiers::NONE };
            h.app.handle_mouse(ev, &l);
        } else if let Some(k) = key(&mut rng) {
            h.app.handle_key(k);
        }
        // Whatever ran in the background lands now and then.
        if step % 25 == 0 {
            h.settle(Duration::from_millis(5)).await;
            cx_tui::ui::render_to_string(&h.app, 100, 30);
        }
        h.app.quit = false;
        h.app.external = None;
        // Keep the run inside the test folder.
        let here = h.app.tab().dir_uri().to_string();
        if !here.contains(&format!("fuzz-{seed}")) && !here.starts_with("cx:") && !here.starts_with("archive:") {
            h.app.navigate(&cx_core::Location::local(&dir).uri());
        }
    }
    h.settle(Duration::from_millis(100)).await;
    cx_tui::ui::render_to_string(&h.app, 100, 30);
}

#[tokio::test(flavor = "multi_thread")]
async fn random_input_explorer_keys() {
    run(Keymap::Explorer, 6).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn random_input_commander_keys() {
    run(Keymap::Commander, 16).await;
}
