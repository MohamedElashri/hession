//! Local-first SQLite storage: core session tables plus the analytics and
//! pricing schemas.
//!
//! WAL readers use independent snapshots; short writes share a priority gate.
//! Permission hardening: data dir `0700`, db file `0600` (unix, best-effort).
//!
//! `init_schema` creates missing tables and indexes without clearing saved data.
//! Bundled SQLite provides FTS5 with the `unicode61 remove_diacritics` tokenizer.

use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

// Errors

/// All database errors flow through this type (`thiserror`, as required).
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Wrapped rusqlite failure with the operation that caused it.
    #[error("sqlite error ({context}): {source}")]
    Sqlite {
        context: String,
        #[source]
        source: rusqlite::Error,
    },
    /// Filesystem failure (mkdir/chmod).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Plain message error (validation / unexpected state).
    #[error("{0}")]
    Message(String),
}

/// Crate-local `Result` so signatures read `-> Result<T>`.
pub type Result<T> = std::result::Result<T, DbError>;

fn sqlite_err(context: impl Into<String>, source: rusqlite::Error) -> DbError {
    DbError::Sqlite {
        context: context.into(),
        source,
    }
}

// Paths & permissions

/// Milliseconds for `busy_timeout`.
pub const BUSY_TIMEOUT_MS: u64 = 5000;

#[derive(Default)]
struct Writers {
    active: bool,
    users_waiting: usize,
}

fn writers() -> &'static (Mutex<Writers>, Condvar) {
    static WRITERS: OnceLock<(Mutex<Writers>, Condvar)> = OnceLock::new();
    WRITERS.get_or_init(|| (Mutex::new(Writers::default()), Condvar::new()))
}

/// Serialize writes without holding a database read snapshot while waiting.
/// Interactive actions take the next slot ahead of import/analytics batches.
pub struct WriteGuard;

impl Drop for WriteGuard {
    fn drop(&mut self) {
        let (state, ready) = writers();
        state.lock().unwrap_or_else(|e| e.into_inner()).active = false;
        ready.notify_all();
    }
}

fn acquire_write(interactive: bool) -> WriteGuard {
    let (state, ready) = writers();
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    if interactive {
        state.users_waiting += 1;
    }
    while state.active || (!interactive && state.users_waiting > 0) {
        state = ready.wait(state).unwrap_or_else(|e| e.into_inner());
    }
    if interactive {
        state.users_waiting -= 1;
    }
    state.active = true;
    WriteGuard
}

pub fn user_write() -> WriteGuard {
    acquire_write(true)
}

pub(crate) fn background_write() -> WriteGuard {
    acquire_write(false)
}

pub(crate) fn user_write_pending() -> bool {
    writers()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .users_waiting
        > 0
}

/// Imported/derived data can be rebuilt. NORMAL avoids an fsync per source;
/// interactive annotations keep FULL durability on their own connections.
pub(crate) fn configure_bulk_writer(conn: &Connection) -> Result<()> {
    conn.execute_batch("PRAGMA synchronous=NORMAL; PRAGMA cache_size=-32768;")
        .map_err(|e| sqlite_err("configure bulk writer", e))
}

/// Data directory: `$<PREFIX>_DATA_DIR` or `~/.<slug>` (see `crate::brand`).
pub fn data_dir() -> PathBuf {
    let leaf = format!(".{}", crate::brand::SLUG);
    if let Ok(dir) = std::env::var(crate::brand::env_var("DATA_DIR")) {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home).join(&leaf);
        }
    }
    PathBuf::from(leaf)
}

/// Full database path: `<data_dir>/sessions.sqlite`.
pub fn db_path() -> PathBuf {
    data_dir().join("sessions.sqlite")
}

#[cfg(unix)]
fn chmod_path(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn ensure_dir_permissions(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    chmod_path(dir, 0o700)?;
    Ok(())
}

fn ensure_file_permissions(file: &Path) -> Result<()> {
    #[cfg(unix)]
    chmod_path(file, 0o600)?;
    Ok(())
}

// Open

#[derive(PartialEq, Eq)]
struct DatabaseIdentity {
    device: u64,
    inode: u64,
}

fn database_identity(path: &Path) -> Result<DatabaseIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::metadata(path)?;
    Ok(DatabaseIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

/// Open (creating parents as needed), configure pragmas, init schema.
///
/// Schema initialization is serialized and runs once per database per process.
/// An idle connection keeps WAL alive between commands, avoiding repeated
/// last-connection checkpoints. Journal mode is set only during initialization.
pub fn open_db(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            ensure_dir_permissions(parent)?;
        }
    }
    static INITIALIZED: OnceLock<Mutex<HashMap<PathBuf, (DatabaseIdentity, Connection)>>> =
        OnceLock::new();
    let mut initialized = INITIALIZED
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| DbError::Message("Could not access database initialization state".into()))?;
    // Check before opening: a retained connection may still refer to the old
    // inode and WAL. Reinitializing a replacement beside it risks saved data.
    let key = if path.exists() {
        fs::canonicalize(path)?
    } else {
        fs::canonicalize(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .join(
            path.file_name()
                .ok_or_else(|| DbError::Message("Invalid database path".into()))?,
        )
    };
    if let Some((identity, _)) = initialized.get(&key) {
        if !path.exists() || *identity != database_identity(path)? {
            return Err(DbError::Message(format!("The database file was replaced while {} was running. Close the app and restore the database with its matching journal files before reopening.", crate::brand::NAME)));
        }
    }
    let conn = Connection::open(path).map_err(|e| sqlite_err("open database", e))?;
    ensure_file_permissions(path)?;
    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))
        .map_err(|e| sqlite_err("set busy_timeout", e))?;
    conn.pragma_update(None, "foreign_keys", true)
        .map_err(|e| sqlite_err("set foreign_keys", e))?;
    conn.set_prepared_statement_cache_capacity(128);
    conn.pragma_update(None, "wal_autocheckpoint", 4096)
        .map_err(|e| sqlite_err("configure WAL checkpoint interval", e))?;
    let key = fs::canonicalize(path)?;
    if let std::collections::hash_map::Entry::Vacant(slot) = initialized.entry(key) {
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(|e| sqlite_err("enable WAL", e))?;
        if mode != "wal" {
            return Err(DbError::Message(
                "The database must be on a local filesystem that supports SQLite WAL.".into(),
            ));
        }
        init_schema(&conn)?;
        let keepalive = Connection::open(path).map_err(|e| sqlite_err("keep WAL open", e))?;
        // Opening is lazy: read the schema so this connection actually attaches
        // to WAL before the initialization connection goes away.
        keepalive
            .query_row("PRAGMA schema_version", [], |row| row.get::<_, i64>(0))
            .map_err(|e| sqlite_err("attach idle WAL connection", e))?;
        slot.insert((database_identity(path)?, keepalive));
    }
    drop(initialized);

    // Harden the file itself (0600) right after open.
    if path.exists() {
        ensure_file_permissions(path)?;
    }
    Ok(conn)
}

// Schema

/// Core tables + indexes + FTS + triggers.
const BASE_SCHEMA: &str = r#"
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
"#;

/// Analytics schema.
const ANALYTICS_SCHEMA: &str = r#"
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
"#;

/// Pricing schema. (`source_url` default is `PRICING_SOURCE_URL`.)
const PRICING_SCHEMA: &str = r#"
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
"#;

/// Reimporting a session upserts its `session` row, so cached analytics for
/// that session must go away until they are recalculated.
const ANALYTICS_INVALIDATE: &str = r#"
CREATE TRIGGER IF NOT EXISTS session_analytics_invalidate AFTER UPDATE ON session BEGIN
  DELETE FROM session_analytics WHERE session_id = new.id;
END;
"#;

/// Create every table/index/trigger.
///
/// Safe to call on every boot (all DDL uses `IF NOT EXISTS`; columns are only
/// ever added). No destructive replacement of saved data is performed here.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(BASE_SCHEMA)
        .map_err(|e| sqlite_err("init base schema", e))?;
    conn.execute_batch(ANALYTICS_SCHEMA)
        .map_err(|e| sqlite_err("init analytics schema", e))?;
    conn.execute_batch(PRICING_SCHEMA)
        .map_err(|e| sqlite_err("init pricing schema", e))?;
    conn.execute_batch(ANALYTICS_INVALIDATE)
        .map_err(|e| sqlite_err("init analytics invalidate trigger", e))?;
    add_session_host(conn)?;
    crate::library_revision::install(conn).map_err(|e| sqlite_err("track library revisions", e))?;
    Ok(())
}

/// Additive column: SSH host a session was mirrored from (NULL = this machine).
fn add_session_host(conn: &Connection) -> Result<()> {
    let present: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session') WHERE name='host')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| sqlite_err("inspect session columns", e))?;
    if !present {
        conn.execute_batch("ALTER TABLE session ADD COLUMN host TEXT")
            .map_err(|e| sqlite_err("add session host", e))?;
    }
    conn.execute_batch("CREATE INDEX IF NOT EXISTS session_host_idx ON session(host)")
        .map_err(|e| sqlite_err("index session host", e))
}

// Upserts

/// Insert-or-update an agent row, returning its id.
///
/// `ON CONFLICT(family, install_root) DO UPDATE SET product=excluded.product`.
/// `None` install root becomes `""`.
pub fn get_or_create_agent(
    conn: &Connection,
    family: &str,
    product: &str,
    install_root: Option<&str>,
) -> Result<i64> {
    let root = install_root.unwrap_or("");
    conn.execute(
        "INSERT INTO agent(family, product, install_root) VALUES (?1, ?2, ?3)
         ON CONFLICT(family, install_root) DO UPDATE SET product=excluded.product",
        params![family, product, root],
    )
    .map_err(|e| sqlite_err("upsert agent", e))?;
    conn.query_row(
        "SELECT id FROM agent WHERE family=?1 AND install_root=?2",
        params![family, root],
        |row| row.get(0),
    )
    .map_err(|e| sqlite_err("read agent id", e))
}

/// Insert-or-ignore a workspace row, returning its id (`None` for empty cwd).
///
/// `display_name` is the path basename (fallback cwd),
/// `ON CONFLICT(path) DO NOTHING`.
pub fn get_or_create_workspace(conn: &Connection, cwd: &str) -> Result<Option<i64>> {
    if cwd.is_empty() {
        return Ok(None);
    }
    let display = Path::new(cwd)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(cwd);
    conn.execute(
        "INSERT INTO workspace(path, display_name) VALUES (?1, ?2)
         ON CONFLICT(path) DO NOTHING",
        params![cwd, display],
    )
    .map_err(|e| sqlite_err("upsert workspace", e))?;
    conn.query_row(
        "SELECT id FROM workspace WHERE path=?1",
        params![cwd],
        |row| row.get::<_, i64>(0),
    )
    .map_err(|e| sqlite_err("read workspace id", e))
    .map(Some)
}

/// Insert-or-ignore a model row, returning its id (`None` for empty key).
///
/// `ON CONFLICT(provider, model_key) DO NOTHING`; `display_name` defaults
/// to the key.
pub fn get_or_create_model(
    conn: &Connection,
    provider: &str,
    model_key: &str,
) -> Result<Option<i64>> {
    if model_key.is_empty() {
        return Ok(None);
    }
    // Most records repeat the same model. Avoid preparing an upsert for every
    // record and only write when the model is first encountered.
    if let Some(id) = conn
        .prepare_cached("SELECT id FROM model WHERE provider=?1 AND model_key=?2")
        .map_err(|e| sqlite_err("prepare model lookup", e))?
        .query_row(params![provider, model_key], |row| row.get::<_, i64>(0))
        .optional()
        .map_err(|e| sqlite_err("read model id", e))?
    {
        return Ok(Some(id));
    }
    conn.execute(
        "INSERT INTO model(provider, model_key, display_name) VALUES (?1, ?2, ?3)
         ON CONFLICT(provider, model_key) DO NOTHING",
        params![provider, model_key, model_key],
    )
    .map_err(|e| sqlite_err("upsert model", e))?;
    conn.query_row(
        "SELECT id FROM model WHERE provider=?1 AND model_key=?2",
        params![provider, model_key],
        |row| row.get::<_, i64>(0),
    )
    .map_err(|e| sqlite_err("read model id", e))
    .map(Some)
}
