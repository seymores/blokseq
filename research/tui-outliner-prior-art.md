# Prior art for a Rust TUI outliner / Logseq clone

**2026-10-08.** Versions/stars pulled this session from the crates.io and GitHub APIs. The brief's "2024–2025" framing is stale; I report current state and flag what is merely old.

---

## 1. TUI outliner / note apps that exist

### 1a. Direct prior art

**[outl](https://github.com/outlmd/outl)** (MIT) is the closest thing to a Rust TUI Logseq shipping today: *"Local-first outliner. Markdown is the source of truth. Sync that doesn't corrupt your tree when two devices edit offline."* Its [TUI manual](https://outl.app/docs/tui.html) documents a journal-first **modal (Normal / Insert / Visual)** editor — `j`/`k` block nav, `Tab` indent, `c` fold with a `▼`/`▶` marker ("two spaces otherwise"), `zR`/`zM` fold-all, `K`/`J` move block, `dd` delete, `Ctrl+P` fuzzy switcher, `/` slash, `:` palette, `B` backlinks. Two decisions to copy: fold state is an `Op::SetCollapsed` in an op log, so folds converge across devices, while `zi`/`zo` zoom is *"client-local view state, never written to the op log, so it's per-device and never syncs."* Its [sync page](https://outl.app/docs/sync.html) criticises Logseq for writing `id::` UUIDs into markdown and shipping "a file-rsync flavor — there is no merge algorithm," and git because it "treats the file as a sequence of lines." [Its shortcuts page](https://outl.app/docs/shortcuts.html) warns that 17 chords diverge between its documented catalog and the TUI's `match` arms, **6 destructively** (`Ctrl+Z` is Undo in the catalog but arms the fold chord).

**[IWE](https://github.com/iwe-org/iwe)** is a Rust Markdown LSP/CLI/MCP, not a TUI; its [comparison page](https://iwe.md/docs/concepts/comparison/) benchmarks marksman, markdown-oxide, Obsidian, `zk`/telekasten.nvim and mdbase.

### 1b. Obsidian-shaped TUIs (most active)

- **[basalt](https://github.com/erikjuhani/basalt)** — "TUI Application to manage Obsidian notes directly from the terminal", ~1.4k★, 2026-10-08, GPL-3.0 app + Apache-2.0 widgets. Not an Obsidian replacement; a WYSIWYG minimalist terminal UI. [Docs](https://basalt.page) cover tabs, panes and modals — best reference for **chrome**.
- **[clin-rs](https://github.com/reekta92/clin-rs)** — "A TUI reimagination of Obsidian", ~560★, 2026-10-02, GPL-3.0, MSRV 1.90.0, ~2–5 MB. Markdown edit *and* render, `.canvas`, force-directed graph, `ratatui-image` — best reference for **graph view + images**.

### 1c. Flat-note CLI tools (not outliners)

None has hierarchical foldable bullets — precisely the gap.

| Tool | What it is | Signal |
|---|---|---|
| [nb](https://github.com/xwmx/nb) | plain-text notes, bookmarks, tags, links; CLI + local web, no TUI | ~8.4k★, AGPL-3.0, 2026-08-26 |
| [zk](https://github.com/zk-org/zk) | "plain text note-taking assistant" + Zettelkasten index | ~2.8k★, GPL-3.0, 2026-10-08 |
| [jrnl](https://github.com/jrnl-org/jrnl) | CLI journaling; one-line capture, then `--edit` | ~7.3k★, GPL-3.0, 2026-10-05 |

### 1d. Editor-plugin ecosystems

- **[org-mode](https://orgmode.org/manual/Global-and-local-cycling.html) + org-roam** is the canonical outliner: "just two commands, bound to `TAB` and `S-TAB`." `org-cycle` rotates a subtree `FOLDED -> CHILDREN -> SUBTREE`; `org-global-cycle` rotates the buffer `OVERVIEW -> CONTENTS -> SHOW ALL`. The three-state local cycle is the most copyable folding convention.
- **[orgro](https://github.com/amake/orgro)** — Org Mode for iOS/Android (~735★, GPL-3.0, 2026-10-02). **Mobile, not TUI**; evidence that read-only fold + render is viable.
- **[vimwiki](https://github.com/vimwiki/vimwiki)** (~9.5k★, 2026-04-30) gets folding from Vim's `foldmethod=expr`, not the app.
- **[taskwiki](https://github.com/tbabej/taskwiki)** (~915★, 2025-06-14) + **[taskwarrior](https://github.com/GothenburgBitFactory/taskwarrior)** (~6.1k★, MIT, 2026-10-08) show that a view over a separate data engine leaks complexity.
- **[neuron](https://github.com/srid/neuron)** (~1.6k★) → **[emanote](https://github.com/srid/emanote)** (~963★): static-site wiki compilers, not TUIs; their backlink panels are a useful spec.

### 1e. Non-outliner TUIs worth studying

- **[meli](https://github.com/meli/meli)** (~897★, GPL-3.0, 2026-10-07) — Rust mail client, modal multi-pane UI; the closest structural analogue for three panes. **[sup](https://github.com/sup-heliotrope/sup)** (~972★, 2026-10-05) originated *threaded pane + tag-driven search as navigation*; **[circumflex](https://github.com/AmirulAndalib/circumflex)** shows clean list/detail navigation.
- **tuir**/`rtv`: no maintained upstream verifiable ([rtv was deleted from the AUR in 2022](https://lists.archlinux.org/pipermail/aur-requests/2022-June/072219.html)).

### 1f. Things that do not really exist

**No meaningful Logseq terminal client.** The only hit is [pitaya1219/logseq-view](https://github.com/pitaya1219/logseq-view) — MIT, **0 stars**, 2026-08-05, no description. No `obsidian-tui` repo surfaced; the ecosystem converged on `basalt`/`clin`. **[SilverBullet](https://github.com/silverbulletmd/silverbullet)** (~6.3k★, MIT, 2026-10-07) has a [CLI](https://silverbullet.md/cli) but is a browser app with a CLI entry point.

**Takeaway:** the niche is open but `outl` occupies it. Differentiate on Logseq fidelity (DB-backed blocks, `((block refs))`, journal-first), not on "TUI outliner" alone.

---

## 2. Ratatui / tui-rs ecosystem

**[ratatui](https://github.com/ratatui/ratatui)** is at **0.30.2** (crates.io 2026-06-19), ~22.9k★, MIT. **[tui-rs is archived](https://github.com/fdehau/tui-rs)** (last push 2023-08-06). 0.30.0 split the workspace [into `ratatui-core`, `ratatui-widgets` and per-backend crates](https://ratatui.rs/highlights/v030/), gained `no_std`, and added `ratatui::run()`. 0.30.2 added a `ratatui-termina` backend on [Helix's Termina VT library](https://github.com/helix-editor/termina) ([highlights](https://ratatui.rs/highlights/v0302/)). Sub-crates: `ratatui-core` 0.1.2, `ratatui-widgets` 0.3.2. **Widget authors should depend on `ratatui-core`** (slower-moving API). [crossterm](https://crates.io/crates/crossterm) **0.29.0** (2025-04-05) remains the safe default backend.

### Editable-text widgets — the critical choice

| Crate | Version (date) | Verdict |
|---|---|---|
| [tui-textarea](https://github.com/rhysd/tui-textarea) | 0.7.0 (2024-10-22) | **Unmaintained** — last push 2024-12-01 |
| [tui-textarea-2](https://crates.io/crates/tui-textarea-2) ([fork](https://github.com/srothgan/tui-textarea)) | **0.13.2** (2026-08-23) | **Best default.** Unicode-aware soft wrap (word/glyph/word-or-glyph) **plus visual-line cursor navigation**, undo/redo with word-sized coalescing, **atomic ranges** for "mentions, tokens, placeholders, and other indivisible editing spans", custom highlighted ranges, mouse hit-testing |
| [edtui](https://github.com/pondpilot/edtui) | **0.11.7** (2026-08-16) | **For widget-level modality.** Vim by default, `emacs_mode()` for modeless, custom keybindings, `wrap(true)`, syntect highlighting, system-editor escape hatch |
| [tui-input](https://github.com/sayanarijit/tui-input) | 0.15.5 (2026-09-26) | Single-line — palette/filter field only |
| [tui-scrollview](https://crates.io/crates/tui-scrollview) | 0.6.8 (2026-09-24) | Now under [`ratatui/tui-widgets`](https://github.com/ratatui/tui-widgets) |

**Recommendation:** hand-roll the outline list (virtualized, one row per visible block) and embed `tui-textarea-2` only for the row being edited. One giant textarea per page fights folding and cursor-in-block semantics.

**Other crates:** [ratatui-image](https://crates.io/crates/ratatui-image) **11.1.0** (2026-10-05; sixel/kitty/iTerm2/halfblocks); [comrak](https://crates.io/crates/comrak) **0.56.0** (CommonMark/GFM parse+format); [syntect](https://crates.io/crates/syntect) **5.3.0** (code fences); [unicode-width](https://crates.io/crates/unicode-width) **0.2.2** + [textwrap](https://crates.io/crates/textwrap) **0.16.4** (measurement).

**SQLite:** [rusqlite](https://crates.io/crates/rusqlite) **0.40.2** (2026-08-08), whose `backup` feature wraps `sqlite3_backup_*`. [libsql](https://crates.io/crates/libsql) **0.9.30** / [turso](https://crates.io/crates/turso) **0.8.2** exist, but their value is remote replication a local app does not need.

---

## 3. TUI interaction conventions worth copying

- **Modal beats modeless.** `outl` ships Normal/Insert/Visual with vim muscle memory (`i`/`a`/`A`, `dd`, `yy`, `p`/`P`, `u`). [edtui](https://github.com/pondpilot/edtui) offering both signals modeless demand — but a folding outliner *needs* Normal mode, because `j`/`k`, `Tab` and `c` must not insert characters.
- **Scheme:** vim-like is the norm (`outl`, `edtui` default, `basalt`). Helix's **selection-first** grammar ([Helix keymap](https://docs.helix-editor.com/keymap.html)) is the strongest alternative; a vim default plus visual-mode batch ops (`Tab` indent, `d` delete) captures most of it.
- **Folding keys:** pick one convention. Org's three-state `TAB`/`S-TAB` is most learnable; `outl` splits it into `c` (one), `zR`/`zM` (all). Don't bind folding to `Tab` if `Tab` indents, and never let a fold chord shadow undo.
- **Navigation:** `j`/`k` **and** arrows between blocks; `h`/`l` in-block; `K`/`J` to move a block. Collapsed subtrees must be *skipped* by `j`/`k` while staying in the tree.
- **Command surfaces:** ship all three — `Ctrl+P` fuzzy switcher, `/` slash menu, `:` palette. Engine: [skim](https://github.com/skim-rs/skim) **5.7.4** (2026-10-04), or in-process [nucleo](https://github.com/helix-editor/nucleo) (0.5.0).
- **Clipboard/undo:** yank via `arboard` with an **OSC 52 fallback** for SSH/tmux; make undo op-log-backed with **coalescing** of typing runs, so undo/redo and sync share one mechanism.

## 4. Rendering techniques for outliners in terminals

- **Bullets vs. box guides.** `outl`'s minimal scheme is robust: the fold column shows `▼`/`▶` when a block has children, "two spaces otherwise," so indentation never shifts. Box guides (`│ ├ └`) read better for deep trees but break under soft wrap and on continuation lines; if used, cap depth (`…` past ~6 levels). **Add child counts** (`▶ 3`) — cheap, and kills the "did I lose content?" anxiety a bare `▶` creates.
- **Soft wrap, never hard wrap** (hard wrap rewrites the file). Use `tui-textarea-2`'s **visual-line cursor navigation**: `↑`/`↓` move by *screen row*, and the cursor column maps back to a source byte offset. Treat "cursor in wrapped text" as a first-class test target, and compute all columns with `unicode-width` (never `char::count()`) — ratatui 0.30.2 fixed stale styling when wide cells were replaced by narrow ones.
- **Byte-faithful editing rows.** `outl` shows `:shortcode:` literals while editing and renders the glyph only in non-editing rows "so cursor columns match source bytes 1:1." Do the same for `[[refs]]`/`#tags`.
- **Inline highlighting:** `tui-textarea-2`'s **atomic ranges** are the right primitive for `[[page]]`, `#tag`, `((blk-XXXXXX))` — a stray backspace can't corrupt a handle.
- **Backlinks pane.** `outl` toggles it inline below the outline (`B`), sort flip `Ctrl+O` persisted to config. Inline-below suits narrow terminals; a right column suits wide ones. Show the *parent context path*, not just the referencing text.
- **Icons & mouse:** plain Unicode default with Nerd Font opt-in; keep mouse capture **off by default** so native terminal selection works.

---

## 5. Sync-free / local-first SQLite

### Schema shape

Logseq's DB version unified the model: "**Blocks and pages are united as nodes**… referenced as `[[]]` and blocks no longer use `(())`," all nodes have "created-at and updated-at timestamps," and "there is no re-index like in file graphs" ([db-version-changes.md](https://github.com/logseq/docs/blob/master/db-version-changes.md), from [PR #9858](https://github.com/logseq/logseq/pull/9858)). A workable core:

```sql
CREATE TABLE nodes (
  id INTEGER PRIMARY KEY, kind TEXT NOT NULL,       -- 'page' | 'block'
  parent_id INTEGER REFERENCES nodes(id), page_id INTEGER NOT NULL,
  ord REAL NOT NULL,                                 -- fractional index
  content TEXT NOT NULL DEFAULT '', collapsed INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER, updated_at INTEGER);
CREATE TABLE refs (src_id INTEGER, dst_id INTEGER, PRIMARY KEY (src_id, dst_id));
```

`ord REAL` is fractional indexing — what `outl` uses so "inserting between two positions doesn't renumber anyone." Logseq's own schema has `blocks` and `pages` tables plus a `blocks_fts_content` shadow table, visible in [issue #10985](https://github.com/logseq/logseq/issues/10985) where the two had diverged.

### FTS5

Use an **external-content** FTS5 table so text isn't duplicated (`content=` names the content table, `content_rowid=` its integer key). SQLite notes "it is still the responsibility of the user to ensure that the contents of an external content FTS5 table are kept up to date," and gives the triggers verbatim ([FTS5 §4.4.3](https://www.sqlite.org/fts5.html)):

```sql
CREATE VIRTUAL TABLE node_fts USING fts5(content, content='nodes',
  content_rowid='id', tokenize='unicode61 porter');
CREATE TRIGGER nodes_ai AFTER INSERT ON nodes BEGIN
  INSERT INTO node_fts(rowid, content) VALUES (new.id, new.content); END;
CREATE TRIGGER nodes_ad AFTER DELETE ON nodes BEGIN
  INSERT INTO node_fts(node_fts, rowid, content) VALUES('delete', old.id, old.content); END;
CREATE TRIGGER nodes_au AFTER UPDATE ON nodes BEGIN
  INSERT INTO node_fts(node_fts, rowid, content) VALUES('delete', old.id, old.content);
  INSERT INTO node_fts(rowid, content) VALUES (new.id, new.content); END;
```

`bm25()` returns **numerically smaller for better matches** (it multiplies by −1 precisely so `ORDER BY bm25(...)` ascending is correct) and takes per-column weights as extra float args. `snippet()`/`highlight()` drive preview and backlinks. Per [§4.4.4](https://www.sqlite.org/fts5.html), drift between index and content table makes `SELECT * FROM ft` (no `MATCH`) return everything while `ft('term')` returns nothing — so ship an `INSERT INTO node_fts(node_fts) VALUES('rebuild');` recovery path.

### CRDTs

[cr-sqlite](https://github.com/vlcn-io/cr-sqlite) (~3.8k★, MIT, 2026-08-10) is a **runtime-loadable SQLite extension** adding multi-master replication and CRDTs; it is not published on crates.io under that name. [automerge](https://crates.io/crates/automerge) is **0.12.0** (2026-09-16), a separate library, not a SQLite extension. `outl` instead uses a tree CRDT ([Kleppmann et al. 2022](https://martin.kleppmann.com/papers/move-op.pdf)) with HLC timestamps and Yrs for in-block text — the DB is a *projection*, not the source of truth. Most robust, most work.

### Why syncing a live SQLite file is risky

SQLite's [How To Corrupt An SQLite Database File](https://www.sqlite.org/howtocorrupt.html) is the canonical citation:

- In WAL mode state lives in **three files** (`.db`, `-wal`, `-shm`); "Copying a database file without also copying its journal" is among actions "likely to lead to corruption."
- "Backup or restore while a transaction is active" can capture "some old and some new content, and thus be corrupt."
- Deleting or mispairing a hot journal breaks automatic recovery.
- Locking is broken on some filesystems — "especially true of network filesystems and NFS in particular."
- On unix an unrelated `close()` on the same file **cancels POSIX advisory locks** for all threads; SQLite only added defenses in 3.51.0 (2025-11-04).
- A real **WAL-reset race** corrupted databases from 3.7.0 through **3.51.2** under concurrent writes/checkpoints.

File-sync tools worsen this by copying whole files on their own schedule. Syncthing illustrates the class of problem: [Checkpoint starvation #10559](https://github.com/syncthing/syncthing/issues/10559) (open, 2026-02-04, v2.0.14) — `wal_checkpoint(TRUNCATE)` gives no guarantee readers are active, so the WAL "will keep its size or even grow further"; plus [SQLITE_BUSY #10529](https://github.com/syncthing/syncthing/issues/10529) (open, 2026-01-16). Both concern Syncthing's *own* v2 DB, but they show the concurrency constraints; the file-level risk to *your* DB is the sidecar/copy problem above.

### Safe alternatives

1. **[`VACUUM INTO 'file.db'`](https://www.sqlite.org/lang_vacuum.html#vacuuminto)** — "transactional in the sense that the generated output database is a consistent snapshot"; target must not already exist; purges deleted content.
2. **[Online Backup API](https://www.sqlite.org/backup.html)** (`sqlite3_backup_*`, wrapped by rusqlite's `backup` feature) — incremental, briefly locks the source, "the destination becomes a 'snapshot'"; `backup_remaining()`/`backup_pagecount()` drive a progress UI.
3. **[`sqlite3_rsync`](https://www.sqlite.org/rsync.html)** — new in SQLite **3.47.0 (2024-10-21)**; consistent copy of a *live* DB over SSH. Best no-server way to move a DB between machines.
4. **`PRAGMA wal_checkpoint(TRUNCATE)`** before any manual copy, and copy `.db` + `-wal` + `-shm` **together**.

**Bottom line:** one writer, WAL mode, never sync the live `.db`. Either treat markdown/JSONL as the syncable artifact and rebuild SQLite locally (Logseq file-graph style — the DB is a cache), or keep SQLite authoritative and sync only `VACUUM INTO` snapshots plus an op log.

---

## Concrete recommendations for the mockup

1. **Three panes** (journal/page tree, centre outline, right backlinks), side panes **hidden by default** — `outl` ships `show_sidebar: false, show_backlinks: false`.
2. **Modal by default:** `Tab`=indent, `c`=fold, `Ctrl+P`=quick switch, `/`=slash, `:`=palette, `Ctrl+B`=backlinks.
3. **Fixed-width fold column** (`▼`/`▶`/two spaces) plus child counts.
4. **Embedded editing row** via `tui-textarea-2`: soft wrap, visual-line navigation, atomic ranges for `[[refs]]`/`#tags`.
5. **SQLite + FTS5 behind an op log**, with `VACUUM INTO` as the export/sync primitive.
