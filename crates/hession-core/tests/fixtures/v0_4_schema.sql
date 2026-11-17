-- Frozen v0.4.0 schema from dfc311e, without application schema metadata.

CREATE TABLE IF NOT EXISTS agent (
  id INTEGER PRIMARY KEY,
  family TEXT NOT NULL,
  product TEXT NOT NULL,
  install_root TEXT,
  UNIQUE(family, install_root)
);
CREATE TABLE IF NOT EXISTS workspace (
  id INTEGER PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  display_name TEXT
);
CREATE TABLE IF NOT EXISTS repository (
  id INTEGER PRIMARY KEY,
  common_dir TEXT,
  remote_url TEXT
);
CREATE TABLE IF NOT EXISTS model (
  id INTEGER PRIMARY KEY,
  provider TEXT,
  model_key TEXT NOT NULL,
  display_name TEXT,
  UNIQUE(provider, model_key)
);
CREATE TABLE IF NOT EXISTS session (
  id INTEGER PRIMARY KEY,
  agent_id INTEGER NOT NULL REFERENCES agent(id),
  workspace_id INTEGER REFERENCES workspace(id),
  external_id TEXT,
  source_uri TEXT NOT NULL UNIQUE,
  source_format TEXT,
  source_version TEXT,
  agent_version TEXT,
  cwd TEXT,
  title TEXT,
  started_at TEXT,
  last_event_at TEXT,
  imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  source_size INTEGER,
  source_mtime REAL,
  raw_metadata_json TEXT,
  token_usage_json TEXT,
  metrics_json TEXT
);
CREATE INDEX IF NOT EXISTS session_started_idx ON session(started_at DESC);
CREATE INDEX IF NOT EXISTS session_workspace_idx ON session(workspace_id, started_at DESC);
CREATE INDEX IF NOT EXISTS session_agent_idx ON session(agent_id, started_at DESC);
CREATE TABLE IF NOT EXISTS session_annotation (
  session_id INTEGER PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
  starred INTEGER NOT NULL DEFAULT 0 CHECK (starred IN (0,1))
);
CREATE TABLE IF NOT EXISTS session_tag (
  session_id INTEGER NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  label TEXT NOT NULL COLLATE NOCASE,
  PRIMARY KEY (session_id, label)
);
CREATE INDEX IF NOT EXISTS session_tag_label_idx ON session_tag(label);
CREATE TABLE IF NOT EXISTS sync_checkpoint (
  family TEXT PRIMARY KEY,
  completed_at TEXT NOT NULL,
  source_cutoff TEXT
);
-- `source_cutoff` records when the successful run started.
CREATE TABLE IF NOT EXISTS session_context (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  entry_id INTEGER REFERENCES entry(id) ON DELETE CASCADE,
  observed_at TEXT,
  cwd TEXT,
  repository_id INTEGER REFERENCES repository(id),
  branch TEXT,
  commit_sha TEXT,
  model_id INTEGER REFERENCES model(id),
  agent_role TEXT,
  permission_mode TEXT,
  raw_json TEXT
);
CREATE INDEX IF NOT EXISTS context_session_time_idx ON session_context(session_id, observed_at);
CREATE INDEX IF NOT EXISTS context_session_latest_idx ON session_context(session_id, id DESC);
CREATE INDEX IF NOT EXISTS context_entry_idx ON session_context(entry_id);
CREATE TABLE IF NOT EXISTS entry (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL,
  external_entry_id TEXT,
  parent_entry_id TEXT,
  turn_id TEXT,
  occurred_at TEXT,
  kind TEXT NOT NULL,
  role TEXT,
  search_text TEXT NOT NULL DEFAULT '',
  payload_json TEXT,
  UNIQUE(session_id, ordinal)
);
CREATE INDEX IF NOT EXISTS entry_session_order_idx ON entry(session_id, ordinal);
CREATE INDEX IF NOT EXISTS entry_time_idx ON entry(occurred_at);
CREATE TABLE IF NOT EXISTS content_part (
  id INTEGER PRIMARY KEY,
  entry_id INTEGER NOT NULL REFERENCES entry(id) ON DELETE CASCADE,
  ordinal INTEGER NOT NULL,
  kind TEXT NOT NULL,
  text TEXT,
  data_json TEXT,
  UNIQUE(entry_id, ordinal)
);
CREATE TABLE IF NOT EXISTS tool_invocation (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  entry_id INTEGER REFERENCES entry(id) ON DELETE SET NULL,
  call_id TEXT,
  tool_name TEXT NOT NULL,
  command_text TEXT,
  arguments_json TEXT,
  result_text TEXT,
  status TEXT,
  exit_code INTEGER,
  started_at TEXT,
  ended_at TEXT,
  duration_ms INTEGER
);
CREATE INDEX IF NOT EXISTS tool_name_idx ON tool_invocation(tool_name);
CREATE INDEX IF NOT EXISTS tool_session_idx ON tool_invocation(session_id);
CREATE INDEX IF NOT EXISTS tool_entry_idx ON tool_invocation(entry_id);
CREATE INDEX IF NOT EXISTS tool_call_idx ON tool_invocation(session_id,call_id);
CREATE TABLE IF NOT EXISTS file_activity (
  id INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES session(id) ON DELETE CASCADE,
  tool_invocation_id INTEGER REFERENCES tool_invocation(id) ON DELETE SET NULL,
  entry_id INTEGER REFERENCES entry(id) ON DELETE SET NULL,
  path TEXT NOT NULL,
  repository_path TEXT,
  action TEXT,
  evidence TEXT,
  confidence TEXT
);
CREATE INDEX IF NOT EXISTS file_path_idx ON file_activity(path);
CREATE INDEX IF NOT EXISTS file_session_idx ON file_activity(session_id);
CREATE INDEX IF NOT EXISTS file_entry_idx ON file_activity(entry_id);
CREATE INDEX IF NOT EXISTS file_tool_idx ON file_activity(tool_invocation_id);
CREATE INDEX IF NOT EXISTS session_activity_idx ON session(COALESCE(last_event_at,started_at) DESC,id DESC);
DROP TABLE IF EXISTS embedding;
-- Incremental sync state: a source is skipped when its current size and
-- modification time (milliseconds) still match the last successful import.
CREATE TABLE IF NOT EXISTS sync_state (
  source_uri TEXT PRIMARY KEY,
  size INTEGER NOT NULL,
  mtime REAL NOT NULL,
  synced_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE VIRTUAL TABLE IF NOT EXISTS entry_fts USING fts5(
  search_text, content='entry', content_rowid='id', tokenize='unicode61 remove_diacritics 2'
);
CREATE VIRTUAL TABLE IF NOT EXISTS entry_fts_vocab USING fts5vocab(entry_fts, 'row');
CREATE TRIGGER IF NOT EXISTS entry_ai AFTER INSERT ON entry BEGIN
  INSERT INTO entry_fts(rowid, search_text) VALUES (new.id, new.search_text);
END;
CREATE TRIGGER IF NOT EXISTS entry_ad AFTER DELETE ON entry BEGIN
  INSERT INTO entry_fts(entry_fts, rowid, search_text) VALUES ('delete', old.id, old.search_text);
END;
CREATE TRIGGER IF NOT EXISTS entry_au AFTER UPDATE OF search_text ON entry BEGIN
  INSERT INTO entry_fts(entry_fts, rowid, search_text) VALUES ('delete', old.id, old.search_text);
  INSERT INTO entry_fts(rowid, search_text) VALUES (new.id, new.search_text);
END;


CREATE TABLE IF NOT EXISTS session_analytics (
  session_id INTEGER PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,
  agent_id INTEGER NOT NULL REFERENCES agent(id),
  workspace_id INTEGER REFERENCES workspace(id),
  started_at TEXT,
  last_event_at TEXT,
  duration_seconds REAL DEFAULT 0,
  model_name TEXT,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cached_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens INTEGER NOT NULL DEFAULT 0,
  estimated_cost REAL NOT NULL DEFAULT 0,
  user_turns INTEGER NOT NULL DEFAULT 0,
  assistant_turns INTEGER NOT NULL DEFAULT 0,
  total_turns INTEGER NOT NULL DEFAULT 0,
  tool_count INTEGER NOT NULL DEFAULT 0,
  tool_error_count INTEGER NOT NULL DEFAULT 0,
  calculated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS analytics_refresh_queue (
  session_id INTEGER PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS sa_time_idx ON session_analytics(started_at DESC);
CREATE INDEX IF NOT EXISTS sa_agent_idx ON session_analytics(agent_id, started_at DESC);
CREATE INDEX IF NOT EXISTS sa_ws_idx ON session_analytics(workspace_id, started_at DESC);
CREATE INDEX IF NOT EXISTS sa_model_idx ON session_analytics(model_name);
CREATE INDEX IF NOT EXISTS tool_duration_idx ON tool_invocation(duration_ms DESC);
CREATE INDEX IF NOT EXISTS tool_status_idx ON tool_invocation(status);
CREATE INDEX IF NOT EXISTS tool_metrics_name_idx ON tool_invocation(tool_name,session_id,status,exit_code,duration_ms);
CREATE INDEX IF NOT EXISTS tool_metrics_status_idx ON tool_invocation(status,session_id);
CREATE INDEX IF NOT EXISTS tool_failures_idx ON tool_invocation(session_id,tool_name,exit_code,started_at)
  WHERE status='error' OR (exit_code IS NOT NULL AND exit_code!=0);


CREATE TABLE IF NOT EXISTS model_pricing_cache (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  payload_json TEXT NOT NULL,
  model_count INTEGER NOT NULL DEFAULT 0,
  source_url TEXT NOT NULL DEFAULT 'https://models.dev/api.json',
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS model_price_overrides (
  provider TEXT NOT NULL DEFAULT '',
  model_id TEXT NOT NULL,
  display_name TEXT,
  in_per_1m REAL NOT NULL,
  out_per_1m REAL NOT NULL,
  cache_per_1m REAL NOT NULL DEFAULT 0,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (provider, model_id)
);


CREATE TRIGGER IF NOT EXISTS session_analytics_invalidate AFTER UPDATE ON session BEGIN
  DELETE FROM session_analytics WHERE session_id = new.id;
END;

ALTER TABLE session ADD COLUMN host TEXT;
CREATE INDEX session_host_idx ON session(host);
