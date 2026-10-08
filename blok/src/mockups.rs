//! Mockup fixtures. Each case builds a throwaway database, drives the same
//! `App` methods the interactive TUI uses, renders one frame at a fixed size and
//! writes it as plain text plus a 24-bit-colour ANSI file.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Duration;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

use crate::app::{App, Focus, ToastKind, View};
use crate::db::Db;
use crate::editor::Mode;
use crate::model::{JournalDay, PageKind};
use crate::ui;

struct Case {
    name: &'static str,
    width: u16,
    height: u16,
    build: fn(&mut App),
}

pub fn dump(dir: &Path) -> Result<usize, Box<dyn std::error::Error>> {
    fs::create_dir_all(dir)?;
    let cases = cases();
    for case in &cases {
        let mut app = build_app(case.name);
        (case.build)(&mut app);
        let mut term = Terminal::new(TestBackend::new(
            if case.width == 118 { 140 } else { case.width },
            if case.height == 38 { 42 } else { case.height },
        ))?;
        term.draw(|f| ui::render(f, &mut app))?;
        let buf = term.backend().buffer().clone();
        fs::write(dir.join(format!("{}.txt", case.name)), buffer_text(&buf))?;
        fs::write(dir.join(format!("{}.ans", case.name)), buffer_ansi(&buf))?;
    }
    Ok(cases.len())
}

/// A fresh database per case, seeded with the demo graph.
fn build_app(name: &str) -> App {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("dumps")
        .join(".db")
        .join(name);
    let _ = fs::remove_dir_all(&root);
    let _ = fs::create_dir_all(&root);
    let path = root.join("blok.db");
    let db = Db::open(&path).expect("open db");
    let today = crate::db::today();
    let mut app = App::new(db, today);
    app.fake_caret = true;
    app.clock = "09:41".into();
    seed(&mut app);
    app.reload();
    app
}

/// Demo content: a plausible week in the life of a terminal outliner user.
fn seed(app: &mut App) {
    let today = app.today;

    // ---------------------------------------------------------- page: Aurora
    let aurora = app.db.ensure_page("Project Aurora", PageKind::Page);
    let a1 = app
        .db
        .create_block(
            aurora.id,
            None,
            None,
            "Project Aurora — offline-first note sync for field teams",
        )
        .id;
    // Page properties: a top-level `key:: value` block, which is where the
    // metadata panel reads them from.
    app.db.create_block(
            aurora.id,
            None,
            None,
            "status:: active\nowner:: Dana\nreview:: Fridays",
        )
        .id;
    let a1a = app
        .db
        .create_block(
            aurora.id,
            Some(a1),
            None,
            "Constraint: laptops on 2G links for weeks at a time",
        )
        .id;
    app.db.create_block(
        aurora.id,
        Some(a1),
        Some(a1a),
        "Decision: ship the **SQLite file** as the export format, not markdown",
    );
    app.db.create_block(
        aurora.id,
        Some(a1),
        None,
        "Rejected: markdown tree — 4k files, slow rename, no transactions",
    );
    let a2 = app
        .db
        .create_block(aurora.id, None, Some(a1), "Milestones")
        .id;
    app.db.create_block(
        aurora.id,
        Some(a2),
        None,
        "TODO Single-file export + import round-trip",
    );
    let a2b = app
        .db
        .create_block(
            aurora.id,
            Some(a2),
            None,
            "DOING Snapshot uploader (rclone backend)",
        )
        .id;
    app.db.create_block(
        aurora.id,
        Some(a2b),
        None,
        "DONE Conflict model: last-writer-wins per *block*, not per file",
    );
    app.db.create_block(
        aurora.id,
        Some(a2),
        None,
        "LATER On-device full-text reindex after a restore",
    );
    let aurora_see = app
        .db
        .create_block(aurora.id, None, Some(a2), "Pointer to the storage argument")
        .id;
    app.db.create_block(
        aurora.id,
        None,
        None,
        "Open question #sync: what happens when two devices both snapshot at 09:00?",
    );

    // ------------------------------------------------- page: blok (the app)
    let blok = app.db.ensure_page("blok TUI", PageKind::Page);
    let b1 = app
        .db
        .create_block(
            blok.id,
            None,
            None,
            "blok TUI — outliner for the terminal, SQLite only",
        )
        .id;
    app.db.create_block(
        blok.id,
        Some(b1),
        None,
        "Journals are views over a date; empty days never get a row",
    );
    app.db.create_block(
        blok.id,
        Some(b1),
        None,
        "Backup = `VACUUM INTO` one file, then copy that file to a remote",
    );
    app.db.create_block(
        blok.id,
        Some(b1),
        None,
        "No markdown on disk. Blocks are rows, links are rows",
    );
    app.db.create_block(
        blok.id,
        None,
        Some(b1),
        "TODO Decide whether the TUI should speak to [[Project Aurora]] over the plugin API",
    );

    // --------------------------------------------------------- other pages
    for (name, text) in [
        (
            "Terminal UX",
            "Ratatui 0.30 · crossterm 0.29 · one frame, whole screen",
        ),
        ("Reading list", "The Design of Everyday Things — re-read ch. 4"),
        ("SQLite", "WAL, FTS5, VACUUM INTO, fractional indexing"),
        (
            "Weekly review",
            "Friday 16:00 — sweep the journal for TODO blocks",
        ),
    ] {
        let p = app.db.ensure_page(name, PageKind::Page);
        app.db.create_block(p.id, None, None, text);
    }

    // ---------------------------------------------------- today's journal
    let page0 = app.db.ensure_journal(JournalDay::new(today));
    let j1 = app
        .db
        .create_block(
            page0.id,
            None,
            None,
            "Deep work 09:00–12:30 on the block store",
        )
        .id;
    let pos_block = app
        .db
        .create_block(
            page0.id,
            Some(j1),
            None,
            "Replaced the integer `position` with a fractional index",
        )
        .id;
    app.db.create_block(
        page0.id,
        Some(j1),
        None,
        "Reordering 4k siblings was O(n) writes → now it is one row",
    );
    let j2 = app
        .db
        .create_block(page0.id, None, Some(j1), "Standup — [[Project Aurora]]")
        .id;
    app.db.create_block(
        page0.id,
        Some(j2),
        None,
        "Export format settled: the SQLite file *is* the format",
    );
    let j2b = app
        .db
        .create_block(
            page0.id,
            Some(j2),
            None,
            "TODO Ask Dana whether the trial keys rotate monthly",
        )
        .id;
    app.db.create_block(
        page0.id,
        Some(j2b),
        None,
        "deadline:: 2026-02-20\npriority:: A",
    );
    app.db.create_block(
        page0.id,
        Some(j2),
        None,
        "DOING Write up the sync story for [[blok TUI]]",
    );
    let idea = app
        .db
        .create_block(
            page0.id,
            None,
            Some(j2),
            "Idea: journals are rows, not files #blok",
        )
        .id;
    app.db.create_block(
        page0.id,
        Some(idea),
        None,
        "An empty day should not exist anywhere — not on disk, not in the DB",
    );
    app.db.create_block(page0.id, None, Some(idea), "Reading: `VACUUM INTO` semantics");
    // Several links in one block, so the `Ctrl-]` chooser has something to
    // choose between.
    app.db.create_block(
        page0.id,
        None,
        Some(idea),
        "Storage trail: [[blok TUI]] · [[SQLite]] · #sync",
    );

    // Real block references, now that the targets exist.
    let idea_uuid = app.db.block(idea).map(|b| b.uuid).unwrap_or_default();
    let pos_uuid = app.db.block(pos_block).map(|b| b.uuid).unwrap_or_default();
    app.db.update_content(
        aurora_see,
        &format!(
            "Storage argument lives in (({})) (today's journal)",
            idea_uuid
        ),
    );

    // ------------------------------------------------- past journals (real)
    let p1 = app.db.ensure_journal(JournalDay::new(today - Duration::days(1)));
    let d1b = app
        .db
        .create_block(
            p1.id,
            None,
            None,
            "Shipped the block-store refactor and wrote the migration notes",
        )
        .id;
    app.db.create_block(
        p1.id,
        Some(d1b),
        None,
        &format!("Touches (({})) — the fractional index decision", pos_uuid),
    );
    let p2 = app.db.ensure_journal(JournalDay::new(today - Duration::days(2)));
    app.db.create_block(
        p2.id,
        None,
        None,
        "Meetings ate the morning. Rescued the afternoon with [[Terminal UX]] sketching",
    );
    let p3 = app.db.ensure_journal(JournalDay::new(today - Duration::days(3)));
    let d3a = app
        .db
        .create_block(p3.id, None, None, "Prototype the three-pane layout")
        .id;
    app.db.set_status(d3a, Some("DONE"));
    let d3b = app
        .db
        .create_block(p3.id, None, Some(d3a), "Draft the block editor keymap")
        .id;
    app.db.set_status(d3b, Some("DOING"));

    // --------------------------------------------------------- empty days
    // Created and left empty on purpose so the prune pass has work to do.
    for d in 4..=7 {
        let day = JournalDay::new(today - Duration::days(d));
        let p = app.db.ensure_journal(day);
        app.db.create_block(p.id, None, None, "");
    }

    // ------------------------------------------------------- backup history
    seed_backup_history(app);
    app.db.reindex_all();
    app.db.refresh_stats();
    app.db.prune_empty_journals();
    app.db.refresh_stats();
}

/// Six plausible historical snapshots so the storage screen is not empty.
fn seed_backup_history(app: &mut App) {
    let now = chrono::Local::now();
    let sizes = [1180_224u64, 1179_008, 1177_472, 1174_944, 1168_320, 1155_072];
    for (i, size) in sizes.iter().enumerate() {
        let ts = now - Duration::hours(3 * (i as i64 + 1));
        let taken = ts.format("%Y-%m-%dT%H:%M:%S").to_string();
        let label = ts.format("%Y%m%d-%H%M%S").to_string();
        let file = format!(
            "{}/snapshots/blok-{}.sqlite",
            app.db
                .path
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default(),
            label
        );
        let state = if i == 0 { "queued" } else { "uploaded" };
        let _ = app.db.conn.execute(
            "INSERT INTO backup_log(file, bytes, taken_at, remote, state) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![file, *size as i64, taken, app.db.remote_target(), state],
        );
    }
}

// ------------------------------------------------------------------- cases

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "01-journal",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 4;
                app.focus = Focus::Main;
                app.scroll_to_selection();
            },
        },
        Case {
            name: "02-journal-first-keystroke",
            width: 118,
            height: 38,
            build: |app| {
                app.clear_journal(app.today);
                app.goto_today();
                app.begin_provisional();
                if let Some(ed) = app.editor.as_mut() {
                    ed.insert_str("Woke up thinking about the sync story");
                }
                app.refresh_popup();
            },
        },
        Case {
            name: "03-block-editing",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 0;
                app.begin_edit();
                if let Some(ed) = app.editor.as_mut() {
                    ed.cursor = "Deep work 09".chars().count();
                }
            },
        },
        Case {
            name: "04-slash-commands",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 5;
                app.begin_edit();
                if let Some(ed) = app.editor.as_mut() {
                    ed.chars.clear();
                    ed.cursor = 0;
                    ed.insert_char('/');
                    ed.insert_char('d');
                }
                app.refresh_popup();
            },
        },
        Case {
            name: "05-page-autocomplete",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 5;
                app.begin_edit();
                if let Some(ed) = app.editor.as_mut() {
                    ed.chars.clear();
                    ed.cursor = 0;
                    ed.insert_str("Follow up next week with ");
                    ed.insert_str("[[");
                    ed.insert_char('p');
                }
                app.refresh_popup();
            },
        },
        Case {
            name: "06-block-ref",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 8;
                app.begin_edit();
                if let Some(ed) = app.editor.as_mut() {
                    ed.chars.clear();
                    ed.cursor = 0;
                    ed.insert_str("See also ");
                    ed.insert_str("((");
                    ed.insert_str("snap");
                }
                app.refresh_popup();
            },
        },
        Case {
            name: "07-multiline-and-properties",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 6;
                app.begin_edit();
                if let Some(ed) = app.editor.as_mut() {
                    ed.cursor = "deadline:: 2026-02-20\nprio".chars().count();
                }
            },
        },
        Case {
            name: "08a-indent-before",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 5;
                app.toast(
                    ToastKind::Info,
                    "Tab re-parents the block under its previous sibling",
                    Some("before — one Tab press"),
                );
            },
        },
        Case {
            name: "08b-indent-after",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 5;
                app.indent_selected();
            },
        },
        Case {
            name: "09a-empty-block",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 3;
                app.new_block_below();
                app.toast(
                    ToastKind::Info,
                    "Enter opened an empty block below, still in insert mode",
                    Some("state lives in the session, not in the database"),
                );
            },
        },
        Case {
            name: "09b-merged-up",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 3;
                app.new_block_below();
                app.commit_edit();
                app.merge_up();
            },
        },
        Case {
            name: "10-visual-multiselect",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.selected = 3;
                app.visual = Some(5);
                app.mode = Mode::Visual;
                app.toast(
                    ToastKind::Info,
                    "3 blocks selected · Tab indents all of them",
                    Some("visual · d deletes · Alt-↑↓ reorders · u undoes"),
                );
            },
        },
        Case {
            name: "11-page-view",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_page("Project Aurora");
                app.selected = 2;
                app.focus = Focus::Main;
            },
        },
        Case {
            name: "12-linked-references",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_page("Project Aurora");
                app.show_meta = true;
                app.focus = Focus::Right;
                app.linked_selected = 2;
            },
        },
        Case {
            name: "13-find-search",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.open_palette();
                for c in "back".chars() {
                    app.palette_input(c);
                }
            },
        },
        Case {
            name: "14-search-fts",
            width: 118,
            height: 38,
            build: |app| {
                app.set_view(View::Search);
                app.search_query = "snapshot".into();
                app.run_search();
            },
        },
        Case {
            name: "15-todo-board",
            width: 118,
            height: 38,
            build: |app| {
                app.set_view(View::Query);
            },
        },
        Case {
            name: "16-backup-remote",
            width: 118,
            height: 38,
            build: |app| {
                app.do_backup();
                app.set_view(View::Backup);
                app.toast(
                    ToastKind::Good,
                    "snapshot → dropbox:Apps/blok",
                    Some("VACUUM INTO, then rclone copy --checksum"),
                );
            },
        },
        Case {
            name: "17-prune-empty-journals",
            width: 118,
            height: 38,
            build: |app| {
                // Days that were visited but never written to, plus one
                // transient empty block on a real day.
                for d in 4..=9 {
                    let day = JournalDay::new(app.today - Duration::days(d));
                    let p = app.db.ensure_journal(day);
                    app.db.create_block(p.id, None, None, "");
                }
                app.selected = app.rows.len().saturating_sub(1);
                app.new_block_below();
                app.commit_edit();
                app.prune_journals();
                app.set_view(View::Backup);
            },
        },
        Case {
            name: "18-help-keymap",
            width: 118,
            height: 38,
            build: |app| {
                app.set_view(View::Help);
            },
        },
        Case {
            name: "19-narrow-80x24",
            width: 80,
            height: 24,
            build: |app| {
                app.goto_today();
                app.selected = 2;
                app.focus = Focus::Main;
            },
        },
        Case {
            name: "20-new-block-hint",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.select_containing("Idea: journals are rows");
                app.begin_edit();
                if let Some(ed) = app.editor.as_mut() {
                    ed.chars.clear();
                    ed.cursor = 0;
                }
                app.refresh_popup();
            },
        },
        // ---- after the second round of feedback ---------------------------
        Case {
            name: "21-follow-link-menu",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.select_containing("Storage trail");
                app.follow_link();
            },
        },
        Case {
            name: "22-text-normal-vim",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.select_containing("Deep work 09:00");
                app.enter_text();
                if let Some(ed) = app.editor.as_mut() {
                    // inside the word, as vim's `w` / `e` would leave it
                    ed.cursor = "Deep work 09:00–12:30 on the ".chars().count();
                }
                app.toast(
                    ToastKind::Info,
                    "TEXT mode: vim Normal, with the cursor inside the block",
                    Some("w b e 0 ^ $ x dw cw · Esc returns to the block list"),
                );
            },
        },
        Case {
            name: "23-ex-command-line",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.open_ex();
                if let Some(ex) = app.ex.as_mut() {
                    ex.input = "set norefs".into();
                }
            },
        },
        Case {
            name: "24-sql-console",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.open_sql();
            },
        },
        Case {
            name: "25-sql-tables",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.open_sql();
                if let Some(c) = app.sql.as_mut() {
                    c.input = ".tables".into();
                }
                app.sql_run();
            },
        },
        Case {
            name: "26-page-metadata",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.select_containing("Standup");
                app.show_meta = true;
                app.toast(
                    ToastKind::Info,
                    "Ctrl-M: what this page is, and what points at it",
                    Some("hidden by default · :set meta keeps it on"),
                );
            },
        },
        Case {
            name: "27-find-recent",
            width: 118,
            height: 38,
            build: |app| {
                app.goto_today();
                app.open_palette();
                app.toast(
                    ToastKind::Info,
                    "Ctrl-P with no query: what you touched last",
                    Some("type to search page names and block text"),
                );
            },
        },
    ]
}

// Fixture helpers: name a block by its text instead of an index, so adding a
// line to the seed can never silently retarget a frame.
impl App {
    pub fn select_containing(&mut self, needle: &str) {
        if let Some(i) = self.rows.iter().position(|r| r.content.contains(needle)) {
            self.selected = i;
            self.scroll_to_selection();
        }
    }
}

// ------------------------------------------------------------ buffer dump

fn buffer_text(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            let s = buf[(x, y)].symbol();
            if s.is_empty() {
                line.push(' ');
            } else {
                line.push_str(s);
            }
        }
        out.push_str(line.trim_end());
        if y + 1 < area.height {
            out.push('\n');
        }
    }
    out.push('\n');
    out
}

fn rgb(c: Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray => (229, 229, 229),
        Color::White => (255, 255, 255),
        _ => fallback,
    }
}

fn buffer_ansi(buf: &Buffer) -> String {
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        let mut last: Option<String> = None;
        let mut line = String::new();
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            let (fr, fg, fb) = rgb(cell.fg, (205, 214, 228));
            let (br, bg, bb) = rgb(cell.bg, (13, 17, 23));
            let mut mods = String::new();
            for (m, code) in [
                (Modifier::BOLD, ";1"),
                (Modifier::DIM, ";2"),
                (Modifier::ITALIC, ";3"),
                (Modifier::UNDERLINED, ";4"),
                (Modifier::CROSSED_OUT, ";9"),
            ] {
                if cell.modifier.contains(m) {
                    mods.push_str(code);
                }
            }
            let sgr = format!(
                "\x1b[0;38;2;{};{};{};48;2;{};{};{}{}m",
                fr, fg, fb, br, bg, bb, mods
            );
            if last.as_deref() != Some(sgr.as_str()) {
                line.push_str(&sgr);
                last = Some(sgr.clone());
            }
            let s = cell.symbol();
            line.push_str(if s.is_empty() { " " } else { s });
        }
        line.push_str("\x1b[0m");
        out.push_str(&line);
        out.push('\n');
    }
    out
}

// Helpers used by the cases above.
impl App {
    /// Wipe a journal day, to show the provisional state.
    pub fn clear_journal(&mut self, day: chrono::NaiveDate) {
        let key = JournalDay::new(day).key();
        let _ = self.db.conn.execute(
            "DELETE FROM blocks WHERE page_id IN (SELECT id FROM pages WHERE name = ?1)",
            [&key],
        );
        let _ = self
            .db
            .conn
            .execute("DELETE FROM pages WHERE name = ?1", [&key]);
        self.db.refresh_stats();
        self.reload();
    }
}

impl Db {
    /// Re-index every block's references (used after the fixture is written).
    pub fn reindex_all(&self) {
        let mut stmt = self
            .conn
            .prepare("SELECT id, content FROM blocks WHERE deleted_at IS NULL")
            .unwrap();
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);
        for (id, content) in rows {
            self.reindex_refs(id, &content);
        }
    }
}
