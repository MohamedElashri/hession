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
];

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
        let selected =
            serde_json::to_string(&ids).map_err(|e| ServiceError::Query(e.to_string()))?;
        // Budget untrusted bytes and row overhead before decoding any payload.
        let mut bytes = 0_i64;
        for (table, predicate) in [
            (
                "entry",
                "id IN (SELECT value FROM json_each(?1)) AND session_id=?2",
            ),
            (
                "content_part",
                "entry_id IN (SELECT value FROM json_each(?1))",
            ),
            (
                "tool_invocation",
                "entry_id IN (SELECT value FROM json_each(?1)) AND session_id=?2",
            ),
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
                .map(|c| {
                    format!(
                        "COALESCE(length(CAST(\"{}\" AS BLOB)),0)",
                        c.replace('"', "\"\"")
                    )
                })
                .collect::<Vec<_>>()
                .join("+");
            // Always bind both parameters even for the attachment-only table.
            let sql = format!(
                "SELECT COALESCE(SUM(256+{sizes}),0) FROM {table} WHERE ({predicate}) AND ?2=?2"
            );
            bytes = bytes.saturating_add(
                tx.query_row(&sql, params![selected, session_id], |r| r.get::<_, i64>(0))
                    .map_err(query)?,
            );
            if bytes > MAX_RESPONSE_BYTES as i64 {
                return Err(ServiceError::ResponseTooLarge);
            }
        }
        let entries=query_all(&tx,"SELECT * FROM entry WHERE session_id=? AND id IN (SELECT value FROM json_each(?)) ORDER BY ordinal",&[SqlValue::Integer(session_id),SqlValue::Text(selected)]).map_err(ServiceError::Query)?;
        let entries =
            crate::sessions::attach_entry_details(&tx, entries).map_err(ServiceError::Query)?;
        let exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session WHERE id=?)",
                [session_id],
                |r| r.get(0),
            )
            .map_err(query)?;
        let result = json!({"revision":revision,"cursor":DeltaCursor{identity:revision.identity.clone(),epoch,seq:next,session_id},"reset":reset,"deleted":!exists,"entries":entries,"hasMore":more});
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
