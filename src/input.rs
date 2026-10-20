use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, Default)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
}
impl Input {
    pub fn set(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }
    pub fn insert(&mut self, s: &str) {
        self.insert_limit(s, 8192);
    }
    fn insert_limit(&mut self, s: &str, limit: usize) {
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        if self.text.len() + s.len() > limit {
            return;
        }
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }
    fn previous(&self) -> usize {
        self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i)
    }
    fn next(&self) -> usize {
        self.text[self.cursor..]
            .graphemes(true)
            .next()
            .map_or(self.cursor, |g| self.cursor + g.len())
    }
    pub fn key(&mut self, key: KeyEvent) {
        self.key_with_limit(key, 8192);
    }
    pub fn key_with_limit(&mut self, key: KeyEvent, limit: usize) {
        match (key.code, key.modifiers.contains(KeyModifiers::CONTROL)) {
            (KeyCode::Char('a'), true) | (KeyCode::Home, _) => self.cursor = 0,
            (KeyCode::Char('e'), true) | (KeyCode::End, _) => self.cursor = self.text.len(),
            (KeyCode::Char('w'), true) => {
                let end = self.cursor;
                while self.cursor > 0
                    && self.text[..self.cursor]
                        .chars()
                        .last()
                        .is_some_and(char::is_whitespace)
                {
                    self.cursor = self.previous();
                }
                while self.cursor > 0
                    && self.text[..self.cursor]
                        .chars()
                        .last()
                        .is_some_and(|c| !c.is_whitespace())
                {
                    self.cursor = self.previous();
                }
                self.text.replace_range(self.cursor..end, "");
            }
            (KeyCode::Left, _) => self.cursor = self.previous(),
            (KeyCode::Right, _) => self.cursor = self.next(),
            (KeyCode::Up | KeyCode::Down, _) => {
                let before = &self.text[..self.cursor];
                let line = before.chars().filter(|c| *c == '\n').count();
                let column = before
                    .rsplit('\n')
                    .next()
                    .unwrap_or("")
                    .graphemes(true)
                    .count();
                let lines: Vec<_> = self.text.split('\n').collect();
                let target = line
                    .saturating_add_signed(if key.code == KeyCode::Up { -1 } else { 1 })
                    .min(lines.len() - 1);
                self.cursor = lines[..target]
                    .iter()
                    .map(|line| line.len() + 1)
                    .sum::<usize>()
                    + lines[target]
                        .grapheme_indices(true)
                        .nth(column)
                        .map(|(i, _)| i)
                        .unwrap_or(lines[target].len());
            }
            (KeyCode::Backspace, _) => {
                let start = self.previous();
                self.text.replace_range(start..self.cursor, "");
                self.cursor = start;
            }
            (KeyCode::Delete, _) => {
                let end = self.next();
                self.text.replace_range(self.cursor..end, "");
            }
            (KeyCode::Char(c), false) if !key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert_limit(&c.to_string(), limit)
            }
            _ => {}
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grapheme_cursor_and_delete() {
        let mut i = Input::default();
        i.insert("é👩‍💻e\u{301}中");
        i.key(KeyCode::Backspace.into());
        assert_eq!(i.text, "é👩‍💻e\u{301}");
        i.key(KeyCode::Backspace.into());
        assert_eq!(i.text, "é👩‍💻");
        i.key(KeyCode::Backspace.into());
        assert_eq!(i.text, "é");
        i.insert(" hello world");
        i.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(i.text, "é hello ");
    }
}
