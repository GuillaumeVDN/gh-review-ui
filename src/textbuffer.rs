//! A tiny multi-line editor buffer used by the modal editors, plus a
//! display-only soft-wrap helper. Pure — no terminal I/O.

#[derive(Clone, Debug)]
pub struct TextArea {
    pub lines: Vec<String>,
    pub row: usize,
    pub col: usize, // column in *characters*
}

/// What the caller must do in response to a key it doesn't fully own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    None,
    Cancel,
    /// Plain Enter — confirm.
    Enter,
    /// Ctrl+Enter (or Ctrl+X fallback) — alternate confirm.
    CtrlEnter,
}

impl Default for TextArea {
    fn default() -> Self {
        TextArea { lines: vec![String::new()], row: 0, col: 0 }
    }
}

impl TextArea {
    pub fn new(initial: &str) -> Self {
        let lines: Vec<String> = if initial.is_empty() {
            vec![String::new()]
        } else {
            initial.split('\n').map(str::to_string).collect()
        };
        let row = lines.len() - 1;
        let col = lines[row].chars().count();
        TextArea { lines, row, col }
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn cur_chars(&self) -> Vec<char> {
        self.lines[self.row].chars().collect()
    }

    fn set_cur(&mut self, chars: &[char]) {
        self.lines[self.row] = chars.iter().collect();
    }

    pub fn insert(&mut self, ch: char) {
        let mut chars = self.cur_chars();
        chars.insert(self.col.min(chars.len()), ch);
        self.set_cur(&chars);
        self.col += 1;
    }

    /// Insert a (possibly multi-line) string at the cursor.
    pub fn insert_str(&mut self, s: &str) {
        for ch in s.chars() {
            if ch == '\n' {
                self.newline();
            } else {
                self.insert(ch);
            }
        }
    }

    pub fn newline(&mut self) {
        let chars = self.cur_chars();
        let (left, right) = chars.split_at(self.col.min(chars.len()));
        let left: String = left.iter().collect();
        let right: String = right.iter().collect();
        self.lines[self.row] = left;
        self.lines.insert(self.row + 1, right);
        self.row += 1;
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            let mut chars = self.cur_chars();
            chars.remove(self.col - 1);
            self.set_cur(&chars);
            self.col -= 1;
        } else if self.row > 0 {
            let cur = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&cur);
        }
    }

    /// Delete the word before the cursor (Alt+Backspace / Ctrl+W).
    pub fn delete_word(&mut self) {
        if self.col == 0 {
            self.backspace();
            return;
        }
        let chars = self.cur_chars();
        let mut i = self.col;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        let mut new: Vec<char> = chars[..i].to_vec();
        new.extend_from_slice(&chars[self.col..]);
        self.set_cur(&new);
        self.col = i;
    }

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
        }
    }

    pub fn right(&mut self) {
        let len = self.cur_chars().len();
        if self.col < len {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    /// Move to the start of the previous word (Ctrl+Left).
    pub fn word_left(&mut self) {
        if self.col == 0 {
            self.left();
            return;
        }
        let chars = self.cur_chars();
        let mut i = self.col;
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[i - 1].is_whitespace() {
            i -= 1;
        }
        self.col = i;
    }

    /// Move past the end of the current/next word (Ctrl+Right).
    pub fn word_right(&mut self) {
        let chars = self.cur_chars();
        let n = chars.len();
        if self.col >= n {
            self.right();
            return;
        }
        let mut i = self.col;
        while i < n && !chars[i].is_whitespace() {
            i += 1;
        }
        while i < n && chars[i].is_whitespace() {
            i += 1;
        }
        self.col = i;
    }

    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].chars().count());
        }
    }

    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].chars().count());
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = self.cur_chars().len();
    }
}

/// Soft-wrap the buffer's logical lines to `width` for display only.
///
/// Returns `(visual_rows, cursor_visual_row, cursor_visual_col)` where each
/// visual row is `(logical_row, start_col, text)`. No real newline is added.
pub fn wrap(ta: &TextArea, width: usize) -> (Vec<(usize, usize, String)>, usize, usize) {
    let width = width.max(1);
    let mut visual = Vec::new();
    for (lrow, line) in ta.lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let rows = chars.len() / width + 1; // ≥1; trailing empty on exact fit
        for k in 0..rows {
            let start = k * width;
            let end = (start + width).min(chars.len());
            let text: String = chars.get(start..end).unwrap_or(&[]).iter().collect();
            visual.push((lrow, start, text));
        }
    }
    let before: usize = ta.lines[..ta.row]
        .iter()
        .map(|l| l.chars().count() / width + 1)
        .sum();
    (visual, before + ta.col / width, ta.col % width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_newline() {
        let mut ta = TextArea::new("ab");
        ta.insert('c');
        assert_eq!(ta.text(), "abc");
        ta.newline();
        ta.insert('d');
        assert_eq!(ta.text(), "abc\nd");
    }

    #[test]
    fn backspace_joins_lines() {
        let mut ta = TextArea::new("ab\ncd");
        ta.row = 1;
        ta.col = 0;
        ta.backspace();
        assert_eq!(ta.text(), "abcd");
        assert_eq!((ta.row, ta.col), (0, 2));
    }

    #[test]
    fn delete_word_removes_previous_word() {
        let mut ta = TextArea::new("hello world");
        ta.delete_word();
        assert_eq!(ta.text(), "hello ");
        ta.delete_word();
        assert_eq!(ta.text(), "");
    }

    #[test]
    fn delete_word_eats_trailing_space() {
        let mut ta = TextArea::new("foo bar   ");
        ta.delete_word();
        assert_eq!(ta.text(), "foo ");
    }

    #[test]
    fn delete_word_at_line_start_joins() {
        let mut ta = TextArea::new("ab\ncd");
        ta.row = 1;
        ta.col = 0;
        ta.delete_word();
        assert_eq!(ta.text(), "abcd");
    }

    #[test]
    fn insert_multiline_str() {
        let mut ta = TextArea::new("x");
        ta.insert_str("a\nb");
        assert_eq!(ta.text(), "xa\nb");
        assert_eq!((ta.row, ta.col), (1, 1));
    }

    #[test]
    fn word_motion() {
        let mut ta = TextArea::new("one two three");
        ta.col = 13;
        ta.word_left();
        assert_eq!(ta.col, 8); // start of "three"
        ta.word_left();
        assert_eq!(ta.col, 4); // start of "two"
        ta.word_right();
        assert_eq!(ta.col, 8); // past "two" + spaces → start of "three"
        ta.word_right();
        assert_eq!(ta.col, 13); // end
    }

    #[test]
    fn wrap_maps_cursor() {
        let mut ta = TextArea::new("abcdefghij");
        ta.col = 10;
        let (vis, vr, vc) = wrap(&ta, 4);
        let texts: Vec<&str> = vis.iter().map(|(_, _, t)| t.as_str()).collect();
        assert_eq!(texts, ["abcd", "efgh", "ij"]);
        assert_eq!((vr, vc), (2, 2));
    }

    #[test]
    fn wrap_exact_multiple_has_trailing_row() {
        let mut ta = TextArea::new("abcdefgh");
        ta.col = 8;
        let (vis, vr, vc) = wrap(&ta, 4);
        assert_eq!(vis.len(), 3);
        assert_eq!((vr, vc), (2, 0));
    }
}
