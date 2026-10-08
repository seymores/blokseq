//! Application state and the action layer. Key handling and the mockup fixtures
//! both go through the same methods, so a screenshot can never show a state the
//! interactive app cannot reach.

use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::db::Db;
use crate::editor::{fuzzy, Candidate, Editor, Mode, Trigger};
use crate::model::*;
use crate::theme;

#[derive(Clone, PartialEq, Debug)]
pub enum View {
    Journal(JournalDay),
    Page(String),
    Search,
    Query,
    Backup,
    Help,
    /// The read-only troubleshooting console (`:sql`).
    Sql,
}

/// Vim's `:` command line. One line at the bottom of the screen, with the
/// matching command names offered as you type.
#[derive(Clone, Debug, Default)]
pub struct ExState {
    pub input: String,
    pub selected: usize,
    pub message: Option<String>,
}

/// The troubleshooting console: a read-only SQL prompt. This is where the
/// storage internals live, instead of in the status bar where they were noise.
#[derive(Clone, Debug, Default)]
pub struct SqlConsole {
    pub input: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub message: String,
    pub history: Vec<String>,
    pub history_pos: Option<usize>,
}

/// A link inside a block, resolved to something we can navigate to.
#[derive(Clone, Debug)]
pub struct LinkTarget {
    pub kind: &'static str,
    pub label: String,
    pub page: Option<String>,
    pub block: Option<i64>,
    /// Character offset of the link inside the block, for the menu.
    pub at: usize,
}

/// Where `i` / `a` / `A` put the cursor.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum InsertAt {
    Start,
    After,
    End,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Focus {
    Sidebar,
    Main,
    Right,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ToastKind {
    Info,
    Good,
    Warn,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub sub: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ItemKind {
    Section,
    Journal,
    Page,
    Favorite,
}

#[derive(Clone, Debug)]
pub struct SidebarItem {
    pub kind: ItemKind,
    pub label: String,
    pub badge: String,
    pub view: Option<View>,
    pub today: bool,
    pub provisional: bool,
}

#[derive(Clone, Debug)]
pub struct PopupState {
    pub trigger: Trigger,
    pub query: String,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
}

#[derive(Clone, Debug)]
pub struct Palette {
    pub query: String,
    pub all: Vec<(String, String, String)>, // label, detail, action
    pub filtered: Vec<usize>,
    pub selected: usize,
}

#[derive(Clone, Debug)]
pub enum UndoOp {
    Content {
        id: i64,
        old: String,
        new: String,
    },
    Created {
        id: i64,
        page_id: i64,
        parent: Option<i64>,
        content: String,
    },
    Deleted {
        id: i64,
        page_id: i64,
        parent: Option<i64>,
        position: f64,
        content: String,
        subtree: Vec<(usize, String, Option<String>)>,
    },
}

pub struct App {
    pub db: Db,
    pub today: NaiveDate,
    pub view: View,
    pub focus: Focus,
    pub mode: Mode,
    pub quit: bool,

    pub page_id: Option<i64>,
    /// A journal day with no row in the database yet.
    pub provisional: bool,
    pub rows: Vec<Row>,
    pub selected: usize,
    pub scroll: u16,
    pub editor: Option<Editor>,
    pub popup: Option<PopupState>,
    pub palette: Option<Palette>,
    /// The vim command line, when it is open.
    pub ex: Option<ExState>,
    /// The troubleshooting console, when the Sql view has one.
    pub sql: Option<SqlConsole>,
    /// True when the Normal-mode cursor sits *inside* the selected block's text
    /// (reached with Enter, or left behind by `Esc` from Insert). A cursor
    /// position, not a mode: any tree motion leaves it, so nobody gets stuck one
    /// `Esc` away from the block list.
    pub text_focus: bool,
    /// Accumulates a partial vim command (`g`, `d`, `z`, `>`, `<`, `c`).
    pub pending: String,
    /// In-app register for `yy` / `dd` + `p` / `P`.
    pub register: Vec<(usize, String, Option<String>)>,
    pub register_label: String,
    /// The destinations offered by the `Ctrl-]` menu.
    pub link_menu: Vec<LinkTarget>,
    /// `:q!` skips the prune pass on the way out.
    pub prune_on_quit: bool,
    pub visual: Option<usize>,    /// Jump list: `Ctrl-]` pushes, `Ctrl-o` / `Ctrl-i` walk it.
    pub history: Vec<(View, Option<i64>)>,
    pub history_pos: usize,
    pub undo: Vec<UndoOp>,
    pub redo: Vec<UndoOp>,

    /// Panels are furniture only if you want them. Both persist in `settings`.
    pub show_sidebar: bool,
    pub show_refs: bool,

    pub sidebar: Vec<SidebarItem>,
    pub sidebar_selected: usize,
    pub sidebar_scroll: usize,
    pub right_selected: usize,
    pub linked: Vec<(String, Vec<RefHit>)>,
    pub linked_selected: usize,

    pub search_query: String,
    pub search_results: Vec<RefHit>,
    pub search_selected: usize,
    /// Board cursor: which status column, and which row in it.
    pub query_col: usize,
    pub query_row: usize,
    /// Blocks that mention the page title without linking it.
    pub unlinked: Vec<RefHit>,
    pub prev_view: Option<View>,

    pub toast: Option<Toast>,
    pub last_backup: Option<String>,
    pub pruned: Vec<String>,
    pub pruned_blocks: i64,
    pub upload_desc: Option<String>,
    pub integrity: String,

    /// Mockup dumps set this so the caret survives a screenshot.
    pub fake_caret: bool,
    /// Mockup dumps use a fixed clock string.
    pub clock: String,
    pub journal_day: JournalDay,
}

impl App {
    pub fn new(db: Db, today: NaiveDate) -> Self {
        // Panel visibility is a preference, not a hard-coded layout.
        let show_sidebar = db.bool_setting("show_sidebar", true);
        let show_refs = db.bool_setting("show_refs", true);
        let mut app = Self {
            db,
            today,
            view: View::Journal(JournalDay::new(today)),
            focus: Focus::Main,
            mode: Mode::Normal,
            quit: false,
            page_id: None,
            provisional: false,
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            editor: None,
            popup: None,
            palette: None,
            ex: None,
            sql: None,
            text_focus: false,
            pending: String::new(),
            register: Vec::new(),
            register_label: String::new(),
            link_menu: Vec::new(),
            prune_on_quit: true,
            visual: None,
            history: Vec::new(),
            history_pos: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            show_sidebar,
            show_refs,
            sidebar: Vec::new(),
            sidebar_selected: 0,
            sidebar_scroll: 0,
            right_selected: 0,
            linked: Vec::new(),
            linked_selected: 0,
            search_query: String::new(),
            search_results: Vec::new(),
            search_selected: 0,
            query_col: 0,
            query_row: 0,
            unlinked: Vec::new(),
            prev_view: None,
            toast: None,
            last_backup: None,
            pruned: Vec::new(),
            pruned_blocks: 0,
            upload_desc: None,
            integrity: String::new(),
            fake_caret: false,
            clock: chrono::Local::now().format("%H:%M").to_string(),
            journal_day: JournalDay::new(today),
        };
        app.reload();
        app
    }

    // ------------------------------------------------------------- loading

    pub fn reload(&mut self) {
        self.build_sidebar();
        self.load_view();
        self.load_right();
    }

    pub fn load_view(&mut self) {
        self.linked.clear();
        match self.view.clone() {
            View::Journal(day) => {
                self.journal_day = day;
                match self.db.journal_page(day) {
                    Some(p) => {
                        self.page_id = Some(p.id);
                        self.provisional = false;
                        self.rows = self.db.rows(p.id);
                    }
                    None => {
                        self.page_id = None;
                        self.provisional = true;
                        self.rows = Vec::new();
                    }
                }
            }
            View::Page(name) => {
                let p = self.db.ensure_page(&name, PageKind::Page);
                self.page_id = Some(p.id);
                self.provisional = false;
                self.rows = self.db.rows(p.id);
            }
            View::Search | View::Query | View::Backup | View::Help | View::Sql => {}
        }
        self.clamp_selection();
    }

    pub fn load_right(&mut self) {
        self.linked = match self.page_id {
            Some(id) => self.db.linked_refs(id),
            None => Vec::new(),
        };
        self.linked_selected = 0;
        // Unlinked references: blocks that mention the title as plain text.
        self.unlinked = match &self.view {
            View::Page(name) => {
                let linked: Vec<i64> = self
                    .linked
                    .iter()
                    .flat_map(|(_, v)| v.iter().map(|h| h.block_id))
                    .collect();
                self.db
                    .search(name, 12)
                    .into_iter()
                    .filter(|h| !linked.contains(&h.block_id) && h.content.contains(name.as_str()))
                    .collect()
            }
            _ => Vec::new(),
        };
    }

    /// Live query for the board view.
    pub fn by_status(&self, status: &str) -> Vec<RefHit> {
        self.db.by_status(status)
    }

    pub fn go_back(&mut self) {
        match self.prev_view.take() {
            Some(v) => {
                self.view = v;
                self.load_view();
                self.load_right();
            }
            None => self.goto_today(),
        }
    }

    fn clamp_selection(&mut self) {
        if self.rows.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.rows.len() {
            self.selected = self.rows.len() - 1;
        }
    }

    pub fn build_sidebar(&mut self) {
        let mut items: Vec<SidebarItem> = Vec::new();
        items.push(SidebarItem {
            kind: ItemKind::Section,
            label: "TODAY".into(),
            badge: String::new(),
            view: None,
            today: false,
            provisional: false,
        });
        let today = JournalDay::new(self.today);
        let jp = self.db.journal_page(today);
        items.push(SidebarItem {
            kind: ItemKind::Favorite,
            label: today.title(),
            badge: match &jp {
                Some(_) => {
                    let n = self.db.rows(jp.as_ref().unwrap().id).len();
                    if n == 0 {
                        "empty · pruned".into()
                    } else {
                        format!("{} blocks", n)
                    }
                }
                None => "provisional".into(),
            },
            view: Some(View::Journal(today)),
            today: true,
            provisional: jp.is_none(),
        });
        items.push(SidebarItem {
            kind: ItemKind::Section,
            label: "JOURNALS".into(),
            badge: String::new(),
            view: None,
            today: false,
            provisional: false,
        });
        for (day, n, _preview) in self.db.journals(14) {
            if day.date == self.today {
                continue;
            }
            items.push(SidebarItem {
                kind: ItemKind::Journal,
                label: day.relative(self.today),
                badge: format!("{} · {}", day.short(), n),
                view: Some(View::Journal(day)),
                today: false,
                provisional: false,
            });
        }
        items.push(SidebarItem {
            kind: ItemKind::Section,
            label: "PAGES".into(),
            badge: String::new(),
            view: None,
            today: false,
            provisional: false,
        });
        for (name, links, blocks) in self.db.pages(40) {
            items.push(SidebarItem {
                kind: ItemKind::Page,
                label: name,
                badge: if links > 0 {
                    format!("{}↗ · {}b", links, blocks)
                } else {
                    format!("{}b", blocks)
                },
                view: Some(View::Page(String::new())),
                today: false,
                provisional: false,
            });
        }
        // The Page items above carry a placeholder view; fill real names in a
        // second pass so we keep the query simple.
        let pages = self.db.pages(40);
        let mut pi = 0usize;
        for it in items.iter_mut() {
            if it.kind == ItemKind::Page {
                if let Some((name, _, _)) = pages.get(pi) {
                    it.view = Some(View::Page(name.clone()));
                }
                pi += 1;
            }
        }
        self.sidebar = items;
        self.sidebar_clamp();
    }

    fn sidebar_clamp(&mut self) {
        let sel = self
            .sidebar
            .iter()
            .enumerate()
            .filter(|(_, i)| i.kind != ItemKind::Section)
            .map(|(idx, _)| idx)
            .collect::<Vec<_>>();
        if sel.is_empty() {
            self.sidebar_selected = 0;
            return;
        }
        if !sel.contains(&self.sidebar_selected) {
            // snap to the closest selectable entry
            self.sidebar_selected = sel
                .iter()
                .copied()
                .min_by_key(|i| (*i as i64 - self.sidebar_selected as i64).abs())
                .unwrap_or(sel[0]);
        }
    }

    // ------------------------------------------------------------- view nav

    pub fn set_view(&mut self, v: View) {
        if v != self.view && !matches!(v, View::Help) {
            self.prev_view = Some(self.view.clone());
        }
        self.view = v;
        self.selected = 0;
        self.scroll = 0;
        self.visual = None;
        self.commit_edit();
        self.load_view();
        self.load_right();
    }

    pub fn goto_journal(&mut self, day: JournalDay) {
        self.set_view(View::Journal(day));
        self.focus = Focus::Main;
    }

    pub fn goto_page(&mut self, name: &str) {
        self.set_view(View::Page(name.to_string()));
        self.focus = Focus::Main;
    }

    pub fn goto_today(&mut self) {
        self.goto_journal(JournalDay::new(self.today));
    }

    pub fn shift_journal(&mut self, days: i64) {
        let d = match self.view.clone() {
            View::Journal(day) => day.date + chrono::Duration::days(days),
            _ => self.today + chrono::Duration::days(days),
        };
        self.goto_journal(JournalDay::new(d));
        self.toast(
            ToastKind::Info,
            &JournalDay::new(d).title(),
            Some("journal navigation · ← / →"),
        );
    }

    // ------------------------------------------------------------ selection

    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }

    pub fn select_block(&mut self, id: i64) {
        if let Some(i) = self.rows.iter().position(|r| r.id == id) {
            self.selected = i;
        }
    }

    pub fn move_selection(&mut self, delta: i64) {
        if self.rows.is_empty() {
            return;
        }
        let n = self.rows.len() as i64;
        let mut i = self.selected as i64 + delta;
        // skip rows hidden inside a collapsed ancestor
        if delta > 0 {
            while i < n && self.rows[i as usize].depth > self.rows[self.selected].depth {
                i += 1;
            }
        }
        self.selected = i.clamp(0, n - 1) as usize;
        self.scroll_to_selection();
    }

    pub fn jump(&mut self, to_end: bool) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = if to_end { self.rows.len() - 1 } else { 0 };
        self.scroll_to_selection();
    }

    pub fn scroll_to_selection(&mut self) {
        let vis = self.viewport_rows();
        let sel = self.selected as u16;
        if sel < self.scroll {
            self.scroll = sel;
        } else if sel >= self.scroll + vis {
            self.scroll = sel + 1 - vis;
        }
    }

    pub fn viewport_rows(&self) -> u16 {
        22
    }

    // -------------------------------------------------------------- editing

    pub fn begin_edit(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        self.begin_edit_block(row.id);
    }

    /// Start typing in a journal day that has no row yet. Nothing is written
    /// until the first keystroke is committed: the day is a view, not a file.
    pub fn begin_provisional(&mut self) {
        let mut ed = Editor::new(-1, "", "", 0);
        ed.fake_caret = self.fake_caret;
        self.editor = Some(ed);
        self.mode = Mode::Insert;
    }

    pub fn begin_edit_block(&mut self, id: i64) {
        if let Some(i) = self.rows.iter().position(|r| r.id == id) {
            self.selected = i;
        }
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        let mut ed = Editor::new(row.id, &row.uuid, &row.content, row.depth);
        ed.fake_caret = self.fake_caret;
        self.editor = Some(ed);
        self.mode = Mode::Insert;
    }

    /// Insert a fresh block underneath and start typing in it.
    pub fn new_block_below(&mut self) {
        let Some(page_id) = self.ensure_current_page() else {
            return;
        };
        let sel_id = self.selected_row().map(|r| r.id);
        let (parent, after) = match sel_id {
            Some(id) => {
                let b = self.db.block(id);
                match b {
                    Some(b) => (b.parent_id, Some(id)),
                    None => (None, None),
                }
            }
            None => (None, None),
        };
        let nb = self.db.create_block(page_id, parent, after, "");
        self.reload();
        self.select_block(nb.id);
        self.begin_edit_block(nb.id);
        self.scroll_to_selection();
    }

    /// Lazily materialise the page row. Used on the first keystroke of a
    /// provisional journal -- the file (and the row) appears only now.
    pub fn ensure_current_page(&mut self) -> Option<i64> {
        if let Some(id) = self.page_id {
            return Some(id);
        }
        match self.view.clone() {
            View::Journal(day) => {
                let p = self.db.ensure_journal(day);
                self.page_id = Some(p.id);
                self.provisional = false;
                self.toast(
                    ToastKind::Good,
                    &format!("{} created in blok.db", day.key()),
                    Some("journal page was provisional until this first block"),
                );
                Some(p.id)
            }
            View::Page(name) => {
                let p = self.db.ensure_page(&name, PageKind::Page);
                self.page_id = Some(p.id);
                Some(p.id)
            }
            _ => None,
        }
    }

    /// Write the editor buffer back. `stay` keeps insert mode for a chain of
    /// typists; otherwise we drop to normal mode on the same block.
    pub fn commit_edit(&mut self) {
        let Some(ed) = self.editor.take() else { return };
        if !ed.dirty {
            self.mode = Mode::Normal;
            self.popup = None;
            return;
        }
        let id = ed.block_id;
        let text = ed.text();
        let page_id = self.ensure_current_page();
        if id < 0 {
            // first block of a day that had no row: this is the moment the
            // journal starts to exist.
            if text.trim().is_empty() {
                self.mode = Mode::Normal;
                self.popup = None;
                return;
            }
            if let Some(pid) = page_id {
                let after = self.rows.last().map(|r| r.id);
                let nb = self.db.create_block(pid, None, after, &text);
                self.undo.push(UndoOp::Created {
                    id: nb.id,
                    page_id: pid,
                    parent: None,
                    content: text.clone(),
                });
                self.redo.clear();
                self.text_focus = true;
                self.mode = Mode::Normal;
                self.popup = None;
                self.reload();
                self.select_block(nb.id);
                self.begin_edit_block(nb.id);
                if let Some(ed) = self.editor.as_mut() {
                    ed.cursor = ed.chars.len();
                }
            }
            return;
        }
        let old = self.db.block(id).map(|b| b.content).unwrap_or_default();
        self.db.update_content(id, &text);
        self.undo.push(UndoOp::Content {
            id,
            old,
            new: text,
        });
        self.mode = Mode::Normal;
        self.popup = None;
        self.reload();
        self.select_block(id);
    }

    pub fn undo(&mut self) {
        let Some(op) = self.undo.pop() else {
            self.toast(ToastKind::Warn, "nothing to undo", None);
            return;
        };
        match op.clone() {
            UndoOp::Content { id, old, .. } => {
                self.db.update_content(id, &old);
                self.toast(ToastKind::Info, "undo · block text restored", Some("u / Ctrl-r"));
            }
            UndoOp::Created { id, .. } => {
                self.db.delete_block(id);
                self.toast(ToastKind::Info, "undo · block removed", Some("u / Ctrl-r"));
            }
            UndoOp::Deleted { id, .. } => {
                self.db.restore_block(id);
                self.toast(ToastKind::Info, "undo · block restored", Some("u / Ctrl-r"));
            }
        }
        self.redo.push(op);
        self.reload();
    }

    pub fn redo(&mut self) {
        let Some(op) = self.redo.pop() else {
            self.toast(ToastKind::Warn, "nothing to redo", None);
            return;
        };
        match op.clone() {
            UndoOp::Content { id, new, .. } => {
                self.db.update_content(id, &new);
                self.toast(ToastKind::Info, "redo · block text", Some("Ctrl-r"));
            }
            UndoOp::Created {
                id,
                page_id,
                parent,
                content,
            } => {
                self.db.restore_block_with(id, page_id, parent, &content);
                self.toast(ToastKind::Info, "redo · block re-created", Some("Ctrl-r"));
            }
            UndoOp::Deleted { id, .. } => {
                self.db.delete_block(id);
                self.toast(ToastKind::Info, "redo · block deleted again", Some("Ctrl-r"));
            }
        }
        self.undo.push(op);
        self.reload();
    }

    pub fn indent_selected(&mut self) {
        let ids: Vec<i64> = match self.visual {
            Some(anchor) => self.range_ids(anchor, self.selected),
            None => self.selected_row().map(|r| vec![r.id]).unwrap_or_default(),
        };
        for id in &ids {
            self.db.indent(*id);
        }
        self.reload();
        if let Some(last) = ids.last() {
            self.select_block(*last);
        }
        self.toast(
            ToastKind::Info,
            &format!("indented {} block{}", ids.len(), plural(ids.len())),
            Some("Tab · Shift-Tab to outdent"),
        );
    }

    pub fn outdent_selected(&mut self) {
        let ids: Vec<i64> = match self.visual {
            Some(anchor) => self.range_ids(anchor, self.selected),
            None => self.selected_row().map(|r| vec![r.id]).unwrap_or_default(),
        };
        for id in &ids {
            self.db.outdent(*id);
        }
        self.reload();
        if let Some(last) = ids.last() {
            self.select_block(*last);
        }
    }

    fn range_ids(&self, a: usize, b: usize) -> Vec<i64> {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        self.rows[lo..=hi.min(self.rows.len().saturating_sub(1))]
            .iter()
            .map(|r| r.id)
            .collect()
    }

    pub fn toggle_collapse(&mut self) {
        if let Some(row) = self.selected_row().cloned() {
            if row.has_children {
                self.db.set_collapsed(row.id, !row.collapsed);
                self.reload();
                self.select_block(row.id);
            }
        }
    }

    pub fn collapse_all(&mut self) {
        if let Some(pid) = self.page_id {
            let _ = self
                .db
                .conn
                .execute("UPDATE blocks SET collapsed = 1 WHERE page_id = ?1", [pid]);
            self.reload();
            self.toast(ToastKind::Info, "folded every block", Some("z"));
        }
    }

    /// Ctrl-Enter. Logseq semantics: the marker is part of the block text, and
    /// `blocks.status` is a mirror of it for querying.
    pub fn cycle_status(&mut self) {
        if let Some(row) = self.selected_row().cloned() {
            let (_, body) = crate::model::split_status(&row.content);
            let next = theme::next_status(row.status.as_deref());
            let text = match next {
                Some(s) => {
                    if body.is_empty() {
                        s.to_string()
                    } else {
                        format!("{} {}", s, body)
                    }
                }
                None => body,
            };
            self.db.update_content(row.id, &text);
            self.reload();
            self.select_block(row.id);
            self.toast(
                ToastKind::Info,
                &format!("status → {}", next.unwrap_or("(cleared)")),
                Some("the marker is written into the block, and mirrored to blocks.status"),
            );
        }
    }

    pub fn set_status_named(&mut self, status: &str) {
        if let Some(row) = self.selected_row().cloned() {
            self.db.set_status(row.id, Some(status));
            self.reload();
            self.select_block(row.id);
        }
    }

    pub fn move_selected_block(&mut self, up: bool) {
        if let Some(row) = self.selected_row().cloned() {
            if self.db.move_block(row.id, up) {
                self.reload();
                self.select_block(row.id);
                self.toast(
                    ToastKind::Info,
                    "block reordered",
                    Some("Alt-↑ / Alt-↓ · position is a fractional index"),
                );
            }
        }
    }

    pub fn delete_selected(&mut self) {
        let ids = match self.visual {
            Some(anchor) => self.range_ids(anchor, self.selected),
            None => self.selected_row().map(|r| vec![r.id]).unwrap_or_default(),
        };
        if ids.is_empty() {
            return;
        }
        for id in &ids {
            // Capture enough to bring it back on `u`.
            if let Some(b) = self.db.block(*id) {
                let subtree = self.db.subtree(*id);
                self.undo.push(UndoOp::Deleted {
                    id: *id,
                    page_id: b.page_id,
                    parent: b.parent_id,
                    position: b.position,
                    content: b.content.clone(),
                    subtree,
                });
            }
            self.db.delete_block(*id);
        }
        self.redo.clear();
        self.visual = None;
        self.reload();
        self.toast(
            ToastKind::Warn,
            &format!("deleted {} block{}", ids.len(), plural(ids.len())),
            Some("soft delete · u to undo · Ctrl-r to redo"),
        );
    }

    /// Backspace on an empty block pulls the text of the previous sibling up.
    pub fn merge_up(&mut self) {
        let Some(row) = self.selected_row().cloned() else {
            return;
        };
        match self.db.merge_into_previous(row.id) {
            Some(prev) => {
                self.reload();
                self.select_block(prev);
                self.toast(ToastKind::Info, "block merged upward", Some("Backspace at start of block"));
            }
            None => self.toast(ToastKind::Warn, "no previous sibling to merge into", None),
        }
    }

    pub fn insert_soft_newline(&mut self) {
        if let Some(ed) = self.editor.as_mut() {
            ed.insert_char('\n');
        }
    }

    // --------------------------------------------------------------- popups

    pub fn refresh_popup(&mut self) {
        let Some(ed) = self.editor.as_ref() else {
            self.popup = None;
            return;
        };
        let Some((trigger, query)) = ed.trigger() else {
            self.popup = None;
            return;
        };
        let candidates = self.candidates_for(trigger.clone(), &query);
        let selected = self.popup.as_ref().map(|p| p.selected).unwrap_or(0);
        self.popup = Some(PopupState {
            trigger,
            query,
            selected: selected.min(candidates.len().saturating_sub(1)),
            candidates,
        });
    }

    fn candidates_for(&self, trigger: Trigger, query: &str) -> Vec<Candidate> {
        let mut scored: Vec<(i32, Candidate)> = Vec::new();
        let mut push = |label: String, detail: String, kind: &str| {
            if let Some((score, _)) = fuzzy(query, &label) {
                scored.push((
                    score,
                    Candidate {
                        match_at: label.to_lowercase().find(&query.to_lowercase()),
                        label,
                        detail,
                        kind: kind.to_string(),
                    },
                ));
            }
        };
        match trigger {
            Trigger::Slash => {
                for (cmd, desc) in SLASH_COMMANDS {
                    push(cmd.to_string(), desc.to_string(), "command");
                }
            }
            Trigger::PageLink | Trigger::Tag => {
                for (name, links, blocks) in self.db.pages(200) {
                    push(name, format!("{} links · {} blocks", links, blocks), "page");
                }
                for (day, n, _) in self.db.journals(30) {
                    push(
                        day.key(),
                        format!("journal · {} blocks", n),
                        "journal",
                    );
                }
            }
            Trigger::Link => {
                // The `Ctrl-]` menu: the destinations already in this block.
                for l in self.link_menu.clone() {
                    push(l.label.clone(), l.kind.to_string(), "link");
                }
            }
            Trigger::BlockRef => {
                for (page, hits) in self.recent_blocks() {
                    for h in hits {
                        let snippet = snippet(&h.content, 52);
                        push(snippet, page.clone(), "block");
                    }
                }
            }
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().take(8).map(|(_, c)| c).collect()
    }

    fn recent_blocks(&self) -> Vec<(String, Vec<RefHit>)> {
        let mut stmt = self
            .db
            .conn
            .prepare(
                "SELECT b.id, b.content, p.name, p.kind FROM blocks b JOIN pages p ON p.id = b.page_id
                  WHERE b.deleted_at IS NULL AND TRIM(b.content) <> ''
                  ORDER BY b.updated_at DESC LIMIT 40",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap();
        let mut out: Vec<(String, Vec<RefHit>)> = Vec::new();
        for (id, content, page, kind) in rows.filter_map(|r| r.ok()) {
            let hit = RefHit {
                block_id: id,
                page: page.clone(),
                is_journal: kind == "journal",
                content,
                kind: "block".into(),
            };
            match out.last_mut() {
                Some((p, v)) if *p == page => v.push(hit),
                _ => out.push((page, vec![hit])),
            }
        }
        out
    }

    pub fn accept_popup(&mut self) {
        let Some(popup) = self.popup.clone() else {
            return;
        };
        let Some(cand) = popup.candidates.get(popup.selected).cloned() else {
            return;
        };
        let value = match popup.trigger {
            Trigger::PageLink => format!("[[{}]]", cand.label),
            Trigger::BlockRef => {
                // resolve the snippet back to a uuid
                let uuid = self
                    .recent_blocks()
                    .into_iter()
                    .flat_map(|(_, v)| v)
                    .find(|h| snippet(&h.content, 52) == cand.label)
                    .and_then(|h| self.db.block(h.block_id).map(|b| b.uuid))
                    .unwrap_or_else(|| "blk-00000000".into());
                format!("(({}))", uuid)
            }
            _ => cand.label.clone(),
        };
        if let Some(ed) = self.editor.as_mut() {
            ed.complete(popup.trigger, &value);
        }
        self.popup = None;
    }

    pub fn popup_move(&mut self, delta: i64) {
        if let Some(p) = self.popup.as_mut() {
            let n = p.candidates.len() as i64;
            if n > 0 {
                p.selected = ((p.selected as i64 + delta).rem_euclid(n)) as usize;
            }
        }
    }

    // -------------------------------------------------------------- palette

    /// `Ctrl-P`: the open-page picker. Pages and journals, most useful first --
    /// this is the answer to "how do I get to another page", which no longer
    /// depends on finding the right pane first. Commands live in `:`.
    pub fn open_palette(&mut self) {
        let mut all: Vec<(String, String, String)> = Vec::new();
        let today = JournalDay::new(self.today);
        all.push((
            format!("{}  (today)", today.key()),
            "journal".into(),
            format!("journal:{}", today.key()),
        ));
        for (day, blocks, _preview) in self.db.journals(30) {
            if day.date == self.today {
                continue;
            }
            all.push((
                day.key(),
                format!("journal · {} · {} blocks", day.relative(self.today), blocks),
                format!("journal:{}", day.key()),
            ));
        }
        for (name, links, blocks) in self.db.pages(200) {
            let action = format!("page:{}", name);
            all.push((
                name,
                format!("page · {} blocks · {} links", blocks, links),
                action,
            ));
        }
        let mut p = Palette {
            query: String::new(),
            all,
            filtered: Vec::new(),
            selected: 0,
        };
        p.filtered = (0..p.all.len()).collect();
        self.palette = Some(p);
    }

    pub fn palette_input(&mut self, c: char) {
        if let Some(p) = self.palette.as_mut() {
            p.query.push(c);
            p.refilter();
        }
    }

    pub fn palette_backspace(&mut self) {
        if let Some(p) = self.palette.as_mut() {
            p.query.pop();
            p.refilter();
        }
    }

    pub fn palette_move(&mut self, delta: i64) {
        if let Some(p) = self.palette.as_mut() {
            let n = p.filtered.len() as i64;
            if n > 0 {
                p.selected = ((p.selected as i64 + delta).rem_euclid(n)) as usize;
            }
        }
    }

    pub fn palette_accept(&mut self) {
        let Some(p) = self.palette.clone() else { return };
        let Some(idx) = p.filtered.get(p.selected) else {
            self.palette = None;
            return;
        };
        let action = p.all[*idx].2.clone();
        let label = p.all[*idx].0.clone();
        self.palette = None;
        if let Some(key) = action.strip_prefix("journal:") {
            match crate::model::parse_journal_key(key) {
                Some(day) => {
                    self.push_history();
                    self.goto_journal(day);
                }
                None => self.toast(ToastKind::Warn, "not a journal day", None),
            }
            return;
        }
        if let Some(name) = action.strip_prefix("page:") {
            self.push_history();
            self.goto_page(name);
            let _ = label;
            return;
        }
        self.toast(ToastKind::Warn, &format!("nothing to open for {}", label), None);
    }

    // --------------------------------------------------------------- search

    pub fn run_search(&mut self) {
        self.search_results = self.db.search(&self.search_query, 60);
        self.search_selected = 0;
    }

    // --------------------------------------------------------------- backup

    pub fn do_backup(&mut self) {
        let dir = self
            .db
            .path
            .parent()
            .map(|p| p.join("snapshots"))
            .unwrap_or_else(|| std::path::PathBuf::from("snapshots"));
        let label = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
        let (file, bytes) = self.db.backup_snapshot(&dir, &label);
        let desc = self
            .db
            .queue_remote_upload(&file, &dir.join("queue"));
        self.last_backup = Some(file.clone());
        self.upload_desc = Some(desc);
        self.integrity = self.db.integrity_check();
        self.db.refresh_stats();
        self.toast(
            ToastKind::Good,
            &format!(
                "snapshot {} → {}",
                crate::db::human_bytes(bytes),
                self.db.remote_target()
            ),
            Some("VACUUM INTO + rclone copy --checksum"),
        );
    }

    pub fn prune_journals(&mut self) {
        self.pruned_blocks = self.db.empty_block_count();
        self.pruned = self.db.prune_empty_journals();
        self.db.refresh_stats();
        let n = self.pruned.len();
        self.toast(
            ToastKind::Warn,
            &format!("pruned {} empty journal page{}", n, plural(n)),
            Some("a journal only exists once it has content"),
        );
    }

    pub fn toast(&mut self, kind: ToastKind, text: &str, sub: Option<&str>) {
        self.toast = Some(Toast {
            text: text.to_string(),
            kind,
            sub: sub.map(|s| s.to_string()),
        });
    }

    pub fn dismiss_toast(&mut self) {
        self.toast = None;
    }

    // ----------------------------------------------------------- key router

    pub fn handle_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        // A message is a message, not a window: any keypress clears it, and an
        // action that produces a new one sets it again below.
        self.toast = None;

        if self.palette.is_some() {
            match k.code {
                KeyCode::Esc => self.palette = None,
                KeyCode::Enter => self.palette_accept(),
                KeyCode::Backspace => self.palette_backspace(),
                KeyCode::Up => self.palette_move(-1),
                KeyCode::Down => self.palette_move(1),
                KeyCode::Char(c) => self.palette_input(c),
                _ => {}
            }
            return;
        }

        if self.popup.is_some() {
            match k.code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Enter | KeyCode::Tab => self.accept_popup(),
                KeyCode::Up => self.popup_move(-1),
                KeyCode::Down => self.popup_move(1),
                KeyCode::Char(c) if !ctrl => {
                    if let Some(ed) = self.editor.as_mut() {
                        ed.insert_char(c);
                    }
                    self.refresh_popup();
                }
                KeyCode::Backspace => {
                    if let Some(ed) = self.editor.as_mut() {
                        ed.backspace();
                    }
                    self.refresh_popup();
                }
                _ => {}
            }
            return;
        }

        if self.mode == Mode::Insert {
            self.insert_key(k, ctrl, alt);
            return;
        }
        match self.mode {
            Mode::Visual => self.visual_key(k, ctrl),
            _ => self.tree_key(k, ctrl),
        }
    }

    fn insert_key(&mut self, k: KeyEvent, ctrl: bool, alt: bool) {
        match k.code {
            KeyCode::Esc => {
                // One Escape always leaves editing, as in vim. The caret stays
                // in the block (text_focus); the next tree motion walks away.
                let id = self.editor.as_ref().map(|e| e.block_id);
                self.commit_edit();
                if let Some(id) = id {
                    self.begin_edit_block(id);
                    self.text_focus = true;
                    self.mode = Mode::Normal;
                }
            }
            KeyCode::Char('w') if ctrl => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.delete_word();
                }
            }
            KeyCode::Char('u') if ctrl => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.delete_to_end();
                }
            }
            KeyCode::Enter if ctrl => {
                self.cycle_status();
            }
            KeyCode::Enter if alt => self.insert_soft_newline(),
            KeyCode::Enter => {
                if self.editor.is_none() {
                    self.new_block_below();
                    return;
                }
                let id = self
                    .editor
                    .as_ref()
                    .map(|e| e.block_id)
                    .unwrap_or(-1);
                if id < 0 {
                    // A provisional day: Enter materialises it, then opens the
                    // next block.
                    self.commit_edit();
                    self.new_block_below();
                    return;
                }
                let (tail, text) = {
                    let ed = self.editor.as_mut().unwrap();
                    let tail = ed.split_at_cursor();
                    let text = ed.text();
                    (tail, text)
                };
                self.ensure_current_page();
                if tail.is_empty() && !text.is_empty() {
                    // plain "new block under this one"
                    self.db.update_content(id, &text);
                    self.editor = None;
                    self.mode = Mode::Normal;
                    self.new_block_below();
                } else if tail.is_empty() {
                    self.editor = None;
                    self.mode = Mode::Normal;
                    self.new_block_below();
                } else {
                    self.db.split_block(id, &text, &tail);
                    self.editor = None;
                    self.mode = Mode::Normal;
                    self.reload();
                    self.select_block(id);
                    self.move_selection(1);
                    self.begin_edit();
                }
            }
            KeyCode::Backspace => {
                let empty = self
                    .editor
                    .as_mut()
                    .map(|ed| ed.backspace())
                    .unwrap_or(false);
                if empty {
                    let id = self.editor.as_ref().map(|e| e.block_id);
                    self.commit_edit();
                    if let Some(id) = id {
                        self.select_block(id);
                        self.merge_up();
                    }
                }
            }
            KeyCode::Delete => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.delete();
                }
            }
            KeyCode::Left if ctrl => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.word_left();
                }
            }
            KeyCode::Right if ctrl => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.word_right();
                }
            }
            KeyCode::Left => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.left();
                }
            }
            KeyCode::Right => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.right();
                }
            }
            KeyCode::Home => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.home();
                }
            }
            KeyCode::End => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.end();
                }
            }
            KeyCode::Tab => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.insert_str("  ");
                }
            }
            KeyCode::Char(c) if !ctrl => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.insert_char(c);
                }
                self.refresh_popup();
            }
            _ => {}
        }
    }

    pub fn sidebar_step(&mut self, delta: i64) {
        let sel: Vec<usize> = self
            .sidebar
            .iter()
            .enumerate()
            .filter(|(_, i)| i.kind != ItemKind::Section)
            .map(|(idx, _)| idx)
            .collect();
        if sel.is_empty() {
            return;
        }
        let cur = sel
            .iter()
            .position(|i| *i == self.sidebar_selected)
            .unwrap_or(0) as i64;
        let next = (cur + delta).clamp(0, sel.len() as i64 - 1) as usize;
        self.sidebar_selected = sel[next];
        if self.sidebar_selected < self.sidebar_scroll {
            self.sidebar_scroll = self.sidebar_selected;
        }
    }
}

impl Palette {
    pub fn refilter(&mut self) {
        let q = self.query.clone();
        let mut scored: Vec<(i32, usize)> = self
            .all
            .iter()
            .enumerate()
            .filter_map(|(i, (label, detail, _))| {
                fuzzy(&q, label)
                    .map(|(s, _)| (s, i))
                    .or_else(|| fuzzy(&q, detail).map(|(s, _)| (s - 10, i)))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        self.filtered = scored.into_iter().map(|(_, i)| i).collect();
        self.selected = 0;
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

pub fn snippet(text: &str, width: usize) -> String {
    let flat = text.replace('\n', " ");
    let t = flat.trim();
    if t.chars().count() <= width {
        return t.to_string();
    }
    let s: String = t.chars().take(width.saturating_sub(1)).collect();
    format!("{}…", s)
}

/// Slash commands, Logseq-flavoured.
pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("TODO", "mark this block as a task"),
    ("DOING", "in progress"),
    ("DONE", "finished"),
    ("LATER", "parked for later"),
    ("NOW", "do it right now"),
    ("Deadline", "add a DEADLINE:: property"),
    ("Scheduled", "add a SCHEDULED:: property"),
    ("Priority", "A / B / C"),
    ("Query", "a saved query block"),
    ("Template", "insert a template"),
    ("Quote", "quote block"),
    ("Code", "fenced code block"),
    ("Page embed", "embed a whole page inline"),
    ("Block embed", "embed another block"),
    ("Cards", "make this block a flashcard"),
    ("Calculator", "inline result"),
    ("Draw", "open the whiteboard (not in TUI)"),
    ("Zotero", "citation picker (plugin API)"),
];

/// Static keymap: single source of truth for `?` and the hint bar.
pub const KEYMAP: &[(&str, &str, &str)] = &[
    ("Modes", "", ""),
    ("Esc", "insert → text → blocks (vim's ladder)", "Any"),
    ("Enter", "put the cursor in this block's text", "Normal"),
    ("i a I A", "insert · append · line start · line end", "Normal"),
    ("o O cc", "open a block below · above · change this one", "Normal"),
    ("v / V", "select a block range / a whole subtree", "Normal"),
    ("Block motions", "", ""),
    ("j k h l", "next · previous · parent · first child", "Normal"),
    ("gg G", "first · last block", "Normal"),
    ("Ctrl-d / Ctrl-u", "half page down / up", "Normal"),
    ("[ / ]", "previous / next journal day", "Normal"),
    ("Ctrl-w h l w", "focus sidebar · references · cycle", "Normal"),
    ("Following links", "", ""),
    ("Ctrl-]", "follow the link here (menu when there are several)", "Normal"),
    ("Ctrl-o / Ctrl-i", "jump back / forward", "Normal"),
    ("Enter in refs", "open the block that references this page", "Normal"),
    ("Operators", "", ""),
    ("dd / x", "delete block, subtree included", "Normal"),
    ("yy / Y", "yank block into the register", "Normal"),
    ("p / P", "paste register after / before", "Normal"),
    (">> / <<", "indent / outdent (Tab and Shift-Tab too)", "Normal, Visual"),
    ("J", "join this block with the one below", "Normal"),
    ("u / Ctrl-r", "undo / redo", "Any"),
    ("za zc zo zR zM", "fold · close · open · all open · all closed", "Normal"),
    ("Text motions", "", ""),
    ("w b e 0 ^ $", "word and line motions inside one block", "Text"),
    ("x dw d$ D", "delete char · word · to line end", "Text"),
    ("cw ciw C", "change word · to line end", "Text"),
    ("Ctrl-w / Ctrl-u", "delete word / to line start", "Insert"),
    ("Inline completion", "", ""),
    ("/", "slash commands inside a block", "Insert"),
    ("[[ (( #", "page link · block ref · tag", "Insert"),
    ("Commands", "", ""),
    (":", "ex command line (Tab completes, :help lists)", "Normal"),
    ("Ctrl-P", "command palette", "Any"),
    ("/ n N", "search the graph · next · previous", "Normal"),
    ("?", "this keymap", "Any"),
    ("Panels and troubleshooting", "", ""),
    ("Ctrl-n", "show / hide the sidebar", "Any"),
    ("Ctrl-b", "show / hide linked references", "Any"),
    (":set sidebar|nosidebar|refs|norefs", "the same, spelled out, and persisted", "Ex"),
    (":sql", "read-only SQL console (out of the status bar, on demand)", "Ex"),
    (":w", "snapshot + queue for the remote", "Ex"),
    ("Ctrl-S", "snapshot, then the storage screen", "Any"),
    ("q / :q", "quit, pruning unwritten journals first", "Normal"),
    ("ZZ / ZQ", "snapshot-and-quit / quit without the prune pass", "Normal"),
    ("Views", "", ""),
    ("/ then type", "the search screen is a prompt: ↑↓ pick, ⏎ opens the block", "Any"),
    ("h l j k ⏎", "on the board: columns, rows, jump to the block", "Any"),
];

/// Every `:` command: completion in the command line, and `:help`.
pub const EX_COMMANDS: &[(&str, &str)] = &[
    ("e ", "edit a page: `:e Project Aurora`, or a day: `:e 2026-10-01`, `:e -2`"),
    ("w", "snapshot the database and queue it for the remote"),
    ("wq", "snapshot, then quit"),
    ("q", "quit (prunes empty journals)"),
    ("q!", "quit without the prune pass"),
    ("set ", "`set sidebar`, `set nosidebar`, `set refs`, `set norefs`"),
    ("board", "open the TODO board"),
    ("storage", "storage and backup screen"),
    ("today", "jump to today's journal"),
    ("journal ", "`:journal 2026-10-01` or `:journal -2`"),
    ("prune", "prune empty journal pages and report what went"),
    ("sql", "read-only SQL console; `.tables`, `.schema blocks`"),
    ("search ", "full-text search: `:search snapshot`"),
    ("m ", "move the block: `:m +1`, `:m -2`"),
    ("help", "the keymap"),
];
