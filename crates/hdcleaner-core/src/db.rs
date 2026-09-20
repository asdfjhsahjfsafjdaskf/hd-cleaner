//! Local SQLite database: settings, operation history/journal, scan index.
//!
//! Operations are journaled: a row is inserted with status `running` before a
//! destructive operation starts and updated when it ends. On startup any row
//! still `running` means the application stopped mid-operation; the UI shows
//! it as interrupted and never assumes it completed.

use crate::{AppError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub struct Database {
    conn: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecord {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub started_ms: i64,
    pub finished_ms: Option<i64>,
    pub summary: String,
    pub item_count: i64,
    pub ok_count: i64,
    pub error_count: i64,
    pub dry_run: bool,
    /// JSON with per-item details (paths, outcomes).
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRecord {
    pub id: i64,
    pub root: String,
    pub method: String,
    pub started_ms: i64,
    pub duration_ms: i64,
    pub files: i64,
    pub dirs: i64,
    pub total_size: i64,
    pub total_alloc: i64,
    pub snapshot_path: Option<String>,
}

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS operations (
        id          INTEGER PRIMARY KEY AUTOINCREMENT,
        kind        TEXT NOT NULL,
        status      TEXT NOT NULL,
        started_ms  INTEGER NOT NULL,
        finished_ms INTEGER,
        summary     TEXT NOT NULL DEFAULT '',
        item_count  INTEGER NOT NULL DEFAULT 0,
        ok_count    INTEGER NOT NULL DEFAULT 0,
        error_count INTEGER NOT NULL DEFAULT 0,
        dry_run     INTEGER NOT NULL DEFAULT 0,
        details     TEXT NOT NULL DEFAULT '{}'
    );
    CREATE INDEX IF NOT EXISTS idx_operations_started ON operations(started_ms DESC);
    CREATE INDEX IF NOT EXISTS idx_operations_status ON operations(status);
    CREATE TABLE IF NOT EXISTS scans (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        root          TEXT NOT NULL,
        method        TEXT NOT NULL,
        started_ms    INTEGER NOT NULL,
        duration_ms   INTEGER NOT NULL,
        files         INTEGER NOT NULL,
        dirs          INTEGER NOT NULL,
        total_size    INTEGER NOT NULL,
        total_alloc   INTEGER NOT NULL,
        snapshot_path TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_scans_root ON scans(root, started_ms DESC);
    "#,
];

impl Database {
    pub fn open(path: &Path) -> Result<Database> {
        if let Some(parent) = path.parent() {
            crate::util::ensure_dir(parent)?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Database> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Database> {
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, m) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            conn.execute_batch(m)?;
            conn.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
        }
        Ok(Database { conn })
    }

    // ---- settings -------------------------------------------------------

    pub fn get_setting(&self, key: &str) -> Result<Option<serde_json::Value>> {
        let v: Option<String> =
            self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?;
        v.map(|s| serde_json::from_str(&s).map_err(|e| AppError::Corrupt(format!("setting {key}: {e}"))))
            .transpose()
    }

    pub fn set_setting(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value.to_string()],
        )?;
        Ok(())
    }

    pub fn all_settings(&self) -> Result<serde_json::Map<String, serde_json::Value>> {
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut map = serde_json::Map::new();
        for row in rows {
            let (k, v) = row?;
            if let Ok(val) = serde_json::from_str(&v) {
                map.insert(k, val);
            }
        }
        Ok(map)
    }

    // ---- operations journal ----------------------------------------------

    /// Journal the start of an operation *before* doing anything.
    pub fn begin_operation(&self, kind: &str, summary: &str, item_count: usize, dry_run: bool, details: &serde_json::Value) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO operations(kind, status, started_ms, summary, item_count, dry_run, details)
             VALUES(?1, 'running', ?2, ?3, ?4, ?5, ?6)",
            params![kind, crate::util::now_unix_ms(), summary, item_count as i64, dry_run as i64, details.to_string()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn finish_operation(&self, id: i64, status: &str, ok: usize, errors: usize, details: &serde_json::Value) -> Result<()> {
        self.conn.execute(
            "UPDATE operations SET status = ?2, finished_ms = ?3, ok_count = ?4, error_count = ?5, details = ?6 WHERE id = ?1",
            params![id, status, crate::util::now_unix_ms(), ok as i64, errors as i64, details.to_string()],
        )?;
        Ok(())
    }

    /// Record a completed, non-destructive operation in one step.
    pub fn record_operation(&self, kind: &str, status: &str, summary: &str, items: usize, details: &serde_json::Value) -> Result<i64> {
        let id = self.begin_operation(kind, summary, items, false, details)?;
        self.finish_operation(id, status, items, 0, details)?;
        Ok(id)
    }

    fn row_to_op(r: &rusqlite::Row) -> rusqlite::Result<OperationRecord> {
        let details: String = r.get(10)?;
        Ok(OperationRecord {
            id: r.get(0)?,
            kind: r.get(1)?,
            status: r.get(2)?,
            started_ms: r.get(3)?,
            finished_ms: r.get(4)?,
            summary: r.get(5)?,
            item_count: r.get(6)?,
            ok_count: r.get(7)?,
            error_count: r.get(8)?,
            dry_run: r.get::<_, i64>(9)? != 0,
            details: serde_json::from_str(&details).unwrap_or(serde_json::Value::Null),
        })
    }

    pub fn list_operations(&self, limit: usize) -> Result<Vec<OperationRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, status, started_ms, finished_ms, summary, item_count, ok_count, error_count, dry_run, details
             FROM operations ORDER BY started_ms DESC, id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], Self::row_to_op)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Operations left `running` by a previous process (crash / power loss).
    pub fn interrupted_operations(&self) -> Result<Vec<OperationRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, kind, status, started_ms, finished_ms, summary, item_count, ok_count, error_count, dry_run, details
             FROM operations WHERE status = 'running' ORDER BY started_ms",
        )?;
        let rows = stmt.query_map([], Self::row_to_op)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Mark all `running` rows as `interrupted` (called once at startup, after
    /// they were read so the UI can report them).
    pub fn mark_interrupted(&self, ids: &[i64]) -> Result<()> {
        for id in ids {
            self.conn.execute("UPDATE operations SET status = 'interrupted' WHERE id = ?1 AND status = 'running'", [id])?;
        }
        Ok(())
    }

    pub fn set_operation_status(&self, id: i64, status: &str) -> Result<()> {
        self.conn.execute("UPDATE operations SET status = ?2 WHERE id = ?1", params![id, status])?;
        Ok(())
    }

    pub fn clear_history(&self) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM operations WHERE status != 'running'", [])?)
    }

    // ---- scans -------------------------------------------------------------

    pub fn add_scan(&self, meta: &crate::scan::ScanMeta, snapshot_path: Option<&str>) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO scans(root, method, started_ms, duration_ms, files, dirs, total_size, total_alloc, snapshot_path)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                meta.root_path,
                meta.method,
                meta.started_ms,
                meta.duration_ms as i64,
                meta.files as i64,
                meta.dirs as i64,
                meta.total_size as i64,
                meta.total_alloc as i64,
                snapshot_path
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_scans(&self, root: Option<&str>, limit: usize) -> Result<Vec<ScanRecord>> {
        let sql = "SELECT id, root, method, started_ms, duration_ms, files, dirs, total_size, total_alloc, snapshot_path
                   FROM scans WHERE (?1 IS NULL OR root = ?1 COLLATE NOCASE) ORDER BY started_ms DESC LIMIT ?2";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![root, limit as i64], |r| {
            Ok(ScanRecord {
                id: r.get(0)?,
                root: r.get(1)?,
                method: r.get(2)?,
                started_ms: r.get(3)?,
                duration_ms: r.get(4)?,
                files: r.get(5)?,
                dirs: r.get(6)?,
                total_size: r.get(7)?,
                total_alloc: r.get(8)?,
                snapshot_path: r.get(9)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn delete_scan(&self, id: i64) -> Result<Option<String>> {
        let path: Option<Option<String>> =
            self.conn.query_row("SELECT snapshot_path FROM scans WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        self.conn.execute("DELETE FROM scans WHERE id = ?1", [id])?;
        Ok(path.flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_and_journal() {
        let db = Database::open_in_memory().unwrap();
        assert!(db.get_setting("theme").unwrap().is_none());
        db.set_setting("theme", &serde_json::json!("dark")).unwrap();
        db.set_setting("theme", &serde_json::json!("light")).unwrap();
        assert_eq!(db.get_setting("theme").unwrap().unwrap(), "light");

        let id = db.begin_operation("delete", "2 items", 2, false, &serde_json::json!({"paths": ["a", "b"]})).unwrap();
        // Simulate a crash: the row stays 'running'.
        let interrupted = db.interrupted_operations().unwrap();
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].id, id);
        db.mark_interrupted(&[id]).unwrap();
        assert!(db.interrupted_operations().unwrap().is_empty());
        assert_eq!(db.list_operations(10).unwrap()[0].status, "interrupted");

        let id2 = db.begin_operation("cleanup", "x", 1, true, &serde_json::json!({})).unwrap();
        db.finish_operation(id2, "completed", 1, 0, &serde_json::json!({"ok": 1})).unwrap();
        let ops = db.list_operations(10).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(ops.iter().any(|o| o.id == id2 && o.status == "completed" && o.dry_run));
    }

    #[test]
    fn file_db_migrates_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.db");
        {
            let db = Database::open(&p).unwrap();
            db.set_setting("k", &serde_json::json!(1)).unwrap();
        }
        let db = Database::open(&p).unwrap();
        assert_eq!(db.get_setting("k").unwrap().unwrap(), 1);
    }
}
