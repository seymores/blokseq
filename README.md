# Logseq in the terminal — research + a working Rust TUI prototype

Three deliverables from one brief: research Logseq's value proposition, extract
the features users actually care about, then mock up a Rust TUI rewrite that is
SQLite-backed (no markdown) with journal-first startup, empty-journal pruning and
snapshot-based remote backup.

| Deliverable | Where |
|---|---|
| **Design + UI/UX mockups** (the main document) | [`blok/DESIGN.md`](blok/DESIGN.md) |
| **Working prototype** (Rust, ratatui + rusqlite) | [`blok/`](blok/README.md) — `cd blok && cargo run` |
| **Logseq value proposition, ranked top-20 features** | [`research/logseq-value-proposition.md`](research/logseq-value-proposition.md) |
| **TUI outliner prior art, including `outl`** | [`research/tui-outliner-prior-art.md`](research/tui-outliner-prior-art.md) |

## The short version

**What users love about Logseq** — ranked from a corpus of 4,278 r/logseq posts,
32,699 comments, the official forum, Hacker News and Product Hunt, with quotes and
56 source URLs in [the research doc](research/logseq-value-proposition.md):
**1** local-first plain-text storage and data ownership, **2** journal-first daily
notes, **3** the block-level outliner with zoom, **4** `((block references))` and
transclusion, **5** bidirectional links with Linked/Unlinked References, then
AGPL licensing, queries, built-in task management, properties, "everything is a
page", templates, namespaces, block refactoring, PDF annotation, plugins,
publishing, sync, aliases, whiteboards, flashcards.

The research also names the three pain points this design attacks head-on: graphs
that die on size (an in-memory datascript DB), sync bugs that lose data, and
**empty journal pages that should never have been created** — "The file should
only be created when you start writing to it" ([forum](https://discuss.logseq.com/t/i-do-not-want-empty-diaries-to-occur/25620)).

**What the prototype does about it.** `blok` is a real, runnable TUI whose 29
screenshots are produced by its own renderer (`blok --dump dumps` renders each
frame through ratatui's `TestBackend`; `tools/render_png.py` rasterises the
24-bit-colour ANSI). The features you asked for are implemented against a real
SQLite database, not faked in the mockup:

* **Journal auto-start on the current day.** The app opens on today. A day with
  no content is *provisional*: the page row is created by the first keystroke,
  not by the visit.
* **Empty journals are pruned.** No page row, no file, no orphan day — pruned on
  startup and on quit. `blok --demo-lifecycle` prints the create/prune trace
  against a real database.
* **SQLite is the document.** Blocks, links, tags, properties and task state are
  rows; search is an FTS5 external-content index with bm25 ranking.
* **Backup = `VACUUM INTO` snapshot → remote copy.** Never sync the live
  database file; the design doc explains exactly why (WAL sidecars, hot
  journals, broken advisory locking on network filesystems, the 3.7.0–3.51.2
  WAL-reset race) and what the safe primitives are.
* **Navigation in one key**: `Ctrl-P` opens **Find** — recent pages first, then a
  live search over page names *and* block text, landing on the matching block.
  There is no top bar and no page sidebar; the pane title carries the context and
  the hint bar is computed from what is possible right now.
* **Block editing in a TUI**: embedded single-block editor with a real caret
  inside soft-wrapped text, `/` command menu, `[[` page autocomplete, `((` block
  ref search, Alt-⏎ newlines inside one block, and structure verbs — indent,
  outdent, split, merge with child re-parenting, reorder, fold, visual
  multi-select, undo. Vim's grammar with the block as the line, three modes, and
  one `Esc` always leaves editing.

![Journal screen](blok/dumps/png/01-journal.png)

**Honest scope.** Whiteboards, plugins, mobile, PDF annotation and a visual graph
view are deliberately out — §10 of the design doc maps every ranked Logseq
feature to *built / partial / roadmap / dropped*, and §11 separates what is real
in the prototype from what is stubbed (the remote uploader writes the descriptor
an `rclone` job would consume; it does not talk to Dropbox).

## Reproduce everything

```bash
cd blok
cargo run -- --dump dumps                       # re-render all 29 frames
python3 tools/render_png.py dumps dumps/png     # frames -> PNG screenshots
cargo run -- --demo-lifecycle                   # journal lifecycle trace
cargo run -- --stats                            # counters + integrity check
cargo run                                       # the interactive TUI
```
