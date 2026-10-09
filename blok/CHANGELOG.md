# Changelog

All notable changes to blok. The format follows [Keep a Changelog][kac], and the
project uses [semantic versioning][semver].

[kac]: https://keepachangelog.com/en/1.1.0/
[semver]: https://semver.org/spec/v2.0.0.html

## [0.1.0] — 2026-10-09

The first release: a working terminal outliner in the spirit of Logseq, where
**SQLite is the document**. Journals start on today and a day that is only looked
at never exists; blocks, links, tags and properties are rows; search is FTS5;
backup is a `VACUUM INTO` snapshot file. The app has no top bar and no page
sidebar — navigation is one prompt, panels are asked for — and every screenshot
in the docs is produced by the app's own renderer.

### Added

**Storage**
* SQLite schema: `pages`, `blocks`, `refs`, `properties`, `backup_log`,
  `settings`, with soft deletes and a `position REAL` fractional index so a
  reorder of 4,000 siblings is one row write.
* FTS5 external-content index over block text (bm25 ranked), with a `LIKE`
  fallback when the SQLite build has no FTS5.
* `VACUUM INTO` snapshots while the database is open, a `backup_log` history, an
  integrity check, `wal_checkpoint(TRUNCATE)` before measuring, and an upload
  descriptor for an external copier. A live `.db` is never synced, and the design
  doc explains exactly why (WAL sidecars, hot journals, NFS locking, the
  3.7.0–3.51.2 WAL-reset race).

**Journals**
* Opens on today. A day is *provisional* until the first keystroke creates it.
* Empty journals are pruned on startup and on quit; `--demo-lifecycle` prints
  the create/prune trace against a real database.
* Day navigation is explicit: `:prev`, `:next`, `:e -1`, `:journal 2026-10-01`.

**Outline**
* Fold, indent, outdent, split at the caret, merge upward (with child
  re-parenting), reorder, delete, yank/paste, visual multi-select of blocks or a
  whole subtree, undo/redo.
* `TODO`/`DOING`/`DONE`/`LATER`/`NOW`/`WAITING`/`CANCELED` markers mirrored into a
  `status` column, so the board is a `WHERE` clause rather than a text scan.
* `key:: value` properties, `((block refs))` rendered as inline snippets,
  `[[page links]]`, `#tags`, and linked/unlinked references computed from `refs`.

**Editing**
* Vim's model with the **block as the line**: three modes, and one `Esc` always
  stops editing. The text cursor is a *position*, not a fourth mode.
* The caret is the terminal's own cursor, placed on the cell it sits on and drawn
  as an inverted cell as well, so a dumped frame is faithful.
* A text layer with counts, word and line motions, character finds (`f t F T ; ,`),
  **text objects** (`iw aw iW aW i" a" i' i\``, `i( a(`, `i[ a[`, `i{ a{`),
  operators (`d c y`) over motions, objects and lines, `x X s r ~ D C`, line
  operations (`dd cc yy p P J >> <<`), and a text register separate from the
  outline's.
* **Code blocks**: a leading fence means "do not interpret this". The body is
  rendered verbatim under a `CODE · lang` badge, and nothing inside reaches the
  `refs` or `properties` tables, the metadata panel or the link marker. With the
  caret inside, `Enter` is a newline (indented like the line above), the closing
  fence moves down, and the completion popups are off. `/Code` opens one.
* **Bracketed paste** as literal text: newlines stay newlines and nothing is
  re-indented. With no caret in a block, a paste becomes one new block.
* Word motions walk the outline: `w`/`e` at the end of a block carry on into the
  block below, `b`/`B` into the one above, as vim's `w` wraps to the next line.

**Navigation**
* `/` is Find: with no query it lists what you touched last, and as you type it
  searches page names *and* block text, landing on the matching block.
* `[` / `]` walk back and forward through the pages you have opened (vim's
  jumplist as a two-stack history); `f` follows the link on a block, with a
  chooser when there are several.
* `m` opens the page metadata: kind, timestamps, block count, links in/out, the
  page's properties, its linked references and its unlinked mentions. `Tab` shows
  and focuses the panel in one press.
* **One letter per action, and no Ctrl in NORMAL mode.** The Ctrl spellings
  (`Ctrl-P`, `Ctrl-M`, `gm`, `Ctrl-]`, `gf`, `Ctrl-o`/`Ctrl-i`, `Ctrl-r`,
  `Ctrl-s`, `Ctrl-w l`) remain as aliases.

**Views and chrome**
* Outline (journal and page), Find, TODO board, storage & backup, a scrollable
  keymap, and a read-only SQL console (`SELECT`, `PRAGMA`, `EXPLAIN`, `WITH`,
  `.tables`, `.schema`) for troubleshooting rather than for display.
* The pane title carries identity; the status bar carries the caret's
  `line, col` inside a block; the hint bar is computed from what is possible right
  now, measures itself, and pins its last pair so the way to the manual is never
  truncated away.
* 31 mockup frames, every one rendered by the app itself (`--dump` →
  `tools/render_png.py`).

### Fixed

Bugs found in review rounds before the first release, each with a regression test
that fails without its fix:

* `q` was documented in the keymap and never wired up; so were the other quit
  spellings (`ZZ`, `ZQ`, `:q`) — all of them now commit an in-flight edit first.
* `Ctrl-M` was unreachable: under the legacy encoding it *is* `Enter`, so the
  metadata panel had no key on most terminals. The app now requests the kitty
  keyboard protocol where available, and the panel has a letter (`m`).
* The caret existed only in screenshots — nothing ever placed the terminal
  cursor, and the glyph was drawn only by the mockup dumper.
* Once visible, the caret *moved the text*: it was an inserted cell, so every
  character after it shifted right and a full line re-wrapped. It is a style now.
* With two or more links in a block, `Enter` on the chosen link did nothing (the
  chooser's accept path was wired to nothing — only the single-link path had a
  test), and typing in the chooser inserted the characters into the *block*.
* `[`/`]` stepped journal days, and the jumplist underneath was off by one: a
  single jump could not be undone at all, and two jumps skipped a page.
* With the caret in a block, fourteen vim text keys fell through to the outline:
  `dd` deleted the block, `yy`/`p` yanked a block, `>>` indented it, `u` undid
  something else, `r`/`s`/`f`/`X`/`~` committed the buffer and then reported "no
  mapping", and `ciw` inserted a stray `w` into the sentence. The text grammar is
  now closed: a key it does not know is refused, and the caret stays put.
* `cw` ate the space after the word (it shared an implementation with `dw`), and
  text objects did not back up to the start of the word they were in.
* `Enter` inside a code block split the block, cutting the fence in half; the
  arrows left the block instead of moving between its lines; and a block taller
  than the pane took the caret off the screen.
* `Esc` lost your place: committing rebuilt the buffer with the caret at the end.
* Pasting did nothing when the terminal sent `Event::Paste` (the event loop
  dropped every event that was not a key), and where it did not, a pasted newline
  arrived as `Enter` — so pasting code split it per line and the code auto-indent
  added the previous line's indentation to every fragment.
* The Help and Storage screens rendered into a zero-height row and their frames
  were silently blank for two revisions.
* The hint bar had grown to 204 cells in a 140-cell row, truncating away exactly
  the two things it needed to show; the help screen's column headings were
  hardcoded over a computed split, so one keymap edit made them lie.

### Notes

* **Deliberate deviations from vim** (five, listed in `DESIGN.md` §7.7): `hjkl`
  navigate the tree, `Enter` puts the caret in a block, `J` joins blocks, `?`
  opens the keymap, and app-level commands live in `:`.
* **Reading mode needs no Ctrl**; the Ctrl spellings are aliases. `Ctrl-d`/`Ctrl-u`
  remain for half-page scrolling alongside PageUp/PageDown.
* **Honest scope**: the remote uploader writes the descriptor an `rclone` job
  would consume and does not talk to Dropbox; the graph "census" is counts, not a
  drawing; there is no restore UI (a documented `cp`); undo is session-level
  rather than per-keystroke; there is no syntax highlighting (only the language
  badge), no zoom, no `alias::`, no namespaces, no templates, no plugins, no
  whiteboards, and CJK width is wrong inside the editor's wrap.
* **There is no LICENSE file yet.** Logseq is AGPL-3.0 and §10 of the design doc
  treats licensing as a real gap in this deliverable rather than a footnote.

[0.1.0]: https://github.com/seymores/blokseq/releases/tag/v0.1.0
