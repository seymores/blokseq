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

/// A half-open range of character indices in the buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }
}

/// What `d`, `c` and `y` can be aimed at besides a motion. `iw`, `a"`, `i(`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    /// `iw` / `aw`: a vim word (a run of `[A-Za-z0-9_]`, or a run of punctuation).
    Word,
    /// `iW` / `aW`: anything up to whitespace.
    BigWord,
    /// `i"` / `a"`, and the same for `'` and `` ` ``.
    Quoted(char),
    /// `i(` / `a(`, given as (open, close).
    Bracketed(char, char),
}

/// What `yy`, `dd` and `y`-motions leave behind for `p`. Linewise means `p`
/// pastes as whole lines rather than at the caret, which is the difference
/// between `yy`+`p` and `yiw`+`p` in vim and is worth keeping.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Yank {
    pub text: String,
    pub linewise: bool,
}

#[derive(Clone, Debug)]
pub struct Editor {
    pub block_id: i64,
    pub uuid: String,
    pub chars: Vec<char>,
    /// Index into `chars`; `chars.len()` means end of text.
    pub cursor: usize,
    pub dirty: bool,
    pub indent: usize,
    /// The *text* register. The outline has its own, for whole blocks: a text
    /// yank must never overwrite a block yank just because the caret was in a
    /// block when you pressed `y`.
    pub yank: Option<Yank>,
    /// The last `f`/`F`/`t`/`T`, so `;` and `,` can repeat it.
    pub last_find: Option<(char, bool, bool)>,
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
            dirty: false,
            indent,
            yank: None,
            last_find: None,
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

    /// Replace the buffer outright. Dirty by definition: a buffer that was
    /// changed and not marked dirty is how a test can spend a whole run editing
    /// text that gets thrown away at the next `Esc`.
    pub fn set_text(&mut self, text: &str) {
        self.chars = text.chars().collect();
        self.cursor = self.chars.len().min(self.cursor);
        self.dirty = true;
    }

    pub fn before_cursor(&self) -> String {
        self.chars[..self.cursor].iter().collect()
    }

    pub fn insert_char(&mut self, c: char) {
        self.chars.insert(self.cursor, c);
        self.cursor += 1;
        self.dirty = true;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.chars.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.dirty = true;
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
    }

    /// `Enter` inside a code block: a new line, indented like the one the caret
    /// is on.
    ///
    /// Splitting the block -- what Enter means in prose -- would cut the fence in
    /// half and leave the rest of the program outside it, so this is the one
    /// place Enter does not mean "new block". The closing fence needs no
    /// handling: it is below the caret, so it simply moves down.
    pub fn code_newline(&mut self) {
        let start = self.home_index();
        let indent: String = self.chars[start..self.cursor]
            .iter()
            .take_while(|c| **c == ' ')
            .collect();
        // Typing the fence by hand: Enter at the end of the opening line brings
        // the closing fence with it, so a code block is never left half-open.
        let fence_and_nothing_else = start == 0
            && self.cursor == self.chars.len()
            && !self.chars.contains(&'\n')
            && self.chars.starts_with(&['`', '`', '`']);
        if fence_and_nothing_else {
            self.insert_str("\n\n```");
            // Land on the blank line *between* the fences, not after the last one.
            self.cursor -= 4;
            return;
        }
        self.insert_str("\n");
        self.insert_str(&indent);
    }


    // ------------------------------------------------------- the text engine
    //
    // Everything below acts on the *block's text*: characters, words and the
    // lines inside the block. The outline's verbs -- `dd` on a block, `yy` on a
    // block, `>>` on a block -- live in the tree, and which set you get is
    // decided by where the caret is. Before this split, seventeen vim text keys
    // fell through to the tree from inside a block: `dd` deleted the block,
    // `p` pasted a block, `ciw` inserted a stray `w`, and `r`/`s`/`f`/`X` left
    // the block entirely to report "no mapping".

    /// The line containing `at`, as (start, end) with `end` *excluding* the
    /// newline: a line's text is what you edit, the newline is what separates.
    pub fn line_bounds(&self, at: usize) -> (usize, usize) {
        let at = at.min(self.chars.len());
        let mut start = at;
        while start > 0 && self.chars[start - 1] != '\n' {
            start -= 1;
        }
        let mut end = at;
        while end < self.chars.len() && self.chars[end] != '\n' {
            end += 1;
        }
        (start, end)
    }

    /// How vim classifies a character for word motions and `iw`.
    fn class(c: char) -> u8 {
        if c.is_whitespace() {
            0
        } else if c.is_alphanumeric() || c == '_' {
            1
        } else {
            2
        }
    }

    /// `w` / `W`: the start of the next word.
    pub fn word_next(&mut self, big: bool) {
        let n = self.chars.len();
        if self.cursor >= n {
            return;
        }
        let start = Self::class(self.chars[self.cursor]);
        if start != 0 {
            while self.cursor < n
                && Self::class(self.chars[self.cursor]) != 0
                && (big || Self::class(self.chars[self.cursor]) == start)
            {
                self.cursor += 1;
            }
        }
        while self.cursor < n && Self::class(self.chars[self.cursor]) == 0 {
            self.cursor += 1;
        }
    }

    /// `b` / `B`: the start of this word, or of the one before it.
    pub fn word_prev(&mut self, big: bool) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        while self.cursor > 0 && Self::class(self.chars[self.cursor]) == 0 {
            self.cursor -= 1;
        }
        let start = Self::class(self.chars[self.cursor]);
        while self.cursor > 0 {
            let prev = Self::class(self.chars[self.cursor - 1]);
            if prev == 0 || (!big && prev != start) {
                break;
            }
            self.cursor -= 1;
        }
    }

    /// `e` / `E`: the *last* character of this word, or of the next one.
    pub fn word_next_end(&mut self, big: bool) {
        let n = self.chars.len();
        if self.cursor + 1 >= n {
            self.cursor = n.saturating_sub(1).max(self.cursor.min(n));
            return;
        }
        self.cursor += 1;
        while self.cursor < n && Self::class(self.chars[self.cursor]) == 0 {
            self.cursor += 1;
        }
        let start = Self::class(self.chars[self.cursor.min(n - 1)]);
        while self.cursor + 1 < n {
            let next = Self::class(self.chars[self.cursor + 1]);
            if next == 0 || (!big && next != start) {
                break;
            }
            self.cursor += 1;
        }
    }

    /// `iw` / `aw` / `iW` / `aW`. `None` when the caret is not on or next to a
    /// word, which is what makes `diw` on an empty block do nothing rather than
    /// invent a range.
    pub fn word_object(&self, big: bool) -> Option<Span> {
        let n = self.chars.len();
        if n == 0 {
            return None;
        }
        let mut i = self.cursor.min(n - 1);
        if Self::class(self.chars[i]) == 0 {
            // On whitespace, vim takes the next word, so `diw` at the end of a
            // sentence still does something sensible.
            while i < n && Self::class(self.chars[i]) == 0 {
                i += 1;
            }
            if i >= n {
                return None;
            }
        }
        let class = Self::class(self.chars[i]);
        let same = |c: char| Self::class(c) != 0 && (big || Self::class(c) == class);
        let mut start = i;
        while start > 0 && same(self.chars[start - 1]) {
            start -= 1;
        }
        let mut end = i;
        while end < n && same(self.chars[end]) {
            end += 1;
        }
        Some(Span { start, end })
    }

    /// `i"` / `a"`, `i(` / `a(`, and the same for the other pairs. Depth-aware
    /// for brackets, and it works from either side of the pair, as in vim.
    pub fn object_span(&self, object: Object, inner: bool) -> Option<Span> {
        let span = match object {
            Object::Word | Object::BigWord => {
                let big = object == Object::BigWord;
                let word = self.word_object(big)?;
                if inner {
                    word
                } else {
                    // `aw` takes the trailing whitespace, or the leading one at
                    // the end of the line.
                    let mut end = word.end;
                    let mut start = word.start;
                    while end < self.chars.len() && self.chars[end] == ' ' {
                        end += 1;
                    }
                    if end == word.end {
                        while start > 0 && self.chars[start - 1] == ' ' {
                            start -= 1;
                        }
                    }
                    Span { start, end }
                }
            }
            Object::Quoted(q) => self.quoted_span(q, inner)?,
            Object::Bracketed(open, close) => self.bracketed_span(open, close, inner)?,
        };
        Some(span)
    }

    fn quoted_span(&self, q: char, inner: bool) -> Option<Span> {
        let (ls, le) = self.line_bounds(self.cursor);
        let mut i = ls;
        while i < le {
            if self.chars[i] == q {
                let mut j = i + 1;
                while j < le && self.chars[j] != q {
                    j += 1;
                }
                if j < le {
                    if self.cursor <= j {
                        return Some(if inner {
                            Span { start: i + 1, end: j }
                        } else {
                            Span { start: i, end: j + 1 }
                        });
                    }
                    i = j;
                }
            }
            i += 1;
        }
        None
    }

    fn bracketed_span(&self, open: char, close: char, inner: bool) -> Option<Span> {
        let (ls, le) = self.line_bounds(self.cursor);
        // Backwards for the unmatched opener...
        let mut depth = 0i32;
        let mut start = None;
        let mut i = self.cursor.min(le);
        while i > ls {
            i -= 1;
            if self.chars[i] == close {
                depth += 1;
            } else if self.chars[i] == open {
                if depth == 0 {
                    start = Some(i);
                    break;
                }
                depth -= 1;
            }
        }
        let start = start?;
        // ...then forwards for its match.
        let mut depth = 0i32;
        let mut j = start;
        while j < le {
            if self.chars[j] == open {
                depth += 1;
            } else if self.chars[j] == close {
                depth -= 1;
                if depth == 0 {
                    return Some(if inner {
                        Span { start: start + 1, end: j }
                    } else {
                        Span { start, end: j + 1 }
                    });
                }
            }
            j += 1;
        }
        None
    }

    /// `f`/`F`/`t`/`T`: within the line, and never off the end of it.
    pub fn find_char(&mut self, target: char, forward: bool, till: bool) -> bool {
        let (ls, le) = self.line_bounds(self.cursor);
        let hit = if forward {
            (self.cursor + 1..le).find(|i| self.chars[*i] == target)
        } else {
            (ls..self.cursor).rev().find(|i| self.chars[*i] == target)
        };
        match hit {
            Some(i) => {
                self.cursor = if till {
                    if forward {
                        i.saturating_sub(1).max(ls)
                    } else {
                        i + 1
                    }
                } else {
                    i
                };
                true
            }
            None => false,
        }
    }

    /// The line(s) starting at the caret, *including* the newline, so a
    /// linewise yank can be pasted back as whole lines.
    pub fn line_span(&self, count: usize) -> Span {
        let count = count.max(1);
        let (start, _) = self.line_bounds(self.cursor);
        let mut end = start;
        for _ in 0..count {
            let (_, le) = self.line_bounds(end);
            end = if le < self.chars.len() { le + 1 } else { le };
        }
        Span { start, end }
    }

    // -------------------------------------------------------------- operators

    /// `d` over a range. The text goes to the register, as vim's does.
    pub fn delete_span(&mut self, span: Span, linewise: bool) {
        let end = span.end.min(self.chars.len());
        if span.start >= end {
            return;
        }
        let text: String = self.chars[span.start..end].iter().collect();
        self.yank = Some(Yank { text, linewise });
        self.chars.drain(span.start..end);
        self.cursor = span.start.min(self.chars.len());
        // A linewise delete that took the last line's newline can leave one
        // pointing at nothing.
        if linewise && self.cursor >= self.chars.len() && self.chars.last() == Some(&'\n') {
            self.chars.pop();
            self.cursor = self.chars.len();
        }
        self.dirty = true;
    }

    /// `y` over a range.
    pub fn yank_span(&mut self, span: Span, linewise: bool) {
        let end = span.end.min(self.chars.len());
        if span.start >= end {
            return;
        }
        let text: String = self.chars[span.start..end].iter().collect();
        self.yank = Some(Yank { text, linewise });
    }

    /// `c` over a range: delete it and start typing. The register is filled
    /// linewise when the range was whole lines.
    pub fn change_span(&mut self, span: Span) -> bool {
        self.delete_span(span, false);
        true
    }

    /// `p` / `P` from the *text* register. Linewise yanks go on their own line;
    /// charwise ones go in the text at (or after) the caret. (Not to be confused
    /// with `App::paste_text`, which is the terminal's bracketed paste.)
    pub fn paste_register(&mut self, before: bool) {
        let Some(y) = self.yank.clone() else {
            return;
        };
        if y.linewise {
            let mut text = y.text.clone();
            if !text.ends_with('\n') {
                text.push('\n');
            }
            let (ls, le) = self.line_bounds(self.cursor);
            if before {
                self.cursor = ls;
                self.insert_str(&text);
                self.first_non_blank();
            } else if le < self.chars.len() {
                self.cursor = le + 1;
                self.insert_str(&text);
                self.first_non_blank();
            } else {
                self.cursor = le;
                self.insert_str("\n");
                self.insert_str(text.trim_end_matches('\n'));
                self.first_non_blank();
            }
            return;
        }
        let at = if before {
            self.cursor
        } else {
            (self.cursor + 1).min(self.chars.len())
        };
        self.cursor = at;
        let text = y.text.clone();
        self.insert_str(&text);
        // Vim leaves the caret on the last character it pasted.
        if !text.is_empty() {
            self.cursor = at + text.chars().count() - 1;
        }
    }

    /// `dd`.
    pub fn delete_lines(&mut self, count: usize) {
        let span = self.line_span(count);
        self.delete_span(span, true);
    }

    pub fn yank_lines(&mut self, count: usize) {
        let span = self.line_span(count);
        self.yank_span(span, true);
    }

    /// `cc` / `S`: clear the line's text but keep its indentation, then type.
    pub fn change_lines(&mut self, count: usize) {
        let span = self.line_span(count);
        let (ls, le) = self.line_bounds(span.start);
        let indent: String = self.chars[ls..le].iter().take_while(|c| **c == ' ').collect();
        let mut text: String = self.chars[span.start..span.end.min(self.chars.len())]
            .iter()
            .collect();
        let trailing_newline = text.ends_with('\n');
        if trailing_newline {
            text.pop();
        }
        self.yank = Some(Yank {
            text,
            linewise: true,
        });
        let end = span.end.min(self.chars.len());
        self.chars.drain(span.start..end);
        self.cursor = span.start;
        // The newline that ended the line survives, so the block keeps its shape.
        if trailing_newline && span.start < self.chars.len() {
            self.chars.insert(span.start, '\n');
            self.cursor = span.start;
        }
        self.insert_str(&indent);
    }

    /// `J` inside the text: join this line with the next one, as vim does with
    /// a single space and no leading whitespace from the joined line.
    pub fn join_lines(&mut self) {
        let (_, le) = self.line_bounds(self.cursor);
        if le >= self.chars.len() {
            return;
        }
        let mut end = le + 1;
        while end < self.chars.len() && self.chars[end] == ' ' {
            end += 1;
        }
        self.chars.splice(le..end, [' ']);
        self.cursor = le;
        self.dirty = true;
    }

    /// `>>` / `<<` on the caret's line. (The *block's* indentation is the
    /// tree's `>>`, or Tab with the caret outside the text.)
    pub fn shift_line(&mut self, right: bool, count: usize) {
        let col = self.cursor - self.home_index();
        let n = count.max(1) * INDENT;
        if right {
            self.home();
            self.insert_str(&" ".repeat(n));
            self.set_col(col + n);
        } else {
            for _ in 0..count.max(1) {
                self.dedent();
            }
            self.set_col(col.saturating_sub(n));
        }
    }

    /// `r`: replace one character and stay put, as vim does.
    pub fn replace_char(&mut self, c: char) {
        if self.cursor < self.chars.len() && self.chars[self.cursor] != '\n' {
            self.chars[self.cursor] = c;
            self.dirty = true;
        }
    }

    /// `s`: substitute the character under the caret and start typing.
    pub fn substitute_char(&mut self, count: usize) -> bool {
        let end = (self.cursor + count.max(1)).min(self.end_of_line());
        self.delete_span(
            Span {
                start: self.cursor,
                end,
            },
            false,
        );
        true
    }

    /// `X`: delete the character before the caret, never a newline (vim's `X`
    /// does nothing at the start of a line rather than joining lines).
    pub fn delete_char_before(&mut self) {
        if self.cursor > 0 && self.chars[self.cursor - 1] != '\n' {
            self.chars.remove(self.cursor - 1);
            self.cursor -= 1;
            self.dirty = true;
        }
    }

    /// `~`: flip the case of the character under the caret and move on, as vim
    /// does.
    pub fn toggle_case(&mut self) {
        if self.cursor >= self.chars.len() {
            return;
        }
        let c = self.chars[self.cursor];
        let flipped: String = if c.is_uppercase() {
            c.to_lowercase().collect()
        } else {
            c.to_uppercase().collect()
        };
        self.chars.remove(self.cursor);
        for (i, ch) in flipped.chars().enumerate() {
            self.chars.insert(self.cursor + i, ch);
        }
        self.cursor = (self.cursor + flipped.chars().count()).min(self.chars.len());
        self.dirty = true;
    }

    /// Apply a named motion, `count` times. `false` means "not a motion", which
    /// the caller turns into a refusal rather than a silent no-op.
    pub fn motion(&mut self, name: &str, count: usize) -> bool {
        let count = count.clamp(1, 1000);
        match name {
            "h" => (0..count).for_each(|_| self.left()),
            "l" => (0..count).for_each(|_| self.right()),
            "w" => (0..count).for_each(|_| self.word_next(false)),
            "W" => (0..count).for_each(|_| self.word_next(true)),
            "b" => (0..count).for_each(|_| self.word_prev(false)),
            "B" => (0..count).for_each(|_| self.word_prev(true)),
            "e" => (0..count).for_each(|_| self.word_next_end(false)),
            "E" => (0..count).for_each(|_| self.word_next_end(true)),
            "0" => self.home(),
            "^" => self.first_non_blank(),
            "$" => self.end(),
            // `gj`/`gk`, not `j`/`k`: standalone `j`/`k` leave the block, which
            // is the app's oldest documented deviation. `dj`/`dk` reach the line
            // motions through the operator path instead.
            "gj" => (0..count).for_each(|_| {
                self.line_down();
            }),
            "gk" => (0..count).for_each(|_| {
                self.line_up();
            }),
            ";" => (0..count).for_each(|_| {
                self.repeat_find(true);
            }),
            "," => (0..count).for_each(|_| {
                self.repeat_find(false);
            }),
            _ => return false,
        }
        true
    }

    /// `;` and `,`: the last `f`/`t`, in the same direction or the opposite one.
    pub fn repeat_find(&mut self, same_direction: bool) -> bool {
        match self.last_find {
            Some((c, forward, till)) => {
                let f = if same_direction { forward } else { !forward };
                self.find_char(c, f, till)
            }
            None => false,
        }
    }

    /// `de` / `ce`: to the end of the word, *inclusive* -- the difference
    /// between `cw` and `dw`, and the reason `cw` must not eat the space after
    /// the word the way `dw` legitimately does.
    pub fn word_end_span(&self) -> Span {
        let mut at = self.cursor;
        let n = self.chars.len();
        while at < n && Self::class(self.chars[at]) == 0 {
            at += 1;
        }
        let class = if at < n { Some(Self::class(self.chars[at])) } else { None };
        while at < n {
            if Self::class(self.chars[at]) == 0 {
                break;
            }
            if Some(Self::class(self.chars[at])) != class {
                break;
            }
            at += 1;
        }
        Span {
            start: self.cursor,
            end: at.min(n),
        }
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


/// What the text grammar did with a key sequence.
///
/// The distinction that matters is `Unknown` versus `Incomplete`: a partial
/// command must wait for its next key (`ci` waiting for the object), while an
/// unknown one must be *refused* rather than handed to the outline. Handing it
/// over is how `dd` came to delete the block you were typing in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Done; the caller's mode does not change.
    Done,
    /// Done, and now start typing: `i`, `s`, `cw`, `C`.
    Insert,
    /// `Esc` in the text: drop the caret, keep the block selected.
    DropCaret,
    /// `q`.
    Quit,
    /// Keep the keys and wait for more.
    Incomplete,
    /// Not part of the text grammar.
    Unknown,
}

/// Split a leading count: `"3w"` is 3 `w`s, `"d2w"` is left as it is for the
/// operator to parse. A lone `0` is the motion, not a count, exactly as in vim.
fn take_count(seq: &str) -> (usize, &str) {
    let digits: String = seq.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits == "0" {
        return (1, seq);
    }
    let n = digits.parse::<usize>().unwrap_or(1).clamp(1, 1000);
    (n, &seq[digits.len()..])
}

/// What `d`/`c`/`y` is aimed at.
enum Target {
    /// A range, and whether it is whole lines.
    Span(Span, bool),
    /// Waiting for more keys (`di`, `df`).
    Incomplete,
    /// Not a motion or an object.
    Invalid,
}

impl Editor {
    /// The whole text grammar, in one place.
    ///
    /// This is deliberately the only path from a key sequence to a text edit:
    /// the key router calls it first whenever the caret is in a block, the tests
    /// call it directly, and anything it does not recognise is refused instead
    /// of falling through to the outline's verbs.
    pub fn command(&mut self, seq: &str) -> Outcome {
        if seq.is_empty() {
            return Outcome::Incomplete;
        }
        let (count, rest) = take_count(seq);
        if rest.is_empty() {
            return Outcome::Incomplete;
        }

        // One-character arguments: `r`, and the find motions.
        if let Some(arg) = rest.strip_prefix('r') {
            return match arg.chars().count() {
                0 => Outcome::Incomplete,
                1 => {
                    self.replace_char(arg.chars().next().unwrap());
                    Outcome::Done
                }
                _ => Outcome::Unknown,
            };
        }
        if let Some(o) = self.find_form(rest, count) {
            return o;
        }

        // Operators.
        if let Some(op) = rest.chars().next().filter(|c| matches!(c, 'd' | 'c' | 'y')) {
            if rest.chars().count() == 1 {
                return Outcome::Incomplete;
            }
            let body = &rest[1..];
            // `dd`, `yy`, `cc`: whole lines. `cc` keeps the line's indentation,
            // which is what makes it usable in a code block.
            if body == "d" || body == "c" || body == "y" {
                match op {
                    'd' => self.delete_lines(count),
                    'y' => self.yank_lines(count),
                    _ => {
                        self.change_lines(count);
                        return Outcome::Insert;
                    }
                }
                return Outcome::Done;
            }
            return match self.target(op, body, count) {
                Target::Incomplete => Outcome::Incomplete,
                Target::Invalid => Outcome::Unknown,
                Target::Span(span, linewise) => {
                    self.apply_operator(op, span, linewise);
                    if op == 'c' {
                        Outcome::Insert
                    } else {
                        Outcome::Done
                    }
                }
            };
        }

        // Plain commands.
        match rest {
            "i" => return Outcome::Insert,
            "a" => {
                self.right();
                return Outcome::Insert;
            }
            "I" => {
                self.first_non_blank();
                return Outcome::Insert;
            }
            "A" => {
                self.end();
                return Outcome::Insert;
            }
            "x" => {
                for _ in 0..count {
                    self.delete_char();
                }
                Outcome::Done
            }
            "X" => {
                for _ in 0..count {
                    self.delete_char_before();
                }
                Outcome::Done
            }
            "D" => {
                self.delete_to_end();
                Outcome::Done
            }
            "C" => {
                self.delete_to_end();
                Outcome::Insert
            }
            "s" => {
                self.substitute_char(count);
                Outcome::Insert
            }
            "S" => {
                self.change_lines(count);
                Outcome::Insert
            }
            "~" => {
                for _ in 0..count {
                    self.toggle_case();
                }
                Outcome::Done
            }
            "J" => {
                for _ in 0..count {
                    self.join_lines();
                }
                Outcome::Done
            }
            "p" => {
                for _ in 0..count {
                    self.paste_register(false);
                }
                Outcome::Done
            }
            "P" => {
                for _ in 0..count {
                    self.paste_register(true);
                }
                Outcome::Done
            }
            // A lone `>` or `<` waits for its twin: `>>` shifts the line.
            ">" | "<" => Outcome::Incomplete,
            "<Tab>" => {
                for _ in 0..count {
                    self.indent();
                }
                Outcome::Done
            }
            "<S-Tab>" => {
                for _ in 0..count {
                    self.dedent();
                }
                Outcome::Done
            }
            "<CR>" => Outcome::Insert,
            ">>" => {
                self.shift_line(true, count);
                Outcome::Done
            }
            "<<" => {
                self.shift_line(false, count);
                Outcome::Done
            }
            "<Esc>" => Outcome::DropCaret,
            "q" => Outcome::Quit,
            other => {
                if self.motion(other, count) {
                    Outcome::Done
                } else if other.starts_with('g') && "gjgk".starts_with(other) {
                    Outcome::Incomplete
                } else {
                    Outcome::Unknown
                }
            }
        }
    }

    /// `f,` `t(` `F"` `T;`, and the `;`/`,` repeats. `None` when `seq` is not a
    /// find form at all.
    fn find_form(&mut self, seq: &str, count: usize) -> Option<Outcome> {
        let k = seq.chars().next()?;
        let (forward, till) = match k {
            'f' => (true, false),
            't' => (true, true),
            'F' => (false, false),
            'T' => (false, true),
            _ => return None,
        };
        let arg: Vec<char> = seq.chars().skip(1).collect();
        Some(match arg.len() {
            0 => Outcome::Incomplete,
            1 => {
                let c = arg[0];
                let mut hit = false;
                for _ in 0..count {
                    hit = self.find_char(c, forward, till);
                }
                self.last_find = Some((c, forward, till));
                let _ = hit;
                Outcome::Done
            }
            _ => Outcome::Unknown,
        })
    }

    /// One `d`/`c`/`y`: what is it aimed at?
    fn target(&mut self, op: char, body: &str, count: usize) -> Target {
        // vim's `cw` is `ce`: it stops at the end of the word instead of eating
        // the space after it the way `dw` legitimately does. A well-known quirk,
        // and the one people notice within a minute of using `cw`.
        if op == 'c' && (body == "w" || body == "W") {
            let start = self.cursor;
            let big = body == "W";
            for _ in 0..count {
                self.word_next_end(big);
            }
            let end = self.cursor;
            self.cursor = start;
            return Target::Span(
                Span {
                    start,
                    end: (end + 1).min(self.chars.len()),
                },
                false,
            );
        }
        // `dj` / `dk` (and their display-line spellings): this line and the ones
        // under/over it, linewise.
        if body == "j" || body == "gj" {
            return Target::Span(self.line_span(count + 1), true);
        }
        if body == "k" || body == "gk" {
            let mut span = self.line_span(count + 1);
            for _ in 0..count {
                self.line_up();
            }
            let (ls, le) = self.line_bounds(self.cursor);
            span = Span {
                start: ls,
                end: span.end.max(le),
            };
            return Target::Span(span, true);
        }
        // Text objects: `i`/`a` plus one more key.
        let inner = match body.chars().next() {
            Some('i') => Some(true),
            Some('a') => Some(false),
            _ => None,
        };
        if let Some(inner) = inner {
            let rest: Vec<char> = body.chars().skip(1).collect();
            if rest.is_empty() {
                return Target::Incomplete;
            }
            if rest.len() > 1 {
                return Target::Invalid;
            }
            let c = rest[0];
            let object = match c {
                'w' => Some(Object::Word),
                'W' => Some(Object::BigWord),
                '"' => Some(Object::Quoted('"')),
                '\'' => Some(Object::Quoted('\'')),
                '`' => Some(Object::Quoted('`')),
                '(' | ')' | 'b' => Some(Object::Bracketed('(', ')')),
                '[' | ']' => Some(Object::Bracketed('[', ']')),
                '{' | '}' | 'B' => Some(Object::Bracketed('{', '}')),
                '<' | '>' => Some(Object::Bracketed('<', '>')),
                _ => None,
            };
            return match object {
                Some(o) => match self.object_span(o, inner) {
                    Some(span) => Target::Span(span, false),
                    // No object there: a no-op, not an error. `diw` on an empty
                    // block should do nothing, and say nothing.
                    None => Target::Span(Span { start: 0, end: 0 }, false),
                },
                None => Target::Invalid,
            };
        }
        // A find form: `df,` needs its character.
        if matches!(body.chars().next(), Some('f' | 'F' | 't' | 'T')) {
            return match body.chars().count() {
                1 => Target::Incomplete,
                2 => {
                    let start = self.cursor;
                    self.find_form(body, count);
                    let target = self.cursor;
                    self.cursor = start;
                    Target::Span(
                        Span {
                            start: start.min(target),
                            end: (start.max(target) + 1).min(self.chars.len()),
                        },
                        false,
                    )
                }
                _ => Target::Invalid,
            };
        }
        // A motion.
        let (mcount, name) = take_count(body);
        let start = self.cursor;
        if name == "g" || name.is_empty() {
            return Target::Incomplete;
        }
        if !self.motion(name, count * mcount) {
            return Target::Invalid;
        }
        let target = self.cursor;
        self.cursor = start;
        let inclusive = matches!(name, "e" | "E");
        Target::Span(
            if target >= start {
                Span {
                    start,
                    end: (target + usize::from(inclusive)).min(self.chars.len()),
                }
            } else {
                Span {
                    start: target,
                    end: start,
                }
            },
            false,
        )
    }

    fn apply_operator(&mut self, op: char, span: Span, linewise: bool) {
        match op {
            'd' => self.delete_span(span, linewise),
            'y' => self.yank_span(span, linewise),
            'c' => {
                self.delete_span(span, linewise);
            }
            _ => {}
        }
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

    // ------------------------------------------------- the text grammar

    fn run(text: &str, cursor: usize, seq: &str) -> Editor {
        let mut e = ed(text, cursor);
        e.command(seq);
        e
    }

    /// `dd`, `yy`, `cc` act on the *text* line. The block's own line verbs live
    /// in the outline, and the caret's position is what decides which you get.
    #[test]
    fn line_ops_act_on_the_text_line() {
        // The middle line of a three-line block, caret in the text.
        let e = run("one\ntwo\nthree", 4, "dd");
        assert_eq!(e.text(), "one\nthree");
        assert_eq!(e.cursor, 4, "the caret is on the line that moved up");

        // dd on the last line takes its newline with it.
        let e = run("one\ntwo", 4, "dd");
        assert_eq!(e.text(), "one");

        let e = run("one\ntwo", 0, "yy");
        assert_eq!(e.yank.as_ref().map(|y| y.text.clone()), Some("one\n".into()));
        assert!(e.yank.as_ref().unwrap().linewise);

        let e = run("one\ntwo\nthree", 0, "2dd");
        assert_eq!(e.text(), "three");

        // cc keeps the line's indentation: this is what makes it usable on code.
        let e = run("  indented code", 4, "cc");
        assert_eq!(e.text(), "  ");
        assert_eq!(e.cursor, 2);
    }

    #[test]
    fn text_objects_find_the_thing_under_the_caret() {
        let e = run("the quick brown", 5, "diw");
        assert_eq!(e.text(), "the  brown");

        let e = run("the quick brown", 5, "daw");
        assert_eq!(e.text(), "the brown", "aw takes the trailing space too");

        let e = run("say \"hi\" now", 6, "di\"");
        assert_eq!(e.text(), "say \"\" now");

        let e = run("say \"hi\" now", 6, "da\"");
        assert_eq!(e.text(), "say  now");

        let e = run("f(a, b)", 3, "di(");
        assert_eq!(e.text(), "f()");

        // Nested: the innermost pair containing the caret.
        let e = run("a(b(c)d)e", 4, "di(");
        assert_eq!(e.text(), "a(b()d)e");

        // `ciw` is the one people use most, and it used to insert a stray `w`.
        let mut e = ed("hello world", 2);
        assert_eq!(e.command("ciw"), Outcome::Insert);
        assert_eq!(e.text(), " world");
    }

    #[test]
    fn find_motions_stay_on_the_line() {
        let e = run("abc def abc", 0, "fa");
        assert_eq!(e.cursor, 8);
        let e = run("abc def abc", 8, "fa");
        assert_eq!(e.cursor, 8, "nothing further on this line: no move, no error");

        // `t` stops *before* the character.
        let e = run("a(b)", 0, "t(");
        assert_eq!(e.cursor, 0);
        let e = run("ab(c)", 0, "t(");
        assert_eq!(e.cursor, 1);

        // A find never crosses a newline, which is what keeps it a line motion.
        let e = run("one\ntwo", 0, "fo");
        assert_eq!(e.cursor, 0);

        // `;` repeats, `,` reverses.
        let e = run("axbxc", 0, "fx");
        assert_eq!(e.cursor, 1);
        let mut e = run("axbxc", 0, "fx");
        e.command(";");
        assert_eq!(e.cursor, 3);
        e.command(",");
        assert_eq!(e.cursor, 1);
    }

    #[test]
    fn counts_repeat() {
        let e = run("one two three", 0, "2w");
        assert_eq!(e.cursor, 8);
        let e = run("abcdef", 0, "3x");
        assert_eq!(e.text(), "def");
        let e = run("hello", 0, "3~");
        assert_eq!(e.text(), "HELlo", "three characters flipped, as in vim");
        // A lone 0 is the motion, not a count.
        let e = run("hello", 3, "0");
        assert_eq!(e.cursor, 0);
    }

    #[test]
    fn paste_is_charwise_or_linewise() {
        let mut e = ed("hello", 5);
        e.command("yiw"); // nothing at the end of the text: no yank, no panic
        let mut e = ed("hello", 0);
        e.command("yiw");
        assert_eq!(e.command("p"), Outcome::Done);
        assert_eq!(e.text(), "hhelloello", "charwise paste goes after the caret");
        assert_eq!(e.cursor, 5, "and the caret lands on the last pasted character");

        let mut e = ed("one\ntwo", 0);
        e.command("yy");
        e.command("p");
        assert_eq!(e.text(), "one\none\ntwo", "linewise paste goes on its own line");
    }

    /// vim's `cw` is `ce`: it stops at the end of the word instead of eating the
    /// space, which `dw` does on purpose. The old code shared one implementation
    /// for both, so `cw` ate the space and `ciw` left a stray character.
    #[test]
    fn cw_stops_at_the_end_of_the_word() {
        let e = run("foo bar", 0, "cw");
        assert_eq!(e.text(), " bar");
        let e = run("foo bar", 0, "dw");
        assert_eq!(e.text(), "bar", "dw eats the space, as vim does");
        let e = run("foo bar", 0, "de");
        assert_eq!(e.text(), " bar", "de is the same range as cw");
    }

    #[test]
    fn shifting_the_line_keeps_the_caret_near_where_it_was() {
        let e = run("code", 2, ">>");
        assert_eq!(e.text(), "  code");
        assert_eq!(e.cursor, 4, "the caret moved with the text");
        let e = run("  code", 4, "<<");
        assert_eq!(e.text(), "code");
        assert_eq!(e.cursor, 2);
    }

    #[test]
    fn case_toggle_and_replace() {
        let e = run("abc", 0, "rZ");
        assert_eq!(e.text(), "Zbc");
        assert_eq!(e.cursor, 0, "r leaves the caret on the character it replaced");
        let e = run("abc", 1, "~");
        assert_eq!(e.text(), "aBc");
        assert_eq!(e.cursor, 2, "~ moves on, as vim does");
        let e = run("abc", 0, "s");
        assert_eq!(e.text(), "bc");
        let e = run("abc", 1, "X");
        assert_eq!(e.text(), "bc");
        let e = run("abc", 0, "X");
        assert_eq!(e.text(), "abc", "X at the start of a line does nothing");
    }

    #[test]
    fn join_lines_stays_inside_the_block() {
        let e = run("one\n   two\nthree", 0, "J");
        assert_eq!(e.text(), "one two\nthree");
        let e = run("one", 0, "J");
        assert_eq!(e.text(), "one", "nothing to join on the last line");
    }

    /// The router's contract, from the engine's side: a partial command waits,
    /// an unknown one is *unknown* (so the caller can refuse it) rather than
    /// silently doing nothing or something else.
    #[test]
    fn partial_commands_wait_and_unknown_ones_are_unknown() {
        assert_eq!(ed("abc", 0).command("d"), Outcome::Incomplete);
        assert_eq!(ed("abc", 0).command("di"), Outcome::Incomplete);
        assert_eq!(ed("abc", 0).command("df"), Outcome::Incomplete);
        assert_eq!(ed("abc", 0).command("r"), Outcome::Incomplete);
        assert_eq!(ed("abc", 0).command("2"), Outcome::Incomplete);
        assert_eq!(ed("abc", 0).command("g"), Outcome::Incomplete);
        for unknown in ["Q", "v", "u", "gg", "Z", "dq", "riw"] {
            assert_eq!(
                ed("abc", 0).command(unknown),
                Outcome::Unknown,
                "{unknown} is not a text command"
            );
        }
        // ...and the ones that are, say so.
        assert_eq!(ed("abc", 0).command("<Esc>"), Outcome::DropCaret);
        assert_eq!(ed("abc", 0).command("q"), Outcome::Quit);
        assert_eq!(ed("abc", 0).command("i"), Outcome::Insert);
        assert_eq!(ed("abc", 0).command("a"), Outcome::Insert);
    }
}
