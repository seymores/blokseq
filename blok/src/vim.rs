//! The vim layer.
//!
//! An outliner complicates vim's model in exactly one way: the "line" is a
//! **block**, and a block can itself hold several lines (Alt-⏎). So there are
//! two Normal modes rather than one:
//!
//! * [`Mode::Normal`] — Normal mode at the tree level. `j`/`k`/`gg`/`G` move
//!   between blocks; operators (`dd`, `yy`, `p`, `>>`, `J`, `cc`) act on
//!   subtrees. A block *is* the line.
//! * [`Mode::Text`] — Normal mode with the cursor inside one block's text, which
//!   is where `Esc` from Insert leaves you, exactly as in vim. Word and line
//!   motions (`w b e 0 ^ $ x dw cw D C`) apply here, and `j`/`k` walk the
//!   block's lines, hopping to the neighbouring block at the boundary.
//!
//! Everything else follows vim: `i a I A o O` entry points, `v`/`V` visual,
//! `u`/`Ctrl-r`, `za zc zo zR zM` folds, `Ctrl-]` to follow a link and
//! `Ctrl-o`/`Ctrl-i` to walk the jumplist, `:` for ex, `Ctrl-w h/l/w` to change
//! window. The deviations are deliberate and listed in DESIGN.md §7:
//! `hjkl` navigate the tree rather than a document, `Enter` puts the cursor in
//! the block's text (Normal mode there, not Insert), `J` joins a block with the
//! one below, and `?` opens the keymap instead of searching backwards.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{
    App, ExState, Focus, InsertAt, LinkTarget, SqlConsole, ToastKind, View, EX_COMMANDS,
};
use crate::editor::Mode;
use crate::model::JournalDay;

/// Keys that act on the *outline* even while the caret is in a block's text.
///
/// Everything else belongs to the text grammar, and a key the text grammar does
/// not know is **refused** rather than handed to the outline. That is the whole
/// point of the list: seventeen vim text keys used to fall through from inside a
/// block, so `dd` deleted the block you were typing in, `p` pasted a block, `u`
/// undid something unrelated, and `ciw` -- `c` waits, `i` became the outline's
/// insert, `w` then typed itself -- left a stray `w` in the sentence.
const OUTLINE_FROM_TEXT: &[&str] = &[
    // Motions that leave the block: the caret's position decides the domain,
    // and `j`/`k` being a one-key escape is a documented deviation.
    "j", "k", "h", "l", "gg", "G", "C-d", "C-u",
    // New blocks: the outline's line.
    "o", "O",
    // Folds, and the block-range selection.
    "za", "zc", "zo", "zR", "zM", "v", "V",
    // Links, history, panels, views, search, commands, help, undo.
    "C-]", "gf", "C-o", "C-i", "[", "]", "C-p", "/", "?", ":", "u", "C-r",
    "C-wl", "C-ww", "C-m", "gm", "n", "N", "ZZ", "ZQ", "C-s",
];

impl App {
    // ------------------------------------------------------------- helpers

    pub fn unknown_key(&mut self, seq: &str) {
        if seq == "<BS>" || seq == "<Esc>" {
            return;
        }
        self.toast(
            ToastKind::Warn,
            &format!("no mapping for \"{}\"", seq),
            Some("? keymap · : commands"),
        );
    }

    /// Esc in Normal mode: unwind one layer at a time.
    pub fn escape_tree(&mut self) {
        self.pending.clear();
        if self.visual.is_some() {
            self.visual = None;
            return;
        }
        if self.toast.is_some() {
            self.toast = None;
            return;
        }
        match self.view {
            View::Journal(_) | View::Page(_) | View::Sql => {}
            _ => self.go_back(),
        }
    }

    /// `h` -- the parent block.
    pub fn go_parent(&mut self) {
        if let Some(row) = self.selected_row().cloned() {
            if row.depth == 0 {
                self.toast(ToastKind::Info, "top level — no parent", None);
                return;
            }
            if let Some(p) = self.db.block(row.id).and_then(|b| b.parent_id) {
                self.select_block(p);
                self.scroll_to_selection();
            }
        }
    }

    /// `l` -- the first child.
    pub fn go_first_child(&mut self) {
        if let Some(row) = self.selected_row().cloned() {
            if !row.has_children {
                return;
            }
            if row.collapsed {
                self.db.set_collapsed(row.id, false);
                self.reload();
                self.select_block(row.id);
            }
            self.move_selection(1);
        }
    }

    // ------------------------------------------------------- insert entry

    pub fn enter_insert(&mut self, at: InsertAt) {
        self.begin_edit();
        if let Some(ed) = self.editor.as_mut() {
            match at {
                InsertAt::Start => ed.cursor = 0,
                InsertAt::After | InsertAt::End => ed.cursor = ed.chars.len(),
            }
        }
        self.mode = Mode::Insert;
    }

    /// `Enter` in Normal mode: put the caret in this block's text, still in
    /// Normal mode. Text motions work from here (`w`, `x`, `dw`); `j`/`k` walk
    /// back out to the block list, so this is a cursor position and not a state
    /// you can get stuck in.
    pub fn enter_text(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        self.begin_edit_block(row.id);
        if let Some(ed) = self.editor.as_mut() {
            ed.first_non_blank();
        }
        self.text_focus = true;
        self.mode = Mode::Normal;
    }

    /// Is this key sequence a *bare* word motion (`w`, `3w`, `e`, …)? `dw` and
    /// the rest belong to the operator path inside the grammar, not here.
    fn is_word_motion(seq: &str) -> bool {
        let rest = seq.trim_start_matches(|c: char| c.is_ascii_digit());
        matches!(rest, "w" | "W" | "b" | "B" | "e" | "E") && !rest.is_empty()
    }

    /// A word motion with the caret in a block.
    ///
    /// Vim's `w` at the end of a line moves to the next line, and in an outliner
    /// the next line is the next *block*: so `w`/`e` that cannot move carry on
    /// into the block below, and `b`/`B` into the one above, landing on the word
    /// they would have landed on. Word motions therefore walk the outline one
    /// word at a time, which is what "move the caret forward to the next word"
    /// means when the file is a tree.
    fn word_motion(&mut self, seq: &str) {
        let before = self.editor.as_ref().map(|e| e.cursor);
        let outcome = self.editor.as_mut().map(|ed| ed.command(seq));
        let after = self.editor.as_ref().map(|e| e.cursor);
        if before != after || outcome.is_none() {
            return;
        }
        let forward = seq.ends_with(['w', 'W', 'e', 'E']);
        let at = self.selected;
        self.leave_text();
        self.move_selection(if forward { 1 } else { -1 });
        if self.selected == at {
            // Nowhere to go: vim stops at the end of the buffer, and it stops
            // *in the buffer*. Leaving the text here would drop the caret out
            // from under you for pressing a key that did nothing.
            self.enter_text();
            if let Some(ed) = self.editor.as_mut() {
                ed.cursor = ed.chars.len().min(before.unwrap_or(ed.cursor));
            }
            return;
        }
        self.enter_text();
        if !forward {
            // Backwards lands on the *last* word of the block above.
            if let Some(ed) = self.editor.as_mut() {
                ed.end();
                ed.word_prev(false);
            }
        }
    }

    /// Leave the block's text but keep the row selected (used by every tree
    /// motion, so a single `j` is always enough to get back to the blocks).
    fn leave_text(&mut self) {
        if self.text_focus {
            let id = self.editor.as_ref().map(|e| e.block_id);
            self.commit_edit();
            if let Some(id) = id {
                self.select_block(id);
            }
            self.text_focus = false;
        }
    }

    /// `o` / `O`.
    pub fn open_sibling(&mut self, above: bool) {
        if !above {
            self.new_block_below();
            return;
        }
        let Some(page_id) = self.ensure_current_page() else {
            return;
        };
        let (parent, after) = match self.selected_row().cloned() {
            Some(r) => {
                let parent = self.db.block(r.id).and_then(|b| b.parent_id);
                (parent, self.prev_sibling_id(r.id))
            }
            None => (None, None),
        };
        let nb = self.db.create_block(page_id, parent, after, "");
        self.reload();
        self.select_block(nb.id);
        self.begin_edit_block(nb.id);
        self.scroll_to_selection();
    }

    /// `cc`.
    pub fn change_block(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        self.db.update_content(row.id, "");
        self.reload();
        self.select_block(row.id);
        self.begin_edit_block(row.id);
    }

    // -------------------------------------------------------------- folds

    pub fn set_collapsed(&mut self, collapsed: bool) {
        if let Some(row) = self.selected_row().cloned() {
            if row.has_children {
                self.db.set_collapsed(row.id, collapsed);
                self.reload();
                self.select_block(row.id);
            }
        }
    }

    pub fn expand_all(&mut self) {
        if let Some(pid) = self.page_id {
            let _ = self
                .db
                .conn
                .execute("UPDATE blocks SET collapsed = 0 WHERE page_id = ?1", [pid]);
            self.reload();
            self.toast(ToastKind::Info, "unfolded every block", Some("zR"));
        }
    }

    // ------------------------------------------------------------- visual

    pub fn enter_visual(&mut self, subtree: bool) {
        self.visual = Some(self.selected);
        self.mode = Mode::Visual;
        let mut n = 1usize;
        if subtree {
            // A subtree is contiguous in the flattened rows, so extending the
            // selection to the last deeper row selects exactly the subtree.
            let base = self.rows.get(self.selected).map(|r| r.depth).unwrap_or(0);
            let mut i = self.selected + 1;
            while i < self.rows.len() && self.rows[i].depth > base {
                i += 1;
            }
            if i > self.selected + 1 {
                let last = i - 1;
                n = last - self.selected + 1;
                self.selected = last;
            }
        }
        self.toast(
            ToastKind::Info,
            &format!(
                "{} block{} selected{}",
                n.max(1),
                if n.max(1) == 1 { "" } else { "s" },
                if subtree { " (subtree)" } else { "" }
            ),
            Some("j/k extend · > < indent · d delete · y yank · Esc leave"),
        );
    }

    // ---------------------------------------------------------- registers

    pub fn yank_selected(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        self.register = self.db.subtree(row.id);
        self.register_label = crate::app::snippet(&row.content, 28);
        let n = self.register.len();
        self.toast(
            ToastKind::Info,
            &format!(
                "yanked {} block{}: \"{}\"",
                n,
                if n == 1 { "" } else { "s" },
                self.register_label
            ),
            Some("p paste after · P paste before"),
        );
    }

    pub fn paste(&mut self, before: bool) {
        if self.register.is_empty() {
            self.toast(
                ToastKind::Warn,
                "register is empty — yy yanks a block first",
                None,
            );
            return;
        }
        let Some(page_id) = self.ensure_current_page() else {
            return;
        };
        let items = self.register.clone();
        let (parent, after) = match self.selected_row().cloned() {
            Some(r) => {
                let parent = self.db.block(r.id).and_then(|b| b.parent_id);
                if before {
                    (parent, self.prev_sibling_id(r.id))
                } else {
                    (parent, Some(r.id))
                }
            }
            None => (None, None),
        };
        if let Some(first) = self.db.paste_subtree(page_id, parent, after, &items) {
            self.redo.clear();
            self.reload();
            self.select_block(first);
            self.scroll_to_selection();
            self.toast(
                ToastKind::Good,
                &format!(
                    "pasted {} block{}",
                    items.len(),
                    if items.len() == 1 { "" } else { "s" }
                ),
                Some("u undoes the paste"),
            );
        }
    }

    fn prev_sibling_id(&self, id: i64) -> Option<i64> {
        let b = self.db.block(id)?;
        let mut stmt = self
            .db
            .conn
            .prepare(
                "SELECT id FROM blocks WHERE page_id = ?1 AND deleted_at IS NULL
                   AND parent_id IS ?2 AND position < ?3
                 ORDER BY position DESC LIMIT 1",
            )
            .ok()?;
        stmt.query_row(rusqlite::params![b.page_id, b.parent_id, b.position], |r| {
            r.get::<_, i64>(0)
        })
        .ok()
    }

    /// `J` -- vim's join, with blocks as lines.
    pub fn merge_with_next(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let ids: Vec<i64> = self.rows.iter().map(|r| r.id).collect();
        let Some(pos) = ids.iter().position(|i| *i == row.id) else {
            return;
        };
        let Some(next) = ids.get(pos + 1).copied() else {
            self.toast(ToastKind::Warn, "nothing below to join with", None);
            return;
        };
        match (self.db.block(row.id), self.db.block(next)) {
            (Some(a), Some(b)) if a.parent_id == b.parent_id => {
                let merged = if a.content.is_empty() {
                    b.content.clone()
                } else if b.content.is_empty() {
                    a.content.clone()
                } else {
                    format!("{} {}", a.content.trim_end(), b.content.trim_start())
                };
                self.db.update_content(a.id, &merged);
                let _ = self.db.conn.execute(
                    "UPDATE blocks SET parent_id = ?1 WHERE parent_id = ?2",
                    rusqlite::params![a.id, b.id],
                );
                self.db.delete_block(b.id);
                self.reload();
                self.select_block(a.id);
                self.toast(ToastKind::Info, "joined with the block below", Some("J"));
            }
            _ => self.toast(ToastKind::Warn, "can only join siblings", None),
        }
    }

    // -------------------------------------------------------------- panels

    /// `Ctrl-M` -- show or hide the page metadata panel (metadata, linked and
    /// unlinked references). Hidden by default, so this is the way in.
    pub fn toggle_meta(&mut self) {
        self.show_meta = !self.show_meta;
        self.db
            .set_setting("show_meta", if self.show_meta { "true" } else { "false" });
        if !self.show_meta && self.focus == Focus::Right {
            self.focus = Focus::Main;
        }
        self.toast(
            ToastKind::Info,
            if self.show_meta {
                "page metadata shown"
            } else {
                "page metadata hidden"
            },
            Some("Ctrl-M toggles · :set meta / nometa"),
        );
    }

    pub fn focus_right(&mut self) {
        self.focus = if self.focus == Focus::Main && self.show_meta {
            Focus::Right
        } else {
            Focus::Main
        };
    }

    pub fn focus_cycle(&mut self) {
        self.focus = match self.focus {
            Focus::Main if self.show_meta => Focus::Right,
            _ => Focus::Main,
        };
    }

    // --------------------------------------------------------------- links

    /// Every `[[page]]`, `((block))` and `#tag` in a block, resolved to a
    /// destination.
    pub fn links_in(&self, block_id: i64) -> Vec<LinkTarget> {
        let Some(b) = self.db.block(block_id) else {
            return Vec::new();
        };
        // Code is not prose: a `[[x]]` in a code block is not a destination.
        if crate::model::is_code(&b.content) {
            return Vec::new();
        }
        let chars: Vec<char> = b.content.chars().collect();
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < chars.len() {
            if chars[i] == '[' && chars.get(i + 1) == Some(&'[') {
                if let Some(end) = find_pair(&chars, i + 2, ']') {
                    let name: String = chars[i + 2..end].iter().collect();
                    let exists = self.db.page_by_name(&name).is_some();
                    out.push(LinkTarget {
                        kind: if exists { "page" } else { "new page" },
                        label: format!("[[{}]]", name),
                        page: Some(name),
                        block: None,
                        at: i,
                    });
                    i = end + 2;
                    continue;
                }
            }
            if chars[i] == '(' && chars.get(i + 1) == Some(&'(') {
                if let Some(end) = find_pair(&chars, i + 2, ')') {
                    let uuid: String = chars[i + 2..end].iter().collect();
                    match self.db.block_by_uuid(&uuid) {
                        Some(t) => out.push(LinkTarget {
                            kind: "block",
                            label: format!("(({}))", crate::app::snippet(&t.content, 40)),
                            page: self.db.page_of_block(t.id).map(|p| p.name),
                            block: Some(t.id),
                            at: i,
                        }),
                        None => out.push(LinkTarget {
                            kind: "missing block",
                            label: format!("(({}))", uuid),
                            page: None,
                            block: None,
                            at: i,
                        }),
                    }
                    i = end + 2;
                    continue;
                }
            }
            if chars[i] == '#' && (i == 0 || chars[i - 1].is_whitespace()) {
                let mut j = i + 1;
                while j < chars.len()
                    && (chars[j].is_alphanumeric()
                        || chars[j] == '-'
                        || chars[j] == '_'
                        || chars[j] == '/')
                {
                    j += 1;
                }
                if j > i + 1 {
                    let name: String = chars[i + 1..j].iter().collect();
                    out.push(LinkTarget {
                        kind: "tag",
                        label: format!("#{}", name),
                        page: Some(name),
                        block: None,
                        at: i,
                    });
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
        out
    }

    /// Links reachable from the cursor: the block itself, or -- in the
    /// references pane -- the referencing block.
    pub fn current_links(&self) -> Vec<LinkTarget> {
        if self.focus == Focus::Right {
            if let Some((page, block)) = self.selected_reference() {
                let mut v = vec![LinkTarget {
                    kind: "reference",
                    label: page.clone(),
                    page: Some(page),
                    block: Some(block),
                    at: 0,
                }];
                v.extend(self.links_in(block));
                return v;
            }
        }
        self.selected_row()
            .map(|r| self.links_in(r.id))
            .unwrap_or_default()
    }

    fn selected_reference(&self) -> Option<(String, i64)> {
        let mut idx = 0usize;
        for (page, hits) in &self.linked {
            for h in hits {
                if idx == self.linked_selected {
                    return Some((page.clone(), h.block_id));
                }
                idx += 1;
            }
        }
        None
    }

    /// `Ctrl-]`. One link follows immediately; several open a menu.
    pub fn follow_link(&mut self) {
        let links = self.current_links();
        match links.len() {
            0 => self.toast(
                ToastKind::Warn,
                "no links here",
                Some("[[page]] · ((block)) · #tag"),
            ),
            1 => self.jump_to(&links[0]),
            _ => {
                self.open_link_menu(links.clone());
                self.toast(
                    ToastKind::Info,
                    &format!("{} links — pick one", self.link_menu.len()),
                    Some("↑↓ choose · Enter follow · Esc cancel"),
                );
            }
        }
    }

    pub fn jump_to(&mut self, t: &LinkTarget) {
        let Some(page) = t.page.clone() else {
            self.toast(
                ToastKind::Warn,
                &format!("{} does not resolve", t.label),
                Some("the target block may have been deleted"),
            );
            return;
        };
        self.push_history();
        let is_journal = self
            .db
            .page_by_name(&page)
            .map(|p| p.kind == crate::model::PageKind::Journal)
            .unwrap_or(false);
        match crate::model::parse_journal_key(&page) {
            Some(day) if is_journal => self.goto_journal(day),
            _ => self.goto_page(&page),
        }
        if let Some(b) = t.block {
            self.select_block(b);
            self.scroll_to_selection();
        }
        self.toast(
            ToastKind::Info,
            &format!("→ {}", crate::app::snippet(&t.label, 34)),
            Some("Ctrl-o jumps back"),
        );
    }

    /// `Enter` in the references pane.
    pub fn open_selected_reference(&mut self) {
        let Some((page, block)) = self.selected_reference() else {
            self.toast(ToastKind::Warn, "no reference selected", None);
            return;
        };
        self.push_history();
        match crate::model::parse_journal_key(&page) {
            Some(day) => self.goto_journal(day),
            None => self.goto_page(&page),
        }
        self.select_block(block);
        self.scroll_to_selection();
        self.focus = Focus::Main;
    }

    // ------------------------------------------------------------- history

    /// Where you are, as a history entry: the view plus the block that was
    /// selected, so going back lands where you left rather than on the page's
    /// first block.
    fn here(&self) -> (View, Option<i64>) {
        (self.view.clone(), self.selected_row().map(|r| r.id))
    }

    /// Called immediately *before* navigating away. This is the two-stack
    /// browser model -- `history` is what is behind you, `history_forward` what
    /// is ahead -- and it replaced a single stack with a cursor that was wrong
    /// in two ways: a single jump could not be undone at all (`history_pos`
    /// started at the only entry), and with two jumps `Ctrl-o` skipped the page
    /// in between. Both bugs were reachable, neither had a test.
    pub fn push_history(&mut self) {
        let here = self.here();
        if self.history.last() != Some(&here) {
            self.history.push(here);
        }
        // A new page is a new branch: whatever was ahead is now unreachable.
        self.history_forward.clear();
    }

    pub fn history_back(&mut self) {
        let Some(prev) = self.history.pop() else {
            self.toast(
                ToastKind::Info,
                "no page to go back to",
                Some("the history starts at this page"),
            );
            return;
        };
        self.history_forward.push(self.here());
        self.go_history(prev.0, prev.1);
        // Naming the destination is the whole point of back/forward: the pane
        // title changes, but "where did that put me" deserves an answer.
        self.toast(
            ToastKind::Info,
            &format!("← {}", crate::app::view_label(&self.view)),
            None,
        );
    }

    pub fn history_forward(&mut self) {
        let Some(next) = self.history_forward.pop() else {
            self.toast(
                ToastKind::Info,
                "no page to go forward to",
                Some("open a page and it joins the history"),
            );
            return;
        };
        self.history.push(self.here());
        self.go_history(next.0, next.1);
        self.toast(
            ToastKind::Info,
            &format!("→ {}", crate::app::view_label(&self.view)),
            None,
        );
    }

    fn go_history(&mut self, view: View, block: Option<i64>) {
        self.view = view;
        self.selected = 0;
        self.load_view();
        self.load_right();
        if let Some(b) = block {
            self.select_block(b);
            self.scroll_to_selection();
        }
        self.focus = Focus::Main;
    }

    // -------------------------------------------------------------- search

    pub fn open_search(&mut self) {
        self.set_view(View::Search);
        self.search_query.clear();
        self.run_search();
    }

    pub fn search_step(&mut self, delta: i64) {
        if self.search_results.is_empty() {
            self.toast(ToastKind::Warn, "nothing to repeat", Some("/ searches"));
            return;
        }
        let n = self.search_results.len() as i64;
        self.search_selected = ((self.search_selected as i64 + delta).rem_euclid(n)) as usize;
        let hit = self.search_results[self.search_selected].clone();
        self.push_history();
        match crate::model::parse_journal_key(&hit.page) {
            Some(day) => self.goto_journal(day),
            None => self.goto_page(&hit.page),
        }
        self.select_block(hit.block_id);
        self.scroll_to_selection();
    }

    // ------------------------------------------------------------------ ex

    pub fn open_ex(&mut self) {
        self.ex = Some(ExState {
            input: String::new(),
            selected: 0,
            message: None,
        });
    }

    pub fn ex_matches(&self) -> Vec<(&'static str, &'static str)> {
        let Some(ex) = self.ex.as_ref() else {
            return Vec::new();
        };
        let q = ex
            .input
            .split(' ')
            .next()
            .unwrap_or("")
            .trim()
            .to_lowercase();
        EX_COMMANDS
            .iter()
            .copied()
            .filter(|(name, _)| name.trim().to_lowercase().starts_with(&q))
            .collect()
    }

    fn ex_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => self.ex = None,
            KeyCode::Enter => {
                if let Some(ex) = self.ex.take() {
                    let input = ex.input.trim().to_string();
                    if !input.is_empty() {
                        self.run_ex(&input);
                    }
                }
            }
            KeyCode::Tab => {
                let matches = self.ex_matches();
                if let Some((name, _)) = matches.first().copied() {
                    if let Some(ex) = self.ex.as_mut() {
                        let arg = ex
                            .input
                            .split_once(' ')
                            .map(|(_, a)| a.to_string())
                            .unwrap_or_default();
                        ex.input = if arg.is_empty() {
                            name.to_string()
                        } else {
                            format!("{} {}", name.trim_end(), arg)
                        };
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(ex) = self.ex.as_mut() {
                    ex.input.pop();
                }
            }
            KeyCode::Char(c) => {
                if let Some(ex) = self.ex.as_mut() {
                    ex.input.push(c);
                }
            }
            _ => {}
        }
    }

    /// Run one `:` line. Returns false when the command is unknown.
    pub fn run_ex(&mut self, line: &str) -> bool {
        let line = line.trim().trim_start_matches(':').to_string();
        let (cmd, arg) = match line.split_once(' ') {
            Some((c, a)) => (c.to_string(), a.trim().to_string()),
            None => (line.clone(), String::new()),
        };
        match cmd.as_str() {
            "q" => {
                self.quit_now(true, false);
                true
            }
            "q!" => {
                self.quit_now(false, false);
                true
            }
            "w" => {
                self.do_backup();
                true
            }
            "wq" | "x" => {
                self.quit_now(true, true);
                true
            }
            "e" | "edit" => {
                if arg.is_empty() {
                    self.reload();
                    self.toast(ToastKind::Info, "reloaded from SQLite", None);
                } else {
                    // A page opened with `:e` is a page you opened, so `[` has
                    // to be able to come back from it.
                    self.push_history();
                    if let Ok(day) = chrono::NaiveDate::parse_from_str(&arg, "%Y-%m-%d") {
                        self.goto_journal(JournalDay::new(day));
                    } else if let Ok(offset) = arg.parse::<i64>() {
                        let day = JournalDay::new(self.today + chrono::Duration::days(offset));
                        self.goto_journal(day);
                    } else {
                        self.goto_page(&arg);
                    }
                }
                true
            }
            // Days moved off `[`/`]`: navigation keys are for the pages you have
            // opened, and stepping the calendar is a deliberate act.
            "prev" | "previous" => {
                self.shift_journal(-1);
                true
            }
            "next" => {
                self.shift_journal(1);
                true
            }
            "set" => {
                match arg.as_str() {
                    "meta" if !self.show_meta => self.toggle_meta(),
                    "nometa" if self.show_meta => self.toggle_meta(),
                    "meta" | "nometa" => {
                        self.toast(ToastKind::Info, "already set that way", None)
                    }
                    _ => self.toast(ToastKind::Warn, "set: meta · nometa", None),
                }
                true
            }
            "board" => {
                self.set_view(View::Query);
                true
            }
            "storage" | "backup" => {
                self.do_backup();
                self.set_view(View::Backup);
                true
            }
            "today" => {
                self.goto_today();
                true
            }
            "journal" | "j" => {
                if let Ok(day) = chrono::NaiveDate::parse_from_str(&arg, "%Y-%m-%d") {
                    self.goto_journal(JournalDay::new(day));
                } else if let Ok(offset) = arg.parse::<i64>() {
                    let day = JournalDay::new(self.today + chrono::Duration::days(offset));
                    self.goto_journal(day);
                } else {
                    self.toast(ToastKind::Warn, "journal: date or offset, e.g. -2", None);
                }
                true
            }
            "prune" => {
                self.prune_journals();
                true
            }
            "search" => {
                self.set_view(View::Search);
                self.search_query = arg;
                self.run_search();
                true
            }
            "help" => {
                self.set_view(View::Help);
                true
            }
            "m" | "move" => {
                let up = arg.starts_with('-');
                let n = arg
                    .trim_start_matches(['+', '-'])
                    .parse::<i64>()
                    .unwrap_or(1)
                    .abs()
                    .max(1);
                for _ in 0..n {
                    self.move_selected_block(up);
                }
                true
            }
            "sql" => {
                self.open_sql();
                if !arg.is_empty() {
                    if let Some(c) = self.sql.as_mut() {
                        c.input = arg;
                    }
                    self.sql_run();
                }
                true
            }
            _ => {
                self.toast(
                    ToastKind::Warn,
                    &format!("not a command: :{}", cmd),
                    Some("Tab completes · :help lists them"),
                );
                false
            }
        }
    }

    // ----------------------------------------------------- SQL console view

    pub fn open_sql(&mut self) {
        if self.view != View::Sql {
            self.prev_view = Some(self.view.clone());
        }
        self.view = View::Sql;
        self.sql = Some(SqlConsole {
            message: "read-only. `.tables` · `.schema blocks` · any SELECT or PRAGMA".into(),
            ..Default::default()
        });
    }

    pub fn toggle_sql(&mut self) {
        if self.view == View::Sql {
            self.sql = None;
            self.go_back();
        } else {
            self.open_sql();
        }
    }

    fn sql_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc => {
                self.sql = None;
                self.go_back();
            }
            KeyCode::Enter => self.sql_run(),
            KeyCode::Backspace => {
                if let Some(c) = self.sql.as_mut() {
                    c.input.pop();
                }
            }
            KeyCode::Up => {
                if let Some(c) = self.sql.as_mut() {
                    if !c.history.is_empty() {
                        let pos = match c.history_pos {
                            Some(p) if p > 0 => p - 1,
                            Some(p) => p,
                            None => c.history.len() - 1,
                        };
                        c.history_pos = Some(pos);
                        c.input = c.history[pos].clone();
                    }
                }
            }
            KeyCode::Down => {
                if let Some(c) = self.sql.as_mut() {
                    match c.history_pos {
                        Some(p) if p + 1 < c.history.len() => {
                            c.history_pos = Some(p + 1);
                            c.input = c.history[p + 1].clone();
                        }
                        _ => {
                            c.history_pos = None;
                            c.input.clear();
                        }
                    }
                }
            }
            KeyCode::Char('l') if ctrl => {
                if let Some(c) = self.sql.as_mut() {
                    c.rows.clear();
                    c.columns.clear();
                    c.message = "cleared".into();
                }
            }
            KeyCode::Char(ch) => {
                if let Some(c) = self.sql.as_mut() {
                    c.input.push(ch);
                }
            }
            _ => {}
        }
    }

    pub fn sql_run(&mut self) {
        let Some(c) = self.sql.as_mut() else { return };
        let input = c.input.trim().to_string();
        if input.is_empty() {
            return;
        }
        c.history.push(input.clone());
        c.history_pos = None;

        if input == ".tables" || input == ".t" {
            let tables = self.db.table_list();
            let n = tables.len();
            if let Some(c) = self.sql.as_mut() {
                c.columns = vec!["table".into(), "rows".into()];
                c.rows = tables
                    .into_iter()
                    .map(|(name, count)| vec![name, count.to_string()])
                    .collect();
                c.message = format!("{} tables and views", n);
            }
            return;
        }
        let sql = if let Some(rest) = input.strip_prefix(".schema") {
            let like = if rest.trim().is_empty() {
                "%".to_string()
            } else {
                rest.trim().to_string()
            };
            format!(
                "SELECT type, name, sql FROM sqlite_master
                  WHERE type IN ('table','view','index','trigger') AND name LIKE '%{}%'
                  ORDER BY type, name",
                like
            )
        } else {
            input.clone()
        };
        match self.db.console_query(&sql) {
            Ok((cols, rows)) => {
                let n = rows.len();
                if let Some(c) = self.sql.as_mut() {
                    c.columns = cols;
                    c.rows = rows;
                    c.message = format!("{} row{}", n, if n == 1 { "" } else { "s" });
                }
            }
            Err(e) => {
                if let Some(c) = self.sql.as_mut() {
                    c.message = e;
                    c.columns.clear();
                    c.rows.clear();
                }
            }
        }
    }
}

fn find_pair(chars: &[char], from: usize, closer: char) -> Option<usize> {
    let mut i = from;
    while i + 1 < chars.len() {
        if chars[i] == '\n' {
            return None;
        }
        if chars[i] == closer && chars[i + 1] == closer {
            return Some(i);
        }
        i += 1;
    }
    None
}

// ------------------------------------------------------- search and board

impl App {
    /// One quit path for every way out of the app, so an in-flight edit is
    /// always committed first -- `q` while typing in a block must not lose it.
    pub fn quit_now(&mut self, prune: bool, snapshot: bool) {
        self.commit_edit();
        if snapshot {
            self.do_backup();
        }
        self.prune_on_quit = prune;
        self.quit = true;
    }

    /// The search screen is a prompt, not a normal buffer: printable keys are
    /// the query, `↑`/`↓` (or `Ctrl-n`/`Ctrl-p`) pick a result, `⏎` opens it.
    pub fn search_key(&mut self, k: KeyEvent, ctrl: bool) {
        match k.code {
            KeyCode::Esc => self.go_back(),
            KeyCode::Enter => self.search_open_selected(),
            KeyCode::Up => self.search_selected = self.search_selected.saturating_sub(1),
            KeyCode::Down => {
                if self.search_selected + 1 < self.search_results.len() {
                    self.search_selected += 1;
                }
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.run_search();
            }
            KeyCode::Char('n') if ctrl => {
                if self.search_selected + 1 < self.search_results.len() {
                    self.search_selected += 1;
                }
            }
            KeyCode::Char('p') if ctrl => {
                self.search_selected = self.search_selected.saturating_sub(1)
            }
            KeyCode::Char(c) if !ctrl => {
                self.search_query.push(c);
                self.run_search();
            }
            _ => {}
        }
    }

    pub fn search_open_selected(&mut self) {
        let Some(hit) = self.search_results.get(self.search_selected).cloned() else {
            self.toast(ToastKind::Warn, "no result selected", Some("type to search"));
            return;
        };
        self.push_history();
        match crate::model::parse_journal_key(&hit.page) {
            Some(day) => self.goto_journal(day),
            None => self.goto_page(&hit.page),
        }
        self.select_block(hit.block_id);
        self.scroll_to_selection();
    }

    /// The board's three columns, in board order.
    pub fn board_columns(&self) -> Vec<(&'static str, Vec<crate::model::RefHit>)> {
        ["TODO", "DOING", "DONE"]
            .iter()
            .map(|s| (*s, self.db.by_status(s)))
            .collect()
    }

    pub fn board_move(&mut self, dx: i64, dy: i64) {
        let cols = self.board_columns();
        if cols.is_empty() {
            return;
        }
        if dx != 0 {
            let n = cols.len() as i64;
            self.query_col = ((self.query_col as i64 + dx).rem_euclid(n)) as usize;
        }
        let len = cols[self.query_col].1.len();
        if dy != 0 && len > 0 {
            self.query_row = ((self.query_row as i64 + dy).rem_euclid(len as i64)) as usize;
        }
        self.query_row = if len == 0 {
            0
        } else {
            self.query_row.min(len - 1)
        };
    }

    pub fn board_open(&mut self) {
        let cols = self.board_columns();
        let Some((_, hits)) = cols.get(self.query_col) else {
            return;
        };
        let Some(hit) = hits.get(self.query_row) else {
            self.toast(ToastKind::Info, "this column is empty", None);
            return;
        };
        let target = LinkTarget {
            kind: "query",
            label: crate::app::snippet(&hit.content, 34),
            page: Some(hit.page.clone()),
            block: Some(hit.block_id),
            at: 0,
        };
        self.jump_to(&target);
    }
}

// --------------------------------------------------------------- dispatch

impl App {
    /// Normal mode at the tree level.
    pub fn tree_key(&mut self, k: KeyEvent, ctrl: bool) {
        // The search screen is a prompt: printable keys are query text, so
        // `j` types a `j` there instead of moving. Navigation is arrows.
        if self.view == View::Search {
            self.search_key(k, ctrl);
            return;
        }
        let Some(name) = key_name(k, ctrl) else { return };

        // With the caret in a block, the text grammar owns the keys. The
        // `editor` half of the test matters: `text_focus` with no buffer is not a
        // state the router may act on.
        if self.text_focus && self.editor.is_some() {
            let seq = format!("{}{}", self.pending, name);
            // A bare word motion is the one text command that needs the outline
            // as well -- vim's `w` at the end of a line moves to the next line --
            // so it is handled before the grammar runs, not after. Running the
            // grammar first and then the helper executed the motion twice, and
            // `e` skipped a word.
            if Self::is_word_motion(&seq) {
                self.pending.clear();
                self.word_motion(&seq);
                return;
            }
            let outcome = self
                .editor
                .as_mut()
                .map(|ed| ed.command(&seq))
                .unwrap_or(crate::editor::Outcome::Unknown);
            match outcome {
                crate::editor::Outcome::Done => {
                    self.pending.clear();
                }
                crate::editor::Outcome::Insert => {
                    self.pending.clear();
                    self.mode = Mode::Insert;
                }
                crate::editor::Outcome::DropCaret => {
                    self.pending.clear();
                    self.leave_text();
                }
                crate::editor::Outcome::Quit => {
                    self.pending.clear();
                    self.quit_now(true, false);
                }
                crate::editor::Outcome::Incomplete => {
                    self.pending = seq;
                }
                crate::editor::Outcome::Unknown => {
                    if OUTLINE_FROM_TEXT.contains(&seq.as_str()) {
                        self.pending.clear();
                        self.leave_text();
                        if !self.tree_command(&seq) {
                            self.unknown_key(&seq);
                        }
                    } else if OUTLINE_FROM_TEXT
                        .iter()
                        .any(|k| k.len() > seq.len() && k.starts_with(&seq))
                    {
                        // A prefix of an outline verb: `z` before `za`, `C-w`
                        // before `C-wl`. Wait for the rest instead of refusing a
                        // key that was going somewhere.
                        self.pending = seq;
                    } else {
                        self.pending.clear();
                        // Refuse, and stay where the caret is. The old failure
                        // mode was to commit, close the buffer and *then* say
                        // there was no mapping for `r`.
                        self.toast(
                            ToastKind::Warn,
                            &format!("no text mapping for \"{}\"", seq),
                            Some("Esc leaves the block · ? keymap"),
                        );
                    }
                }
            }
            return;
        }

        // From the tree, a word motion puts the caret in this block and moves:
        // the only place words live is a block's text, so `w` is the one-key
        // version of `Enter` then `w`. It used to be "no mapping for w", which
        // is what "I cannot move the caret forward to a word" looked like.
        if matches!(name.as_str(), "w" | "W" | "b" | "B" | "e" | "E") {
            self.enter_text();
            self.word_motion(&name);
            return;
        }

        if !self.pending.is_empty() {
            let seq = format!("{}{}", self.pending, name);
            self.pending.clear();
            if self.tree_command(&seq) {
                return;
            }
            if self.tree_command(&name) {
                return;
            }
            self.unknown_key(&seq);
            return;
        }
        if matches!(
            name.as_str(),
            "g" | "d" | "y" | "z" | "Z" | ">" | "<" | "c" | "C-w"
        ) {
            self.pending = name;
            return;
        }
        if !self.tree_command(&name) {
            self.unknown_key(&name);
        }
    }

    /// Returns true when the key was consumed.
    pub fn tree_command(&mut self, seq: &str) -> bool {
        // The keymap is the manual, and longer than any pane, so `j`/`k` and
        // friends scroll it. This lives here rather than in the router because
        // `gg` arrives as a *pending* sequence, not as a single key.
        if self.view == View::Help {
            match seq {
                "j" => {
                    self.help_scroll = self.help_scroll.saturating_add(1);
                    return true;
                }
                "k" => {
                    self.help_scroll = self.help_scroll.saturating_sub(1);
                    return true;
                }
                "C-d" => {
                    self.help_scroll = self.help_scroll.saturating_add(12);
                    return true;
                }
                "C-u" => {
                    self.help_scroll = self.help_scroll.saturating_sub(12);
                    return true;
                }
                "gg" => {
                    self.help_scroll = 0;
                    return true;
                }
                "G" => {
                    // Clamped where it is drawn, because only the renderer knows
                    // how long the columns are.
                    self.help_scroll = usize::MAX / 2;
                    return true;
                }
                _ => {}
            }
        }
        // Focus decides what the same keys mean, exactly like vim windows.
        if self.focus == Focus::Right {
            match seq {
                "j" => {
                    self.linked_selected = self.linked_selected.saturating_add(1);
                    return true;
                }
                "k" => {
                    self.linked_selected = self.linked_selected.saturating_sub(1);
                    return true;
                }
                "<CR>" | "C-]" => {
                    self.open_selected_reference();
                    return true;
                }
                "q" | "<Esc>" | "l" => {
                    self.focus = Focus::Main;
                    return true;
                }
                _ => {}
            }
        }
        // The board is a set of columns, so h/l and j/k walk it rather than the
        // block list underneath.
        if self.view == View::Query {
            match seq {
                "j" => {
                    self.board_move(0, 1);
                    return true;
                }
                "k" => {
                    self.board_move(0, -1);
                    return true;
                }
                "l" | "<Tab>" => {
                    self.board_move(1, 0);
                    return true;
                }
                "h" | "<S-Tab>" => {
                    self.board_move(-1, 0);
                    return true;
                }
                "<CR>" | "C-]" => {
                    self.board_open();
                    return true;
                }
                "<Esc>" => {
                    self.go_back();
                    return true;
                }
                _ => {}
            }
        }
        match seq {
            "j" => self.move_selection(1),
            "k" => self.move_selection(-1),
            "h" => self.go_parent(),
            "l" => self.go_first_child(),
            "gg" => self.jump(false),
            "G" => self.jump(true),
            "C-d" => self.move_selection(12),
            "C-u" => self.move_selection(-12),
            // Ctrl-M only arrives as itself on terminals that disambiguate
            // it from Enter (see main.rs). `gm` is the spelling that always
            // works, which is why the hint bar may name it instead.
            "C-m" | "gm" => self.toggle_meta(),
            // Ctrl-P is the navigation: "Find" is everything the sidebar and
            // the page tree used to be, minus the pane you had to find first.
            "C-p" => self.open_palette(),
            "C-r" => self.redo(),
            "C-]" => self.follow_link(),
            "gf" => self.follow_link(),
            "C-o" | "C-t" => self.history_back(),
            "C-i" => self.history_forward(),
            "C-s" => {
                self.do_backup();
                self.set_view(View::Backup);
            }
            "C-wl" | "C-ww" => self.focus_right(),
            "u" => self.undo(),
            "dd" | "x" => self.delete_selected(),
            "yy" | "Y" => self.yank_selected(),
            "p" => self.paste(false),
            "P" => self.paste(true),
            "J" => self.merge_with_next(),
            "cc" => self.change_block(),
            ">>" | "<Tab>" => self.indent_selected(),
            "<<" | "<S-Tab>" => self.outdent_selected(),
            "za" => self.toggle_collapse(),
            "zc" => self.set_collapsed(true),
            "zo" => self.set_collapsed(false),
            "zM" => self.collapse_all(),
            "zR" => self.expand_all(),
            "i" => self.enter_insert(InsertAt::Start),
            "a" | "A" => self.enter_insert(InsertAt::End),
            "I" => self.enter_insert(InsertAt::Start),
            "o" => self.open_sibling(false),
            "O" => self.open_sibling(true),
            "<CR>" => self.enter_text(),
            "v" => self.enter_visual(false),
            "V" => self.enter_visual(true),
            ":" => self.open_ex(),
            "/" => self.open_search(),
            "n" => self.search_step(1),
            "N" => self.search_step(-1),
            "?" => self.set_view(View::Help),
            // Back and forward through the pages you have opened -- the same
            // two motions as `Ctrl-o`/`Ctrl-i`, on the keys a browser taught
            // everyone. Journal days are *not* on these: `]` used to walk to
            // tomorrow, which made the pair useless for going back to the page
            // you just came from. Days are `:prev` / `:next` now.
            "[" => self.history_back(),
            "]" => self.history_forward(),
            // `q` quits here rather than recording a macro: macros are not
            // implemented, and a key that does nothing is worse than a small
            // deviation. `ZZ` / `ZQ` are vim's own quit pair.
            "q" => self.quit_now(true, false),
            "ZZ" => self.quit_now(true, true),
            "ZQ" => self.quit_now(false, false),
            "<Esc>" => self.escape_tree(),
            _ => return false,
        }
        true
    }

    /// Vim's text grammar, applied to the block the caret is in. Returns false
    /// for anything that is not a text operation, so the caller can treat the
    /// key as a tree command instead.
    pub fn visual_key(&mut self, k: KeyEvent, ctrl: bool) {
        let Some(name) = key_name(k, ctrl) else { return };
        match name.as_str() {
            "j" => self.move_selection(1),
            "k" => self.move_selection(-1),
            "G" => self.jump(true),
            "gg" => self.jump(false),
            ">" => self.indent_selected(),
            "<" => self.outdent_selected(),
            "d" | "x" => self.delete_selected(),
            "y" => {
                self.yank_selected();
                self.visual = None;
                self.mode = Mode::Normal;
            }
            "J" => self.move_selected_block(false),
            "K" => self.move_selected_block(true),
            "v" | "V" | "<Esc>" | "q" => {
                self.visual = None;
                self.mode = Mode::Normal;
            }
            "C-]" => {
                self.visual = None;
                self.mode = Mode::Normal;
                self.follow_link();
            }
            _ => {}
        }
    }
}

/// Translate a key event into the token vim would print (`C-d`, `gg`, `<CR>`).
pub fn key_name(k: KeyEvent, ctrl: bool) -> Option<String> {
    Some(match k.code {
        KeyCode::Char('w') if ctrl => "C-w".to_string(),
        KeyCode::Char(c) if ctrl => format!("C-{}", c),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "<CR>".to_string(),
        KeyCode::Esc => "<Esc>".to_string(),
        KeyCode::Tab => "<Tab>".to_string(),
        KeyCode::BackTab => "<S-Tab>".to_string(),
        KeyCode::Backspace => "<BS>".to_string(),
        KeyCode::Down => "j".to_string(),
        KeyCode::Up => "k".to_string(),
        KeyCode::Left => "h".to_string(),
        KeyCode::Right => "l".to_string(),
        KeyCode::Home => "gg".to_string(),
        KeyCode::End => "G".to_string(),
        KeyCode::PageDown => "C-d".to_string(),
        KeyCode::PageUp => "C-u".to_string(),
        _ => return None,
    })
}

/// Regression tests for the key router. The `q` bug (documented in KEYMAP but
/// never wired into the dispatcher) is exactly the class of mistake these are
/// here to catch: a key that the help screen promises must do something.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Trigger;
    use crate::model::PageKind;

    /// One database per test: tests run in parallel threads of the same process,
    /// so the path has to be unique or one test deletes another's schema.
    fn app(name: &str) -> App {
        let path =
            std::env::temp_dir().join(format!("blok-test-{}-{}.db", std::process::id(), name));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
        let db = crate::db::Db::open(&path).expect("open test db");
        let today = crate::db::today();
        let mut app = App::new(db, today);
        let page = app.db.ensure_page("Test Page", PageKind::Page);
        app.db.create_block(page.id, None, None, "first block");
        app.db
            .create_block(page.id, None, None, "second block with [[Test Page]]");
        app.goto_page("Test Page");
        app
    }

    fn key(app: &mut App, c: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    fn ctrl(app: &mut App, c: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }
    fn code(app: &mut App, c: KeyCode) {
        app.handle_key(KeyEvent::new(c, KeyModifiers::NONE));
    }

    #[test]
    fn q_quits() {
        let mut a = app("q_quits");
        key(&mut a, 'q');
        assert!(a.quit, "`q` is in the keymap, so it must quit");
    }

    #[test]
    fn colon_q_quits() {
        let mut a = app("colon_q");
        key(&mut a, ':');
        assert!(a.ex.is_some(), "`:` opens the command line");
        key(&mut a, 'q');
        code(&mut a, KeyCode::Enter);
        assert!(a.quit, "`:q` must quit");
    }

    #[test]
    fn q_while_typing_commits_before_quitting() {
        let mut a = app("q_commits");
        code(&mut a, KeyCode::Enter); // caret into the block
        assert!(a.text_focus);
        let id = a.rows[0].id;
        // A dirty buffer, however it got that way.
        if let Some(ed) = a.editor.as_mut() {
            ed.insert_str("tail");
        }
        key(&mut a, 'q');
        assert!(a.quit);
        let content = a.db.block(id).map(|b| b.content).unwrap_or_default();
        assert!(
            content.contains("tail"),
            "an in-flight edit must be committed, not dropped: {content:?}"
        );
    }

    #[test]
    fn q_in_insert_mode_types_a_q() {
        let mut a = app("q_insert");
        code(&mut a, KeyCode::Enter); // caret into the block
        key(&mut a, 'i'); // -> INSERT
        key(&mut a, 'q');
        assert!(!a.quit, "in INSERT, `q` is a character, exactly as in vim");
        assert!(a
            .editor
            .as_ref()
            .map(|e| e.text().contains('q'))
            .unwrap_or(false));
    }

    #[test]
    fn one_escape_always_leaves_editing() {
        let mut a = app("esc_once");
        code(&mut a, KeyCode::Enter);
        assert!(a.text_focus, "Enter puts the caret in the block");
        assert_eq!(a.mode, Mode::Normal, "and does not change the mode");
        key(&mut a, 'i');
        assert_eq!(a.mode, Mode::Insert);
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.mode, Mode::Normal, "one Esc leaves INSERT");
        assert!(a.text_focus, "the caret stays where it was");
        code(&mut a, KeyCode::Esc);
        assert!(!a.text_focus, "a second Esc drops the caret");
    }

    #[test]
    fn a_tree_motion_leaves_the_block_text() {
        let mut a = app("motion_leaves");
        code(&mut a, KeyCode::Enter);
        assert!(a.text_focus);
        let first = a.selected;
        key(&mut a, 'j');
        assert!(!a.text_focus, "j must not leave the caret stuck in the block");
        assert_eq!(a.selected, first + 1, "and it must still move");
    }

    #[test]
    fn gf_and_ctrl_bracket_both_follow_links() {
        for (name, keys) in [("gf", vec!['g', 'f']), ("ctrl-bracket", vec![])] {
            let mut a = app(&format!("follow_{}", name));
            a.selected = 1; // "second block with [[Test Page]]"
            if keys.is_empty() {
                ctrl(&mut a, ']');
            } else {
                for k in keys {
                    key(&mut a, k);
                }
            }
            assert_eq!(a.view, View::Page("Test Page".into()), "{name} follows");
        }
    }

    #[test]
    fn find_is_the_navigation() {
        let mut a = app("find_pages");
        ctrl(&mut a, 'p');
        {
            let p = a.palette.as_ref().expect("Ctrl-P opens Find");
            assert!(p.recent, "with no query it lists what you touched last");
            assert!(!p.items.is_empty(), "and that list is not empty");
        }
        for c in "Test".chars() {
            key(&mut a, c);
        }
        {
            let p = a.palette.as_ref().unwrap();
            assert!(!p.recent, "a query switches from recent to matches");
            assert!(
                p.items.iter().any(|i| i.label.contains("Test Page")),
                "the matching page is listed"
            );
        }
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.view, View::Page("Test Page".into()));
    }

    #[test]
    fn find_matches_block_text_and_lands_on_the_block() {
        let mut a = app("find_content");
        // "second block" appears only inside a block, never in a page name.
        ctrl(&mut a, 'p');
        for c in "second".chars() {
            key(&mut a, c);
        }
        {
            let p = a.palette.as_ref().unwrap();
            let hit = p
                .items
                .iter()
                .find(|i| i.label.contains("Test Page"))
                .expect("a content match still surfaces the page");
            assert!(
                hit.detail.contains("matched in a block"),
                "and says why it matched: {}",
                hit.detail
            );
        }
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.view, View::Page("Test Page".into()));
        let selected = a
            .selected_row()
            .map(|r| r.content.clone())
            .unwrap_or_default();
        assert!(
            selected.contains("second block"),
            "Enter lands on the matching block, not just the page: {selected:?}"
        );
    }

    #[test]
    fn ctrl_w_l_reaches_the_metadata_panel_once_it_is_shown() {
        let mut a = app("refs_focus");
        assert!(!a.show_meta, "hidden by default");
        ctrl(&mut a, 'w');
        key(&mut a, 'l');
        assert_eq!(
            a.focus,
            Focus::Main,
            "there is no panel to focus while it is hidden"
        );
        ctrl(&mut a, 'm');
        ctrl(&mut a, 'w');
        key(&mut a, 'l');
        assert_eq!(a.focus, Focus::Right, "Ctrl-w l focuses the metadata panel");
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.focus, Focus::Main, "Esc goes back to the blocks");
    }

    #[test]
    fn hints_are_contextual() {
        let a = app("hints");
        // The block with a link exists, so the hint bar must be able to say so.
        let row = a.rows.iter().find(|r| r.content.contains("[[")).cloned();
        assert!(row.is_some(), "the fixture has a linking block");
        let links = row.map(|r| a.links_in(r.id).len()).unwrap_or(0);
        assert_eq!(links, 1);
    }

    #[test]
    fn a_message_clears_on_the_next_keypress() {
        let mut a = app("message_clears");
        key(&mut a, 'd');
        key(&mut a, 'd'); // dd deletes, and reports it
        assert!(a.toast.is_some(), "an action reports what it did");
        key(&mut a, 'j');
        assert!(a.toast.is_none(), "the next keypress clears the message");
    }

    #[test]
    fn dd_deletes_and_u_restores() {
        let mut a = app("dd_undo");
        let before = a.rows.len();
        let id = a.rows[0].id;
        key(&mut a, 'd');
        assert!(a.pending == "d", "`d` is an operator waiting for a motion");
        key(&mut a, 'd');
        assert_eq!(a.rows.len(), before - 1, "dd deletes one block");
        key(&mut a, 'u');
        assert_eq!(a.rows.len(), before, "u brings it back");
        assert!(a.db.block(id).is_some());
    }

    #[test]
    fn ctrl_bracket_follows_a_link() {
        let mut a = app("follow_link");
        a.selected = 1; // the block that links to [[Test Page]]
        a.follow_link();
        assert_eq!(a.view, View::Page("Test Page".into()));
    }

    #[test]
    fn gm_always_toggles_metadata() {
        // Ctrl-M and Enter share a byte, so the always-available spelling needs
        // its own guarantee.
        let mut a = app("gm_meta");
        assert!(!a.show_meta);
        key(&mut a, 'g');
        key(&mut a, 'm');
        assert!(a.show_meta, "gm shows the page metadata");
        assert_eq!(a.meta_key(), "gm", "without terminal support, gm is named");
    }

    #[test]
    fn enter_edits_and_does_not_toggle_metadata() {
        // The reported bug: Ctrl-M arriving as Enter started editing. Enter must
        // keep meaning "edit this block".
        let mut a = app("enter_not_meta");
        code(&mut a, KeyCode::Enter);
        assert!(a.text_focus, "Enter puts the caret in the block");
        assert!(!a.show_meta, "and leaves the metadata panel alone");
    }

    #[test]
    fn metadata_is_hidden_by_default_and_ctrl_m_shows_it() {
        let mut a = app("panels");
        assert!(!a.show_meta, "the panel starts hidden");
        ctrl(&mut a, 'm');
        assert!(a.show_meta, "Ctrl-M shows the page metadata");
        assert_eq!(a.db.get_setting("show_meta").as_deref(), Some("true"));
        ctrl(&mut a, 'm');
        assert!(!a.show_meta);
        assert_eq!(a.db.get_setting("show_meta").as_deref(), Some("false"));
    }

    #[test]
    fn page_metadata_has_the_facts_the_panel_claims() {
        let a = app("page_meta");
        let id = a.view_meta_id().expect("a page is open");
        let m = a.db.page_meta(id).expect("metadata exists");
        assert_eq!(m.name, "Test Page");
        assert!(!m.is_journal);
        assert_eq!(m.blocks, 2);
        assert!(!m.created_at.is_empty() && !m.updated_at.is_empty());
    }

    #[test]
    fn console_is_read_only() {
        let a = app("console");
        assert!(a.db.console_query("DELETE FROM blocks").is_err());
        assert!(a.db.console_query("SELECT count(*) FROM blocks").is_ok());
    }

    #[test]
    fn every_documented_quit_key_ends_the_session() {
        for path in ["q", "ZZ", "ZQ"] {
            let mut a = app(&format!("quit_{}", path));
            for c in path.chars() {
                key(&mut a, c);
            }
            assert!(a.quit, "`{path}` must quit");
        }
    }

    /// `[` and `]` are back and forward through the pages you have opened -- the
    /// same two motions as `Ctrl-o`/`Ctrl-i`, on the keys a browser taught
    /// everyone -- and they name where they landed.
    #[test]
    fn brackets_walk_back_and_forward_through_opened_pages() {
        let mut a = app("brackets_history");
        let other = a.db.ensure_page("Other Page", PageKind::Page);
        a.db.create_block(other.id, None, None, "elsewhere");

        a.run_ex("e Other Page");
        assert_eq!(a.view, View::Page("Other Page".into()));

        key(&mut a, '[');
        assert_eq!(
            a.view,
            View::Page("Test Page".into()),
            "`[` returns to the page you came from"
        );
        assert!(
            a.toast.as_ref().is_some_and(|t| t.text.contains("Test Page")),
            "going back names the page it landed on"
        );

        key(&mut a, ']');
        assert_eq!(a.view, View::Page("Other Page".into()));
        assert!(
            a.toast.as_ref().is_some_and(|t| t.text.contains("Other Page")),
            "and so does going forward"
        );

        // The end of the history is an answer, not a silent no-op.
        key(&mut a, ']');
        assert_eq!(a.view, View::Page("Other Page".into()), "nothing newer");
        assert!(
            a.toast.as_ref().is_some_and(|t| t.text.contains("no page to go forward")),
            "and it says so"
        );
    }

    /// The report: `]` walked to tomorrow's journal. Journal days are a
    /// deliberate step now (`:prev` / `:next`), never a default for the
    /// navigation keys.
    #[test]
    fn brackets_do_not_step_the_journal_day() {
        let mut a = app("brackets_days");
        a.goto_journal(JournalDay::new(a.today));
        let today = JournalDay::new(a.today);

        key(&mut a, ']');
        assert_eq!(a.view, View::Journal(today), "`]` must not walk to tomorrow");
        key(&mut a, '[');
        assert_eq!(a.view, View::Journal(today), "nor `[` back to yesterday");

        a.run_ex("next");
        let tomorrow = JournalDay::new(a.today + chrono::Duration::days(1));
        assert_eq!(a.view, View::Journal(tomorrow), "`:next` still steps a day");
        a.run_ex("prev");
        assert_eq!(a.view, View::Journal(today), "and `:prev` steps back");

        // Stepping is opening a page, so the page you left is still behind you.
        a.run_ex("next");
        key(&mut a, '[');
        assert_eq!(a.view, View::Journal(today));
    }

    /// The jumplist skipped the page in the middle. `history_pos` sat on the only
    /// recorded entry, so a single jump could not be undone at all and the second
    /// `Ctrl-o` went back two pages. Reachable, untested, and the reason `[` and
    /// `]` needed the same machinery to be worth anything.
    #[test]
    fn the_jumplist_walks_every_page_it_recorded() {
        let mut a = app("history_chain");
        for name in ["One", "Two", "Three"] {
            let p = a.db.ensure_page(name, PageKind::Page);
            a.db.create_block(p.id, None, None, name);
        }
        a.run_ex("e One");
        a.run_ex("e Two");
        a.run_ex("e Three");
        assert_eq!(a.view, View::Page("Three".into()));

        ctrl(&mut a, 'o');
        assert_eq!(a.view, View::Page("Two".into()), "one step back, not two");
        ctrl(&mut a, 'o');
        assert_eq!(a.view, View::Page("One".into()));
        ctrl(&mut a, 'o');
        assert_eq!(
            a.view,
            View::Page("Test Page".into()),
            "and then the page we started on"
        );

        ctrl(&mut a, 'i');
        assert_eq!(a.view, View::Page("One".into()), "forward retraces the chain");
    }

    /// Going back and then somewhere else is a new branch: what was ahead is
    /// dropped, as in a browser. Without this, `]` would walk into a trail that
    /// no longer connects to where you are.
    #[test]
    fn a_new_page_drops_the_forward_trail() {
        let mut a = app("history_branch");
        for name in ["Other Page", "Another"] {
            let p = a.db.ensure_page(name, PageKind::Page);
            a.db.create_block(p.id, None, None, name);
        }
        a.run_ex("e Other Page");
        key(&mut a, '[');
        assert_eq!(a.view, View::Page("Test Page".into()));

        a.run_ex("e Another");
        key(&mut a, ']');
        assert_eq!(
            a.view,
            View::Page("Another".into()),
            "`]` must not resurrect a page from the abandoned trail"
        );
        assert!(
            a.toast
                .as_ref()
                .is_some_and(|t| t.text.contains("no page to go forward")),
            "and it says there is nothing ahead"
        );
    }

    /// The slash menu advertised a code block and wrote the word "Code". Now it
    /// writes a fence, and the caret lands on the line between the fences where
    /// the code goes.
    #[test]
    fn slash_code_opens_a_fence_with_the_caret_inside_it() {
        let mut a = app("slash_code");
        code(&mut a, KeyCode::Enter); // caret into the block
        key(&mut a, 'i'); // insert

        for c in "/Code".chars() {
            key(&mut a, c);
        }
        assert!(a.popup.is_some(), "typing / opens the slash menu");
        code(&mut a, KeyCode::Enter);

        let ed = a.editor.as_ref().expect("still editing");
        assert_eq!(ed.text(), "```\n\n```", "a fence, not the word Code");
        assert_eq!(ed.position(), (2, 1), "the caret is inside the fence");
    }

    /// A code block is the one place the app must not interpret anything:
    /// `#include` is not a tag, `[[x]]` in a string is not a page, and a
    /// `key:: value` in a sample is not a property of yours.
    #[test]
    fn a_code_block_creates_no_refs_and_no_properties() {
        let a = app("code_refs");
        let page = a.db.ensure_page("Snippets", PageKind::Page);
        let code = "```c\n#include <stdio.h>\nchar *s = \"[[not a page]]\";\nid:: 7\n#not a tag\n```";
        a.db.create_block(page.id, None, None, code);

        let meta = a.db.page_meta(page.id).expect("meta");
        assert_eq!(
            meta.properties.len(),
            0,
            "nothing inside a fence is a property: {:?}",
            meta.properties
        );
        assert_eq!(meta.refs_out, 0);

        // The same shape outside a fence *is* interpreted, which is the whole
        // reason the fence has to be respected.
        let prose = a.db.ensure_page("Prose", PageKind::Page);
        a.db.create_block(prose.id, None, None, "#realtag\nid:: 7");
        let meta = a.db.page_meta(prose.id).expect("meta");
        assert_eq!(meta.properties.len(), 1);
        assert_eq!(meta.refs_out, 1);
    }

    /// Turning a prose block into a code block has to clean up after it: the
    /// refs it created must not linger just because the text now says "do not
    /// interpret this".
    #[test]
    fn fencing_a_block_removes_the_refs_it_had() {
        let a = app("code_cleanup");
        let page = a.db.ensure_page("Cleanup", PageKind::Page);
        let b = a.db.create_block(page.id, None, None, "see [[Test Page]] and #atag");
        assert_eq!(a.db.page_meta(page.id).unwrap().refs_out, 2);

        a.db.update_content(b.id, "```\nsee [[Test Page]] and #atag\n```");
        assert_eq!(
            a.db.page_meta(page.id).unwrap().refs_out,
            0,
            "the fence takes the refs away"
        );
    }

    /// Report: "tab when in edit mode should indent." With the caret in the
    /// text, Tab indents the *text* and Shift-Tab gives it back. From the tree,
    /// where there is no caret in a block, Tab still re-indents the block.
    #[test]
    fn tab_indents_the_text_when_the_caret_is_in_it() {
        let mut a = app("tab_text");
        code(&mut a, KeyCode::Enter);
        assert!(a.text_focus);
        let id = a.rows[0].id;
        let depth_before = a.rows[0].depth;

        code(&mut a, KeyCode::Tab);
        assert_eq!(
            a.editor.as_ref().unwrap().text(),
            "  first block",
            "Tab writes an indent at the caret"
        );
        code(&mut a, KeyCode::BackTab);
        assert_eq!(a.editor.as_ref().unwrap().text(), "first block");

        // In INSERT too, because that is what "edit mode" usually means.
        key(&mut a, 'i');
        code(&mut a, KeyCode::Tab);
        assert!(a.editor.as_ref().unwrap().text().starts_with("  "));

        key(&mut a, 'x');
        code(&mut a, KeyCode::Esc); // commits
        let content = a.db.block(id).map(|b| b.content).unwrap_or_default();
        assert_eq!(content, "  xfirst block", "and it is saved");
        assert_eq!(
            a.rows.iter().find(|r| r.id == id).map(|r| r.depth),
            Some(depth_before),
            "the block's own indentation is untouched"
        );
        assert!(a.db.block(id).and_then(|b| b.parent_id).is_none());
    }

    /// The other half of the report: from the tree, Tab is still a structural
    /// indent, and `>>` still does it with the caret in the text.
    #[test]
    fn tab_from_the_tree_still_indents_the_block() {
        let mut a = app("tab_structure");
        assert!(!a.text_focus);
        // The first block has no previous sibling to adopt, so indenting it is
        // correctly a no-op: use the second.
        key(&mut a, 'j');
        let id = a.rows[1].id;
        code(&mut a, KeyCode::Tab);
        assert_eq!(
            a.rows.iter().find(|r| r.id == id).map(|r| r.depth),
            Some(1),
            "Tab from the tree indents the block"
        );
        assert!(a.editor.is_none(), "and it did not open an editor");

        // `>>` with the caret in the text says what it does: structure, not text.
        code(&mut a, KeyCode::Enter);
        assert!(a.text_focus);
        let depth = a.rows.iter().find(|r| r.id == id).map(|r| r.depth);
        key(&mut a, '>');
        key(&mut a, '>');
        assert_eq!(
            a.rows.iter().find(|r| r.id == id).map(|r| r.depth),
            depth,
            "there is no previous sibling at this level, so nothing moves"
        );
        assert!(a.db.block(id).and_then(|b| b.parent_id).is_some());
    }

    /// Report: "How to edit or input multiline codeblock?" The answer used to be
    /// "press Alt-Enter for every line, and do not press Enter" -- Enter split
    /// the block, which cut the fence in half and left the rest of the program
    /// outside it as prose. In a code block Enter is now a newline, indented like
    /// the line you are on, and the closing fence simply moves down.
    #[test]
    fn enter_in_a_code_block_adds_a_line_instead_of_splitting_it() {
        let mut a = app("code_multiline");
        code(&mut a, KeyCode::Enter);
        key(&mut a, 'i');
        for c in "/Code".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.editor.as_ref().unwrap().text(), "```\n\n```");
        assert_eq!(a.rows.len(), 2, "still two blocks: nothing was split");

        for c in "fn main() {".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Enter);
        for c in "    let x = 1;".chars() {
            key(&mut a, c);
        }
        code(&mut a, KeyCode::Enter);
        // The new line is indented like the one above it, and Shift-Tab takes
        // back one step at a time (2 spaces each, as in vim: one shiftwidth per
        // press), which is what closing a brace looks like.
        code(&mut a, KeyCode::BackTab);
        code(&mut a, KeyCode::BackTab);
        key(&mut a, '}');

        let ed = a.editor.as_ref().unwrap();
        assert_eq!(
            ed.text(),
            "```\nfn main() {\n    let x = 1;\n}\n```",
            "the fence is intact and the closing fence moved down"
        );
        assert_eq!(a.rows.len(), 2, "one block, however many lines it has");
        assert_eq!(ed.position(), (4, 2), "the caret is inside the fence");

        // And it commits as one block with real newlines in it.
        code(&mut a, KeyCode::Esc);
        let id = a.rows[0].id;
        let content = a.db.block(id).map(|b| b.content).unwrap_or_default();
        assert!(content.contains("let x = 1;"), "{content}");
        assert!(content.ends_with("```"), "{content}");
    }

    /// Enter keeps the line's indentation, which is the whole reason to have it
    /// in code rather than a bare newline.
    #[test]
    fn code_newlines_keep_the_line_indentation() {
        let mut a = app("code_indent");
        let id = a.rows[0].id;
        a.begin_edit_block(id);
        if let Some(ed) = a.editor.as_mut() {
            ed.set_text("```\n    deep()\n```");
            ed.goto_line(1);
            ed.end();
        }
        code(&mut a, KeyCode::Enter);
        key(&mut a, 'x');
        let ed = a.editor.as_ref().unwrap();
        assert_eq!(ed.text(), "```\n    deep()\n    x\n```");
    }

    /// Typing the fence by hand: Enter at the end of the opening line brings the
    /// closing fence with it, so a code block is never left half-open.
    #[test]
    fn enter_after_a_hand_typed_fence_closes_it() {
        let mut a = app("code_hand");
        let id = a.rows[0].id;
        a.begin_edit_block(id);
        if let Some(ed) = a.editor.as_mut() {
            ed.set_text("```rust");
            ed.cursor = ed.chars.len();
        }
        code(&mut a, KeyCode::Enter);
        let ed = a.editor.as_ref().unwrap();
        assert_eq!(ed.text(), "```rust\n\n```");
        assert_eq!(ed.position(), (2, 1), "and the caret is between the fences");
    }

    /// In NORMAL mode the arrows move *within* the block, and `j`/`k` still walk
    /// out of it. Before this, Up/Down were `k`/`j`, so a multi-line block could
    /// not be navigated at all without going back into INSERT.
    #[test]
    fn arrows_move_inside_the_block_without_leaving_it() {
        let mut a = app("code_arrows");
        let id = a.rows[0].id;
        a.begin_edit_block(id);
        if let Some(ed) = a.editor.as_mut() {
            ed.set_text("one\ntwo\nthree");
            ed.goto_line(0);
        }
        code(&mut a, KeyCode::Esc); // NORMAL, caret still in the block
        assert!(a.text_focus);
        code(&mut a, KeyCode::Down);
        assert_eq!(a.editor.as_ref().unwrap().position(), (2, 1));
        code(&mut a, KeyCode::Down);
        assert_eq!(a.editor.as_ref().unwrap().position(), (3, 1));
        code(&mut a, KeyCode::Down);
        assert_eq!(
            a.editor.as_ref().unwrap().position(),
            (3, 1),
            "the last line is the last line"
        );
        code(&mut a, KeyCode::Up);
        assert_eq!(a.editor.as_ref().unwrap().position(), (2, 1));
        assert!(a.text_focus, "and we never left the block");

        // `gj`/`gk` are the vim spelling of the same two motions.
        key(&mut a, 'g');
        key(&mut a, 'k');
        assert_eq!(a.editor.as_ref().unwrap().position(), (1, 1));
        assert!(a.text_focus);

        // `j` alone is still the way out.
        key(&mut a, 'j');
        assert!(!a.text_focus, "`j` leaves the block, as it always did");
    }

    /// `Esc` used to hand the caret to the end of the block: committing takes the
    /// buffer, and the reload rebuilt it with the cursor at `chars.len()`. In a
    /// multi-line block that means losing your place every time you stop typing.
    #[test]
    fn escape_keeps_the_caret_where_it_was() {
        let mut a = app("esc_caret");
        let id = a.rows[0].id;
        a.begin_edit_block(id);
        if let Some(ed) = a.editor.as_mut() {
            ed.set_text("alpha\nbeta\ngamma");
            ed.goto_line(1);
            ed.cursor += 2; // two characters into "beta"
        }
        assert_eq!(a.editor.as_ref().unwrap().position(), (2, 3));

        code(&mut a, KeyCode::Esc);
        assert!(a.text_focus);
        assert_eq!(
            a.editor.as_ref().unwrap().position(),
            (2, 3),
            "the caret is where it was, not at the end of the text"
        );

        // And a second Esc drops the caret entirely, as documented.
        code(&mut a, KeyCode::Esc);
        assert!(!a.text_focus);
        assert!(a.editor.is_none());
    }

    /// Report: with more than one link in a block, Enter on the chosen link did
    /// nothing. The *single*-link path (`jump_to` directly) had a test; the
    /// chooser, which only exists when there are two or more links, did not --
    /// so nothing ever noticed that its Enter was wired to nothing.
    #[test]
    fn the_link_chooser_follows_the_link_you_pick() {
        let mut a = app("link_chooser");
        let page = a.db.ensure_page("Test Page", PageKind::Page);
        let other = a.db.ensure_page("Other Page", PageKind::Page);
        a.db.create_block(other.id, None, None, "elsewhere");
        let b = a
            .db
            .create_block(
                page.id,
                None,
                None,
                "links: [[Other Page]] and [[Missing Page]]",
            )
            .id;
        a.reload();
        a.select_block(b);
        assert_eq!(a.links_in(b).len(), 2, "this is the chooser's case");

        ctrl(&mut a, ']');
        {
            let p = a.popup.as_ref().expect("two links: a chooser, not a jump");
            assert_eq!(p.trigger, Trigger::Link);
            assert_eq!(p.candidates.len(), 2);
            assert_eq!(p.candidates[0].label, "[[Other Page]]", "the label is the syntax");
            assert!(
                p.candidates[0].detail.contains("Other Page"),
                "the detail says where it goes: {}",
                p.candidates[0].detail
            );
        }

        code(&mut a, KeyCode::Enter);
        assert_eq!(
            a.view,
            View::Page("Other Page".into()),
            "Enter follows the selected link"
        );
        assert!(a.popup.is_none());

        // Following is navigation, so back comes here -- not to the top of the page.
        key(&mut a, '[');
        assert_eq!(a.view, View::Page("Test Page".into()));

        // Down picks the other one, and a page that does not exist yet is still a
        // destination.
        ctrl(&mut a, ']');
        code(&mut a, KeyCode::Down);
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.view, View::Page("Missing Page".into()));
    }

    /// The chooser is a menu, not a text field. Typing in it used to insert the
    /// characters into the *block* and then close the popup, which is a strange
    /// thing for a menu to do to your document.
    #[test]
    fn typing_in_the_link_chooser_filters_and_leaves_the_block_alone() {
        let mut a = app("link_chooser_filter");
        let page = a.db.ensure_page("Test Page", PageKind::Page);
        let other = a.db.ensure_page("Other Page", PageKind::Page);
        a.db.create_block(other.id, None, None, "elsewhere");
        let b = a
            .db
            .create_block(page.id, None, None, "see [[Other Page]] and [[Test Page]]")
            .id;
        a.reload();
        a.select_block(b);
        // `Ctrl-]` is navigation, so it leaves the block's text first (a tree
        // motion), which is why this opens the chooser with no editor. The hard
        // case is a chooser open *while* a buffer is being edited, so set that
        // up directly rather than relying on an obscure key sequence.
        a.begin_edit_block(b);
        a.open_link_menu(a.links_in(b));
        let before = a.editor.as_ref().unwrap().text();

        for c in "Other".chars() {
            key(&mut a, c);
        }
        {
            let p = a.popup.as_ref().expect("still a chooser");
            assert_eq!(p.query, "Other");
            assert_eq!(p.candidates.len(), 1, "the query filters the list");
            assert_eq!(p.candidates[0].label, "[[Other Page]]");
        }
        assert_eq!(
            a.editor.as_ref().unwrap().text(),
            before,
            "and the buffer is untouched: a menu does not type into your block"
        );

        code(&mut a, KeyCode::Enter);
        assert_eq!(a.view, View::Page("Other Page".into()));

        // The same keys, reached the way a user reaches them: back to the page
        // holding the block, open the chooser from the tree, filter, un-filter,
        // and cancel.
        key(&mut a, '[');
        assert_eq!(a.view, View::Page("Test Page".into()));
        ctrl(&mut a, ']');
        assert_eq!(a.popup.as_ref().unwrap().candidates.len(), 2);
        key(&mut a, 'o');
        assert_eq!(a.popup.as_ref().unwrap().candidates.len(), 1, "'o' is in one");
        code(&mut a, KeyCode::Backspace);
        assert_eq!(a.popup.as_ref().unwrap().query, "");
        assert_eq!(a.popup.as_ref().unwrap().candidates.len(), 2);
        code(&mut a, KeyCode::Esc);
        assert!(a.popup.is_none());
        assert_eq!(
            a.view,
            View::Page("Test Page".into()),
            "Esc cancels the chooser; it does not follow anything"
        );
    }

    /// The contract of the text layer, as a table.
    ///
    /// Every row is a bug that shipped. The audit that produced it put the caret
    /// in a block and pressed one vim text key: `dd` deleted the block, `yy`
    /// yanked the block, `p` pasted one, `>>` indented the block, `u` undid
    /// something else, `v` started a block selection, and `r`/`s`/`f`/`X`/`~`
    /// committed the buffer, closed it, and *then* said "no mapping".
    #[test]
    fn text_keys_do_text_things_not_outline_things() {
        // (keys, buffer afterwards, still in the text?, block count)
        let cases: &[(&str, &str, bool, usize)] = &[
            ("dd", "", true, 2),                       // the line's text, not the block
            ("yy", "first block", true, 2),            // yank, do not leave
            ("yyp", "first block\nfirst block", true, 2),
            ("ciw", " block", true, 2),                // no stray `w`
            ("rX", "Xirst block", true, 2),
            ("~", "First block", true, 2),
            ("X", "first block", true, 2),
            ("x", "irst block", true, 2),
            ("dw", "block", true, 2),
            ("d$", "", true, 2),
            (">>", "  first block", true, 2),
            ("fa", "first block", true, 2),            // a find, not a no-mapping toast
            ("Q", "first block", true, 2),             // unknown: refused, NOT ejected
            ("zq", "first block", true, 2),            // ...and neither is this
            // The outline keys still leave: the caret's position decides.
            ("j", "first block", false, 2),
            // `o` opens a new block *and* puts the caret in it, so the buffer
            // afterwards is the new, empty one.
            ("o", "", false, 3),
            ("v", "first block", false, 2),
            ("u", "first block", false, 2),
        ];
        for (keys, buffer, in_text, blocks) in cases {
            let mut a = app(&format!("text_keys_{}", keys.replace(['$', '>', '~'], "x")));
            code(&mut a, KeyCode::Enter); // caret into the block
            assert!(a.text_focus, "{keys}: the caret starts in the text");
            for c in keys.chars() {
                key(&mut a, c);
            }
            assert_eq!(
                a.text_focus, *in_text,
                "{keys}: text_focus was {} (buffer {:?})",
                a.text_focus,
                a.editor.as_ref().map(|e| e.text())
            );
            assert_eq!(a.rows.len(), *blocks, "{keys}: block count");
            if let Some(ed) = a.editor.as_ref() {
                assert_eq!(ed.text(), *buffer, "{keys}: the buffer");
            } else {
                assert_eq!(*buffer, "first block", "{keys}: no buffer left");
            }
        }
    }

    /// `dd` empties the line it is on. The *block* delete is the tree's, one
    /// `Esc` (or `j`) away -- and the hint bar and the ruler are what tell you
    /// which domain you are in.
    #[test]
    fn deleting_the_line_and_deleting_the_block_are_different_keys() {
        let mut a = app("dd_domains");
        code(&mut a, KeyCode::Enter);
        key(&mut a, 'd');
        key(&mut a, 'd');
        assert_eq!(a.rows.len(), 2, "the caret was in the text: the line went");
        assert!(a.editor.is_some());

        // Same keys, caret in the tree.
        let mut a = app("dd_tree");
        key(&mut a, 'd');
        key(&mut a, 'd');
        assert_eq!(a.rows.len(), 1, "the caret was in the tree: the block went");
    }

    /// A paste is text, not keystrokes. Before bracketed paste was handled the
    /// loop threw `Event::Paste` away entirely, so pasting did nothing at all in
    /// a terminal that sends it -- and in one that does not, a pasted newline
    /// arrived as Enter, so pasting code split the block per line and the
    /// auto-indent copied the previous line's indentation onto each one.
    #[test]
    fn a_paste_is_text_and_not_keystrokes() {
        let mut a = app("paste_text");
        code(&mut a, KeyCode::Enter);
        let id = a.rows[0].id;
        a.paste_text("fn main() {\n    let x = 1;\n}\n");

        let ed = a.editor.as_ref().expect("still editing");
        // Enter put the caret at the first non-blank, so the paste lands at the
        // front: what matters is that it is *verbatim*, not where the caret is.
        assert_eq!(
            ed.text(),
            "fn main() {\n    let x = 1;\n}\nfirst block",
            "verbatim, newlines and all"
        );
        assert!(
            !ed.text().contains("\n    fn main"),
            "and no auto-indent was added to the pasted lines"
        );

        // And it commits as one block with real newlines in it.
        code(&mut a, KeyCode::Esc);
        assert_eq!(a.rows.len(), 2, "a paste does not split blocks");
        let content = a.db.block(id).map(|b| b.content).unwrap_or_default();
        assert!(content.contains("\n    let x = 1;\n"), "{content:?}");
    }

    /// With no caret in a block, a paste becomes a block -- the whole paste in
    /// it, because splitting it into one block per line is a judgement about the
    /// content that a paste cannot make.
    #[test]
    fn a_paste_with_no_caret_opens_a_block() {
        let mut a = app("paste_block");
        assert!(a.editor.is_none());
        a.paste_text("one\ntwo\nthree");
        assert_eq!(a.rows.len(), 3);
        let ed = a.editor.as_ref().expect("the new block is being edited");
        assert_eq!(ed.text(), "one\ntwo\nthree");
        code(&mut a, KeyCode::Esc);
        // The new block goes under the selection, so it is not necessarily last.
        let id = a
            .rows
            .iter()
            .find(|r| r.content.contains("one"))
            .map(|r| r.id)
            .expect("the pasted block");
        assert_eq!(
            a.db.block(id).map(|b| b.content),
            Some("one\ntwo\nthree".to_string())
        );
    }

    /// Ctrl-P while typing: commit and go, rather than doing nothing and taking
    /// the uncommitted line with it.
    #[test]
    fn app_keys_work_while_typing_and_commit_first() {
        let mut a = app("insert_ctrl_p");
        code(&mut a, KeyCode::Enter);
        let id = a.rows[0].id;
        key(&mut a, 'A'); // append at the end of the block's text
        for c in " tail".chars() {
            key(&mut a, c);
        }
        ctrl(&mut a, 'p');
        assert!(a.palette.is_some(), "Ctrl-P opened Find from INSERT");
        assert!(a.editor.is_none(), "and the buffer was committed, not dropped");
        let content = a.db.block(id).map(|b| b.content).unwrap_or_default();
        assert_eq!(content, "first block tail");

        // Ctrl-M is the panel toggle, and it commits too.
        code(&mut a, KeyCode::Esc);
        code(&mut a, KeyCode::Enter);
        key(&mut a, 'A');
        key(&mut a, '!');
        ctrl(&mut a, 'm');
        assert!(a.show_meta);
        assert_eq!(
            a.db.block(id).map(|b| b.content),
            Some("first block tail!".to_string())
        );
    }

    /// `Ctrl-P` while typing commits the buffer, and committing closes it. If
    /// `text_focus` survived that, the caret counted as being in text that no
    /// longer existed: the router reads the flag first, finds no buffer, and
    /// refuses every key that is not app-level. Find, Esc, and then nothing
    /// works -- not even Enter to get back into the block.
    #[test]
    fn committing_without_reopening_clears_the_caret_in_text() {
        let mut a = app("stale_focus");
        code(&mut a, KeyCode::Enter);
        let id = a.rows[0].id;
        key(&mut a, 'A');
        key(&mut a, '!');
        ctrl(&mut a, 'p');
        assert!(a.editor.is_none(), "Ctrl-P committed the buffer");
        assert!(!a.text_focus, "and there is no caret in any text now");
        code(&mut a, KeyCode::Esc); // close Find
        code(&mut a, KeyCode::Enter); // back into the block
        assert!(a.editor.is_some(), "Enter still opens the block");
        assert!(a.text_focus);
        // Normal mode in the text: `y` is an operator now, so type with `A`.
        key(&mut a, 'A');
        key(&mut a, 'y');
        assert!(
            a.editor.as_ref().unwrap().text().ends_with('y'),
            "and typing reaches the buffer again"
        );
        code(&mut a, KeyCode::Esc);
        assert!(a.db.block(id).map(|b| b.content).unwrap_or_default().contains('!'));
    }

    /// Report: "in normal mode, I cannot move carret forward to words position."
    ///
    /// In the *text* it already worked -- the caret moved, and the frame showed
    /// it moving. What did not work was `w` from the tree, where the caret is
    /// not in any words yet: it was "no mapping for w", which is exactly what
    /// the report describes. A word motion now enters the block and moves, so
    /// `w` is the one-key version of `Enter` then `w`.
    #[test]
    fn word_motions_work_from_the_tree() {
        let mut a = app("words_from_tree");
        assert!(!a.text_focus, "the caret starts in the tree");
        let id = a.rows[0].id;

        key(&mut a, 'w');
        assert!(a.text_focus, "w put the caret in the block");
        assert_eq!(a.selected_row().map(|r| r.id), Some(id), "same block");
        assert_eq!(
            a.editor.as_ref().unwrap().position(),
            (1, 7),
            "and moved to the next word, as w does from the start of a line"
        );

        // Backwards, and within the text, from either state.
        key(&mut a, 'b');
        assert_eq!(a.editor.as_ref().unwrap().position(), (1, 1));
        key(&mut a, 'e');
        assert_eq!(a.editor.as_ref().unwrap().position(), (1, 5));

        // One `Esc` leaves the text (the app's rule), and the word motions work
        // from the tree too -- so `e` on its own gets back in and lands on the
        // end of the first word.
        code(&mut a, KeyCode::Esc);
        assert!(!a.text_focus, "one Esc leaves the text");
        key(&mut a, 'e');
        assert!(a.text_focus, "and e comes back in");
        assert_eq!(
            a.editor.as_ref().unwrap().position(),
            (1, 5),
            "on the end of the first word"
        );
    }

    /// Vim's `w` at the end of a line moves to the next line. Here the next line
    /// is the next block, so word motions walk the outline one word at a time
    /// instead of stopping dead at the end of a block.
    #[test]
    fn word_motions_carry_on_into_the_next_block() {
        let mut a = app("words_across");
        key(&mut a, 'w'); // into block 1, on "block"
        assert_eq!(a.editor.as_ref().unwrap().position(), (1, 7));
        key(&mut a, 'w'); // the end of block 1's text, exactly as in vim
        assert_eq!(a.editor.as_ref().unwrap().position(), (1, 12));
        key(&mut a, 'w'); // ...and now it carries on into the block below
        assert_eq!(a.selected, 1, "w moved to the block below");
        assert!(a.text_focus);
        assert_eq!(
            a.editor.as_ref().unwrap().position(),
            (1, 1),
            "on the first word of that block"
        );

        // ...and `b` comes back to the last word of the block above.
        key(&mut a, 'b');
        assert_eq!(a.selected, 0, "b moved back up");
        assert_eq!(
            a.editor.as_ref().unwrap().position(),
            (1, 7),
            "on the last word of the block it left"
        );

        // The two blocks are still two blocks: this is navigation, not editing.
        assert_eq!(a.rows.len(), 2);
    }

    /// At the end of the outline there is nowhere to go, and vim stops at the
    /// end of the buffer rather than wrapping.
    #[test]
    fn word_motions_stop_at_the_end_of_the_outline() {
        let mut a = app("words_end");
        key(&mut a, 'k'); // first block
        key(&mut a, 'e');
        for _ in 0..40 {
            key(&mut a, 'w');
        }
        assert_eq!(a.selected, 1, "the last block");
        let last = a.editor.as_ref().unwrap().position();
        key(&mut a, 'w');
        assert_eq!(
            a.editor.as_ref().unwrap().position(),
            last,
            "and pressing w again does not wrap around"
        );
    }
}
