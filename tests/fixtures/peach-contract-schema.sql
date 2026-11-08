-- Sanitized structural fixture from observed Peach schema (2026-11-08).
-- Exact core table DDL; no production rows, filesystem locations or hostnames.
-- This is deliberately NOT a full Peach initialization schema. See rust-execution-design.md.
PRAGMA foreign_keys = ON;

-- asset
CREATE TABLE asset(
  id INTEGER PRIMARY KEY,
  location TEXT NOT NULL,
  path TEXT NOT NULL,
  name TEXT,
  medium TEXT,
  size INTEGER,
  mtime TEXT,
  hash_kind TEXT,
  hash TEXT,
  creator TEXT,
  studio TEXT,
  series TEXT,
  code TEXT,
  duration REAL,
  width INTEGER,
  height INTEGER,
  vcodec TEXT,
  fps REAL,
  has_audio INTEGER,
  ctx_length TEXT,
  ctx_orient TEXT,
  ctx_quality TEXT,
  ctx_pace TEXT,
  ctx_people TEXT,
  play_count INTEGER DEFAULT 0,
  last_played TEXT,
  rating INTEGER,
  o_count INTEGER,
  watch_ratio REAL,
  source_id INTEGER,
  stash_scene_id INTEGER,
  snapshot_path TEXT,
  first_seen TEXT,
  last_seen TEXT,
  feedback TEXT,
  disposal TEXT,
  leave_ratio REAL,
  play_seconds REAL,
  feedback_at REAL,
  seek_count INTEGER,
  max_reached REAL, release_date TEXT, catalog_title TEXT, original_title TEXT, field_owners TEXT, mutation_revision INTEGER NOT NULL DEFAULT 0, region TEXT, disposal_reason TEXT
CHECK(disposal_reason IS NULL OR disposal_reason IN ('manual','junk','duplicate','missing')),
  UNIQUE(location, path)
);

-- entity
CREATE TABLE "entity"(
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('performer','studio','tag','creator','series','agency')),
  canonical_name TEXT NOT NULL,
  normalized_name TEXT NOT NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, region TEXT,
  UNIQUE(kind, normalized_name)
);

-- asset_entity
CREATE TABLE asset_entity(
  asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  entity_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  source TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 1.0,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  first_seen_at TEXT,
  last_seen_at TEXT,
  PRIMARY KEY(asset_id, entity_id, role, source)
);

-- entity_alias
CREATE TABLE entity_alias(
  entity_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  alias TEXT NOT NULL,
  normalized_alias TEXT NOT NULL,
  source TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 1.0,
  PRIMARY KEY(entity_id, normalized_alias, source)
);

-- entity_redirect
CREATE TABLE entity_redirect(
  old_id INTEGER PRIMARY KEY,
  target_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  source TEXT NOT NULL,
  merged_at TEXT NOT NULL,
  CHECK(old_id<>target_id)
);

-- entity_classification
CREATE TABLE entity_classification(
  entity_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  facet TEXT NOT NULL CHECK(facet IN ('identity','occupation','market','account_role')),
  value TEXT NOT NULL,
  source TEXT NOT NULL,
  source_url TEXT NOT NULL DEFAULT '',
  evidence TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('candidate','observed','approved','rejected')),
  confidence REAL NOT NULL CHECK(confidence BETWEEN 0 AND 1),
  checked_at TEXT NOT NULL,
  PRIMARY KEY(entity_id,facet,value,source)
);

-- entity_identity_link
CREATE TABLE entity_identity_link(
  left_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  right_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  relation TEXT NOT NULL CHECK(relation IN ('same_person','operates_account')),
  source TEXT NOT NULL,
  source_url TEXT NOT NULL,
  evidence TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('candidate','observed','approved','rejected')),
  checked_at TEXT NOT NULL,
  CHECK(left_id<>right_id),
  PRIMARY KEY(left_id,right_id,relation,source)
);

-- Existing legacy receipt table (if present in observed database).
CREATE TABLE _face_admin_person_ops (id TEXT PRIMARY KEY, signature TEXT NOT NULL, state TEXT NOT NULL, data TEXT NOT NULL);
