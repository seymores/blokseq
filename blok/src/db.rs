//! SQLite storage. There is no markdown document on disk, ever: blocks are rows,
//! links are rows, and full-text search is an FTS5 index over the block table.
//!//! Durability model (what the "remote backup" feature leans on):
//!   * WAL journal mode, so a crash never corrupts the file.
//!   * `VACUUM INTO` produces a consistent, compacted single-file snapshot even
//!     while the database is open -- safe to copy to a remote.
//!   * `backup_log` records every snapshot, its size and where it was sent.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};
use rusqlite::{params, Connection, OptionalExtension};

use crate::model::{
    parse_refs, Block, DbStats, JournalDay, Page, PageKind, ParsedRef, RefHit, Row, Snapshot,
};

pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS pages (
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE,          -- journal: '2026-02-14', page: 'Project Aurora'
  kind       TEXT NOT NULL CHECK (kind IN ('journal','page')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT
);
CREATE INDEX IF NOT EXISTS pages_kind ON pages(kind, name);

CREATE TABLE IF NOT EXISTS blocks (
  id         INTEGER PRIMARY KEY,
  uuid       TEXT NOT NULL UNIQUE,
  page_id    INTEGER NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
  parent_id  INTEGER REFERENCES blocks(id) ON DELETE CASCADE,
  position   REAL NOT NULL,                 -- fractional index among siblings
  content    TEXT NOT NULL DEFAULT '',      -- markdown-ish *syntax*, never a file
  status     TEXT,                          -- TODO/DOING/DONE/LATER/CANCELED
  priority   TEXT,
  collapsed  INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  deleted_at TEXT
);
CREATE INDEX IF NOT EXISTS blocks_page ON blocks(page_id, parent_id, position);
CREATE INDEX IF NOT EXISTS blocks_parent ON blocks(parent_id, position);

CREATE TABLE IF NOT EXISTS refs (
  id           INTEGER PRIMARY KEY,
  from_block   INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
  kind         TEXT NOT NULL CHECK (kind IN ('page','block','tag')),
  page_id      INTEGER REFERENCES pages(id) ON DELETE CASCADE,
  target_block INTEGER REFERENCES blocks(id) ON DELETE CASCADE,
  UNIQUE (from_block, kind, page_id, target_block)
);
CREATE INDEX IF NOT EXISTS refs_target ON refs(target_block);
CREATE INDEX IF NOT EXISTS refs_page ON refs(page_id);

CREATE TABLE IF NOT EXISTS backup_log (
  id        INTEGER PRIMARY KEY,
  file      TEXT NOT NULL,
  bytes     INTEGER NOT NULL,
  taken_at  TEXT NOT NULL,
  remote    TEXT NOT NULL,
  state     TEXT NOT NULL                    -- queued | uploaded | local-only
);

CREATE TABLE IF NOT EXISTS properties (
  block_id INTEGER NOT NULL REFERENCES blocks(id) ON DELETE CASCADE,
  key      TEXT NOT NULL,
  value    TEXT NOT NULL,
  PRIMARY KEY (block_id, key)
);

CREATE TABLE IF NOT EXISTS settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT OR IGNORE INTO settings(key, value) VALUES
  ('remote_target', 'dropbox:Apps/blok'),
  ('snapshot_keep', '30'),
  ('prune_empty_journals', 'true');
"#;

pub const FTS_SCHEMA: &str = r#"
CREATE VIRTUAL TABLE IF NOT EXISTS blocks_fts
  USING fts5(content, content='blocks', content_rowid='id', tokenize='porter unicode61');

CREATE TRIGGER IF NOT EXISTS blocks_ai AFTER INSERT ON blocks BEGIN
  INSERT INTO blocks_fts(rowid, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER IF NOT EXISTS blocks_ad AFTER DELETE ON blocks BEGIN
  INSERT INTO blocks_fts(blocks_fts, rowid, content) VALUES('delete', old.id, old.content);
END;
CREATE TRIGGER IF NOT EXISTS blocks_au AFTER UPDATE ON blocks BEGIN
  INSERT INTO blocks_fts(blocks_fts, rowid, content) VALUES('delete', old.id, old.content);
  INSERT INTO blocks_fts(rowid, content) VALUES (new.id, new.content);
END;
"#;

pub struct Db {
    pub conn: Connection,
    pub path: PathBuf,
    pub fts: bool,
    pub stats: DbStats,
}

fn now() -> String {
    Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

impl Db {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        let fts = conn.execute_batch(FTS_SCHEMA).is_ok();
        let mut db = Self {
            conn,
            path: path.to_path_buf(),
            fts,
            stats: DbStats::default(),
        };
        db.stats = db.compute_stats();
        Ok(db)
    }

    // ---------------------------------------------------------------- pages

    pub fn page_by_name(&self, name: &str) -> Option<Page> {
        self.conn
            .query_row(
                "SELECT id, name, kind, created_at FROM pages WHERE name = ?1 AND deleted_at IS NULL",
                params![name],
                row_to_page,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn journal_page(&self, day: JournalDay) -> Option<Page> {
        self.page_by_name(&day.key())
    }

    /// Create the day's page row. Only ever called when the first block is
    /// committed -- an untouched journal never exists in the database.
    pub fn ensure_journal(&self, day: JournalDay) -> Page {
        self.ensure_page(&day.key(), PageKind::Journal)
    }

    pub fn ensure_page(&self, name: &str, kind: PageKind) -> Page {
        if let Some(p) = self.page_by_name(name) {
            return p;
        }
        let ts = now();
        let _ = self.conn.execute(
            "INSERT OR IGNORE INTO pages(name, kind, created_at, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![name, kind.as_str(), ts, ts],
        );
        self.page_by_name(name).unwrap_or(Page {
            id: -1,
            name: name.to_string(),
            kind,
            created_at: ts,
        })
    }

    pub fn page_block_ids(&self, page_id: i64) -> Vec<i64> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM blocks WHERE page_id = ?1 AND deleted_at IS NULL")
            .unwrap();
        let rows = stmt
            .query_map(params![page_id], |r| r.get::<_, i64>(0))
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Sidebar: journals that actually have content, newest first.
    pub fn journals(&self, limit: i64) -> Vec<(JournalDay, i64, String)> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.name,
                        (SELECT COUNT(*) FROM blocks b WHERE b.page_id = p.id AND b.deleted_at IS NULL),
                        COALESCE((SELECT b.content FROM blocks b
                                   WHERE b.page_id = p.id AND TRIM(b.content) <> ''
                                   ORDER BY b.position LIMIT 1), '')
                 FROM pages p
                 WHERE p.kind = 'journal' AND p.deleted_at IS NULL
                   AND EXISTS (SELECT 1 FROM blocks b WHERE b.page_id = p.id AND TRIM(b.content) <> '')
                 ORDER BY p.name DESC LIMIT ?1",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })
            .unwrap();
        rows.filter_map(|r| r.ok())
            .filter_map(|(name, n, preview)| {
                crate::model::parse_journal_key(&name).map(|d| (d, n, preview))
            })
            .collect()
    }

    /// Sidebar: named pages, most-linked first.
    pub fn pages(&self, limit: i64) -> Vec<(String, i64, i64)> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.name,
                        (SELECT COUNT(*) FROM refs r WHERE r.page_id = p.id),
                        (SELECT COUNT(*) FROM blocks b WHERE b.page_id = p.id AND b.deleted_at IS NULL)
                 FROM pages p
                 WHERE p.kind = 'page' AND p.deleted_at IS NULL
                 ORDER BY 2 DESC, p.name ASC LIMIT ?1",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    // --------------------------------------------------------------- blocks

    pub fn page_blocks(&self, page_id: i64) -> Vec<Block> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, uuid, page_id, parent_id, position, content, status, collapsed, created_at
                 FROM blocks WHERE page_id = ?1 AND deleted_at IS NULL
                 ORDER BY position, id",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![page_id], row_to_block)
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Flatten the block tree into visible rows, honouring collapse state.
    pub fn rows(&self, page_id: i64) -> Vec<Row> {
        let blocks = self.page_blocks(page_id);
        let mut kids: HashMap<Option<i64>, Vec<Block>> = HashMap::new();
        for b in blocks {
            kids.entry(b.parent_id).or_default().push(b);
        }
        for v in kids.values_mut() {
            v.sort_by(|a, b| {
                a.position
                    .partial_cmp(&b.position)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.id.cmp(&b.id))
            });
        }
        let mut out = Vec::new();
        walk(&kids, None, 0, &mut Vec::new(), &mut out);
        out
    }

    fn next_position(&self, page_id: i64, parent: Option<i64>) -> f64 {
        let max: Option<f64> = self
            .conn
            .query_row(
                "SELECT MAX(position) FROM blocks WHERE page_id = ?1
                   AND deleted_at IS NULL AND parent_id IS ?2",
                params![page_id, parent],
                |r| r.get::<_, Option<f64>>(0),
            )
            .ok()
            .flatten();
        max.unwrap_or(0.0) + 1.0
    }

    pub fn create_block(
        &self,
        page_id: i64,
        parent_id: Option<i64>,
        after: Option<i64>,
        content: &str,
    ) -> Block {
        let pos = match after {
            Some(prev) => self
                .conn
                .query_row(
                    "SELECT position FROM blocks WHERE id = ?1",
                    params![prev],
                    |r| r.get::<_, f64>(0),
                )
                .optional()
                .ok()
                .flatten()
                .map(|p| p + 0.5)
                .unwrap_or_else(|| self.next_position(page_id, parent_id)),
            None => self.next_position(page_id, parent_id),
        };
        let ts = now();
        self.conn
            .execute(
                "INSERT INTO blocks(uuid, page_id, parent_id, position, content, created_at, updated_at)
                 VALUES (lower(hex(randomblob(8))), ?1, ?2, ?3, ?4, ?5, ?6)",
                params![page_id, parent_id, pos, content, ts, ts],
            )
            .unwrap();
        let id = self.conn.last_insert_rowid();
        // Human-readable, stable handle: blk-0000002a. Synced as text, never as
        // a row id.
        let uuid = crate::model::uuid_for(id);
        self.conn
            .execute("UPDATE blocks SET uuid = ?1 WHERE id = ?2", params![uuid, id])
            .unwrap();
        if !content.trim().is_empty() {
            self.reindex_refs(id, content);
        }
        self.touch_page(page_id);
        Block {
            id,
            uuid,
            page_id,
            parent_id,
            position: pos,
            content: content.to_string(),
            status: None,
            collapsed: false,
            created_at: ts,
        }
    }

    pub fn block(&self, id: i64) -> Option<Block> {
        self.conn
            .query_row(
                "SELECT id, uuid, page_id, parent_id, position, content, status, collapsed, created_at
                 FROM blocks WHERE id = ?1",
                params![id],
                row_to_block,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn block_by_uuid(&self, uuid: &str) -> Option<Block> {
        self.conn
            .query_row(
                "SELECT id, uuid, page_id, parent_id, position, content, status, collapsed, created_at
                 FROM blocks WHERE uuid = ?1",
                params![uuid],
                row_to_block,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn update_content(&self, id: i64, content: &str) {
        let _ = self.conn.execute(
            "UPDATE blocks SET content = ?1, updated_at = ?2 WHERE id = ?3",
            params![content, now(), id],
        );
        self.reindex_refs(id, content);
        if let Some(b) = self.block(id) {
            self.touch_page(b.page_id);
        }
    }

    pub fn set_status(&self, id: i64, status: Option<&str>) {
        let _ = self.conn.execute(
            "UPDATE blocks SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now(), id],
        );
    }

    pub fn set_collapsed(&self, id: i64, collapsed: bool) {
        let _ = self.conn.execute(
            "UPDATE blocks SET collapsed = ?1 WHERE id = ?2",
            params![collapsed as i64, id],
        );
    }

    /// Tab: adopt the previous sibling as parent.
    pub fn indent(&self, id: i64) {
        let Some(b) = self.block(id) else { return };
        let prev: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM blocks WHERE page_id = ?1 AND deleted_at IS NULL
                   AND parent_id IS ?2 AND position < ?3
                 ORDER BY position DESC LIMIT 1",
                params![b.page_id, b.parent_id, b.position],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .ok()
            .flatten();
        if let Some(prev) = prev {
            let pos = self.next_position(b.page_id, Some(prev));
            let _ = self.conn.execute(
                "UPDATE blocks SET parent_id = ?1, position = ?2, updated_at = ?3 WHERE id = ?4",
                params![prev, pos, now(), id],
            );
        }
    }

    /// Shift-Tab: become a sibling of the parent, right after it.
    pub fn outdent(&self, id: i64) {
        let Some(b) = self.block(id) else { return };
        let Some(parent) = b.parent_id else { return };
        let Some(pb) = self.block(parent) else { return };
        let _ = self.conn.execute(
            "UPDATE blocks SET parent_id = ?1, position = ?2, updated_at = ?3 WHERE id = ?4",
            params![pb.parent_id, pb.position + 0.5, now(), id],
        );
    }

    pub fn delete_block(&self, id: i64) {
        let _ = self.conn.execute(
            "UPDATE blocks SET deleted_at = ?1 WHERE id = ?2",
            params![now(), id],
        );
    }

    /// Enter at end of a block: new empty sibling underneath, plus any children
    /// stay attached to the original block (Logseq semantics).
    pub fn insert_sibling_after(&self, id: i64) -> Option<Block> {
        let b = self.block(id)?;
        Some(self.create_block(b.page_id, b.parent_id, Some(id), ""))
    }

    /// Enter in the middle of a block: split the text, keep children on top.
    pub fn split_block(&self, id: i64, head: &str, tail: &str) -> Option<Block> {
        let b = self.block(id)?;
        self.update_content(id, head);
        let nb = self.create_block(b.page_id, b.parent_id, Some(id), tail);
        // children mention nothing about their parent in the UI, so they stay put
        Some(nb)
    }

    /// Backspace on an empty block: fold it into the previous sibling.
    pub fn merge_into_previous(&self, id: i64) -> Option<i64> {
        let b = self.block(id)?;
        let prev: Option<Block> = self
            .conn
            .query_row(
                "SELECT id, uuid, page_id, parent_id, position, content, status, collapsed, created_at
                 FROM blocks WHERE page_id = ?1 AND deleted_at IS NULL
                   AND parent_id IS ?2 AND position < ?3
                 ORDER BY position DESC LIMIT 1",
                params![b.page_id, b.parent_id, b.position],
                row_to_block,
            )
            .optional()
            .ok()
            .flatten();
        let Some(prev) = prev else { return None };
        if !b.content.is_empty() {
            let joined = format!("{}{}", prev.content, b.content);
            self.update_content(prev.id, &joined);
        }
        // orphan children move up to the previous sibling
        let _ = self.conn.execute(
            "UPDATE blocks SET parent_id = ?1 WHERE parent_id = ?2",
            params![prev.id, id],
        );
        self.delete_block(id);
        Some(prev.id)
    }

    /// Alt-Up / Alt-Down: swap position with the neighbouring sibling.
    pub fn move_block(&self, id: i64, up: bool) -> bool {
        let Some(b) = self.block(id) else { return false };
        let cmp = if up { "<" } else { ">" };
        let ord = if up { "DESC" } else { "ASC" };
        let sql = format!(
            "SELECT id, position FROM blocks WHERE page_id = ?1 AND deleted_at IS NULL
               AND parent_id IS ?2 AND position {cmp} ?3 ORDER BY position {ord} LIMIT 1"
        );
        let nb: Option<(i64, f64)> = self
            .conn
            .query_row(&sql, params![b.page_id, b.parent_id, b.position], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, f64>(1)?))
            })
            .optional()
            .ok()
            .flatten();
        let Some((nid, npos)) = nb else { return false };
        let _ = self.conn.execute(
            "UPDATE blocks SET position = ?1 WHERE id = ?2",
            params![npos, id],
        );
        let _ = self.conn.execute(
            "UPDATE blocks SET position = ?1 WHERE id = ?2",
            params![b.position, nid],
        );
        true
    }

    // ----------------------------------------------------------------- refs

    pub fn reindex_refs(&self, block_id: i64, content: &str) {
        let _ = self
            .conn
            .execute("DELETE FROM refs WHERE from_block = ?1", params![block_id]);
        let _ = self
            .conn
            .execute("DELETE FROM properties WHERE block_id = ?1", params![block_id]);
        // A leading TODO/DOING/DONE marker becomes a typed column, so the board
        // and any query never have to look at text.
        let (marker, _rest) = crate::model::split_status(content);
        match marker {
            Some(m) => {
                let _ = self.conn.execute(
                    "UPDATE blocks SET status = ?1 WHERE id = ?2",
                    params![m, block_id],
                );
            }
            None => {
                let _ = self.conn.execute(
                    "UPDATE blocks SET status = NULL WHERE id = ?1 AND status IN
                       ('TODO','DOING','DONE','LATER','NOW','WAITING','CANCELED')",
                    params![block_id],
                );
            }
        }
        for (k, v) in crate::model::properties(content) {
            let _ = self.conn.execute(
                "INSERT OR REPLACE INTO properties(block_id, key, value) VALUES (?1, ?2, ?3)",
                params![block_id, k, v],
            );
            if k == "status" {
                let _ = self
                    .conn
                    .execute("UPDATE blocks SET status = ?1 WHERE id = ?2", params![v, block_id]);
            }
        }
        for r in parse_refs(content) {
            match r {
                ParsedRef::Page(name) => {
                    let page = self.ensure_page(&name, PageKind::Page);
                    let _ = self.conn.execute(
                        "INSERT OR IGNORE INTO refs(from_block, kind, page_id) VALUES (?1, 'page', ?2)",
                        params![block_id, page.id],
                    );
                }
                ParsedRef::Tag(name) => {
                    let page = self.ensure_page(&name, PageKind::Page);
                    let _ = self.conn.execute(
                        "INSERT OR IGNORE INTO refs(from_block, kind, page_id) VALUES (?1, 'tag', ?2)",
                        params![block_id, page.id],
                    );
                }
                ParsedRef::BlockRef(uuid) => {
                    if let Some(target) = self.block_by_uuid(&uuid) {
                        let _ = self.conn.execute(
                            "INSERT OR IGNORE INTO refs(from_block, kind, target_block) VALUES (?1, 'block', ?2)",
                            params![block_id, target.id],
                        );
                    }
                }
            }
        }
    }

    /// Blocks that link to this page or to any block on it.
    pub fn linked_refs(&self, page_id: i64) -> Vec<(String, Vec<RefHit>)> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT b.id, b.content, p.name, p.kind
                   FROM refs r
                   JOIN blocks b ON b.id = r.from_block AND b.deleted_at IS NULL
                   JOIN pages  p ON p.id = b.page_id
                  WHERE r.page_id = ?1
                     OR r.target_block IN (SELECT id FROM blocks WHERE page_id = ?1 AND deleted_at IS NULL)
                  ORDER BY p.name DESC, b.position
                  LIMIT 60",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![page_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap();
        let mut grouped: Vec<(String, Vec<RefHit>)> = Vec::new();
        for (id, content, page, kind) in rows.filter_map(|r| r.ok()) {
            let hit = RefHit {
                block_id: id,
                page: page.clone(),
                is_journal: kind == "journal",
                content,
                kind: "page".into(),
            };
            match grouped.last_mut() {
                Some((name, v)) if *name == page => v.push(hit),
                _ => grouped.push((page, vec![hit])),
            }
        }
        grouped
    }

    /// FTS5 when available, LIKE as the fallback.
    pub fn search(&self, q: &str, limit: i64) -> Vec<RefHit> {
        let trimmed = q.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        if self.fts {
            let matchq = trimmed
                .split_whitespace()
                .map(|t| format!("\"{}\"*", t.replace('"', "")))
                .collect::<Vec<_>>()
                .join(" ");
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT b.id, b.content, p.name, p.kind
                       FROM blocks_fts f
                       JOIN blocks b ON b.id = f.rowid AND b.deleted_at IS NULL
                       JOIN pages  p ON p.id = b.page_id
                      WHERE blocks_fts MATCH ?1
                      ORDER BY bm25(blocks_fts) LIMIT ?2",
                )
                .unwrap();
            let rows = stmt
                .query_map(params![matchq, limit], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })
                .unwrap();
            return rows
                .filter_map(|r| r.ok())
                .map(|(id, content, page, kind)| RefHit {
                    block_id: id,
                    page,
                    is_journal: kind == "journal",
                    content,
                    kind: "fts".into(),
                })
                .collect();
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT b.id, b.content, p.name, p.kind
                   FROM blocks b JOIN pages p ON p.id = b.page_id
                  WHERE b.deleted_at IS NULL AND b.content LIKE ?1
                  ORDER BY b.updated_at DESC LIMIT ?2",
            )
            .unwrap();
        let like = format!("%{}%", trimmed);
        let rows = stmt
            .query_map(params![like, limit], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap();
        rows.filter_map(|r| r.ok())
            .map(|(id, content, page, kind)| RefHit {
                block_id: id,
                page,
                is_journal: kind == "journal",
                content,
                kind: "like".into(),
            })
            .collect()
    }

    // ------------------------------------------------------------ lifecycle

    /// Journals are prunable: a day with no non-empty block does not deserve a
    /// row. Called on startup and on quit.
    pub fn prune_empty_journals(&self) -> Vec<String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.name FROM pages p
                  WHERE p.kind = 'journal' AND p.deleted_at IS NULL
                    AND NOT EXISTS (SELECT 1 FROM blocks b
                                     WHERE b.page_id = p.id AND TRIM(b.content) <> '' AND b.deleted_at IS NULL)
                  ORDER BY p.name",
            )
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);
        for name in &names {
            let _ = self.conn.execute(
                "DELETE FROM blocks WHERE page_id IN (SELECT id FROM pages WHERE name = ?1)",
                params![name],
            );
            let _ = self
                .conn
                .execute("DELETE FROM pages WHERE name = ?1", params![name]);
        }
        names
    }

    /// Blocks that exist but hold nothing -- transient state that a save pass
    /// would drop. Reported on the maintenance screen.
    pub fn empty_block_count(&self) -> i64 {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM blocks b
                  WHERE b.deleted_at IS NULL AND TRIM(b.content) = ''
                    AND NOT EXISTS (SELECT 1 FROM blocks c WHERE c.parent_id = b.id AND c.deleted_at IS NULL)",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    fn touch_page(&self, page_id: i64) {
        let _ = self.conn.execute(
            "UPDATE pages SET updated_at = ?1 WHERE id = ?2",
            params![now(), page_id],
        );
    }

    // --------------------------------------------------------------- backup

    pub fn remote_target(&self) -> String {
        self.get_setting("remote_target")
            .unwrap_or_else(|| "dropbox:Apps/blok".into())
    }

    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .ok()
            .flatten()
    }

    /// Consistent single-file snapshot, safe while the app is running.
    /// This is the whole sync story: copy one file to a remote.
    pub fn backup_snapshot(&self, dir: &Path, label: &str) -> (String, u64) {
        let _ = fs::create_dir_all(dir);
        let file = dir.join(format!("blok-{}.sqlite", label));
        let _ = fs::remove_file(&file);
        let path = file.to_string_lossy().to_string();
        if self
            .conn
            .execute("VACUUM INTO ?1", params![path])
            .is_err()
        {
            // Older SQLite fallback: plain file copy after a checkpoint.
            let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
            let _ = fs::copy(&self.path, &file);
        }
        let bytes = fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        let _ = self.conn.execute(
            "INSERT INTO backup_log(file, bytes, taken_at, remote, state) VALUES (?1, ?2, ?3, ?4, 'queued')",
            params![path, bytes as i64, now(), self.remote_target()],
        );
        (path, bytes)
    }

    /// Stand-in for the remote upload (`rclone copy` in the real thing): writes
    /// the descriptor that the uploader would consume.
    pub fn queue_remote_upload(&self, snapshot: &str, dir: &Path) -> String {
        let _ = fs::create_dir_all(dir);
        let desc = dir.join(format!(
            "{}.upload.json",
            Path::new(snapshot)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "snapshot".into())
        ));
        let body = format!(
            "{{\n  \"file\": \"{}\",\n  \"bytes\": {},\n  \"remote\": \"{}\",\n  \"op\": \"rclone copy --checksum\",\n  \"created\": \"{}\"\n}}\n",
            snapshot,
            fs::metadata(snapshot).map(|m| m.len()).unwrap_or(0),
            self.remote_target(),
            now()
        );
        let _ = fs::write(&desc, body);
        let _ = self.conn.execute(
            "UPDATE backup_log SET state = 'uploaded' WHERE file = ?1",
            params![snapshot],
        );
        desc.to_string_lossy().to_string()
    }

    pub fn snapshots(&self, limit: i64) -> Vec<Snapshot> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT file, bytes, taken_at, remote, state FROM backup_log
                  ORDER BY id DESC LIMIT ?1",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(Snapshot {
                    file: r.get(0)?,
                    bytes: r.get::<_, i64>(1)? as u64,
                    taken_at: r.get(2)?,
                    remote: r.get(3)?,
                    state: r.get(4)?,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    pub fn prop_count(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM properties", [], |r| r.get(0))
            .unwrap_or(0)
    }

    /// Live query behind the TODO board.
    pub fn by_status(&self, status: &str) -> Vec<RefHit> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT b.id, b.content, p.name, p.kind
                   FROM blocks b JOIN pages p ON p.id = b.page_id
                  WHERE b.deleted_at IS NULL AND b.status = ?1
                  ORDER BY b.updated_at DESC LIMIT 40",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![status], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .unwrap();
        rows.filter_map(|r| r.ok())
            .map(|(id, content, page, kind)| RefHit {
                block_id: id,
                page,
                is_journal: kind == "journal",
                content,
                kind: "query".into(),
            })
            .collect()
    }

    pub fn integrity_check(&self) -> String {
        self.conn
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap_or_else(|_| "unknown".into())
    }

    pub fn compute_stats(&self) -> DbStats {
        let pages: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM pages WHERE deleted_at IS NULL", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        let journals: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pages WHERE kind = 'journal' AND deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let blocks: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM blocks WHERE deleted_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let refs: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM refs", [], |r| r.get(0))
            .unwrap_or(0);
        let file_bytes = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let wal_bytes = fs::metadata(format!("{}-wal", self.path.to_string_lossy()))
            .map(|m| m.len())
            .unwrap_or(0);
        DbStats {
            pages,
            journals,
            blocks,
            refs,
            file_bytes,
            wal_bytes,
            fts: self.fts,
        }
    }

    pub fn refresh_stats(&mut self) {
        // Checkpoint and truncate before measuring, so the numbers (and any
        // snapshot taken next) reflect a compacted file rather than a fat WAL.
        // TRUNCATE can report SQLITE_BUSY with concurrent readers; harmless here.
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        self.stats = self.compute_stats();
    }
}

fn walk(
    kids: &HashMap<Option<i64>, Vec<Block>>,
    parent: Option<i64>,
    depth: usize,
    ancestor_last: &mut Vec<bool>,
    out: &mut Vec<Row>,
) {
    let Some(children) = kids.get(&parent) else {
        return;
    };
    let n = children.len();
    for (i, b) in children.iter().enumerate() {
        let last = i + 1 == n;
        let grandkids = kids.get(&Some(b.id)).map(|v| v.len()).unwrap_or(0);
        out.push(Row {
            id: b.id,
            uuid: b.uuid.clone(),
            depth,
            content: b.content.clone(),
            status: b.status.clone(),
            collapsed: b.collapsed,
            has_children: grandkids > 0,
            child_count: grandkids,
            last_sibling: last,
            ancestor_last: ancestor_last.clone(),
        });
        if grandkids > 0 && !b.collapsed {
            ancestor_last.push(last);
            walk(kids, Some(b.id), depth + 1, ancestor_last, out);
            ancestor_last.pop();
        }
    }
}

fn row_to_page(r: &rusqlite::Row) -> rusqlite::Result<Page> {
    let kind: String = r.get(2)?;
    Ok(Page {
        id: r.get(0)?,
        name: r.get(1)?,
        kind: if kind == "journal" {
            PageKind::Journal
        } else {
            PageKind::Page
        },
        created_at: r.get(3)?,
    })
}

fn row_to_block(r: &rusqlite::Row) -> rusqlite::Result<Block> {
    Ok(Block {
        id: r.get(0)?,
        uuid: r.get(1)?,
        page_id: r.get(2)?,
        parent_id: r.get(3)?,
        position: r.get(4)?,
        content: r.get(5)?,
        status: r.get(6)?,
        collapsed: r.get::<_, i64>(7)? != 0,
        created_at: r.get(8)?,
    })
}

pub fn human_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, U[0])
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

/// Second `impl` block: everything the vim layer needs that the first pass did
/// not have -- link resolution, registers, panel settings and the read-only
/// console the troubleshooting view runs on.
impl Db {
    /// Bring a soft-deleted block (and its subtree) back.
    pub fn restore_block(&self, id: i64) {
        let _ = self.conn.execute(
            "UPDATE blocks SET deleted_at = NULL WHERE id = ?1",
            params![id],
        );
        for child in self.descendant_ids(id) {
            let _ = self.conn.execute(
                "UPDATE blocks SET deleted_at = NULL WHERE id = ?1",
                params![child],
            );
        }
    }

    /// Undo of a create: the row is still there, soft-deleted by `delete_block`,
    /// so redo means clearing the flag again.
    pub fn restore_block_with(&self, id: i64, _page_id: i64, _parent: Option<i64>, content: &str) {
        self.restore_block(id);
        self.update_content(id, content);
    }

    /// Which page a block lives on. Needed to follow a `((block ref))` to the
    /// right page, which may be a journal day.
    pub fn page_of_block(&self, block_id: i64) -> Option<Page> {
        self.conn
            .query_row(
                "SELECT p.id, p.name, p.kind, p.created_at
                   FROM blocks b JOIN pages p ON p.id = b.page_id
                  WHERE b.id = ?1",
                params![block_id],
                row_to_page,
            )
            .optional()
            .ok()
            .flatten()
    }

    pub fn set_setting(&self, key: &str, value: &str) {
        let _ = self.conn.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }

    pub fn bool_setting(&self, key: &str, default: bool) -> bool {
        match self.get_setting(key).as_deref() {
            Some("true") | Some("1") => true,
            Some("false") | Some("0") => false,
            _ => default,
        }
    }

    /// A block and its descendants in document order, for `yy` / `dd` / `p`.
    /// Depth is relative to the block itself, so the register survives the move.
    pub fn subtree(&self, block_id: i64) -> Vec<(usize, String, Option<String>)> {
        let Some(root) = self.block(block_id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut stack = vec![(block_id, 0usize)];
        // iterative DFS so a pathological tree cannot blow the stack
        while let Some((id, depth)) = stack.pop() {
            let Some(b) = self.block(id) else { continue };
            out.push((depth, b.content.clone(), b.status.clone()));
            let mut kids: Vec<Block> = self
                .page_blocks(b.page_id)
                .into_iter()
                .filter(|c| c.parent_id == Some(id))
                .collect();
            kids.sort_by(|a, b| {
                a.position
                    .partial_cmp(&b.position)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for k in kids.into_iter().rev() {
                stack.push((k.id, depth + 1));
            }
        }
        let _ = root;
        out
    }

    /// Re-create a register's contents under `parent`, after `after` at depth 0.
    pub fn paste_subtree(
        &self,
        page_id: i64,
        parent: Option<i64>,
        after: Option<i64>,
        items: &[(usize, String, Option<String>)],
    ) -> Option<i64> {
        let mut last_at: Vec<i64> = Vec::new();
        let mut anchor = after;
        let mut first = None;
        let mut root_parent = parent;
        for (depth, content, status) in items {
            let (par, aft) = if *depth == 0 {
                (root_parent, anchor)
            } else {
                let p = last_at.get(depth - 1).copied().or(parent);
                (p, None)
            };
            let nb = self.create_block(page_id, par, aft, content);
            if let Some(s) = status {
                self.set_status(nb.id, Some(s));
            }
            last_at.truncate(*depth);
            last_at.push(nb.id);
            if *depth == 0 {
                anchor = Some(nb.id);
                if first.is_none() {
                    first = Some(nb.id);
                }
            }
            let _ = &mut root_parent;
        }
        first
    }

    pub fn descendant_ids(&self, block_id: i64) -> Vec<i64> {
        let mut out = Vec::new();
        let mut stack = vec![block_id];
        while let Some(id) = stack.pop() {
            for b in self.page_blocks(self.block(id).map(|b| b.page_id).unwrap_or(-1)) {
                if b.parent_id == Some(id) {
                    out.push(b.id);
                    stack.push(b.id);
                }
            }
        }
        out
    }

    /// The troubleshooting console. Deliberately an allow-list rather than a
    /// read-only connection: the app already holds a writable handle, and a
    /// typo must not be able to run `DELETE FROM blocks` from a UI prompt.
    pub fn console_query(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
        let trimmed = sql.trim().trim_end_matches(';').trim();
        let upper = trimmed.to_uppercase();
        let allowed = ["SELECT", "PRAGMA", "EXPLAIN", "WITH", "VALUES"]
            .iter()
            .any(|kw| upper.starts_with(kw));
        if !allowed {
            return Err("read-only console: only SELECT / PRAGMA / EXPLAIN / WITH are allowed".into());
        }
        let mut stmt = self.conn.prepare(trimmed).map_err(|e| e.to_string())?;
        let columns: Vec<String> = stmt
            .column_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let n = columns.len();
        if n == 0 {
            return Err("statement returned no columns (it is not a query)".into());
        }
        let mut rows_out: Vec<Vec<String>> = Vec::new();
        let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let mut r = Vec::with_capacity(n);
            for i in 0..n {
                let v = match row.get_ref(i).map_err(|e| e.to_string())? {
                    rusqlite::types::ValueRef::Null => "NULL".to_string(),
                    rusqlite::types::ValueRef::Integer(i) => i.to_string(),
                    rusqlite::types::ValueRef::Real(f) => format!("{:.3}", f),
                    rusqlite::types::ValueRef::Text(t) => String::from_utf8_lossy(t).to_string(),
                    rusqlite::types::ValueRef::Blob(b) => format!("<{} bytes>", b.len()),
                };
                r.push(v);
            }
            rows_out.push(r);
            if rows_out.len() >= 200 {
                break;
            }
        }
        Ok((columns, rows_out))
    }

    /// `.tables` for the console.
    pub fn table_list(&self) -> Vec<(String, i64)> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name FROM sqlite_master
                  WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%'
                  ORDER BY name",
            )
            .unwrap();
        let names: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        drop(stmt);
        names
            .into_iter()
            .map(|n| {
                let c: i64 = self
                    .conn
                    .query_row(&format!("SELECT COUNT(*) FROM \"{}\"", n), [], |r| r.get(0))
                    .unwrap_or(-1);
                (n, c)
            })
            .collect()
    }
}

/// One row of the Find list: which page, why it matched, and where to land.
#[derive(Clone, Debug)]
pub struct PageHit {
    pub name: String,
    pub is_journal: bool,
    pub blocks: i64,
    pub updated_at: String,
    /// The query matched the page *name*.
    pub title_match: bool,
    /// Set when the match was in a block, so `⏎` can land on that block.
    pub block_id: Option<i64>,
    pub snippet: Option<String>,
}

/// The Find picker's backend: a page-name match and a block-content match,
/// merged in Rust so the ordering rule is explicit rather than an artefact of
/// one clever UNION.
impl Db {
    /// The default list: whatever was touched last. Journals included, because a
    /// day you wrote in is a page you will want back.
    pub fn recent_pages(&self, limit: i64) -> Vec<PageHit> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.name, p.kind, p.updated_at,
                        (SELECT COUNT(*) FROM blocks b
                          WHERE b.page_id = p.id AND b.deleted_at IS NULL),
                        (SELECT b.id FROM blocks b
                          WHERE b.page_id = p.id AND b.deleted_at IS NULL AND TRIM(b.content) <> ''
                          ORDER BY b.updated_at DESC LIMIT 1),
                        (SELECT b.content FROM blocks b
                          WHERE b.page_id = p.id AND b.deleted_at IS NULL AND TRIM(b.content) <> ''
                          ORDER BY b.updated_at DESC LIMIT 1)
                   FROM pages p
                  WHERE p.deleted_at IS NULL
                    AND EXISTS (SELECT 1 FROM blocks b
                                 WHERE b.page_id = p.id AND b.deleted_at IS NULL
                                   AND TRIM(b.content) <> '')
                  ORDER BY p.updated_at DESC, p.name
                  LIMIT ?1",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(PageHit {
                    name: r.get(0)?,
                    is_journal: r.get::<_, String>(1)? == "journal",
                    updated_at: r.get(2)?,
                    blocks: r.get(3)?,
                    block_id: r.get(4)?,
                    snippet: r.get(5)?,
                    title_match: false,
                })
            })
            .unwrap();
        rows.filter_map(|r| r.ok()).collect()
    }

    /// Title matches first, then block matches by relevance, one row per page.
    pub fn find_pages(&self, query: &str, limit: i64) -> Vec<PageHit> {
        let needle = query.trim();
        if needle.is_empty() {
            return self.recent_pages(limit);
        }
        let mut out: Vec<PageHit> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // 1. page names, case-insensitively
        if let Ok(mut stmt) = self.conn.prepare(
            "SELECT p.name, p.kind, p.updated_at,
                    (SELECT COUNT(*) FROM blocks b
                      WHERE b.page_id = p.id AND b.deleted_at IS NULL)
               FROM pages p
              WHERE p.deleted_at IS NULL AND p.name LIKE '%' || ?1 || '%'
              ORDER BY p.updated_at DESC, p.name
              LIMIT ?2",
        ) {
            if let Ok(rows) = stmt.query_map(params![needle, limit], |r| {
                Ok(PageHit {
                    name: r.get(0)?,
                    is_journal: r.get::<_, String>(1)? == "journal",
                    updated_at: r.get(2)?,
                    blocks: r.get(3)?,
                    title_match: true,
                    block_id: None,
                    snippet: None,
                })
            }) {
                for hit in rows.filter_map(|r| r.ok()) {
                    if seen.insert(hit.name.clone()) {
                        out.push(hit);
                    }
                }
            }
        }

        // 2. block content -- FTS5 when the build has it, LIKE otherwise
        let content_sql = if self.fts {
            "SELECT p.name, p.kind, p.updated_at, b.id, b.content,
                    (SELECT COUNT(*) FROM blocks c WHERE c.page_id = p.id AND c.deleted_at IS NULL)
               FROM blocks_fts f
               JOIN blocks b ON b.id = f.rowid
               JOIN pages  p ON p.id = b.page_id
              WHERE blocks_fts MATCH ?1
                AND b.deleted_at IS NULL AND p.deleted_at IS NULL
              ORDER BY bm25(blocks_fts)
              LIMIT ?2"
        } else {
            "SELECT p.name, p.kind, p.updated_at, b.id, b.content,
                    (SELECT COUNT(*) FROM blocks c WHERE c.page_id = p.id AND c.deleted_at IS NULL)
               FROM blocks b
               JOIN pages p ON p.id = b.page_id
              WHERE b.deleted_at IS NULL AND p.deleted_at IS NULL
                AND b.content LIKE '%' || ?1 || '%'
              ORDER BY b.updated_at DESC
              LIMIT ?2"
        };
        let param = if self.fts {
            needle
                .split_whitespace()
                .map(|t| format!("\"{}\"*", t.replace('"', "")))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            needle.to_string()
        };
        if let Ok(mut stmt) = self.conn.prepare(content_sql) {
            if let Ok(rows) = stmt.query_map(params![param, limit], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            }) {
                for (name, kind, updated, block_id, content, blocks) in rows.filter_map(|r| r.ok()) {
                    if seen.insert(name.clone()) {
                        out.push(PageHit {
                            name,
                            is_journal: kind == "journal",
                            blocks,
                            updated_at: updated,
                            title_match: false,
                            block_id: Some(block_id),
                            snippet: Some(content),
                        });
                    }
                }
            }
        }
        out.truncate(limit as usize);
        out
    }
}
