# blok

A terminal-native, SQLite-backed outliner in the spirit of Logseq.

* **Journals auto-start on today**; a day that is only *looked at* is never
  written, and empty journals are pruned on startup and quit.
* **SQLite is the document.** Blocks are rows, links are rows, search is FTS5.
  There is no markdown file anywhere in the pipeline.
* **Backup is one file**: `VACUUM INTO` a consistent snapshot, copy the snapshot
  to a remote. Never sync a live database file.
* **Modal, vim-shaped editing** for block structure: fold, indent, split, merge,
  reorder, visual multi-select.

![Journal](dumps/png/01-journal.png)

Full design write-up, schema, keymap and every mockup frame:
[DESIGN.md](DESIGN.md). Research: [Logseq's value proposition](../research/logseq-value-proposition.md)
and [TUI outliner prior art](../research/tui-outliner-prior-art.md).

## Build and run

```bash
cargo run                             # the TUI, using ~/.blok/blok.db
cargo run -- --db ./scratch.db        # use a throwaway database
cargo run -- --dump dumps             # re-render every mockup frame
cargo run -- --demo-lifecycle         # print the journal create/prune trace
cargo run -- --stats                  # row counts, FTS5 availability, integrity
cargo test                            # 81 regression tests, one per bug
```

Dependencies: `ratatui` 0.30, `crossterm` 0.29, `rusqlite` 0.40 (`bundled`, so
SQLite and FTS5 are compiled in — no system SQLite needed), `chrono`.

## Layout

```
src/
  main.rs      CLI, TUI loop, --dump, --demo-lifecycle
  model.rs     Block / Page / Row / JournalDay, reference + property parsing
  db.rs        schema, WAL, refs, FTS5, journal lifecycle, VACUUM INTO backups
  editor.rs    single-block editor: cursor, soft wrap, `[[`/`((`/`/` triggers
  app.rs       state machine: views, panes, structural edits, undo, keymap
  ui.rs        every screen (the mockups are this file's real output)
  mockups.rs   the 31 framed fixtures driven through the real renderer
  theme.rs     palette, badges, TODO cycling
dumps/         NN-name.txt (plain) · NN-name.ans (24-bit) · png/ (rasterised)
tools/         render_png.py — ANSI frame -> PNG screenshot
```

## Frames

| # | Frame | What it shows |
|---|---|---|
| 01 | `01-journal` | three-pane journal, guides, fold markers, inline links |
| 02 | `02-journal-first-keystroke` | provisional day: typed but not yet written |
| 03 | `03-block-editing` | caret inside soft-wrapped text |
| 04-06 | `04-slash-commands`, `05-page-autocomplete`, `06-block-ref` | the three inline popups |
| 07 | `07-multiline-and-properties` | Alt-⏎ inside one block, `key:: value` |
| 08 | `08a/08b-indent` | Tab before/after |
| 09 | `09a/09b-merge` | empty block, then Backspace at its start |
| 10 | `10-visual-multiselect` | visual range + structural verbs |
| 11-12 | `11-page-view`, `12-linked-references` | page view, page-metadata panel |
| 13-15 | `13-find-search`, `14-search-fts`, `15-todo-board` | Find, search, board |
| 16-17 | `16-backup-remote`, `17-prune-empty-journals` | snapshots + prune report |
| 18-20 | `18-help-keymap`, `19-narrow-80x24`, `20-new-block-hint` | keymap, compact layout |
| 21 | `21-follow-link-menu` | `Ctrl-]` following a link, with a chooser when there are several |
| 22 | `22-text-normal-vim` | TEXT mode: vim's Normal mode with the cursor in the block |
| 23 | `23-ex-command-line` | the `:` command line, with completion |
| 24-25 | `24-sql-console`, `25-sql-tables` | the read-only troubleshooting console |
| 26 | `26-page-metadata` | `Ctrl-M`: page facts, backlinks, unlinked mentions |
| 27 | `27-find-recent` | `Ctrl-P` with no query: what you touched last |

## Editing model

Vim's, with the **block** as the line, in three modes and no more: `NORMAL`,
`INSERT`, `VISUAL`. **One `Esc` always stops editing** — the caret can sit inside
a block's text (press `Enter`) and still be in NORMAL, and any tree motion leaves
it, so the text cursor is a position rather than a fourth mode you can get stuck
in. `Ctrl-]` or `gf` follows a link (`↗N` marks the blocks that have them),
`[`/`]` (and `Ctrl-o`/`Ctrl-i`) walk back and forward through the pages you have
opened, `Tab` indents the line the caret is on (the block, from the tree), and
the keys mean the *text* while the caret is in a block — `dd` the line, `ciw` the
word, `dd` again the block once you `Esc` out. From the tree `dd`/`yy`/`p`
operate on whole subtrees,
`u`/`Ctrl-r` undo and redo, and everything app-level is a `:` command (`:e`,
`:w`, `:set`, `:sql`, `:board`, `:m +1`). The five deliberate deviations from vim
are listed in [DESIGN.md §7.7](DESIGN.md).

**Getting around** is one key: `Ctrl-P` opens **Find** — with no query it lists
what you touched last, and as you type it searches page names *and* block text,
landing on the matching block. `[`/`]` step through journal days, `Ctrl-M` shows the page metadata, and `/` searches every block. There is no top bar and no
page sidebar; the pane title says what you are looking at and the hint bar is
computed from what is possible right now, so it doubles as the manual.

**Page metadata** is one key: `Ctrl-M` shows a panel with what the page *is*
(kind, created/updated, block count, links in/out, and its `key:: value`
properties) followed by its linked references and unlinked mentions. It is hidden
by default and the choice persists; `Ctrl-w l` focuses it once visible. Storage internals are not in the status bar at all —
they live behind `:sql`, a read-only SQL console (`SELECT`, `PRAGMA`, `EXPLAIN`,
`WITH`, plus `.tables` and `.schema`). The last action prints on one line above
the status bar and clears on the next keypress.
