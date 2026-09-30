//! A one-line text field with the usual readline keys: ←/→, Home/End
//! (Ctrl+A/E), Backspace/Delete, Ctrl+U/K (kill to start/end), Ctrl+W and
//! Alt+Backspace (kill word).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    pub text: String,
    /// Cursor position in chars.
    pub cursor: usize,
    /// Mask the text (passwords).
    pub secret: bool,
}

impl TextInput {
    pub fn new(text: impl Into<String>) -> TextInput {
        let text = text.into();
        let cursor = text.chars().count();
        TextInput {
            text,
            cursor,
            secret: false,
        }
    }

    pub fn secret() -> TextInput {
        TextInput {
            secret: true,
            ..Default::default()
        }
    }

    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.chars().count();
    }

    pub fn value(&self) -> &str {
        &self.text
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map(|(i, _)| i)
            .unwrap_or(self.text.len())
    }

    fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn insert(&mut self, c: char) {
        let at = self.byte_at(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert(c);
        }
    }

    fn word_start(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        let mut i = self.cursor;
        while i > 0 && !chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        while i > 0 && chars[i - 1].is_alphanumeric() {
            i -= 1;
        }
        i
    }

    fn delete_range(&mut self, from: usize, to: usize) {
        let (a, b) = (self.byte_at(from), self.byte_at(to));
        self.text.replace_range(a..b, "");
        self.cursor = from;
    }

    /// Apply an editing key; returns true when it was one (and the text or
    /// cursor may have changed).
    pub fn handle(&mut self, e: &KeyEvent) -> bool {
        let ctrl = e.modifiers.contains(KeyModifiers::CONTROL);
        let alt = e.modifiers.contains(KeyModifiers::ALT);
        match e.code {
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.len(),
            KeyCode::Char('u') if ctrl => self.delete_range(0, self.cursor),
            KeyCode::Char('k') if ctrl => self.delete_range(self.cursor, self.len()),
            KeyCode::Char('w') if ctrl => self.delete_range(self.word_start(), self.cursor),
            KeyCode::Backspace if alt || ctrl => self.delete_range(self.word_start(), self.cursor),
            KeyCode::Char('b') if alt => self.cursor = self.word_start(),
            KeyCode::Char(c) if !ctrl && !alt && !c.is_control() => self.insert(c),
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.delete_range(self.cursor - 1, self.cursor);
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.len() {
                    self.delete_range(self.cursor, self.cursor + 1);
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.len(),
            _ => return false,
        }
        true
    }

    /// What to draw (masked for secrets).
    pub fn display(&self) -> String {
        if self.secret {
            "•".repeat(self.len())
        } else {
            self.text.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn editing() {
        let mut t = TextInput::new("hello world");
        assert!(t.handle(&key(KeyCode::Char('w'), KeyModifiers::CONTROL)));
        assert_eq!(t.text, "hello ");
        t.handle(&key(KeyCode::Home, KeyModifiers::NONE));
        t.handle(&key(KeyCode::Char('é'), KeyModifiers::NONE));
        assert_eq!(t.text, "éhello ");
        t.handle(&key(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(t.text, "éello ");
        t.handle(&key(KeyCode::Char('k'), KeyModifiers::CONTROL));
        assert_eq!(t.text, "é");
        t.handle(&key(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(t.text, "");
        assert!(!t.handle(&key(KeyCode::F(5), KeyModifiers::NONE)));
        let mut p = TextInput::secret();
        p.insert_str("pw");
        assert_eq!(p.display(), "••");
    }
}
