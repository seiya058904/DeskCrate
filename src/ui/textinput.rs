//! Minimal single-field text editing model (rename box, filter box).
//! Rendering and IME placement live with the owning window.

#[derive(Clone, Debug, Default)]
pub struct TextInput {
    chars: Vec<char>,
    /// Caret position (char index).
    pub caret: usize,
    /// Selection anchor (char index); selection is `anchor..caret` (unordered).
    pub anchor: usize,
}

impl TextInput {
    pub fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        TextInput { chars, caret: n, anchor: n }
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn len(&self) -> usize {
        self.chars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn select(&mut self, from: usize, to: usize) {
        self.anchor = from.min(self.len());
        self.caret = to.min(self.len());
    }

    pub fn select_all(&mut self) {
        self.select(0, self.len());
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    pub fn has_selection(&self) -> bool {
        self.anchor != self.caret
    }

    fn delete_selection(&mut self) -> bool {
        let (a, b) = self.selection();
        if a == b {
            return false;
        }
        self.chars.drain(a..b);
        self.caret = a;
        self.anchor = a;
        true
    }

    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        for c in s.chars().filter(|c| !c.is_control()) {
            self.chars.insert(self.caret, c);
            self.caret += 1;
        }
        self.anchor = self.caret;
    }

    pub fn backspace(&mut self) {
        if !self.delete_selection() && self.caret > 0 {
            self.caret -= 1;
            self.chars.remove(self.caret);
            self.anchor = self.caret;
        }
    }

    pub fn delete(&mut self) {
        if !self.delete_selection() && self.caret < self.len() {
            self.chars.remove(self.caret);
        }
    }

    pub fn move_to(&mut self, pos: usize, extend: bool) {
        self.caret = pos.min(self.len());
        if !extend {
            self.anchor = self.caret;
        }
    }

    pub fn left(&mut self, extend: bool) {
        if !extend && self.has_selection() {
            let (a, _) = self.selection();
            self.move_to(a, false);
        } else {
            self.move_to(self.caret.saturating_sub(1), extend);
        }
    }

    pub fn right(&mut self, extend: bool) {
        if !extend && self.has_selection() {
            let (_, b) = self.selection();
            self.move_to(b, false);
        } else {
            self.move_to(self.caret + 1, extend);
        }
    }

    pub fn selected_text(&self) -> String {
        let (a, b) = self.selection();
        self.chars[a..b].iter().collect()
    }

    /// UTF-16 offset of a char index (DirectWrite positions are UTF-16).
    pub fn utf16_at(&self, char_idx: usize) -> u32 {
        self.chars[..char_idx.min(self.len())].iter().map(|c| c.len_utf16() as u32).sum()
    }

    /// Char index for a UTF-16 offset.
    pub fn char_at_utf16(&self, pos: u32) -> usize {
        let mut acc = 0u32;
        for (i, c) in self.chars.iter().enumerate() {
            if acc >= pos {
                return i;
            }
            acc += c.len_utf16() as u32;
        }
        self.len()
    }
}

/// Characters Windows forbids in file names.
pub fn valid_file_name(s: &str) -> bool {
    let t = s.trim();
    !t.is_empty()
        && t != "."
        && t != ".."
        && !t.ends_with('.')
        && !s.chars().any(|c| "\\/:*?\"<>|".contains(c) || (c as u32) < 32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing() {
        let mut t = TextInput::new("报告.txt");
        t.select(0, 2);
        t.insert("总结");
        assert_eq!(t.text(), "总结.txt");
        t.move_to(0, false);
        t.delete();
        assert_eq!(t.text(), "结.txt");
        t.select_all();
        t.backspace();
        assert!(t.is_empty());
        let t = TextInput::new("a😀b");
        assert_eq!(t.utf16_at(2), 3);
        assert_eq!(t.char_at_utf16(3), 2);
    }

    #[test]
    fn file_names() {
        assert!(valid_file_name("新建 文档.txt"));
        assert!(!valid_file_name("a:b"));
        assert!(!valid_file_name("  "));
        assert!(!valid_file_name("name."));
    }
}
