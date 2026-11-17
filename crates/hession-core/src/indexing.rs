//! Local, regenerable indexing state. These tables are excluded from portable
//! history and encrypted sync; only normalized history participates in revisions.
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub const LOCAL_TABLES: &[&str] = &[
    "index_checkpoint",
    "index_usage",
    "index_result",
    "index_change",
    "index_exclusion",
    "index_maintenance",
    "index_full_source",
    "index_source_session",
    "index_source_generation",
];

pub(crate) fn migrate_full_sources(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE INDEX index_checkpoint_session ON index_checkpoint(session_id);
      CREATE TABLE index_full_source(source_uri TEXT PRIMARY KEY,capability TEXT NOT NULL,fingerprint_json TEXT NOT NULL CHECK(length(fingerprint_json)<=16384),invalidated INTEGER NOT NULL DEFAULT 0);
      CREATE TABLE index_source_generation(source_hash TEXT PRIMARY KEY,generation INTEGER NOT NULL DEFAULT 0 CHECK(generation>=0));
      CREATE TABLE index_source_session(session_id INTEGER PRIMARY KEY REFERENCES session(id) ON DELETE CASCADE,source_uri TEXT NOT NULL,source_hash TEXT NOT NULL);
      CREATE INDEX index_source_session_source ON index_source_session(source_uri);
      CREATE TRIGGER full_source_mapping_delete AFTER DELETE ON index_source_session BEGIN UPDATE index_source_generation SET generation=generation+1 WHERE source_hash=OLD.source_hash;DELETE FROM index_full_source WHERE source_uri=OLD.source_uri AND NOT EXISTS(SELECT 1 FROM index_source_session WHERE source_uri=OLD.source_uri); END;
      CREATE TRIGGER full_source_mapping_update AFTER UPDATE OF source_uri ON index_source_session WHEN OLD.source_uri<>NEW.source_uri BEGIN UPDATE index_source_generation SET generation=generation+1 WHERE source_hash=OLD.source_hash;DELETE FROM index_full_source WHERE source_uri=OLD.source_uri AND NOT EXISTS(SELECT 1 FROM index_source_session WHERE source_uri=OLD.source_uri); END;")?;
    for (table, session) in [
        ("entry", "session_id"),
        ("tool_invocation", "session_id"),
        ("session_context", "session_id"),
        ("file_activity", "session_id"),
        ("session", "id"),
    ] {
        for action in ["INSERT", "UPDATE", "DELETE"] {
            let rows = if action == "UPDATE" {
                vec!["OLD", "NEW"]
            } else if action == "DELETE" {
                vec!["OLD"]
            } else {
                vec!["NEW"]
            };
            let statements = rows.iter().map(|row| format!("UPDATE index_source_generation SET generation=generation+1 WHERE source_hash IN(SELECT source_hash FROM index_source_session WHERE session_id={row}.{session});UPDATE index_full_source SET invalidated=1 WHERE source_uri IN(SELECT source_uri FROM index_source_session WHERE session_id={row}.{session}) AND invalidated=0;")).collect::<String>();
            // Before DELETE is essential: session's cascading mapping removal
            // would otherwise hide the owning source from invalidation.
            conn.execute_batch(&format!("CREATE TRIGGER full_source_{table}_{} BEFORE {action} ON {table} BEGIN {statements} END;",action.to_lowercase()))?;
        }
    }
    for action in ["INSERT", "UPDATE", "DELETE"] {
        let rows = if action == "UPDATE" {
            vec!["OLD", "NEW"]
        } else if action == "DELETE" {
            vec!["OLD"]
        } else {
            vec!["NEW"]
        };
        let statements = rows.iter().map(|row| format!("UPDATE index_source_generation SET generation=generation+1 WHERE source_hash IN(SELECT m.source_hash FROM index_source_session m JOIN entry e ON e.session_id=m.session_id WHERE e.id={row}.entry_id);UPDATE index_full_source SET invalidated=1 WHERE source_uri IN(SELECT m.source_uri FROM index_source_session m JOIN entry e ON e.session_id=m.session_id WHERE e.id={row}.entry_id) AND invalidated=0;")).collect::<String>();
        conn.execute_batch(&format!("CREATE TRIGGER full_source_content_part_{} BEFORE {action} ON content_part BEGIN {statements} END;", action.to_lowercase()))?;
    }
    Ok(())
}

pub fn full_source(
    conn: &Connection,
    uri: &str,
    capability: &str,
) -> rusqlite::Result<Option<crate::source::SourceSnapshot>> {
    let json: Option<String> = conn.query_row("SELECT fingerprint_json FROM index_full_source WHERE source_uri=? AND capability=? AND invalidated=0", params![uri,capability], |r| r.get(0)).optional()?;
    Ok(json
        .and_then(|value| serde_json::from_str::<crate::source::SourceSnapshot>(&value).ok())
        .filter(crate::source::SourceSnapshot::valid))
}
pub fn save_full_source(
    conn: &Connection,
    uri: &str,
    capability: &str,
    snapshot: &crate::source::SourceSnapshot,
) -> rusqlite::Result<()> {
    let json = serde_json::to_string(snapshot)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    conn.execute("INSERT INTO index_full_source(source_uri,capability,fingerprint_json) VALUES(?,?,?) ON CONFLICT(source_uri) DO UPDATE SET capability=excluded.capability,fingerprint_json=excluded.fingerprint_json,invalidated=0",params![uri,capability,json])?;
    Ok(())
}
pub fn map_source_session(conn: &Connection, uri: &str, session: i64) -> rusqlite::Result<()> {
    let hash = source_hash(uri);
    let previous: Option<String> = conn
        .query_row(
            "SELECT source_hash FROM index_source_session WHERE session_id=?",
            [session],
            |r| r.get(0),
        )
        .optional()?;
    if previous.as_deref() == Some(&hash) {
        return Ok(());
    }
    ensure_source_generation(conn, uri)?;
    conn.execute(
        "UPDATE index_source_generation SET generation=generation+1 WHERE source_hash=?",
        [&hash],
    )?;
    conn.execute("INSERT INTO index_source_session(session_id,source_uri,source_hash) VALUES(?,?,?) ON CONFLICT(session_id) DO UPDATE SET source_uri=excluded.source_uri,source_hash=excluded.source_hash", params![session,uri,hash])?;
    Ok(())
}

pub fn ensure_source_generation(conn: &Connection, uri: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO index_source_generation(source_hash) VALUES(?)",
        [source_hash(uri)],
    )?;
    Ok(())
}
pub fn source_generation(conn: &Connection, uri: &str) -> rusqlite::Result<i64> {
    Ok(conn
        .query_row(
            "SELECT generation FROM index_source_generation WHERE source_hash=?",
            [source_hash(uri)],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(0))
}
pub fn check_source_generation(conn: &Connection, uri: &str, expected: i64) -> Result<(), String> {
    if source_generation(conn, uri).map_err(|e| e.to_string())? != expected {
        Err("Source-derived history changed during preparation; retry indexing".into())
    } else {
        Ok(())
    }
}

pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
CREATE TABLE index_checkpoint(
 source_uri TEXT PRIMARY KEY, session_id INTEGER NOT NULL REFERENCES session(id) ON DELETE CASCADE,
 cursor_json TEXT NOT NULL, summary_json TEXT NOT NULL, invalidated INTEGER NOT NULL DEFAULT 0,
 committed_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE index_usage(
 session_id INTEGER NOT NULL REFERENCES session(id) ON DELETE CASCADE,
 message_key TEXT NOT NULL, input INTEGER, output INTEGER, cached INTEGER,
 PRIMARY KEY(session_id,message_key));
CREATE TABLE index_result(
 session_id INTEGER NOT NULL REFERENCES session(id) ON DELETE CASCADE,
 call_id TEXT NOT NULL, result_json TEXT NOT NULL, occurred_at TEXT,
 PRIMARY KEY(session_id,call_id));
CREATE TABLE index_change(seq INTEGER PRIMARY KEY AUTOINCREMENT,
 session_id INTEGER NOT NULL, entry_id INTEGER, reset INTEGER NOT NULL DEFAULT 0);
CREATE TABLE index_exclusion(source_hash TEXT PRIMARY KEY);
CREATE TABLE index_maintenance(id INTEGER PRIMARY KEY CHECK(id=1),epoch INTEGER NOT NULL);
INSERT INTO index_maintenance VALUES(1,0);
CREATE TRIGGER index_change_bound AFTER INSERT ON index_change BEGIN
 DELETE FROM index_change WHERE seq<=NEW.seq-10000;
END;
"#,
    )?;
    for (table, session, entry) in [
        ("entry", "session_id", "id"),
        ("tool_invocation", "session_id", "entry_id"),
    ] {
        for action in ["INSERT", "UPDATE", "DELETE"] {
            let row = if action == "DELETE" { "OLD" } else { "NEW" };
            let reset = i32::from(action == "DELETE");
            conn.execute_batch(&format!(
                "CREATE TRIGGER index_{table}_{} AFTER {action} ON {table} BEGIN
                 UPDATE index_checkpoint SET invalidated=1 WHERE session_id={row}.{session} AND invalidated=0;
                 INSERT INTO index_change(session_id,entry_id,reset) VALUES({row}.{session},{row}.{entry},{reset}); END;",
                action.to_lowercase()
            ))?;
        }
    }
    conn.execute_batch("CREATE TRIGGER index_session_delete AFTER DELETE ON session BEGIN INSERT INTO index_change(session_id,reset) VALUES(OLD.id,1); END;")
}

pub fn epoch(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT epoch FROM index_maintenance WHERE id=1", [], |r| {
        r.get(0)
    })
}

/// Call inside the same transaction that restores/deletes history, after all
/// previews/confirmation. Already prepared imports must not replay stale data.
pub fn maintenance(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute("UPDATE index_maintenance SET epoch=epoch+1 WHERE id=1", [])?;
    conn.execute("UPDATE index_checkpoint SET invalidated=1", [])?;
    conn.execute("UPDATE index_full_source SET invalidated=1", [])?;
    Ok(())
}

pub fn check_epoch(conn: &Connection, expected: i64) -> Result<(), String> {
    if epoch(conn).map_err(|e| e.to_string())? != expected {
        Err("Saved history changed during indexing; retry the operation".into())
    } else {
        Ok(())
    }
}

fn source_hash(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.as_bytes()))
}

pub fn exclude(conn: &Connection, source: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO index_exclusion VALUES(?)",
        [source_hash(source)],
    )?;
    Ok(())
}

pub fn excluded(conn: &Connection, source: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM index_exclusion WHERE source_hash=?)",
        [source_hash(source)],
        |r| r.get(0),
    )
}

pub fn include(conn: &Connection, source: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM index_exclusion WHERE source_hash=?",
        [source_hash(source)],
    )?;
    Ok(())
}

pub fn checkpoint(
    conn: &Connection,
    source: &str,
) -> rusqlite::Result<Option<(i64, String, String)>> {
    conn.query_row("SELECT session_id,cursor_json,summary_json FROM index_checkpoint WHERE source_uri=? AND invalidated=0", [source], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()
}

pub fn save_checkpoint(
    conn: &Connection,
    source: &str,
    session: i64,
    cursor: &str,
    summary: &str,
) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO index_checkpoint(source_uri,session_id,cursor_json,summary_json) VALUES(?1,?2,?3,?4) ON CONFLICT(source_uri) DO UPDATE SET session_id=?2,cursor_json=?3,summary_json=?4,invalidated=0,committed_at=CURRENT_TIMESTAMP",params![source,session,cursor,summary])?;
    Ok(())
}

/// A backup must not carry private native paths or transient result caches in
/// derived state. Keep empty tables so application_schema remains truthful.
pub fn clear_local_state(conn: &Connection) -> rusqlite::Result<()> {
    for table in LOCAL_TABLES {
        if *table != "index_maintenance" {
            conn.execute(&format!("DELETE FROM {table}"), [])?;
        }
    }
    maintenance(conn)
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeltaCursor {
    pub identity: String,
    pub epoch: i64,
    pub seq: i64,
    pub session_id: i64,
}

impl crate::service::Library {
    /// Durable changes include updates to old tool calls, not only new ordinals.
    /// A reset/retention gap returns a bounded tail and explicitly replaces the
    /// caller's prior window. All reads share a WAL snapshot and SQL deadline.
    pub fn session_delta(
        &self,
        session_id: i64,
        cursor: Option<DeltaCursor>,
        limit: u32,
    ) -> Result<serde_json::Value, crate::service::ServiceError> {
        use crate::{
            service::{ServiceError, MAX_RESPONSE_BYTES},
            sql::query_all,
        };
        use rusqlite::types::Value as SqlValue;
        use serde_json::json;
        if session_id <= 0 || limit == 0 || limit > 100 {
            return Err(ServiceError::Query(
                "Use a positive session and delta limit of 1–100".into(),
            ));
        }
        let mut conn = crate::db::open_read_only(self.path())?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        conn.progress_handler(1000, Some(move || std::time::Instant::now() >= deadline))
            .map_err(|e| ServiceError::Query(e.to_string()))?;
        let tx = conn
            .transaction()
            .map_err(|e| ServiceError::Query(e.to_string()))?;
        let query = |e: rusqlite::Error| ServiceError::Query(e.to_string());
        let revision = crate::library_revision::current(&tx).map_err(query)?;
        let epoch = epoch(&tx).map_err(query)?;
        let (low, high): (i64, i64) = tx
            .query_row(
                "SELECT COALESCE(MIN(seq),0),COALESCE(MAX(seq),0) FROM index_change",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(query)?;
        let mut reset = cursor.as_ref().is_none_or(|c| {
            c.identity != revision.identity
                || c.epoch != epoch
                || c.session_id != session_id
                || c.seq < 0
                || c.seq > high
                || c.seq < low.saturating_sub(1)
        });
        let after = cursor.as_ref().map_or(high, |c| c.seq);
        let mut next = high;
        let mut more = false;
        let mut ids = Vec::new();
        if !reset {
            let changes=query_all(&tx,"SELECT seq,entry_id,reset FROM index_change WHERE session_id=? AND seq>? ORDER BY seq LIMIT ?",&[SqlValue::Integer(session_id),SqlValue::Integer(after),SqlValue::Integer(i64::from(limit)+1)]).map_err(ServiceError::Query)?;
            more = changes.len() > limit as usize;
            for change in changes.iter().take(limit as usize) {
                if change["reset"].as_i64() == Some(1) {
                    reset = true;
                    break;
                }
                if let Some(id) = change["entry_id"].as_i64() {
                    ids.push(id);
                }
            }
            if more {
                next = changes[limit as usize - 1]["seq"].as_i64().unwrap_or(high);
            }
        }
        if reset {
            ids = tx
                .prepare("SELECT id FROM entry WHERE session_id=? ORDER BY ordinal DESC LIMIT ?")
                .map_err(query)?
                .query_map(params![session_id, limit], |r| r.get::<_, i64>(0))
                .map_err(query)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(query)?;
            next = high;
            more = false;
        }
        ids.sort_unstable();
        ids.dedup();
        // Size each event before decoding any payload. SQLite octet_length on
        // a column reads its stored byte count without loading the text cell.
        // Reserve room for worst-case JSON escaping plus metadata/markers.
        let payload_budget = MAX_RESPONSE_BYTES / 8;
        let mut size_queries = Vec::new();
        for (table, predicate) in [
            ("entry", "id=?1 AND session_id=?2"),
            ("content_part", "entry_id=?1"),
            ("tool_invocation", "entry_id=?1 AND session_id=?2"),
        ] {
            let columns = tx
                .prepare(&format!("PRAGMA table_info({table})"))
                .map_err(query)?
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(query)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(query)?;
            let sizes = columns
                .iter()
                .map(|c| format!("COALESCE(octet_length(\"{}\"),0)", c.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join("+");
            size_queries.push(format!(
                "SELECT COALESCE(SUM(1024+{sizes}),0) FROM {table} WHERE ({predicate}) AND ?2=?2"
            ));
        }
        let mut bytes = 0_usize;
        let mut selected_ids = Vec::new();
        let mut oversized = Vec::new();
        for id in ids {
            let mut size = 0_i64;
            for sql in &size_queries {
                size = size.saturating_add(
                    tx.query_row(sql, params![id, session_id], |r| r.get::<_, i64>(0))
                        .map_err(query)?,
                );
            }
            let size = usize::try_from(size).unwrap_or(usize::MAX);
            if size > payload_budget || size > payload_budget.saturating_sub(bytes) {
                // Numeric metadata only: even an oversized kind/role cannot
                // become an accidental large allocation in this fallback.
                let marker = tx.query_row("SELECT id,ordinal,(SELECT COUNT(*) FROM entry prior WHERE prior.session_id=e.session_id AND prior.ordinal<e.ordinal) FROM entry e WHERE id=?1 AND session_id=?2",params![id,session_id],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"ordinal":r.get::<_,i64>(1)?,"position":r.get::<_,i64>(2)?,"reason":if size>payload_budget {"payload_too_large"} else {"page_budget"}}))).optional().map_err(query)?;
                if let Some(marker) = marker {
                    oversized.push(marker);
                }
            } else {
                bytes += size;
                selected_ids.push(id);
            }
        }
        let selected =
            serde_json::to_string(&selected_ids).map_err(|e| ServiceError::Query(e.to_string()))?;
        let entries=query_all(&tx,"SELECT e.*,(SELECT COUNT(*) FROM entry prior WHERE prior.session_id=e.session_id AND prior.ordinal<e.ordinal) AS position FROM entry e WHERE e.session_id=? AND e.id IN (SELECT value FROM json_each(?)) ORDER BY e.ordinal",&[SqlValue::Integer(session_id),SqlValue::Text(selected)]).map_err(ServiceError::Query)?;
        let entries =
            crate::sessions::attach_entry_details(&tx, entries).map_err(ServiceError::Query)?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session WHERE id=?)",
                [session_id],
                |r| r.get(0),
            )
            .map_err(query)?;
        let result = json!({"revision":revision,"cursor":DeltaCursor{identity:revision.identity.clone(),epoch,seq:next,session_id},"reset":reset,"deleted":!exists,"entries":entries,"oversizedEntries":oversized,"hasMore":more});
        if serde_json::to_vec(&result)
            .map_err(|e| ServiceError::Query(e.to_string()))?
            .len()
            > MAX_RESPONSE_BYTES
        {
            return Err(ServiceError::ResponseTooLarge);
        }
        Ok(result)
    }
}
