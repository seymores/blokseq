// dead_code is allowed deliberately: the domain types and the db layer model
// more than the prototype's UI reads today (schema fields kept for the road map
// in DESIGN.md §12) rather than being trimmed to what the mockups happen to use.
#![allow(dead_code)]

//! `blok` — a terminal-native, SQLite-backed outliner in the spirit of Logseq.
//!
//! Two modes:
//!   * default          interactive TUI (crossterm + ratatui)
//!   * `--dump <dir>`   render every mockup frame through the real renderer into
//!                      `NN-name.txt` (plain) and `NN-name.ans` (24-bit colour),
//!                      which is how the design document's screenshots are made.

mod app;
mod db;
mod editor;
mod mockups;
mod model;
mod theme;
mod ui;
mod vim;

use std::io::Write;
use std::path::{Path, PathBuf};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};

use app::App;
use db::Db;

#[derive(PartialEq)]
enum Mode {
    Tui,
    Stats,
    Lifecycle,
    Help,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let mut dump: Option<String> = None;
    let mut db_path = default_db_path();
    let mut mode = Mode::Tui;
    // Two passes over the arguments, so flag order never matters
    // (`--stats --db x` and `--db x --stats` must behave the same).
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--dump" => {
                if let Some(v) = args.get(i + 1) {
                    dump = Some(v.clone());
                }
                i += 2;
            }
            "--db" => {
                if let Some(v) = args.get(i + 1) {
                    db_path = PathBuf::from(v);
                }
                i += 2;
            }
            "--demo-lifecycle" => {
                mode = Mode::Lifecycle;
                i += 1;
            }
            "--stats" => {
                mode = Mode::Stats;
                i += 1;
            }
            "-h" | "--help" => {
                mode = Mode::Help;
                i += 1;
            }
            _ => i += 1,
        }
    }

    match mode {
        Mode::Help => {
            println!("blok — SQLite-backed terminal outliner\n");
            println!("  blok                     run the TUI (default: ~/.blok/blok.db)");
            println!("  blok --db PATH           use PATH as the database");
            println!("  blok --dump DIR          render the mockup frames into DIR");
            println!("  blok --demo-lifecycle    print the journal create/prune trace");
            println!("  blok --stats             print row counts, FTS5 and integrity");
            return Ok(());
        }
        Mode::Lifecycle => {
            demo_lifecycle(&db_path)?;
            return Ok(());
        }
        Mode::Stats => {
            let db = open_or_explain(&db_path)?;
            let s = db.compute_stats();
            println!("database       {}", db_path.display());
            println!("pages          {}", s.pages);
            println!("  journals     {}", s.journals);
            println!("blocks         {}", s.blocks);
            println!("refs           {}", s.refs);
            println!("fts5           {}", s.fts);
            println!("integrity      {}", db.integrity_check());
            return Ok(());
        }
        Mode::Tui => {}
    }

    if let Some(dir) = dump {
        let n = mockups::dump(Path::new(&dir))?;
        println!("rendered {} mockup frames into {}", n, dir);
        return Ok(());
    }

    run_tui(&db_path)
}

/// A missing default directory is worth explaining, because the fix is a flag.
fn open_or_explain(path: &Path) -> Result<Db, Box<dyn std::error::Error>> {
    match Db::open(path) {
        Ok(db) => Ok(db),
        Err(e) => {
            eprintln!(
                "blok: cannot open the database at {}\n      {}\n      \
                 hint: pass --db <path> to use a different file (or set $BLOK_DB).",
                path.display(),
                e
            );
            Err(e.into())
        }
    }
}

fn default_db_path() -> PathBuf {
    if let Ok(p) = std::env::var("BLOK_DB") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".blok").join("blok.db")
}

/// One terminal event. Returns true when the session should end.
///
/// This is a function rather than the body of the loop so that the paste path --
/// the one that used to be dropped on the floor, because the loop matched
/// `Event::Key` and `continue`d on everything else -- can be tested without a
/// terminal.
fn step(app: &mut App, event: Event) -> bool {
    let k = match event {
        Event::Key(k) => k,
        // A bracketed paste is text, not keystrokes: it goes straight into the
        // buffer, newlines and all, and never becomes Enter.
        Event::Paste(text) => {
            app.paste_text(&text);
            return false;
        }
        _ => return false,
    };
    if k.kind != KeyEventKind::Press {
        return false;
    }
    if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
        // Committing first means Ctrl-C during an edit is a save, not a data
        // loss. The prune pass at the end still runs.
        app.commit_edit();
        return true;
    }
    // Ctrl-S is the one global that is not a vim binding: a snapshot is an
    // application concern, not a text one. Everything else -- panels, views,
    // prune, backup -- goes through the keymap or `:`.
    if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('s')) {
        // Commit first: a snapshot that is missing the line you just typed is
        // not a snapshot of your work.
        app.commit_edit();
        app.do_backup();
        app.view = app::View::Backup;
        return false;
    }
    app.handle_key(k);
    app.quit
}

fn run_tui(db_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let db = open_or_explain(db_path)?;
    let today = db::today();
    let mut app = App::new(db, today);
    app.pruned = app.db.prune_empty_journals();
    app.db.refresh_stats();

    let mut terminal = ratatui::init();
    // Ask the terminal to send a paste as one event instead of a burst of
    // keystrokes. Without this a pasted newline is *Enter*: pasting code split
    // the block per line, and each line got the previous line's indentation
    // copied onto it by the code-block auto-indent.
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableBracketedPaste
    );

    // Ask the terminal to disambiguate the keys that share a control byte under
    // the legacy encoding: Ctrl-M/Enter, Ctrl-I/Tab, Ctrl-H/Backspace,
    // Ctrl-[/Esc, Ctrl-J/Enter. Terminals that speak the kitty keyboard protocol
    // (kitty, WezTerm, foot, Ghostty, recent tmux with extended-keys) will; the
    // rest ignore the request, and then `gm` is the reliable way to the panel.
    // Detecting it lets the hint bar name the key that actually works here.
    let enhanced = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );
    }

    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        loop {
            terminal.draw(|f| ui::render(f, &mut app))?;
            if step(&mut app, event::read()?) {
                break;
            }
        }
        Ok(())
    })();
    if enhanced {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::PopKeyboardEnhancementFlags);
    }
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableBracketedPaste
    );
    ratatui::restore();
    // Quitting is a save point: prune the days that were never written to.
    app.prune_journals();
    if let Err(e) = &result {
        eprintln!("blok: {}", e);
    }
    Ok(())
}

/// Prints the journal create/prune trace against a throwaway database. Used as
/// evidence in the design doc that the "no empty journal pages" rule is real.
fn demo_lifecycle(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let db_file = dir.join("lifecycle-demo.db");
    let _ = std::fs::remove_file(&db_file);
    let _ = std::fs::remove_file(dir.join("lifecycle-demo.db-wal"));
    let _ = std::fs::remove_file(dir.join("lifecycle-demo.db-shm"));

    let db = Db::open(&db_file)?;
    let today = db::today();
    let mut out = std::io::stdout();

    writeln!(out, "blok journal lifecycle, against {}", db_file.display())?;
    writeln!(out, "today = {}\n", today)?;

    writeln!(out, "1. launch: open today's journal")?;
    writeln!(
        out,
        "   rows in pages where kind='journal': {}",
        db.compute_stats().journals
    )?;
    writeln!(out, "   -> the journal is a view over a date, not a file")?;

    writeln!(out, "\n2. visit and abandon five past days (no typing)")?;
    for d in 1..=5 {
        let day = model::JournalDay::new(today - chrono::Duration::days(d));
        // Browsing a day writes nothing. Days 4 and 5 simulate an older build
        // (or a hard kill) that did leave an empty row behind.
        if d >= 4 {
            db.ensure_journal(day);
            let p = db.journal_page(day).unwrap();
            db.create_block(p.id, None, None, "");
        }
        writeln!(
            out,
            "   opened {}  -> row exists: {}",
            day.key(),
            db.journal_page(day).is_some()
        )?;
    }

    writeln!(out, "\n3. type into today's journal (first keystroke)")?;
    let day = model::JournalDay::new(today);
    let page = db.ensure_journal(day);
    let b = db.create_block(page.id, None, None, "first real thought of the day");
    writeln!(out, "   created page row id={} name={}", page.id, page.name)?;
    writeln!(out, "   created block id={} uuid={}", b.id, b.uuid)?;

    writeln!(out, "\n4. transient empty block appears and is abandoned")?;
    let scratch = db.create_block(page.id, None, Some(b.id), "");
    writeln!(
        out,
        "   empty block id={} content={:?}",
        scratch.id, scratch.content
    )?;

    writeln!(out, "\n5. a page linked from a block is materialised")?;
    let p2 = db.create_block(
        page.id,
        None,
        Some(scratch.id),
        "planning [[Project Aurora]] for next week",
    );
    writeln!(
        out,
        "   block {} refs -> pages: {:?}",
        p2.id,
        db.pages(10)
            .into_iter()
            .map(|(n, l, _)| format!("{} ({} links)", n, l))
            .collect::<Vec<_>>()
    )?;

    writeln!(out, "\n6. quit: prune pass")?;
    let pruned = db.prune_empty_journals();
    writeln!(out, "   pruned journal pages: {:?}", pruned)?;
    writeln!(
        out,
        "   empty blocks still present: {} (dropped on the next save pass)",
        db.empty_block_count()
    )?;
    let s = db.compute_stats();
    writeln!(
        out,
        "\nfinal: {} pages ({} journals), {} blocks, {} refs, {} bytes, fts5={}",
        s.pages, s.journals, s.blocks, s.refs, s.file_bytes, s.fts
    )?;
    writeln!(out, "\nsqlite> SELECT name, kind FROM pages;")?;
    let mut stmt = db
        .conn
        .prepare("SELECT name, kind FROM pages ORDER BY name")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    for r in rows {
        let (n, k) = r?;
        writeln!(out, "        {}  {}", k, n)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    fn app(name: &str) -> App {
        let path: PathBuf = std::env::temp_dir().join(format!(
            "blok-main-{}-{}.db",
            std::process::id(),
            name
        ));
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
        let db = crate::db::Db::open(&path).expect("open test db");
        let today = crate::db::today();
        let mut app = App::new(db, today);
        let page = app.db.ensure_page("Test Page", crate::model::PageKind::Page);
        app.db.create_block(page.id, None, None, "first block");
        app.goto_page("Test Page");
        app
    }

    /// The paste path, end to end: the event is *handled*, not dropped. The bug
    /// was that the loop matched `Event::Key` and threw everything else away, so
    /// in a terminal that sends bracketed paste, pasting did nothing at all.
    #[test]
    fn a_paste_event_reaches_the_buffer_verbatim() {
        let mut a = app("paste_event");
        a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(!step(&mut a, Event::Paste("one\ntwo".into())));
        let ed = a.editor.as_ref().expect("still editing");
        assert_eq!(
            ed.text(),
            "one\ntwofirst block",
            "the paste is verbatim, and a pasted newline is a newline"
        );

        // And Enter is still Enter.
        step(&mut a, Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        assert!(!a.text_focus);
    }

    /// Ctrl-S snapshots the buffer you are typing in, not the last commit.
    #[test]
    fn ctrl_s_commits_before_it_snapshots() {
        let mut a = app("ctrl_s");
        a.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        a.handle_key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE));
        a.handle_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));

        step(
            &mut a,
            Event::Key(KeyEvent::new(
                KeyCode::Char('s'),
                KeyModifiers::CONTROL,
            )),
        );
        assert_eq!(a.view, app::View::Backup);
        let id = a.rows[0].id;
        assert_eq!(
            a.db.block(id).map(|b| b.content),
            Some("first block!".to_string()),
            "the line being typed is in the snapshot"
        );
    }
}
