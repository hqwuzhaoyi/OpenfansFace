use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboxItem {
    pub attempt_id: String,
    pub node_id: String,
    pub vid: String,
    pub stage: String,
    pub payload_json: String,
    pub retry_count: i32,
    pub created_at: f64,
    pub last_sent_at: Option<f64>,
    pub next_retry_at: f64,
    pub status: String, // "pending" | "committed" | "abandoned" | "rejected"
    pub last_error: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone)]
pub struct AgentOutbox {
    conn: Arc<Mutex<Connection>>,
    db_path: PathBuf,
}

#[allow(dead_code)]
impl AgentOutbox {
    pub fn open(db_path: &Path) -> Result<Self, rusqlite::Error> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(db_path)?;
        conn.busy_timeout(std::time::Duration::from_secs(15))?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;

            CREATE TABLE IF NOT EXISTS outbox_receipts (
                attempt_id TEXT PRIMARY KEY,
                node_id TEXT NOT NULL,
                vid TEXT NOT NULL,
                stage TEXT NOT NULL,
                payload_json TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                created_at REAL NOT NULL,
                last_sent_at REAL,
                next_retry_at REAL NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                last_error TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_outbox_pending ON outbox_receipts(status, next_retry_at);
            "#,
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path: db_path.to_path_buf(),
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn enqueue(&self, item: &OutboxItem) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO outbox_receipts (
                attempt_id, node_id, vid, stage, payload_json, retry_count,
                created_at, last_sent_at, next_retry_at, status, last_error
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                item.attempt_id,
                item.node_id,
                item.vid,
                item.stage,
                item.payload_json,
                item.retry_count,
                item.created_at,
                item.last_sent_at,
                item.next_retry_at,
                item.status,
                item.last_error,
            ],
        )?;
        Ok(())
    }

    pub fn enqueue_receipt(
        &self,
        attempt_id: &str,
        node_id: &str,
        vid: &str,
        stage: &str,
        payload_json: &str,
        now: f64,
    ) -> Result<(), rusqlite::Error> {
        let item = OutboxItem {
            attempt_id: attempt_id.to_string(),
            node_id: node_id.to_string(),
            vid: vid.to_string(),
            stage: stage.to_string(),
            payload_json: payload_json.to_string(),
            retry_count: 0,
            created_at: now,
            last_sent_at: None,
            next_retry_at: now,
            status: "pending".to_string(),
            last_error: None,
        };
        self.enqueue(&item)
    }

    pub fn poll_pending(&self, now: f64, limit: usize) -> Result<Vec<OutboxItem>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT attempt_id, node_id, vid, stage, payload_json, retry_count,
                    created_at, last_sent_at, next_retry_at, status, last_error
             FROM outbox_receipts
             WHERE status = 'pending' AND next_retry_at <= ?1
             ORDER BY created_at ASC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![now, limit as i64], |row| {
            Ok(OutboxItem {
                attempt_id: row.get(0)?,
                node_id: row.get(1)?,
                vid: row.get(2)?,
                stage: row.get(3)?,
                payload_json: row.get(4)?,
                retry_count: row.get(5)?,
                created_at: row.get(6)?,
                last_sent_at: row.get(7)?,
                next_retry_at: row.get(8)?,
                status: row.get(9)?,
                last_error: row.get(10)?,
            })
        })?;

        let mut items = Vec::new();
        for r in rows {
            items.push(r?);
        }
        Ok(items)
    }

    pub fn mark_committed(&self, attempt_id: &str, now: f64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE outbox_receipts
             SET status = 'committed', last_sent_at = ?1, last_error = NULL
             WHERE attempt_id = ?2",
            params![now, attempt_id],
        )?;
        Ok(())
    }

    pub fn mark_abandoned(&self, attempt_id: &str, reason: &str, now: f64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE outbox_receipts
             SET status = 'abandoned', last_sent_at = ?1, last_error = ?2
             WHERE attempt_id = ?3",
            params![now, reason, attempt_id],
        )?;
        Ok(())
    }

    pub fn mark_rejected(&self, attempt_id: &str, reason: &str, now: f64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE outbox_receipts
             SET status = 'rejected', last_sent_at = ?1, last_error = ?2
             WHERE attempt_id = ?3",
            params![now, reason, attempt_id],
        )?;
        Ok(())
    }

    pub fn record_transient_failure(
        &self,
        attempt_id: &str,
        error: &str,
        now: f64,
    ) -> Result<f64, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let retry_count: i32 = conn
            .query_row(
                "SELECT retry_count FROM outbox_receipts WHERE attempt_id = ?1",
                params![attempt_id],
                |r| r.get(0),
            )
            .unwrap_or(0);

        let new_count = retry_count + 1;
        // Exponential backoff: 1s, 2s, 4s, 8s, 16s, capped at 30s
        let exponent = (new_count - 1).clamp(0, 5);
        let delay = (1.0 * (1 << exponent) as f64).min(30.0);
        let next_retry_at = now + delay;

        conn.execute(
            "UPDATE outbox_receipts
             SET retry_count = ?1, next_retry_at = ?2, last_error = ?3, last_sent_at = ?4, status = 'pending'
             WHERE attempt_id = ?5",
            params![new_count, next_retry_at, error, now, attempt_id],
        )?;

        Ok(delay)
    }

    pub fn get_item(&self, attempt_id: &str) -> Result<Option<OutboxItem>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT attempt_id, node_id, vid, stage, payload_json, retry_count,
                    created_at, last_sent_at, next_retry_at, status, last_error
             FROM outbox_receipts
             WHERE attempt_id = ?1",
        )?;

        let mut rows = stmt.query_map(params![attempt_id], |row| {
            Ok(OutboxItem {
                attempt_id: row.get(0)?,
                node_id: row.get(1)?,
                vid: row.get(2)?,
                stage: row.get(3)?,
                payload_json: row.get(4)?,
                retry_count: row.get(5)?,
                created_at: row.get(6)?,
                last_sent_at: row.get(7)?,
                next_retry_at: row.get(8)?,
                status: row.get(9)?,
                last_error: row.get(10)?,
            })
        })?;

        if let Some(r) = rows.next() {
            Ok(Some(r?))
        } else {
            Ok(None)
        }
    }

    pub fn prune_committed(&self, older_than_secs: f64, now: f64) -> Result<usize, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let cutoff = now - older_than_secs;
        let deleted = conn.execute(
            "DELETE FROM outbox_receipts WHERE status = 'committed' AND last_sent_at <= ?1",
            params![cutoff],
        )?;
        Ok(deleted)
    }
}
