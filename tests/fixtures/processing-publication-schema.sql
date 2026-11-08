-- Frozen #19 VM queue schema; publication tests never run the production worker.

PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tasks (
 vid TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
 size INTEGER NOT NULL, mtime_ns TEXT NOT NULL, duration REAL,
 stage TEXT NOT NULL DEFAULT 'queued', retry_stage TEXT NOT NULL DEFAULT 'text',
 text_attempts INTEGER NOT NULL DEFAULT 0, face_attempts INTEGER NOT NULL DEFAULT 0,
 next_retry REAL NOT NULL DEFAULT 0, priority INTEGER NOT NULL DEFAULT 0,
 is_new INTEGER NOT NULL DEFAULT 0, batch_id TEXT NOT NULL,
 created_at REAL NOT NULL, updated_at REAL NOT NULL,
 error TEXT, manual INTEGER NOT NULL DEFAULT 0, deferred INTEGER NOT NULL DEFAULT 0,
 note TEXT NOT NULL DEFAULT '', data TEXT NOT NULL DEFAULT '{}', run_id TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS processing_stage ON tasks(stage, priority, is_new, created_at);
CREATE TABLE IF NOT EXISTS events (
 id INTEGER PRIMARY KEY AUTOINCREMENT, at REAL NOT NULL, vid TEXT, kind TEXT NOT NULL, message TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS face_groups (id TEXT PRIMARY KEY, version TEXT NOT NULL, embedding BLOB NOT NULL);
CREATE TABLE IF NOT EXISTS batch_items (
 batch_id TEXT NOT NULL, vid TEXT NOT NULL, name TEXT NOT NULL, path TEXT NOT NULL,
 size INTEGER NOT NULL, mtime_ns TEXT NOT NULL, added_at REAL NOT NULL,
 text_done INTEGER NOT NULL DEFAULT 0, face_required INTEGER, face_done INTEGER NOT NULL DEFAULT 0,
 face_skipped INTEGER NOT NULL DEFAULT 0, completed INTEGER NOT NULL DEFAULT 0,
 completed_at REAL, excluded INTEGER NOT NULL DEFAULT 0, legacy INTEGER NOT NULL DEFAULT 0,
 PRIMARY KEY(batch_id,vid)
);
CREATE TABLE IF NOT EXISTS processing_runs (
 id TEXT PRIMARY KEY, vid TEXT NOT NULL, name TEXT NOT NULL, batch_id TEXT NOT NULL,
 stage TEXT NOT NULL, started_at REAL NOT NULL, finished_at REAL, status TEXT NOT NULL,
 error TEXT, execution TEXT
);
CREATE INDEX IF NOT EXISTS processing_run_time ON processing_runs(started_at DESC);

ALTER TABLE tasks ADD COLUMN lease_expires_at REAL NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN coordinator_generation TEXT NOT NULL DEFAULT '';
