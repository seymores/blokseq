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
    App, ExState, Focus, InsertAt, LinkTarget, PopupState, SqlConsole, ToastKind, View, EX_COMMANDS,
};
use crate::editor::{Candidate, Mode, Trigger};
use crate::model::JournalDay;

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
            ed.mode = Mode::Insert;
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

    pub fn toggle_sidebar(&mut self) {
        self.show_sidebar = !self.show_sidebar;
        self.db.set_setting(
            "show_sidebar",
            if self.show_sidebar { "true" } else { "false" },
        );
        if !self.show_sidebar && self.focus == Focus::Sidebar {
            self.focus = Focus::Main;
        }
        let shown = self.show_sidebar;
        self.toast(
            ToastKind::Info,
            if shown {
                "sidebar shown"
            } else {
                "sidebar hidden — the outline gets the width"
            },
            Some("Ctrl-n toggles · :set sidebar / nosidebar"),
        );
    }

    pub fn toggle_refs(&mut self) {
        self.show_refs = !self.show_refs;
        self.db
            .set_setting("show_refs", if self.show_refs { "true" } else { "false" });
        if !self.show_refs && self.focus == Focus::Right {
            self.focus = Focus::Main;
        }
        self.toast(
            ToastKind::Info,
            if self.show_refs {
                "linked references shown"
            } else {
                "linked references hidden"
            },
            Some("Ctrl-b toggles · :set refs / norefs"),
        );
    }

    pub fn focus_left(&mut self) {
        self.focus = if self.focus == Focus::Main && self.show_sidebar {
            Focus::Sidebar
        } else {
            Focus::Main
        };
    }

    pub fn focus_right(&mut self) {
        self.focus = if self.focus == Focus::Main && self.show_refs {
            Focus::Right
        } else {
            Focus::Main
        };
    }

    pub fn focus_cycle(&mut self) {
        self.focus = match self.focus {
            Focus::Main if self.show_refs => Focus::Right,
            Focus::Main if self.show_sidebar => Focus::Sidebar,
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
                self.link_menu = links.clone();
                self.popup = Some(PopupState {
                    trigger: Trigger::Link,
                    query: String::new(),
                    selected: 0,
                    candidates: links
                        .iter()
                        .map(|l| Candidate {
                            label: l.label.clone(),
                            detail: match (&l.page, l.block) {
                                (Some(p), Some(b)) => format!("{} · {} · block #{}", l.kind, p, b),
                                (Some(p), None) => format!("{} · {}", l.kind, p),
                                _ => l.kind.to_string(),
                            },
                            kind: "link".into(),
                            match_at: None,
                        })
                        .collect(),
                });
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

    // ------------------------------------------------------------ jumplist

    pub fn push_history(&mut self) {
        let here = (self.view.clone(), self.selected_row().map(|r| r.id));
        if self.history.get(self.history_pos) == Some(&here) {
            return;
        }
        self.history.truncate(self.history_pos + 1);
        self.history.push(here);
        self.history_pos = self.history.len() - 1;
    }

    pub fn history_back(&mut self) {
        if self.history.is_empty() || self.history_pos == 0 {
            self.toast(ToastKind::Info, "start of the jump list", None);
            return;
        }
        self.history_pos -= 1;
        let (view, block) = self.history[self.history_pos].clone();
        self.go_history(view, block);
    }

    pub fn history_forward(&mut self) {
        if self.history_pos + 1 >= self.history.len() {
            self.toast(ToastKind::Info, "end of the jump list", None);
            return;
        }
        self.history_pos += 1;
        let (view, block) = self.history[self.history_pos].clone();
        self.go_history(view, block);
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
                } else if let Ok(day) = chrono::NaiveDate::parse_from_str(&arg, "%Y-%m-%d") {
                    self.goto_journal(JournalDay::new(day));
                } else if let Ok(offset) = arg.parse::<i64>() {
                    let day = JournalDay::new(self.today + chrono::Duration::days(offset));
                    self.goto_journal(day);
                } else {
                    self.goto_page(&arg);
                }
                true
            }
            "set" => {
                match arg.as_str() {
                    "sidebar" if !self.show_sidebar => self.toggle_sidebar(),
                    "nosidebar" if self.show_sidebar => self.toggle_sidebar(),
                    "refs" if !self.show_refs => self.toggle_refs(),
                    "norefs" if self.show_refs => self.toggle_refs(),
                    "sidebar" | "nosidebar" | "refs" | "norefs" => {
                        self.toast(ToastKind::Info, "already set that way", None)
                    }
                    _ => self.toast(
                        ToastKind::Warn,
                        "set: sidebar · nosidebar · refs · norefs",
                        None,
                    ),
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
        // Focus decides what the same keys mean, exactly like vim windows.
        if self.focus == Focus::Sidebar {
            match seq {
                "j" => {
                    self.sidebar_step(1);
                    return true;
                }
                "k" => {
                    self.sidebar_step(-1);
                    return true;
                }
                "<CR>" => {
                    let target = self
                        .sidebar
                        .get(self.sidebar_selected)
                        .and_then(|i| i.view.clone());
                    if let Some(v) = target {
                        self.push_history();
                        self.set_view(v);
                        self.focus = Focus::Main;
                    }
                    return true;
                }
                "q" | "<Esc>" | "h" => {
                    self.focus = Focus::Main;
                    return true;
                }
                _ => {}
            }
        }
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
        // With the caret inside a block, vim's text grammar applies first.
        // Anything it does not handle leaves the text and runs as a tree
        // command, which is what makes `j` (or any other motion) a one-key
        // escape from the block's text.
        if self.text_focus && self.text_op(seq) {
            return true;
        }
        self.leave_text();
        match seq {
            "j" => self.move_selection(1),
            "k" => self.move_selection(-1),
            "h" => self.go_parent(),
            "l" => self.go_first_child(),
            "gg" => self.jump(false),
            "G" => self.jump(true),
            "C-d" => self.move_selection(12),
            "C-u" => self.move_selection(-12),
            "C-n" => self.toggle_sidebar(),
            "C-b" => self.toggle_refs(),
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
            "C-wh" => self.focus_left(),
            "C-wl" => self.focus_right(),
            "C-ww" => self.focus_cycle(),
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
            "[" => self.shift_journal(-1),
            "]" => self.shift_journal(1),
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
    fn text_op(&mut self, seq: &str) -> bool {
        if self.editor.is_none() {
            return false;
        }
        let mut start_inserting = false;
        let mut drop_caret = false;
        let mut quit = false;
        {
            let ed = self.editor.as_mut().unwrap();
            match seq {
                "h" => ed.left(),
                "l" => ed.right(),
                "w" => ed.word_right(),
                "b" => ed.word_left(),
                "e" => ed.word_end(),
                "0" => ed.home(),
                "^" => ed.first_non_blank(),
                "$" => ed.end(),
                "x" => ed.delete_char(),
                "D" | "d$" => ed.delete_to_end(),
                "dw" => ed.delete_word(),
                "d0" => {
                    let mut home = ed.cursor;
                    while home > 0 && ed.chars[home - 1] != '\n' {
                        home -= 1;
                    }
                    ed.chars.drain(home..ed.cursor);
                    ed.cursor = home;
                    ed.dirty = true;
                }
                "cw" | "ciw" => {
                    ed.change_word();
                    start_inserting = true;
                }
                "C" => {
                    ed.change_to_end();
                    start_inserting = true;
                }
                "i" => start_inserting = true,
                "a" => {
                    ed.right();
                    start_inserting = true;
                }
                "I" => {
                    ed.home();                    start_inserting = true;
                }
                "A" => {
                    ed.end();
                    start_inserting = true;
                }
                // In a block, Enter means "start typing here" -- `o` is the
                // key that opens a new block, and the hint bar says so.
                "<CR>" => start_inserting = true,
                "<Esc>" => drop_caret = true,
                // Same as everywhere in Normal mode: `q` quits.
                "q" => quit = true,
                _ => return false,
            }
        }
        if quit {
            self.quit_now(true, false);
            return true;
        }
        if start_inserting {
            if let Some(ed) = self.editor.as_mut() {
                ed.mode = Mode::Insert;
            }
            self.mode = Mode::Insert;
        }
        if drop_caret {
            self.leave_text();
        }
        true
    }

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
    fn ctrl_p_opens_any_page() {
        let mut a = app("page_picker");
        ctrl(&mut a, 'p');
        assert!(a.palette.is_some(), "Ctrl-P opens the page picker");
        // typing filters
        for c in "Test".chars() {
            key(&mut a, c);
        }
        let filtered = a.palette.as_ref().map(|p| p.filtered.len()).unwrap_or(0);
        assert!(filtered >= 1, "the query filters the list");
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.view, View::Page("Test Page".into()));
    }

    #[test]
    fn ctrl_w_h_reaches_the_sidebar_and_enter_opens() {
        let mut a = app("sidebar_focus");
        assert!(a.show_sidebar);
        ctrl(&mut a, 'w');
        key(&mut a, 'h');
        assert_eq!(a.focus, Focus::Sidebar, "Ctrl-w h focuses the pages panel");
        // j/k move the selection, Enter opens whatever is selected
        key(&mut a, 'j');
        code(&mut a, KeyCode::Enter);
        assert_eq!(a.focus, Focus::Main, "Enter returns to the blocks");
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
    fn panels_toggle_and_persist() {
        let mut a = app("panels");
        assert!(a.show_sidebar && a.show_refs);
        ctrl(&mut a, 'n');
        ctrl(&mut a, 'b');
        assert!(!a.show_sidebar && !a.show_refs);
        assert_eq!(a.db.get_setting("show_sidebar").as_deref(), Some("false"));
        assert_eq!(a.db.get_setting("show_refs").as_deref(), Some("false"));
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
}
