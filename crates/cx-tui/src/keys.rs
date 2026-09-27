//! Key combos: parsed from the command table's strings ("Ctrl+Shift+P",
//! "Alt+F7", "F5") and built from terminal key events, then compared.
//!
//! Terminals differ in what they report. Without the kitty keyboard
//! protocol, Ctrl+Shift+X arrives as Ctrl+X and Ctrl+\ as Ctrl+4; shifted
//! symbols come with or without a Shift flag. Normalising both sides the
//! same way (letters carry Shift, symbols never do) keeps bindings working
//! on every terminal, and each command lists a fallback that legacy
//! terminals can send.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    /// "a" (letters lower-case), "F5", "Enter", "Tab", "Space", "Up", "+", …
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyCombo {
    /// Parse "Ctrl+Alt+Shift+Key". The last `+`-separated part is the key,
    /// so "Ctrl++" and "+" work.
    pub fn parse(s: &str) -> Option<KeyCombo> {
        let (mods, key) = match s.len() {
            0 => return None,
            1 => ("", s),
            _ if s.ends_with("++") => (&s[..s.len() - 2], "+"),
            _ => match s.rfind('+') {
                Some(i) if i + 1 < s.len() => (&s[..i], &s[i + 1..]),
                _ => ("", s),
            },
        };
        let mut c = KeyCombo { key: String::new(), ctrl: false, alt: false, shift: false };
        for m in mods.split('+').filter(|m| !m.is_empty()) {
            match m {
                "Ctrl" | "Mod" => c.ctrl = true,
                "Alt" => c.alt = true,
                "Shift" => c.shift = true,
                _ => return None,
            }
        }
        // Letters are written upper-case in bindings ("Ctrl+P") without
        // meaning Shift; only an explicit "Shift+" does.
        let key = normalize_name(key);
        c.key = if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() { key.to_ascii_lowercase() } else { key };
        Some(c.normalized())
    }

    fn normalized(mut self) -> KeyCombo {
        let mut chars = self.key.chars();
        if let (Some(ch), None) = (chars.next(), chars.next()) {
            if ch.is_ascii_uppercase() {
                self.key = ch.to_ascii_lowercase().to_string();
                self.shift = true;
            } else if !ch.is_alphanumeric() && ch != ' ' {
                // Symbols: whether Shift was involved depends on the keyboard layout.
                self.shift = false;
            }
        }
        self
    }

    pub fn from_event(e: &KeyEvent) -> Option<KeyCombo> {
        let m = e.modifiers;
        let mut c = KeyCombo { key: String::new(), ctrl: m.contains(KeyModifiers::CONTROL), alt: m.contains(KeyModifiers::ALT), shift: m.contains(KeyModifiers::SHIFT) };
        c.key = match e.code {
            KeyCode::Char(' ') => "Space".into(),
            // Legacy terminals send Ctrl+\ as 0x1c, which crossterm reports as Ctrl+4.
            KeyCode::Char('4') if c.ctrl => "\\".into(),
            KeyCode::Char(ch) => ch.to_string(),
            KeyCode::F(n) => format!("F{n}"),
            KeyCode::Enter => "Enter".into(),
            KeyCode::Tab => "Tab".into(),
            KeyCode::BackTab => {
                c.shift = true;
                "Tab".into()
            }
            KeyCode::Backspace => "Backspace".into(),
            KeyCode::Delete => "Delete".into(),
            KeyCode::Insert => "Insert".into(),
            KeyCode::Esc => "Esc".into(),
            KeyCode::Up => "Up".into(),
            KeyCode::Down => "Down".into(),
            KeyCode::Left => "Left".into(),
            KeyCode::Right => "Right".into(),
            KeyCode::Home => "Home".into(),
            KeyCode::End => "End".into(),
            KeyCode::PageUp => "PageUp".into(),
            KeyCode::PageDown => "PageDown".into(),
            _ => return None,
        };
        Some(c.normalized())
    }

    /// A printable character typed without Ctrl/Alt (type-to-filter).
    pub fn typed_char(e: &KeyEvent) -> Option<char> {
        if e.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return None;
        }
        match e.code {
            KeyCode::Char(c) if !c.is_control() => Some(c),
            _ => None,
        }
    }
}

fn normalize_name(k: &str) -> String {
    match k.to_ascii_lowercase().as_str() {
        "enter" | "return" => "Enter".into(),
        "esc" | "escape" => "Esc".into(),
        "space" => "Space".into(),
        "tab" => "Tab".into(),
        "backspace" => "Backspace".into(),
        "delete" | "del" => "Delete".into(),
        "insert" | "ins" => "Insert".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" | "pgup" => "PageUp".into(),
        "pagedown" | "pgdn" => "PageDown".into(),
        f if f.len() >= 2 && f.starts_with('f') && f[1..].chars().all(|c| c.is_ascii_digit()) => format!("F{}", &f[1..]),
        _ => k.to_string(),
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        let k = match self.key.as_str() {
            "Up" => "↑",
            "Down" => "↓",
            "Left" => "←",
            "Right" => "→",
            k if k.len() == 1 => return f.write_str(&k.to_uppercase()),
            k => k,
        };
        f.write_str(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(code: KeyCode, m: KeyModifiers) -> KeyCombo {
        KeyCombo::from_event(&KeyEvent::new(code, m)).unwrap()
    }

    #[test]
    fn parse_and_events_agree() {
        assert_eq!(KeyCombo::parse("Ctrl+Shift+P").unwrap(), ev(KeyCode::Char('P'), KeyModifiers::CONTROL | KeyModifiers::SHIFT));
        assert_eq!(KeyCombo::parse("Ctrl+P").unwrap(), ev(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(KeyCombo::parse("Alt+F7").unwrap(), ev(KeyCode::F(7), KeyModifiers::ALT));
        assert_eq!(KeyCombo::parse("Shift+Tab").unwrap(), ev(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(KeyCombo::parse("Ctrl+\\").unwrap(), ev(KeyCode::Char('4'), KeyModifiers::CONTROL));
        assert_eq!(KeyCombo::parse("Ctrl+\\").unwrap(), ev(KeyCode::Char('\\'), KeyModifiers::CONTROL));
        assert_eq!(KeyCombo::parse("+").unwrap(), ev(KeyCode::Char('+'), KeyModifiers::SHIFT));
        assert_eq!(KeyCombo::parse("*").unwrap(), ev(KeyCode::Char('*'), KeyModifiers::NONE));
        assert_eq!(KeyCombo::parse("Space").unwrap(), ev(KeyCode::Char(' '), KeyModifiers::NONE));
        assert_eq!(KeyCombo::parse("Alt+Shift+D").unwrap(), ev(KeyCode::Char('D'), KeyModifiers::ALT));
        assert_eq!(KeyCombo::parse("Ctrl++").unwrap().key, "+");
        assert!(KeyCombo::parse("Hyper+X").is_none());
    }

    #[test]
    fn display() {
        assert_eq!(KeyCombo::parse("Ctrl+Shift+p").unwrap().to_string(), "Ctrl+Shift+P");
        assert_eq!(KeyCombo::parse("Alt+Left").unwrap().to_string(), "Alt+←");
        assert_eq!(KeyCombo::parse("f5").unwrap().to_string(), "F5");
    }
}
