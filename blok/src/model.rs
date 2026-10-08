//! Core domain types for blok.
//!
//! Everything the UI shows is derived from these; there is no markdown document
//! anywhere in the pipeline. Markdown-ish *syntax* is accepted in block text and
//! parsed into typed references, but it is never the storage format.

use chrono::{Datelike, NaiveDate};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageKind {
    Journal,
    Page,
}

impl PageKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PageKind::Journal => "journal",
            PageKind::Page => "page",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Page {
    pub id: i64,
    pub name: String,
    pub kind: PageKind,
    pub created_at: String,
}

/// A journal day. Journals are *virtual* until they hold content: the row in
/// `pages` is only created when the first block is committed, and it is removed
/// again by `prune_empty_journals`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JournalDay {
    pub date: NaiveDate,
}

impl JournalDay {
    pub fn new(date: NaiveDate) -> Self {
        Self { date }
    }

    /// Stable key used as `pages.name`.
    pub fn key(&self) -> String {
        self.date.format("%Y-%m-%d").to_string()
    }

    /// Human title, e.g. `Friday, February 14, 2026`.
    pub fn title(&self) -> String {
        let weekday = self.date.format("%A").to_string();
        let month = self.date.format("%B").to_string();
        format!(
            "{}, {} {}, {}",
            weekday,
            month,
            self.date.day(),
            self.date.year()
        )
    }

    /// Compact day label, e.g. `Fri 14`.
    pub fn short(&self) -> String {
        self.date.format("%a %d").to_string()
    }

    pub fn relative(&self, today: NaiveDate) -> String {
        let d = (today - self.date).num_days();
        match d {
            0 => "Today".to_string(),
            1 => "Yesterday".to_string(),
            2..=6 => format!("{}d ago", d),
            _ => self.date.format("%b %d, %Y").to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    pub id: i64,
    pub uuid: String,
    pub page_id: i64,
    pub parent_id: Option<i64>,
    pub position: f64,
    pub content: String,
    pub status: Option<String>,
    pub collapsed: bool,
    pub created_at: String,
}

impl Block {
    pub fn is_empty(&self) -> bool {
        self.content.trim().is_empty()
    }
}

/// One rendered line of the outliner: a block flattened with its depth, ready
/// for the viewport. Folding and indentation are resolved before the UI sees it.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: i64,
    pub uuid: String,
    pub depth: usize,
    pub content: String,
    pub status: Option<String>,
    pub collapsed: bool,
    pub has_children: bool,
    pub child_count: usize,
    /// True for the last sibling at this depth, used to pick the guide glyph.
    pub last_sibling: bool,
    /// For each ancestor level: was that ancestor the last of its siblings?
    pub ancestor_last: Vec<bool>,
}

#[derive(Clone, Debug)]
pub struct RefHit {
    pub block_id: i64,
    /// Page the referencing block lives on (journal key or page name).
    pub page: String,
    pub is_journal: bool,
    pub content: String,
    /// block | page | tag
    pub kind: String,
}

#[derive(Clone, Debug, Default)]
pub struct DbStats {
    pub pages: i64,
    pub journals: i64,
    pub blocks: i64,
    pub refs: i64,
    pub file_bytes: u64,
    pub wal_bytes: u64,
    pub fts: bool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub file: String,
    pub bytes: u64,
    pub taken_at: String,
    pub remote: String,
    pub state: String,
}

/// A fenced code block: the one piece of markup blok keeps. Not because markup
/// is good, but because "do not interpret this" needs *some* spelling, and the
/// alternative is that a `#include`, a `[[placeholder]]` in a string, or a
/// `key:: value` in a YAML sample becomes a page, a link or a property. A fence
/// is the spelling everyone already knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodeBlock {
    /// The token after the opening fence, empty when there is none. A label,
    /// never a promise: blok does not highlight, it just stops interpreting.
    pub lang: String,
    /// Everything between the fences, verbatim -- leading spaces included.
    pub body: String,
}

/// `Some` when a block's content opens with a fence.
///
/// The opening fence is the first line, and the language token must be a single
/// word: ``` ```not a fence ``` ``` is a paragraph that begins with backticks.
/// A closing fence is optional, so a block stays code while you type in it.
pub fn code_block(content: &str) -> Option<CodeBlock> {
    let mut lines = content.split('\n');
    let first = lines.next()?;
    let rest = first.strip_prefix("```")?;
    let lang = rest.trim();
    if lang.chars().any(|c| c.is_whitespace() || c == '`') {
        return None;
    }
    let mut body: Vec<&str> = lines.collect();
    // The closing fence is a delimiter, not the last line of the program.
    if body.last().map(|l| l.trim_end() == "```").unwrap_or(false) {
        body.pop();
    }
    Some(CodeBlock {
        lang: lang.to_string(),
        body: body.join("\n"),
    })
}

/// Is this block code? The question every reader of block text has to ask
/// before it interprets anything.
pub fn is_code(content: &str) -> bool {
    code_block(content).is_some()
}

/// Parse level-1 reference syntax out of block text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedRef {
    Page(String),
    Tag(String),
    BlockRef(String),
}

pub fn parse_refs(text: &str) -> Vec<ParsedRef> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '[' && i + 1 < chars.len() && chars[i + 1] == '[' {
            if let Some(end) = find(&chars, i + 2, &[']', ']']) {
                let name: String = chars[i + 2..end].iter().collect();
                let name = name.trim().to_string();
                if !name.is_empty() {
                    out.push(ParsedRef::Page(name));
                }
                i = end + 2;
                continue;
            }
        }
        if chars[i] == '(' && i + 1 < chars.len() && chars[i + 1] == '(' {
            if let Some(end) = find(&chars, i + 2, &[')', ')']) {
                let uuid: String = chars[i + 2..end].iter().collect();
                let uuid = uuid.trim().to_string();
                if !uuid.is_empty() {
                    out.push(ParsedRef::BlockRef(uuid));
                }
                i = end + 2;
                continue;
            }
        }
        if chars[i] == '#' {
            let start = i + 1;
            let mut j = start;
            while j < chars.len()
                && (chars[j].is_alphanumeric() || chars[j] == '-' || chars[j] == '_' || chars[j] == '/')
            {
                j += 1;
            }
            if j > start {
                let name: String = chars[start..j].iter().collect();
                out.push(ParsedRef::Tag(name));
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn find(chars: &[char], from: usize, pat: &[char; 2]) -> Option<usize> {
    let mut i = from;
    while i + 1 < chars.len() {
        if chars[i] == pat[0] && chars[i + 1] == pat[1] {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// The task markers Logseq understands. They are stored twice on purpose: the
/// leading word stays in the block text (so it round-trips like Logseq) while
/// the same word is mirrored into `blocks.status`, which is what queries and
/// the TODO board read.
pub const STATUS_WORDS: [&str; 7] = [
    "TODO", "DOING", "DONE", "LATER", "NOW", "WAITING", "CANCELED",
];

/// Split a leading task marker off a block's text.
pub fn split_status(text: &str) -> (Option<&'static str>, String) {
    for w in STATUS_WORDS {
        if text == w {
            return (Some(w), String::new());
        }
        if let Some(rest) = text.strip_prefix(w) {
            if rest.starts_with(' ') || rest.starts_with('\n') {
                return (Some(w), rest.trim_start().to_string());
            }
        }
    }
    (None, text.to_string())
}

/// Extract the `key:: value` properties from a block, Logseq-style.
pub fn properties(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(idx) = line.find("::") {
            let key = line[..idx].trim();
            if !key.is_empty()
                && key.len() < 24
                && key
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            {
                out.push((key.to_string(), line[idx + 2..].trim().to_string()));
            }
        }
    }
    out
}

/// `2026-02-14` -> JournalDay
pub fn parse_journal_key(name: &str) -> Option<JournalDay> {
    NaiveDate::parse_from_str(name, "%Y-%m-%d")
        .ok()
        .map(JournalDay::new)
}

pub fn uuid_for(id: i64) -> String {
    // Deterministic, readable stand-in for a v4 uuid: stable per install because
    // it is derived from the rowid plus a per-database salt.
    format!("blk-{:08x}", id as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fence is what stops the app interpreting code as prose, so the
    /// interesting cases are the ones where it must *refuse*: a paragraph that
    /// merely starts with backticks is not a code block, and getting that wrong
    /// would silently swallow a block's links.
    #[test]
    fn only_a_real_fence_is_code() {
        let c = code_block("```rust\nfn main() {}\n```").expect("a fenced block");
        assert_eq!(c.lang, "rust");
        assert_eq!(c.body, "fn main() {}");

        // Unterminated: still code, because you are probably typing in it.
        let c = code_block("```rust\nfn main() {}").expect("an open fence is still a fence");
        assert_eq!(c.body, "fn main() {}");

        // No language, and no body at all: the state a fresh `/Code` starts in.
        let c = code_block("```").expect("a bare fence");
        assert_eq!(c.lang, "");
        assert_eq!(c.body, "");

        // Not fences:
        assert!(code_block("``` one two ```").is_none(), "two words is prose");
        assert!(code_block("````").is_none(), "four backticks is not a fence");
        assert!(code_block("see ```rust``` inline").is_none(), "not at the start");
        assert!(code_block("text\n```rust").is_none(), "not on the first line");
        assert!(code_block("").is_none());
    }

    /// The body keeps its indentation: reformatting code is not a rendering
    /// decision to make on someone's behalf.
    #[test]
    fn the_body_is_verbatim() {
        let c = code_block("```python\nif x:\n    y = 1\n\nz = 2\n```").unwrap();
        assert_eq!(c.body, "if x:\n    y = 1\n\nz = 2");
    }

    /// The reason the fence exists: none of this is markup any more.
    #[test]
    fn code_is_not_prose() {
        let src = "```c\n#include <stdio.h>\nchar *s = \"[[not a link]]\";\nid:: not a property\n#notatag\n```";
        assert!(is_code(src));
        // The parsers are still happy to interpret it -- which is exactly why
        // every reader has to ask `is_code` first.
        assert!(!parse_refs(&code_block(src).unwrap().body).is_empty());
    }
}
