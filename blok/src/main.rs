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

fn run_tui(db_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let db = open_or_explain(db_path)?;
    let today = db::today();
    let mut app = App::new(db, today);
    app.pruned = app.db.prune_empty_journals();
    app.db.refresh_stats();

    let mut terminal = ratatui::init();
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        loop {
            terminal.draw(|f| ui::render(f, &mut app))?;
            let Event::Key(k) = event::read()? else {
                continue;
            };
            if k.kind != KeyEventKind::Press {
                continue;
            }
            if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
                // Committing first means Ctrl-C during an edit is a save, not a
                // data loss. The prune pass at the end still runs.
                app.commit_edit();
                break;
            }
            // Ctrl-S is the one global that is not a vim binding: a snapshot is
            // an application concern, not a text one. Everything else -- panels,
            // views, prune, backup -- goes through the keymap or `:`.
            if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('s')) {
                app.do_backup();
                app.view = app::View::Backup;
                continue;
            }
            app.handle_key(k);
            if app.quit {
                break;
            }
        }
        Ok(())
    })();
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
