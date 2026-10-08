//! The block editor: a single-block, soft-wrapping text field with a real
//! cursor, plus the trigger detection for the three inline popups
//! (`/` commands, `[[` page links, `((` block refs).

use crate::model::properties;

/// One indent step. Two spaces because a block is not a file: deep indentation
/// in an outliner costs width that the outline needs, and code inside a block
/// is short by nature.
pub const INDENT: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Tree normal: vim's Normal mode with the *block* as the line.
    Normal,
    Insert,
    Visual,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Visual => "VISUAL",
        }
    }

    /// What the hint bar shows; the mode chip alone is too terse.
    pub fn scope(self) -> &'static str {
        match self {
            Mode::Normal => "blocks",
            Mode::Insert => "typing",
            Mode::Visual => "range",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trigger {
    Slash,
    PageLink,
    BlockRef,
    /// The `Ctrl-]` menu: the links already in this block.
    Link,
    Tag,
}

impl Trigger {
    pub fn prefix(self) -> &'static str {
        match self {
            Trigger::Slash => "/",
            Trigger::PageLink => "[[",
            Trigger::BlockRef => "((",
            Trigger::Tag => "#",
            Trigger::Link => "",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Trigger::Slash => "Block commands",
            Trigger::PageLink => "Link to page",
            Trigger::BlockRef => "Reference a block",
            Trigger::Tag => "Tag",
            Trigger::Link => "Links in this block",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Editor {
    pub block_id: i64,
    pub uuid: String,
    pub chars: Vec<char>,
    /// Index into `chars`; `chars.len()` means end of text.
    pub cursor: usize,
    pub mode: Mode,
    /// Selection anchor for Visual mode / shift-selection.
    pub anchor: Option<usize>,
    pub dirty: bool,
    pub indent: usize,
}

impl Editor {
    pub fn new(block_id: i64, uuid: &str, text: &str, indent: usize) -> Self {
        let chars: Vec<char> = text.chars().collect();
        let cursor = chars.len();
        Self {
            block_id,
            uuid: uuid.to_string(),
            chars,
            cursor,
            mode: Mode::Insert,
            anchor: None,
            dirty: false,
            indent,
        }
    }

    /// The caret as vim's ruler reads it: 1-based `(line, column)` *inside this
    /// block*, counting the real newlines that `Alt-⏎` inserts. Soft wrapping is
    /// a display detail and does not move the ruler, exactly as in vim.
    pub fn position(&self) -> (usize, usize) {
        let before = &self.chars[..self.cursor.min(self.chars.len())];
        let line = before.iter().filter(|c| **c == '\n').count() + 1;
        let col = before.iter().rev().take_while(|c| **c != '\n').count() + 1;
        (line, col)
    }

    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    pub fn set_text(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len().min(self.cursor);
    }

    pub fn before_cursor(&self) -> String {
        self.chars[..self.cursor].iter().collect()
    }

    pub fn insert_char(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
        self.dirty = true;
        self.anchor = None;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.chars.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.dirty = true;
        self.anchor = None;
    }

    /// Returns true when the block became empty and should be merged upward.
    pub fn backspace(&mut self) -> bool {
        if self.cursor > 0 {
            self.chars.remove(self.cursor - 1);
            self.cursor -= 1;
            self.dirty = true;
        }
        self.chars.is_empty()
    }

    pub fn delete(&mut self) {
        if self.cursor < self.chars.len() {
            self.chars.remove(self.cursor);
            self.dirty = true;
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }
    pub fn right(&mut self) {
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
    }
    pub fn home(&mut self) {
        // start of the current visual line
        while self.cursor > 0 && self.chars[self.cursor - 1] != '\n' {
            self.cursor -= 1;
        }
    }
    pub fn end(&mut self) {
        while self.cursor < self.chars.len() && self.chars[self.cursor] != '\n' {
            self.cursor += 1;
        }
    }
    pub fn word_left(&mut self) {
        while self.cursor > 0 && self.chars[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
        while self.cursor > 0 && !self.chars[self.cursor - 1].is_whitespace() {
            self.cursor -= 1;
        }
    }
    pub fn word_right(&mut self) {
        while self.cursor < self.chars.len() && !self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        while self.cursor < self.chars.len() && self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
    }

    /// `e` -- forward to the end of the current or next word.
    pub fn word_end(&mut self) {
        let n = self.chars.len();
        if self.cursor < n {
            self.cursor += 1;
        }
        while self.cursor < n && self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        while self.cursor < n && !self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        if self.cursor > 0 {
            self.cursor -= 1; // land *on* the last character, vim-style
        }
    }

    /// `x` / `dl`
    pub fn delete_char(&mut self) {
        self.delete();
    }

    /// `dw`: delete to the start of the next word, eating the trailing space.
    pub fn delete_word(&mut self) {
        let start = self.cursor;
        while self.cursor < self.chars.len() && self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        while self.cursor < self.chars.len() && !self.chars[self.cursor].is_whitespace() {
            self.cursor += 1;
        }
        if self.cursor < self.chars.len() {
            self.cursor += 1;
        }
        let end = self.cursor.min(self.chars.len());
        self.chars.drain(start..end);
        self.cursor = start;
        self.dirty = true;
    }

    pub fn end_of_line(&self) -> usize {
        let mut i = self.cursor;
        while i < self.chars.len() && self.chars[i] != '\n' {
            i += 1;
        }
        i
    }

    /// `Tab` in the text: two spaces at the caret. In a code block this is code
    /// indentation; in prose it is prose indentation. What it is *not* is a
    /// structural re-indent of the whole block, which is what Tab used to do the
    /// moment the caret was in the text.
    pub fn indent(&mut self) {
        self.insert_str(&" ".repeat(INDENT));
    }

    /// `Shift-Tab`: give back one indent step of leading whitespace, on the line
    /// the caret is on and no other. Nothing to give back means nothing happens.
    pub fn dedent(&mut self) {
        let mut start = self.cursor.min(self.chars.len());
        while start > 0 && self.chars[start - 1] != '\n' {
            start -= 1;
        }
        let mut n = 0usize;
        while n < INDENT && self.chars.get(start + n) == Some(&' ') {
            n += 1;
        }
        if n == 0 {
            return;
        }
        self.chars.drain(start..start + n);
        self.cursor = if self.cursor >= start + n {
            self.cursor - n
        } else {
            start
        };
        self.dirty = true;
        self.anchor = None;
    }

    /// Put the cursor at the start of 0-based line `n`, clamped to the last line.
    /// Used by `/Code`, where the caret belongs between the fences.
    pub fn goto_line(&mut self, n: usize) {
        let mut line = 0usize;
        let mut i = 0usize;
        while i < self.chars.len() && line < n {
            if self.chars[i] == '\n' {
                line += 1;
            }
            i += 1;
        }
        self.cursor = i;
    }

    /// `D` / `d$`
    pub fn delete_to_end(&mut self) {
        let end = self.end_of_line();
        self.chars.drain(self.cursor..end);
        self.dirty = true;
    }

    /// `C`
    pub fn change_to_end(&mut self) {
        self.delete_to_end();
    }

    /// `ciw` / `cw`
    pub fn change_word(&mut self) {
        self.delete_word();
    }

    pub fn first_non_blank(&mut self) {
        self.home();
        while self.cursor < self.chars.len() && self.chars[self.cursor] == ' ' {
            self.cursor += 1;
        }
    }

    /// `j` inside one block: down a logical line, keeping the column. Returns
    /// false at the boundary so the caller can hop to the neighbouring block.
    pub fn line_down(&mut self) -> bool {
        let end = self.end_of_line();
        if end >= self.chars.len() {
            return false;
        }
        let col = self.cursor - self.home_index();
        self.cursor = end + 1;
        self.set_col(col);
        true
    }

    /// `k` inside one block.
    pub fn line_up(&mut self) -> bool {
        let start = self.home_index();
        if start == 0 {
            return false;
        }
        let col = self.cursor - start;
        self.cursor = start - 1;
        self.home();
        self.set_col(col);
        true
    }

    fn home_index(&self) -> usize {
        let mut i = self.cursor;
        while i > 0 && self.chars[i - 1] != '\n' {
            i -= 1;
        }
        i
    }

    fn set_col(&mut self, col: usize) {
        let end = self.end_of_line();
        self.cursor = (self.home_index() + col).min(end);
    }

    /// Split at the cursor, returning the text that moves to the new block.
    pub fn split_at_cursor(&mut self) -> String {
        let tail: String = self.chars[self.cursor..].iter().collect();
        self.chars.truncate(self.cursor);
        self.dirty = true;
        tail
    }

    /// Detect an open inline trigger immediately before the cursor.
    pub fn trigger(&self) -> Option<(Trigger, String)> {
        let before = self.before_cursor();
        // /command -- only at the start of a line
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line = &before[line_start..];
        if let Some(idx) = line.rfind('/') {
            if line[..idx].trim().is_empty() && !line[idx..].contains(' ') {
                return Some((Trigger::Slash, line[idx + 1..].to_string()));
            }
        }
        if let Some(idx) = before.rfind("[[") {
            let tail = &before[idx + 2..];
            if !tail.contains("]]") && !tail.contains('\n') {
                return Some((Trigger::PageLink, tail.to_string()));
            }
        }
        if let Some(idx) = before.rfind("((") {
            let tail = &before[idx + 2..];
            if !tail.contains("))") && !tail.contains('\n') {
                return Some((Trigger::BlockRef, tail.to_string()));
            }
        }
        if let Some(idx) = before.rfind('#') {
            let tail = &before[idx + 1..];
            if !tail.is_empty()
                && !tail.contains(' ')
                && !tail.contains('\n')
                && (idx == 0 || before[..idx].ends_with(' ') || before[..idx].ends_with('\n'))
            {
                return Some((Trigger::Tag, tail.to_string()));
            }
        }
        None
    }

    /// Replace the open trigger plus query with the accepted candidate.
    pub fn complete(&mut self, trigger: Trigger, value: &str) {
        if trigger == Trigger::Link {
            // the link menu navigates; it never writes text
            return;
        }
        let before = self.before_cursor();
        let start = match trigger {
            Trigger::Slash => before.rfind('/').map(|i| i),
            Trigger::PageLink => before.rfind("[[").map(|i| i + 2),
            Trigger::BlockRef => before.rfind("((").map(|i| i + 2),
            Trigger::Tag => before.rfind('#').map(|i| i + 1),
            Trigger::Link => None,
        };
        if let Some(start) = start {
            let byte_start: usize = self.chars[..start].len();
            let _ = byte_start;
            self.chars.drain(start..self.cursor);
            self.cursor = start;
            match trigger {
                Trigger::Slash => {
                    // Slash commands replace the whole block text by design.
                    let tail = self.split_at_cursor();
                    self.chars.clear();
                    self.cursor = 0;
                    self.insert_str(value);
                    let _ = tail;
                }
                Trigger::PageLink => self.insert_str(&format!("{}]]", value)),
                Trigger::BlockRef => self.insert_str(&format!("{}))", value)),
                Trigger::Tag => self.insert_str(value),
                Trigger::Link => {}
            }
        } else {
            self.insert_str(value);
        }
    }

    /// `key:: value` properties rendered as a dim preamble.
    pub fn property_lines(&self) -> Vec<(String, String)> {
        properties(&self.text())
    }
}

/// Candidate row for the inline popups.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub label: String,
    pub detail: String,
    pub kind: String,
    /// Byte index of the matching span, for highlighting.
    pub match_at: Option<usize>,
}

/// Cheap subsequence fuzzy match, returning a score and match positions.
pub fn fuzzy(needle: &str, hay: &str) -> Option<(i32, Vec<usize>)> {
    if needle.is_empty() {
        return Some((0, Vec::new()));
    }
    let n: Vec<char> = needle.to_lowercase().chars().collect();
    let h: Vec<char> = hay.chars().collect();
    let hl: Vec<char> = hay.to_lowercase().chars().collect();
    let mut pos = Vec::new();
    let mut ni = 0usize;
    let mut score = 0i32;
    let mut last: Option<usize> = None;
    for (i, c) in hl.iter().enumerate() {
        if ni < n.len() && *c == n[ni] {
            if let Some(l) = last {
                if i == l + 1 {
                    score += 6;
                } else {
                    score -= (i - l) as i32 / 2;
                }
            }
            if i == 0 {
                score += 10;
            }
            score += 4;
            last = Some(i);
            pos.push(i);
            ni += 1;
        }
    }
    if ni == n.len() {
        score -= h.len() as i32 / 8;
        Some((score, pos))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str, cursor: usize) -> Editor {
        let mut e = Editor::new(1, "blk-1", text, 0);
        e.cursor = cursor;
        e
    }

    /// The ruler is what the status bar shows, so it is worth being exact about
    /// it: 1-based, and a newline is a line, not a column.
    #[test]
    fn position_is_one_based_line_and_column() {
        let e = ed("hello", 0);
        assert_eq!(e.position(), (1, 1));
        let e = ed("hello", 5);
        assert_eq!(e.position(), (1, 6), "the end of the text is one past it");
        let e = ed("hello", 2);
        assert_eq!(e.position(), (1, 3));

        let e = ed("one\ntwo", 4);
        assert_eq!(e.position(), (2, 1), "just after the newline is column 1");
        let e = ed("one\ntwo", 3);
        assert_eq!(e.position(), (1, 4), "on the newline, still line 1");
        let e = ed("one\ntwo", 7);
        assert_eq!(e.position(), (2, 4));
    }

    /// A cursor that cannot be past the end: `set_text` clamps it, and the ruler
    /// must not index out of bounds if some other path forgets to.
    #[test]
    fn position_survives_a_cursor_past_the_end() {
        let mut e = ed("abc", 0);
        e.cursor = 99;
        assert_eq!(e.position(), (1, 4));
    }
}

#[cfg(test)]
mod indent_tests {
    use super::*;

    fn ed(text: &str, cursor: usize) -> Editor {
        let mut e = Editor::new(1, "blk-1", text, 0);
        e.cursor = cursor;
        e
    }

    /// `Tab` in the text writes two spaces and nothing else -- it must never be
    /// a structural re-indent of the block the caret is sitting in.
    #[test]
    fn indent_inserts_spaces_at_the_caret() {
        let mut e = ed("abc", 1);
        e.indent();
        assert_eq!(e.text(), "a  bc");
        assert_eq!(e.cursor, 3);
        assert!(e.dirty);

        // In a code block, mid-line indentation is the whole point.
        let mut e = ed("if x:\nreturn 1", 6);
        e.indent();
        assert_eq!(e.text(), "if x:\n  return 1");
    }

    /// Shift-Tab takes back one indent step, and only from the line the caret is
    /// on. It is the inverse of `indent`, so two Tabs and two Shift-Tabs
    /// round-trip.
    #[test]
    fn dedent_removes_one_step_from_this_line_only() {
        let mut e = ed("    one\n        two", 20);
        e.dedent();
        assert_eq!(e.text(), "    one\n      two", "only the caret's line");

        let mut e = ed("    one", 7);
        e.dedent();
        assert_eq!(e.text(), "  one");
        assert_eq!(e.cursor, 5);

        // A cursor inside the leading whitespace is clamped to the new start.
        let mut e = ed("    one", 1);
        e.dedent();
        assert_eq!(e.text(), "  one");
        assert_eq!(e.cursor, 0);

        // Nothing to take: no change, and no dirt.
        let mut e = ed("one\ntwo", 5);
        e.dedent();
        assert_eq!(e.text(), "one\ntwo");
        assert!(!e.dirty);
    }

    #[test]
    fn indent_and_dedent_round_trip() {
        let mut e = ed("code", 0);
        e.indent();
        e.indent();
        assert_eq!(e.text(), "    code");
        e.dedent();
        e.dedent();
        assert_eq!(e.text(), "code");
    }
}
