//! Bounded read services. Adapter code decides whether an explicit write or
//! migration is authorized; reads here always open an existing library.
use crate::{db, library_revision, sessions};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub const MAX_PAGE_SIZE: u32 = 500;
pub const MAX_OFFSET: u64 = 1_000_000;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("{0}")]
    Database(#[from] db::DbError),
    #[error("{0}")]
    Query(String),
    #[error("Invalid pagination: use a limit of 1–500 and offset at most 1000000")]
    InvalidPage,
    #[error("Response exceeds 1 MiB; request a smaller page")]
    ResponseTooLarge,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    pub limit: u32,
    pub offset: u64,
}

impl Default for Page {
    fn default() -> Self {
        Self {
            limit: 20,
            offset: 0,
        }
    }
}

impl Page {
    pub fn validate(self) -> Result<Self, ServiceError> {
        if self.limit == 0 || self.limit > MAX_PAGE_SIZE || self.offset > MAX_OFFSET {
            return Err(ServiceError::InvalidPage);
        }
        Ok(self)
    }
}

#[derive(Clone, Debug)]
pub struct Library {
    path: PathBuf,
}

impl Library {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Metadata, transcript and revision come from a single WAL snapshot.
    pub fn session(&self, id: i64, page: Page) -> Result<Value, ServiceError> {
        let page = page.validate()?;
        let mut conn = db::open_read_only(&self.path)?;
        let tx = conn
            .transaction()
            .map_err(|e| ServiceError::Query(e.to_string()))?;
        check_page_budget(&tx, id, page)?;
        let mut offset = page.offset as i64;
        let mut result =
            sessions::get_session_from_conn(&tx, id, i64::from(page.limit), &mut offset)
                .map_err(ServiceError::Query)?;
        result["revision"] = serde_json::to_value(
            library_revision::current(&tx).map_err(|e| ServiceError::Query(e.to_string()))?,
        )
        .map_err(|e| ServiceError::Query(e.to_string()))?;
        let size = serde_json::to_vec(&result)
            .map_err(|e| ServiceError::Query(e.to_string()))?
            .len();
        if size > MAX_RESPONSE_BYTES {
            return Err(ServiceError::ResponseTooLarge);
        }
        Ok(result)
    }
}

// Count byte lengths in SQLite before materializing untrusted transcript
// payloads. Include attachment row overhead as well as text. The final encoded
// size check additionally covers JSON escaping and response metadata.
fn check_page_budget(conn: &Connection, id: i64, page: Page) -> Result<(), ServiceError> {
    let total: i64 = conn
        .query_row("SELECT count(*) FROM entry WHERE session_id=?", [id], |r| {
            r.get(0)
        })
        .map_err(|e| ServiceError::Query(e.to_string()))?;
    let limit = i64::from(page.limit);
    let offset = if total > 0 && page.offset >= total as u64 {
        ((total - 1) / limit) * limit
    } else {
        page.offset as i64
    };
    let mut bytes = 0_i64;
    // Tables and predicates are application constants; column names come from
    // SQLite and are quoted defensively. No transcript/query text becomes SQL.
    for (table, predicate) in [
        ("session", "id=?1"),
        ("agent", "id IN (SELECT agent_id FROM session WHERE id=?1)"),
        ("workspace", "id IN (SELECT workspace_id FROM session WHERE id=?1)"),
        ("model", "id=(SELECT c.model_id FROM session_context c JOIN model m ON m.id=c.model_id WHERE c.session_id=?1 ORDER BY c.id DESC LIMIT 1)"),
        ("entry", "id IN selected"),
        ("content_part", "entry_id IN selected"),
        ("tool_invocation", "session_id=?1 AND entry_id IN selected"),
        ("session_tag", "session_id=?1"),
        ("session_context", "session_id=?1"),
    ] {
        // Only session_context branch/model fields are returned by this path.
        // Budget all columns on the latest context(s), conservatively.
        let predicate = if table == "session_context" {
            "session_id=?1 AND (id=(SELECT max(id) FROM session_context WHERE session_id=?1) OR id=(SELECT max(id) FROM session_context WHERE session_id=?1 AND branch IS NOT NULL))"
        } else { predicate };
        let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .map_err(|e| ServiceError::Query(e.to_string()))?;
        let columns = stmt.query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| ServiceError::Query(e.to_string()))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| ServiceError::Query(e.to_string()))?;
        let sizes = columns.iter().map(|name| {
            format!("coalesce(length(CAST(\"{}\" AS BLOB)),0)", name.replace('"', "\"\""))
        }).collect::<Vec<_>>().join("+");
        let sql = format!("WITH selected AS (SELECT id FROM entry WHERE session_id=?1 ORDER BY ordinal LIMIT ?2 OFFSET ?3)
            SELECT coalesce(sum(256+{sizes}),0) FROM \"{table}\" WHERE {predicate}");
        let count: i64 = conn.query_row(&sql, params![id, limit, offset], |r| r.get(0))
            .map_err(|e| ServiceError::Query(e.to_string()))?;
        bytes = bytes.saturating_add(count);
        if bytes > MAX_RESPONSE_BYTES as i64 { return Err(ServiceError::ResponseTooLarge); }
    }
    Ok(())
}
