PRAGMA foreign_keys=ON;
CREATE TABLE _face_admin_person_ops (id TEXT PRIMARY KEY, signature TEXT NOT NULL, state TEXT NOT NULL, data TEXT NOT NULL);
CREATE TABLE "activity_event"(
  id INTEGER PRIMARY KEY,
  asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  profile_id TEXT REFERENCES profile(id) ON DELETE SET NULL,
  kind TEXT NOT NULL,
  occurred_at TEXT NOT NULL,
  position_seconds REAL,
  duration_seconds REAL,
  delta_seconds REAL,
  value_json TEXT NOT NULL DEFAULT '{}',
  source TEXT NOT NULL,
  session_id TEXT,
  event_key TEXT UNIQUE
);
CREATE TABLE app_user(
  id TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  created_at TEXT NOT NULL
);
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
CREATE TABLE asset_preference(
  profile_id TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
  asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  liked INTEGER NOT NULL DEFAULT 0 CHECK(liked IN (0,1)),
  reason TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL DEFAULT 'web',
  updated_at TEXT NOT NULL,
  PRIMARY KEY(profile_id,asset_id)
);
CREATE TABLE asset_quality_goal(
  profile_id TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
  asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  wanted INTEGER NOT NULL DEFAULT 1 CHECK(wanted IN (0,1)),
  reason TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL, replaced_at TEXT,
  PRIMARY KEY(profile_id,asset_id)
);
CREATE VIRTUAL TABLE asset_search USING fts5(
  asset_id UNINDEXED,
  name,
  code,
  entities,
  search_terms,
  tokenize='trigram'
);
CREATE TABLE asset_subtitle(
  id INTEGER PRIMARY KEY,
  asset_id INTEGER REFERENCES asset(id) ON DELETE CASCADE,
  location TEXT NOT NULL,
  path TEXT NOT NULL,
  name TEXT NOT NULL,
  language TEXT NOT NULL DEFAULT '',
  format TEXT NOT NULL,
  size INTEGER,
  mtime TEXT,
  pairing TEXT NOT NULL,
  first_seen TEXT NOT NULL,
  last_seen TEXT NOT NULL,
  UNIQUE(location,path),
  CHECK(pairing IN ('exact','suffix','code','orphan')),
  CHECK(asset_id IS NOT NULL OR pairing='orphan')
);
CREATE TABLE "asset_tag"(
  asset_id INTEGER REFERENCES asset(id) ON DELETE CASCADE,
  tag TEXT,
  confidence REAL DEFAULT 1.0,
  source TEXT,
  UNIQUE(asset_id, tag)
);
CREATE TABLE "asset_tag_preference"(
  profile_id TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
  asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  normalized_tag TEXT NOT NULL,
  hidden INTEGER NOT NULL DEFAULT 1 CHECK(hidden IN (0,1)),
  updated_at TEXT NOT NULL,
  PRIMARY KEY(profile_id,asset_id,normalized_tag)
);
CREATE TABLE code_sample_image(
  code TEXT NOT NULL,
  position INTEGER NOT NULL CHECK(position>=1),
  url TEXT NOT NULL,
  site TEXT NOT NULL,
  width INTEGER,
  height INTEGER,
  source TEXT NOT NULL,
  fetched_at TEXT NOT NULL,
  PRIMARY KEY(code,position)
);
CREATE TABLE download_submission(
  id INTEGER PRIMARY KEY,
  task_id INTEGER,
  info_hash TEXT,
  provider TEXT NOT NULL,
  source_uri TEXT NOT NULL,
  target TEXT NOT NULL,
  code TEXT,
  origin TEXT,
  outcome TEXT NOT NULL,
  failure TEXT,
  detail TEXT,
  created_at TEXT NOT NULL
);
CREATE TABLE download_task(
  id INTEGER PRIMARY KEY,
  info_hash TEXT,
  provider TEXT NOT NULL,
  source_uri TEXT NOT NULL,
  display_name TEXT NOT NULL DEFAULT '',
  target TEXT NOT NULL,
  code TEXT,
  title TEXT,
  origin TEXT,
  state TEXT NOT NULL,
  failure TEXT,
  failure_detail TEXT,
  remote_id TEXT,
  remote_name TEXT,
  progress REAL,
  ledger_path TEXT,
  asset_id INTEGER,
  checks INTEGER NOT NULL DEFAULT 0,
  submitted_at TEXT,
  remote_done_at TEXT,
  updated_at TEXT NOT NULL,
  finished_at TEXT,
  next_check_at TEXT,
  blocked_at TEXT
, postprocess_state TEXT NOT NULL DEFAULT 'not_requested', postprocess_detail TEXT, postprocess_run_id INTEGER, postprocessed_at TEXT);
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
CREATE TABLE entity_alias(
  entity_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  alias TEXT NOT NULL,
  normalized_alias TEXT NOT NULL,
  source TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 1.0,
  PRIMARY KEY(entity_id, normalized_alias, source)
);
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
CREATE TABLE "entity_external_ref"(
  entity_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  external_kind TEXT NOT NULL,
  external_id TEXT NOT NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  last_synced_at TEXT,
  PRIMARY KEY(provider, external_kind, external_id),
  UNIQUE(entity_id, provider, external_kind, external_id)
);
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
CREATE TABLE entity_link(
  id INTEGER PRIMARY KEY,
  entity_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  link_kind TEXT NOT NULL CHECK(link_kind IN ('official','social','catalog','source_reference')),
  label TEXT NOT NULL,
  url TEXT NOT NULL,
  hostname TEXT,
  is_sensitive INTEGER NOT NULL DEFAULT 0 CHECK(is_sensitive IN (0,1)),
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(entity_id,url)
);
CREATE TABLE entity_membership(
  member_id INTEGER PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  agency_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  source TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 1.0,
  checked_at TEXT NOT NULL
);
CREATE TABLE entity_redirect(
  old_id INTEGER PRIMARY KEY,
  target_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  source TEXT NOT NULL,
  merged_at TEXT NOT NULL,
  CHECK(old_id<>target_id)
);
CREATE TABLE entity_search_term(
  entity_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  term TEXT NOT NULL,
  purpose TEXT NOT NULL CHECK(purpose IN ('discovery','source_lookup')),
  source TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(entity_id,term,purpose)
);
CREATE TABLE feed_discovery (
  id INTEGER PRIMARY KEY,
  code TEXT NOT NULL,
  source_id INTEGER REFERENCES feed_source(id) ON DELETE SET NULL,
  title TEXT,
  link TEXT,
  cover_url TEXT,
  release_date TEXT,
  studio TEXT,
  performers TEXT,
  scraped_at TEXT,
  scrape_error TEXT,
  discovered_at TEXT NOT NULL,
  read_at TEXT,
  ignored_at TEXT,
  UNIQUE(code)
);
CREATE TABLE feed_discovery_entity (
  discovery_id INTEGER NOT NULL REFERENCES feed_discovery(id) ON DELETE CASCADE,
  entity_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  PRIMARY KEY(discovery_id, entity_id)
);
CREATE TABLE feed_item (
  id INTEGER PRIMARY KEY,
  source_id INTEGER NOT NULL REFERENCES feed_source(id) ON DELETE CASCADE,
  item_key TEXT NOT NULL,
  code TEXT,
  title TEXT,
  link TEXT,
  published_at TEXT,
  first_seen TEXT NOT NULL,
  UNIQUE(source_id, item_key)
);
CREATE TABLE feed_source (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  url TEXT NOT NULL,
  entity_id INTEGER REFERENCES entity(id) ON DELETE SET NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  interval_minutes INTEGER NOT NULL DEFAULT 360,
  etag TEXT,
  last_modified TEXT,
  next_fetch_at TEXT,
  last_fetched_at TEXT,
  last_error TEXT,
  last_seen_count INTEGER NOT NULL DEFAULT 0,
  last_new_count INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  UNIQUE(url)
);
CREATE TABLE follow_author_alias(
  alias_key TEXT PRIMARY KEY,
  alias_name TEXT NOT NULL,
  canonical_key TEXT NOT NULL,
  canonical_name TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'manual',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK(alias_key<>''),
  CHECK(canonical_key<>'')
);
CREATE TABLE follow_item(
  id INTEGER PRIMARY KEY,
  source_id INTEGER NOT NULL REFERENCES follow_source(id) ON DELETE CASCADE,
  external_id TEXT NOT NULL,
  title TEXT NOT NULL,
  url TEXT,
  media_url TEXT,
  thumb_url TEXT,
  published_at TEXT,
  -- 站点只给相对时间时写 'approximate'；界面必须照实显示，不得冒充精确发布时间。
  published_precision TEXT NOT NULL DEFAULT 'exact'
    CHECK(published_precision IN ('exact','approximate','unknown')),
  version TEXT,
  duration REAL,
  release_key TEXT NOT NULL,
  variant_kind TEXT NOT NULL DEFAULT 'main' CHECK(variant_kind IN ('main','wip','alt')),
  variant_label TEXT,
  group_hint TEXT,
  status TEXT NOT NULL DEFAULT 'new'
    CHECK(status IN ('new','seen','saved','ignored')),
  asset_id INTEGER REFERENCES asset(id) ON DELETE SET NULL,
  evidence_path TEXT,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL, hidden_media_json TEXT,
  UNIQUE(source_id, external_id)
);
CREATE TABLE "follow_playback"(
  follow_item_id INTEGER NOT NULL REFERENCES follow_item(id) ON DELETE CASCADE,
  profile_id TEXT NOT NULL DEFAULT 'local-default'
             REFERENCES profile(id) ON DELETE CASCADE,
  play_count INTEGER NOT NULL DEFAULT 0 CHECK(play_count >= 0),
  play_seconds REAL NOT NULL DEFAULT 0 CHECK(play_seconds >= 0),
  max_reached REAL NOT NULL DEFAULT 0 CHECK(max_reached >= 0 AND max_reached <= 1),
  last_played REAL,
  PRIMARY KEY(follow_item_id, profile_id)
);
CREATE TABLE follow_source(
  id INTEGER PRIMARY KEY,
  entity_id INTEGER REFERENCES entity(id) ON DELETE SET NULL,
  provider TEXT NOT NULL,
  ref TEXT NOT NULL,
  label TEXT NOT NULL,
  url TEXT NOT NULL,
  semantics TEXT NOT NULL DEFAULT 'work' CHECK(semantics IN ('work','release')),
  enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
  etag TEXT,
  last_modified TEXT,
  last_checked_at TEXT,
  last_status TEXT,
  last_error TEXT,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, backfill_page INTEGER NOT NULL DEFAULT 0,
  UNIQUE(provider, ref)
);
CREATE TABLE genre_decision(
  source_genre TEXT PRIMARY KEY,
  raw_genre TEXT NOT NULL,
  peach_tag TEXT,
  decided_at TEXT NOT NULL,
  CHECK(peach_tag IS NULL OR length(trim(peach_tag))>0)
);
CREATE TABLE label_maker(
  label_id INTEGER PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  maker_id INTEGER NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  source TEXT NOT NULL,
  confidence REAL NOT NULL DEFAULT 1.0,
  checked_at TEXT NOT NULL,
  CHECK(label_id<>maker_id)
);
CREATE TABLE ledger_revision(
  tbl TEXT PRIMARY KEY,
  n INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
CREATE TABLE ledger_upgrade_repair(
  id TEXT PRIMARY KEY,
  applied_at TEXT NOT NULL,
  result_json TEXT NOT NULL
);
CREATE TABLE "media_binding"(
  asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  backend TEXT NOT NULL,
  external_id TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 100,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  last_synced_at TEXT,
  PRIMARY KEY(asset_id, backend),
  UNIQUE(backend, external_id)
);
CREATE TABLE performer_profile(
  entity_id INTEGER PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  kana TEXT,
  romaji TEXT,
  birth_date TEXT,
  height_cm INTEGER,
  bust_cm INTEGER,
  cup TEXT,
  waist_cm INTEGER,
  hip_cm INTEGER,
  blood_type TEXT,
  birthplace TEXT,
  hobbies TEXT,
  debut_year INTEGER,
  active_until INTEGER,
  debut_title TEXT,
  debut_date TEXT,
  blog_url TEXT,
  site_url TEXT,
  tags_json TEXT NOT NULL DEFAULT '[]',
  raw_json TEXT NOT NULL DEFAULT '{}',
  source TEXT NOT NULL,
  source_url TEXT NOT NULL,
  fetched_at TEXT NOT NULL
);
CREATE TABLE playlist(
  id INTEGER PRIMARY KEY,
  profile_id TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
  name TEXT NOT NULL CHECK(length(trim(name)) BETWEEN 1 AND 80),
  source_kind TEXT NOT NULL DEFAULT 'manual' CHECK(source_kind IN ('manual','mix')),
  source_seed_asset_id INTEGER REFERENCES asset(id) ON DELETE SET NULL,
  current_asset_id INTEGER REFERENCES asset(id) ON DELETE SET NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE playlist_item(
  playlist_id INTEGER NOT NULL REFERENCES playlist(id) ON DELETE CASCADE,
  asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  position INTEGER NOT NULL CHECK(position >= 0),
  added_at TEXT NOT NULL,
  PRIMARY KEY(playlist_id,asset_id),
  UNIQUE(playlist_id,position)
);
CREATE TABLE profile(
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES app_user(id),
  name TEXT NOT NULL,
  is_default INTEGER NOT NULL DEFAULT 0 CHECK(is_default IN (0,1)),
  settings_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(user_id, name)
);
CREATE TABLE provider_profile(
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK(kind IN ('inference','agent')),
  provider TEXT NOT NULL,
  label TEXT NOT NULL,
  auth_mode TEXT NOT NULL,
  secret_ref TEXT,
  config_json TEXT NOT NULL DEFAULT '{}',
  enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE quest(
  id INTEGER PRIMARY KEY,
  keyword TEXT,
  origin TEXT,
  created_at TEXT,
  resolved_at TEXT,
  outcome TEXT
);
CREATE TABLE record_rehome(
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL,
  rule TEXT NOT NULL,
  old_asset_id INTEGER NOT NULL,
  new_asset_id INTEGER NOT NULL,
  snapshot_json TEXT NOT NULL,
  moved_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  reverted_at TEXT
);
CREATE TABLE review_decision(
  category TEXT NOT NULL,
  item_key TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('approved','rejected','skipped')),
  reviewer TEXT NOT NULL DEFAULT 'local-default',
  note TEXT NOT NULL DEFAULT '',
  updated_at TEXT NOT NULL,
  PRIMARY KEY(category,item_key)
);
CREATE TABLE review_decision_history(
  id INTEGER PRIMARY KEY,
  category TEXT NOT NULL,
  item_key TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('rejected','skipped')),
  reviewer TEXT NOT NULL,
  note TEXT NOT NULL,
  decided_at TEXT NOT NULL,
  restored_at TEXT NOT NULL
);
CREATE TABLE schema_migration(
  version TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  checksum TEXT NOT NULL,
  applied_at TEXT NOT NULL
);
CREATE TABLE search_history(
  query TEXT PRIMARY KEY,
  used_count INTEGER NOT NULL DEFAULT 1,
  last_used_at TEXT NOT NULL
);
CREATE TABLE seed_pack_history(
    version TEXT PRIMARY KEY,
    revision INTEGER NOT NULL CHECK(revision >= 0),
    content_sha256 TEXT NOT NULL,
    withdrawals_json TEXT NOT NULL DEFAULT '[]',
    imported_at TEXT NOT NULL
);
CREATE TABLE source(
  id INTEGER PRIMARY KEY,
  batch_id TEXT,
  url TEXT,
  url_norm TEXT UNIQUE,
  title TEXT,
  password TEXT,
  platform TEXT,
  note TEXT,
  registered_at TEXT,
  status TEXT
);
CREATE TABLE task_run(
  id INTEGER PRIMARY KEY,
  task_key TEXT NOT NULL,
  trigger TEXT NOT NULL CHECK(trigger IN ('manual','scheduled','startup','cli')),
  status TEXT NOT NULL CHECK(status IN ('pending','running','succeeded','failed','cancelled','interrupted')),
  mutex_key TEXT,
  pid INTEGER,
  host TEXT NOT NULL DEFAULT '',
  started_at TEXT,
  finished_at TEXT,
  heartbeat_at TEXT,
  progress_current INTEGER,
  progress_total INTEGER,
  progress_label TEXT,
  result_summary TEXT NOT NULL DEFAULT '{}',
  error TEXT, parent_run_id INTEGER REFERENCES task_run(id) ON DELETE SET NULL, root_run_id INTEGER REFERENCES task_run(id) ON DELETE SET NULL, followup_key TEXT, followup_depth INTEGER NOT NULL DEFAULT 0,
  CHECK((status IN ('pending','running')) = (finished_at IS NULL))
);
CREATE TABLE want_item(
  id INTEGER PRIMARY KEY,
  code TEXT,
  code_key TEXT,
  follow_item_id INTEGER REFERENCES follow_item(id) ON DELETE SET NULL,
  origin TEXT NOT NULL CHECK(origin IN ('code','feed','follow')),
  title TEXT,
  link TEXT,
  cover_url TEXT,
  release_date TEXT,
  studio TEXT,
  performers TEXT,
  scraped_at TEXT,
  scrape_error TEXT,
  state TEXT NOT NULL DEFAULT 'wanted' CHECK(state IN ('wanted','given_up','acquired')),
  search_count INTEGER NOT NULL DEFAULT 0,
  last_search_at TEXT,
  last_search_outcome TEXT CHECK(last_search_outcome IN ('found','none','error')),
  last_search_note TEXT,
  given_up_at TEXT,
  acquired_asset_id INTEGER,
  acquired_at TEXT,
  acquired_source TEXT,
  acquired_batch TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE TABLE watch_queue(
  profile_id TEXT NOT NULL REFERENCES profile(id) ON DELETE CASCADE,
  asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  added_at TEXT NOT NULL,
  source TEXT NOT NULL DEFAULT 'web',
  PRIMARY KEY(profile_id,asset_id)
);
CREATE VIEW asset_search_source AS
SELECT
  a.id AS asset_id,
  trim(
    COALESCE(a.name,'') || ' ' ||
    COALESCE(a.catalog_title,'') || ' ' ||
    COALESCE(a.original_title,'')
  ) AS name,
  COALESCE(a.code,'') AS code,
  COALESCE((
    SELECT group_concat(value,' ') FROM (
      SELECT DISTINCT e.canonical_name AS value
      FROM asset_entity ae JOIN entity e ON e.id=ae.entity_id
      WHERE ae.asset_id=a.id
      UNION
      SELECT DISTINCT alias.alias AS value
      FROM asset_entity ae JOIN entity_alias alias ON alias.entity_id=ae.entity_id
      WHERE ae.asset_id=a.id
    )
  ),'') AS entities,
  COALESCE((
    SELECT group_concat(DISTINCT term.term)
    FROM asset_entity ae JOIN entity_search_term term ON term.entity_id=ae.entity_id
    WHERE ae.asset_id=a.id
  ),'') AS search_terms
FROM asset a;
CREATE INDEX idx_activity_event_asset_time
ON activity_event(asset_id, occurred_at DESC);
CREATE INDEX idx_activity_event_profile_time
ON activity_event(profile_id, occurred_at DESC);
CREATE INDEX idx_asset_disposal_id ON asset(disposal, id DESC)
WHERE disposal IS NOT NULL;
CREATE INDEX idx_asset_entity_entity_role
ON asset_entity(entity_id, role);
CREATE INDEX idx_asset_hash ON asset(hash_kind, hash);
CREATE INDEX idx_asset_location ON asset(location);
CREATE INDEX idx_asset_name ON asset(name);
CREATE INDEX idx_asset_preference_profile_liked
ON asset_preference(profile_id,liked,updated_at DESC);
CREATE INDEX idx_asset_quality_goal_profile_wanted
ON asset_quality_goal(profile_id,wanted,updated_at DESC);
CREATE INDEX idx_asset_region ON asset(region) WHERE region IS NOT NULL;
CREATE INDEX idx_asset_size ON asset(size);
CREATE INDEX idx_asset_subtitle_asset ON asset_subtitle(asset_id,name);
CREATE INDEX idx_asset_tag_preference_asset
ON asset_tag_preference(asset_id,profile_id,hidden);
CREATE INDEX idx_asset_tag_source_asset ON asset_tag(source, asset_id);
CREATE INDEX idx_asset_tag_tag ON asset_tag(tag);
CREATE INDEX idx_code_sample_image_source ON code_sample_image(source);
CREATE INDEX idx_download_submission_task ON download_submission(task_id, id);
CREATE INDEX idx_download_task_due ON download_task(next_check_at) WHERE next_check_at IS NOT NULL;
CREATE UNIQUE INDEX idx_download_task_info_hash ON download_task(info_hash)
  WHERE info_hash IS NOT NULL;
CREATE INDEX idx_entity_alias_normalized
ON entity_alias(normalized_alias);
CREATE INDEX idx_entity_classification_filter ON entity_classification(facet,value,status,entity_id);
CREATE INDEX idx_entity_identity_link_right ON entity_identity_link(right_id);
CREATE INDEX idx_entity_link_entity_kind
ON entity_link(entity_id,link_kind);
CREATE INDEX idx_entity_membership_agency ON entity_membership(agency_id);
CREATE INDEX idx_entity_redirect_target ON entity_redirect(target_id);
CREATE INDEX idx_entity_search_term_entity
ON entity_search_term(entity_id,purpose);
CREATE INDEX idx_feed_discovery_entity_entity
  ON feed_discovery_entity(entity_id, discovery_id);
CREATE INDEX idx_feed_discovery_recent ON feed_discovery(discovered_at) WHERE ignored_at IS NULL;
CREATE INDEX idx_feed_item_code ON feed_item(code) WHERE code IS NOT NULL;
CREATE INDEX idx_feed_source_due ON feed_source(next_fetch_at) WHERE enabled=1;
CREATE INDEX idx_follow_author_alias_canonical
ON follow_author_alias(canonical_key, alias_name);
CREATE INDEX idx_follow_item_release
ON follow_item(release_key, variant_kind);
CREATE INDEX idx_follow_item_source
ON follow_item(source_id, last_seen_at DESC);
CREATE INDEX idx_follow_item_status
ON follow_item(status, published_at DESC);
CREATE INDEX idx_follow_playback_profile_time
ON follow_playback(profile_id, last_played DESC);
CREATE INDEX idx_follow_source_entity
ON follow_source(entity_id, provider);
CREATE INDEX idx_label_maker_maker ON label_maker(maker_id);
CREATE INDEX idx_media_binding_backend_external
ON media_binding(backend, external_id);
CREATE INDEX idx_performer_profile_fetched ON performer_profile(fetched_at);
CREATE INDEX idx_performer_profile_source ON performer_profile(source);
CREATE INDEX idx_playlist_item_asset
ON playlist_item(asset_id,playlist_id);
CREATE INDEX idx_playlist_profile_updated
ON playlist(profile_id,updated_at DESC,id DESC);
CREATE UNIQUE INDEX idx_profile_one_default_per_user
ON profile(user_id) WHERE is_default=1;
CREATE INDEX idx_record_rehome_source ON record_rehome(source,reverted_at,id);
CREATE INDEX idx_review_decision_history_item
ON review_decision_history(category,item_key,restored_at DESC);
CREATE INDEX idx_review_decision_status
ON review_decision(category,status,updated_at DESC);
CREATE INDEX idx_search_history_recent
ON search_history(last_used_at DESC, query);
CREATE UNIQUE INDEX idx_seed_pack_history_revision
ON seed_pack_history(revision) WHERE revision > 0;
CREATE UNIQUE INDEX idx_task_run_active_mutex ON task_run(mutex_key)
  WHERE mutex_key IS NOT NULL AND status IN ('pending','running');
CREATE INDEX idx_task_run_followup_queue ON task_run(id)
  WHERE followup_key IS NOT NULL AND status='pending';
CREATE INDEX idx_task_run_key_recent ON task_run(task_key, id DESC);
CREATE INDEX idx_task_run_parent ON task_run(parent_run_id, id);
CREATE INDEX idx_task_run_status_recent ON task_run(status, id DESC);
CREATE INDEX idx_want_item_acquired_source ON want_item(acquired_source) WHERE acquired_source IS NOT NULL;
CREATE UNIQUE INDEX idx_want_item_code_key ON want_item(code_key) WHERE code_key IS NOT NULL;
CREATE UNIQUE INDEX idx_want_item_follow ON want_item(follow_item_id) WHERE follow_item_id IS NOT NULL;
CREATE INDEX idx_want_item_state ON want_item(state, created_at);
CREATE INDEX idx_watch_queue_profile_time
ON watch_queue(profile_id,added_at DESC);
CREATE TRIGGER asset_search_alias_delete AFTER DELETE ON entity_alias BEGIN
  DELETE FROM asset_search WHERE asset_id IN (
    SELECT asset_id FROM asset_entity WHERE entity_id=OLD.entity_id
  );
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms FROM asset_search_source
  WHERE asset_id IN (SELECT asset_id FROM asset_entity WHERE entity_id=OLD.entity_id);
END;
CREATE TRIGGER asset_search_alias_insert AFTER INSERT ON entity_alias BEGIN
  DELETE FROM asset_search WHERE asset_id IN (
    SELECT asset_id FROM asset_entity WHERE entity_id=NEW.entity_id
  );
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms FROM asset_search_source
  WHERE asset_id IN (SELECT asset_id FROM asset_entity WHERE entity_id=NEW.entity_id);
END;
CREATE TRIGGER asset_search_alias_update AFTER UPDATE OF alias,entity_id ON entity_alias BEGIN
  DELETE FROM asset_search WHERE asset_id IN (
    SELECT asset_id FROM asset_entity WHERE entity_id IN (OLD.entity_id,NEW.entity_id)
  );
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms FROM asset_search_source
  WHERE asset_id IN (
    SELECT asset_id FROM asset_entity WHERE entity_id IN (OLD.entity_id,NEW.entity_id)
  );
END;
CREATE TRIGGER asset_search_asset_delete AFTER DELETE ON asset BEGIN
  DELETE FROM asset_search WHERE asset_id=OLD.id;
END;
CREATE TRIGGER asset_search_asset_insert AFTER INSERT ON asset BEGIN
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms FROM asset_search_source WHERE asset_id=NEW.id;
END;
CREATE TRIGGER asset_search_asset_update
AFTER UPDATE OF name,code,catalog_title,original_title ON asset BEGIN
  DELETE FROM asset_search WHERE asset_id=OLD.id;
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms
  FROM asset_search_source WHERE asset_id=NEW.id;
END;
CREATE TRIGGER asset_search_entity_update AFTER UPDATE OF canonical_name ON entity BEGIN
  DELETE FROM asset_search WHERE asset_id IN (
    SELECT asset_id FROM asset_entity WHERE entity_id=NEW.id
  );
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms FROM asset_search_source
  WHERE asset_id IN (SELECT asset_id FROM asset_entity WHERE entity_id=NEW.id);
END;
CREATE TRIGGER asset_search_relation_delete AFTER DELETE ON asset_entity BEGIN
  DELETE FROM asset_search WHERE asset_id=OLD.asset_id;
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms FROM asset_search_source WHERE asset_id=OLD.asset_id;
END;
CREATE TRIGGER asset_search_relation_insert AFTER INSERT ON asset_entity BEGIN
  DELETE FROM asset_search WHERE asset_id=NEW.asset_id;
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms
  FROM asset_search_source WHERE asset_id=NEW.asset_id;
END;
CREATE TRIGGER asset_search_relation_update AFTER UPDATE OF asset_id,entity_id ON asset_entity BEGIN
  DELETE FROM asset_search WHERE asset_id IN (OLD.asset_id,NEW.asset_id);
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms FROM asset_search_source
  WHERE asset_id IN (OLD.asset_id,NEW.asset_id);
END;
CREATE TRIGGER asset_search_term_delete AFTER DELETE ON entity_search_term BEGIN
  DELETE FROM asset_search WHERE asset_id IN (
    SELECT asset_id FROM asset_entity WHERE entity_id=OLD.entity_id
  );
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms FROM asset_search_source
  WHERE asset_id IN (SELECT asset_id FROM asset_entity WHERE entity_id=OLD.entity_id);
END;
CREATE TRIGGER asset_search_term_insert AFTER INSERT ON entity_search_term BEGIN
  DELETE FROM asset_search WHERE asset_id IN (
    SELECT asset_id FROM asset_entity WHERE entity_id=NEW.entity_id
  );
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms FROM asset_search_source
  WHERE asset_id IN (SELECT asset_id FROM asset_entity WHERE entity_id=NEW.entity_id);
END;
CREATE TRIGGER asset_search_term_update AFTER UPDATE OF term,entity_id ON entity_search_term BEGIN
  DELETE FROM asset_search WHERE asset_id IN (
    SELECT asset_id FROM asset_entity WHERE entity_id IN (OLD.entity_id,NEW.entity_id)
  );
  INSERT INTO asset_search(asset_id,name,code,entities,search_terms)
  SELECT asset_id,name,code,entities,search_terms FROM asset_search_source
  WHERE asset_id IN (
    SELECT asset_id FROM asset_entity WHERE entity_id IN (OLD.entity_id,NEW.entity_id)
  );
END;
CREATE TRIGGER entity_classification_cleanup AFTER DELETE ON entity BEGIN
  DELETE FROM entity_classification WHERE entity_id=OLD.id;
  DELETE FROM entity_identity_link WHERE left_id=OLD.id OR right_id=OLD.id;
END;
CREATE TRIGGER entity_redirect_id_reuse AFTER INSERT ON entity BEGIN
  DELETE FROM entity_redirect WHERE old_id=NEW.id;
END;
CREATE TRIGGER entity_redirect_target_delete AFTER DELETE ON entity BEGIN
  DELETE FROM entity_redirect WHERE target_id=OLD.id;
END;
CREATE TRIGGER rev_activity_event_delete AFTER DELETE ON activity_event BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='activity_event';
END;
CREATE TRIGGER rev_activity_event_insert AFTER INSERT ON activity_event BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='activity_event';
END;
CREATE TRIGGER rev_activity_event_update AFTER UPDATE ON activity_event BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='activity_event';
END;
CREATE TRIGGER rev_app_user_delete AFTER DELETE ON app_user BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='app_user';
END;
CREATE TRIGGER rev_app_user_insert AFTER INSERT ON app_user BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='app_user';
END;
CREATE TRIGGER rev_app_user_update AFTER UPDATE ON app_user BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='app_user';
END;
CREATE TRIGGER rev_asset_delete AFTER DELETE ON asset BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset';
END;
CREATE TRIGGER rev_asset_entity_delete AFTER DELETE ON asset_entity BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_entity';
END;
CREATE TRIGGER rev_asset_entity_insert AFTER INSERT ON asset_entity BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_entity';
END;
CREATE TRIGGER rev_asset_entity_update AFTER UPDATE ON asset_entity BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_entity';
END;
CREATE TRIGGER rev_asset_insert AFTER INSERT ON asset BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset';
END;
CREATE TRIGGER rev_asset_preference_delete AFTER DELETE ON asset_preference BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_preference';
END;
CREATE TRIGGER rev_asset_preference_insert AFTER INSERT ON asset_preference BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_preference';
END;
CREATE TRIGGER rev_asset_preference_update AFTER UPDATE ON asset_preference BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_preference';
END;
CREATE TRIGGER rev_asset_quality_goal_delete AFTER DELETE ON asset_quality_goal BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_quality_goal';
END;
CREATE TRIGGER rev_asset_quality_goal_insert AFTER INSERT ON asset_quality_goal BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_quality_goal';
END;
CREATE TRIGGER rev_asset_quality_goal_update AFTER UPDATE ON asset_quality_goal BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_quality_goal';
END;
CREATE TRIGGER rev_asset_subtitle_delete AFTER DELETE ON asset_subtitle BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_subtitle';
END;
CREATE TRIGGER rev_asset_subtitle_insert AFTER INSERT ON asset_subtitle BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_subtitle';
END;
CREATE TRIGGER rev_asset_subtitle_update AFTER UPDATE ON asset_subtitle BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_subtitle';
END;
CREATE TRIGGER rev_asset_tag_delete AFTER DELETE ON asset_tag BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_tag';
END;
CREATE TRIGGER rev_asset_tag_insert AFTER INSERT ON asset_tag BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_tag';
END;
CREATE TRIGGER rev_asset_tag_preference_delete AFTER DELETE ON asset_tag_preference BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_tag_preference';
END;
CREATE TRIGGER rev_asset_tag_preference_insert AFTER INSERT ON asset_tag_preference BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_tag_preference';
END;
CREATE TRIGGER rev_asset_tag_preference_update AFTER UPDATE ON asset_tag_preference BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_tag_preference';
END;
CREATE TRIGGER rev_asset_tag_update AFTER UPDATE ON asset_tag BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset_tag';
END;
CREATE TRIGGER rev_asset_update AFTER UPDATE ON asset BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='asset';
END;
CREATE TRIGGER rev_code_sample_image_delete AFTER DELETE ON code_sample_image BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='code_sample_image';
END;
CREATE TRIGGER rev_code_sample_image_insert AFTER INSERT ON code_sample_image BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='code_sample_image';
END;
CREATE TRIGGER rev_code_sample_image_update AFTER UPDATE ON code_sample_image BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='code_sample_image';
END;
CREATE TRIGGER rev_download_submission_delete AFTER DELETE ON download_submission BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='download_submission';
END;
CREATE TRIGGER rev_download_submission_insert AFTER INSERT ON download_submission BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='download_submission';
END;
CREATE TRIGGER rev_download_submission_update AFTER UPDATE ON download_submission BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='download_submission';
END;
CREATE TRIGGER rev_download_task_delete AFTER DELETE ON download_task BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='download_task';
END;
CREATE TRIGGER rev_download_task_insert AFTER INSERT ON download_task BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='download_task';
END;
CREATE TRIGGER rev_download_task_update AFTER UPDATE ON download_task BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='download_task';
END;
CREATE TRIGGER rev_entity_alias_delete AFTER DELETE ON entity_alias BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_alias';
END;
CREATE TRIGGER rev_entity_alias_insert AFTER INSERT ON entity_alias BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_alias';
END;
CREATE TRIGGER rev_entity_alias_update AFTER UPDATE ON entity_alias BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_alias';
END;
CREATE TRIGGER rev_entity_classification_delete AFTER DELETE ON entity_classification BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_classification';
END;
CREATE TRIGGER rev_entity_classification_insert AFTER INSERT ON entity_classification BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_classification';
END;
CREATE TRIGGER rev_entity_classification_update AFTER UPDATE ON entity_classification BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_classification';
END;
CREATE TRIGGER rev_entity_delete AFTER DELETE ON entity BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity';
END;
CREATE TRIGGER rev_entity_external_ref_delete AFTER DELETE ON entity_external_ref BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_external_ref';
END;
CREATE TRIGGER rev_entity_external_ref_insert AFTER INSERT ON entity_external_ref BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_external_ref';
END;
CREATE TRIGGER rev_entity_external_ref_update AFTER UPDATE ON entity_external_ref BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_external_ref';
END;
CREATE TRIGGER rev_entity_identity_link_delete AFTER DELETE ON entity_identity_link BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_identity_link';
END;
CREATE TRIGGER rev_entity_identity_link_insert AFTER INSERT ON entity_identity_link BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_identity_link';
END;
CREATE TRIGGER rev_entity_identity_link_update AFTER UPDATE ON entity_identity_link BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_identity_link';
END;
CREATE TRIGGER rev_entity_insert AFTER INSERT ON entity BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity';
END;
CREATE TRIGGER rev_entity_link_delete AFTER DELETE ON entity_link BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_link';
END;
CREATE TRIGGER rev_entity_link_insert AFTER INSERT ON entity_link BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_link';
END;
CREATE TRIGGER rev_entity_link_update AFTER UPDATE ON entity_link BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_link';
END;
CREATE TRIGGER rev_entity_membership_delete AFTER DELETE ON entity_membership BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_membership';
END;
CREATE TRIGGER rev_entity_membership_insert AFTER INSERT ON entity_membership BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_membership';
END;
CREATE TRIGGER rev_entity_membership_update AFTER UPDATE ON entity_membership BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_membership';
END;
CREATE TRIGGER rev_entity_redirect_delete AFTER DELETE ON entity_redirect BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_redirect';
END;
CREATE TRIGGER rev_entity_redirect_insert AFTER INSERT ON entity_redirect BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_redirect';
END;
CREATE TRIGGER rev_entity_redirect_update AFTER UPDATE ON entity_redirect BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_redirect';
END;
CREATE TRIGGER rev_entity_search_term_delete AFTER DELETE ON entity_search_term BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_search_term';
END;
CREATE TRIGGER rev_entity_search_term_insert AFTER INSERT ON entity_search_term BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_search_term';
END;
CREATE TRIGGER rev_entity_search_term_update AFTER UPDATE ON entity_search_term BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity_search_term';
END;
CREATE TRIGGER rev_entity_update AFTER UPDATE ON entity BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='entity';
END;
CREATE TRIGGER rev_feed_discovery_delete AFTER DELETE ON feed_discovery BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_discovery';
END;
CREATE TRIGGER rev_feed_discovery_entity_delete AFTER DELETE ON feed_discovery_entity BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_discovery_entity';
END;
CREATE TRIGGER rev_feed_discovery_entity_insert AFTER INSERT ON feed_discovery_entity BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_discovery_entity';
END;
CREATE TRIGGER rev_feed_discovery_entity_update AFTER UPDATE ON feed_discovery_entity BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_discovery_entity';
END;
CREATE TRIGGER rev_feed_discovery_insert AFTER INSERT ON feed_discovery BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_discovery';
END;
CREATE TRIGGER rev_feed_discovery_update AFTER UPDATE ON feed_discovery BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_discovery';
END;
CREATE TRIGGER rev_feed_item_delete AFTER DELETE ON feed_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_item';
END;
CREATE TRIGGER rev_feed_item_insert AFTER INSERT ON feed_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_item';
END;
CREATE TRIGGER rev_feed_item_update AFTER UPDATE ON feed_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_item';
END;
CREATE TRIGGER rev_feed_source_delete AFTER DELETE ON feed_source BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_source';
END;
CREATE TRIGGER rev_feed_source_insert AFTER INSERT ON feed_source BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_source';
END;
CREATE TRIGGER rev_feed_source_update AFTER UPDATE ON feed_source BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='feed_source';
END;
CREATE TRIGGER rev_follow_author_alias_delete AFTER DELETE ON follow_author_alias BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_author_alias';
END;
CREATE TRIGGER rev_follow_author_alias_insert AFTER INSERT ON follow_author_alias BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_author_alias';
END;
CREATE TRIGGER rev_follow_author_alias_update AFTER UPDATE ON follow_author_alias BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_author_alias';
END;
CREATE TRIGGER rev_follow_item_delete AFTER DELETE ON follow_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_item';
END;
CREATE TRIGGER rev_follow_item_insert AFTER INSERT ON follow_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_item';
END;
CREATE TRIGGER rev_follow_item_update AFTER UPDATE ON follow_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_item';
END;
CREATE TRIGGER rev_follow_playback_delete AFTER DELETE ON follow_playback BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_playback';
END;
CREATE TRIGGER rev_follow_playback_insert AFTER INSERT ON follow_playback BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_playback';
END;
CREATE TRIGGER rev_follow_playback_update AFTER UPDATE ON follow_playback BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_playback';
END;
CREATE TRIGGER rev_follow_source_delete AFTER DELETE ON follow_source BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_source';
END;
CREATE TRIGGER rev_follow_source_insert AFTER INSERT ON follow_source BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_source';
END;
CREATE TRIGGER rev_follow_source_update AFTER UPDATE ON follow_source BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='follow_source';
END;
CREATE TRIGGER rev_genre_decision_delete AFTER DELETE ON genre_decision BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='genre_decision';
END;
CREATE TRIGGER rev_genre_decision_insert AFTER INSERT ON genre_decision BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='genre_decision';
END;
CREATE TRIGGER rev_genre_decision_update AFTER UPDATE ON genre_decision BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='genre_decision';
END;
CREATE TRIGGER rev_label_maker_delete AFTER DELETE ON label_maker BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='label_maker';
END;
CREATE TRIGGER rev_label_maker_insert AFTER INSERT ON label_maker BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='label_maker';
END;
CREATE TRIGGER rev_label_maker_update AFTER UPDATE ON label_maker BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='label_maker';
END;
CREATE TRIGGER rev_ledger_upgrade_repair_delete AFTER DELETE ON ledger_upgrade_repair BEGIN
    UPDATE ledger_revision SET n=n+1 WHERE tbl='ledger_upgrade_repair';
END;
CREATE TRIGGER rev_ledger_upgrade_repair_insert AFTER INSERT ON ledger_upgrade_repair BEGIN
    UPDATE ledger_revision SET n=n+1 WHERE tbl='ledger_upgrade_repair';
END;
CREATE TRIGGER rev_ledger_upgrade_repair_update AFTER UPDATE ON ledger_upgrade_repair BEGIN
    UPDATE ledger_revision SET n=n+1 WHERE tbl='ledger_upgrade_repair';
END;
CREATE TRIGGER rev_media_binding_delete AFTER DELETE ON media_binding BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='media_binding';
END;
CREATE TRIGGER rev_media_binding_insert AFTER INSERT ON media_binding BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='media_binding';
END;
CREATE TRIGGER rev_media_binding_update AFTER UPDATE ON media_binding BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='media_binding';
END;
CREATE TRIGGER rev_performer_profile_delete AFTER DELETE ON performer_profile BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='performer_profile';
END;
CREATE TRIGGER rev_performer_profile_insert AFTER INSERT ON performer_profile BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='performer_profile';
END;
CREATE TRIGGER rev_performer_profile_update AFTER UPDATE ON performer_profile BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='performer_profile';
END;
CREATE TRIGGER rev_playlist_delete AFTER DELETE ON playlist BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='playlist';
END;
CREATE TRIGGER rev_playlist_insert AFTER INSERT ON playlist BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='playlist';
END;
CREATE TRIGGER rev_playlist_item_delete AFTER DELETE ON playlist_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='playlist_item';
END;
CREATE TRIGGER rev_playlist_item_insert AFTER INSERT ON playlist_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='playlist_item';
END;
CREATE TRIGGER rev_playlist_item_update AFTER UPDATE ON playlist_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='playlist_item';
END;
CREATE TRIGGER rev_playlist_update AFTER UPDATE ON playlist BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='playlist';
END;
CREATE TRIGGER rev_profile_delete AFTER DELETE ON profile BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='profile';
END;
CREATE TRIGGER rev_profile_insert AFTER INSERT ON profile BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='profile';
END;
CREATE TRIGGER rev_profile_update AFTER UPDATE ON profile BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='profile';
END;
CREATE TRIGGER rev_provider_profile_delete AFTER DELETE ON provider_profile BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='provider_profile';
END;
CREATE TRIGGER rev_provider_profile_insert AFTER INSERT ON provider_profile BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='provider_profile';
END;
CREATE TRIGGER rev_provider_profile_update AFTER UPDATE ON provider_profile BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='provider_profile';
END;
CREATE TRIGGER rev_quest_delete AFTER DELETE ON quest BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='quest';
END;
CREATE TRIGGER rev_quest_insert AFTER INSERT ON quest BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='quest';
END;
CREATE TRIGGER rev_quest_update AFTER UPDATE ON quest BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='quest';
END;
CREATE TRIGGER rev_record_rehome_delete AFTER DELETE ON record_rehome BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='record_rehome';
END;
CREATE TRIGGER rev_record_rehome_insert AFTER INSERT ON record_rehome BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='record_rehome';
END;
CREATE TRIGGER rev_record_rehome_update AFTER UPDATE ON record_rehome BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='record_rehome';
END;
CREATE TRIGGER rev_review_decision_delete AFTER DELETE ON review_decision BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='review_decision';
END;
CREATE TRIGGER rev_review_decision_history_delete AFTER DELETE ON review_decision_history BEGIN
    UPDATE ledger_revision SET n=n+1 WHERE tbl='review_decision_history';
END;
CREATE TRIGGER rev_review_decision_history_insert AFTER INSERT ON review_decision_history BEGIN
    UPDATE ledger_revision SET n=n+1 WHERE tbl='review_decision_history';
END;
CREATE TRIGGER rev_review_decision_history_update AFTER UPDATE ON review_decision_history BEGIN
    UPDATE ledger_revision SET n=n+1 WHERE tbl='review_decision_history';
END;
CREATE TRIGGER rev_review_decision_insert AFTER INSERT ON review_decision BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='review_decision';
END;
CREATE TRIGGER rev_review_decision_update AFTER UPDATE ON review_decision BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='review_decision';
END;
CREATE TRIGGER rev_search_history_delete AFTER DELETE ON search_history BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='search_history';
END;
CREATE TRIGGER rev_search_history_insert AFTER INSERT ON search_history BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='search_history';
END;
CREATE TRIGGER rev_search_history_update AFTER UPDATE ON search_history BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='search_history';
END;
CREATE TRIGGER rev_seed_pack_history_delete AFTER DELETE ON seed_pack_history BEGIN
    UPDATE ledger_revision SET n=n+1 WHERE tbl='seed_pack_history';
END;
CREATE TRIGGER rev_seed_pack_history_insert AFTER INSERT ON seed_pack_history BEGIN
    UPDATE ledger_revision SET n=n+1 WHERE tbl='seed_pack_history';
END;
CREATE TRIGGER rev_seed_pack_history_update AFTER UPDATE ON seed_pack_history BEGIN
    UPDATE ledger_revision SET n=n+1 WHERE tbl='seed_pack_history';
END;
CREATE TRIGGER rev_source_delete AFTER DELETE ON source BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='source';
END;
CREATE TRIGGER rev_source_insert AFTER INSERT ON source BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='source';
END;
CREATE TRIGGER rev_source_update AFTER UPDATE ON source BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='source';
END;
CREATE TRIGGER rev_want_item_delete AFTER DELETE ON want_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='want_item';
END;
CREATE TRIGGER rev_want_item_insert AFTER INSERT ON want_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='want_item';
END;
CREATE TRIGGER rev_want_item_update AFTER UPDATE ON want_item BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='want_item';
END;
CREATE TRIGGER rev_watch_queue_delete AFTER DELETE ON watch_queue BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='watch_queue';
END;
CREATE TRIGGER rev_watch_queue_insert AFTER INSERT ON watch_queue BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='watch_queue';
END;
CREATE TRIGGER rev_watch_queue_update AFTER UPDATE ON watch_queue BEGIN
  UPDATE ledger_revision SET n=n+1 WHERE tbl='watch_queue';
END;
CREATE TRIGGER task_run_clear_deleted_references
BEFORE DELETE ON task_run
BEGIN
  UPDATE task_run
  SET parent_run_id = CASE WHEN parent_run_id = OLD.id THEN NULL ELSE parent_run_id END,
      root_run_id = CASE WHEN root_run_id = OLD.id THEN NULL ELSE root_run_id END
  WHERE parent_run_id = OLD.id OR root_run_id = OLD.id;
END;
