//! Tauri commands for library search, transcripts, exports, analytics, and sync.
//! SQLite work runs in blocking helpers with a database path from `AppState`.

use rusqlite::types::{ToSql, Value as SqlValue, ValueRef};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};

use super::AppState;
use crate::{analytics, db, importer, pricing, sync};

/// Agent families accepted by `start_sync`.
const ALLOWED_FAMILIES: &[&str] = &[
    "cline",
    "codex",
    "claude",
    "opencode",
    "pi",
    "commandcode",
    "muse",
    "hermes",
    "antigravity",
];

/// Host filter value for sessions recorded on this machine.
const LOCAL_HOST: &str = "@local";

// Small helpers

/// Clone the DB path out of managed state (keeps the mutex hold tiny).
fn db_path_of(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .db_path
        .lock()
        .map(|path| path.clone())
        .map_err(|_| "App state lock was poisoned".to_string())
}

/// Open (creating + migrating as needed) or return the error as `String`.
fn open_db(db_path: &Path) -> Result<Connection, String> {
    db::open_db(db_path).map_err(|e| e.to_string())
}

fn ref_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => json!(n),
        ValueRef::Real(n) => json!(n),
        ValueRef::Text(text) => json!(String::from_utf8_lossy(text).into_owned()),
        ValueRef::Blob(blob) => json!(String::from_utf8_lossy(blob).into_owned()),
    }
}

fn row_to_json(cols: &[String], row: &rusqlite::Row<'_>) -> Result<Value, rusqlite::Error> {
    let mut map = Map::new();
    for (index, name) in cols.iter().enumerate() {
        map.insert(name.clone(), ref_to_json(row.get_ref(index)?));
    }
    Ok(Value::Object(map))
}

/// `SELECT` returning every row as `serde_json::Value` objects keyed by
/// column name. Params are owned [`SqlValue`]s so callers can mix text/ints.
fn query_all(conn: &Connection, sql: &str, params: &[SqlValue]) -> Result<Vec<Value>, String> {
    let refs: Vec<&dyn ToSql> = params.iter().map(|v| v as &dyn ToSql).collect();
    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| format!("Could not prepare query: {e}"))?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mapped = stmt
        .query_map(refs.as_slice(), |row| row_to_json(&cols, row))
        .map_err(|e| format!("Query failed: {e}"))?;
    let mut out = Vec::new();
    for row in mapped {
        out.push(row.map_err(|e| format!("Could not decode row: {e}"))?);
    }
    Ok(out)
}

/// `SELECT` expecting zero or one row.
fn query_one(conn: &Connection, sql: &str, params: &[SqlValue]) -> Result<Option<Value>, String> {
    let mut rows = query_all(conn, sql, params)?;
    Ok(rows.pop())
}

/// LIKE wildcards must be escaped; SQLite has no default escape char, so
/// every LIKE pattern uses `ESCAPE '\'` together with this helper.
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Simplified FTS5 `MATCH` expression: deduped lowercase word prefixes joined
/// with `OR`.
fn fts_expression(query: &str) -> Option<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in query.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        let word = word.to_lowercase();
        if word.is_empty() || terms.contains(&word) {
            continue;
        }
        terms.push(word);
        if terms.len() >= 50 {
            break;
        }
    }
    if terms.is_empty() {
        return None;
    }
    Some(
        terms
            .iter()
            .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

fn parse_i64(value: &str, default: i64) -> i64 {
    value.trim().parse::<i64>().unwrap_or(default)
}

fn clamp_limit(value: Option<&str>, default: i64, min: i64, max: i64) -> i64 {
    let parsed = value.map(|v| parse_i64(v, default)).unwrap_or(default);
    parsed.clamp(min, max)
}

/// `since` filter for `search_sessions`: `day/week/month/year` presets or an
/// ISO date. Returns the UTC cutoff ISO string, or `None` for no filter.
fn sessions_since_cutoff(since: &str) -> Option<String> {
    let trimmed = since.trim();
    if trimmed.is_empty() {
        return None;
    }
    let days: Option<i64> = match trimmed {
        "day" => Some(1),
        "week" => Some(7),
        "month" => Some(30),
        "year" => Some(365),
        _ => None,
    };
    if let Some(days) = days {
        let cutoff = chrono::Utc::now() - chrono::TimeDelta::milliseconds(days * 86_400_000);
        return Some(cutoff.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    }
    let parsed = chrono::DateTime::parse_from_rfc3339(trimmed)
        .ok()
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0).map(|t| t.and_utc()))
        });
    parsed.map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}

fn opt_str(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("")
}

// ping (pre-existing liveness probe)

/// Liveness probe. Always available, even before sync has ever run.
#[tauri::command]
pub async fn ping(_state: State<'_, AppState>) -> Result<String, String> {
    Ok("ok".to_string())
}

// get_health

fn get_health_inner(db_path: &Path) -> Result<Value, String> {
    let conn = open_db(db_path)?;
    let row = query_one(&conn, "SELECT COUNT(*) n FROM session", &[])?.unwrap_or(json!({"n": 0}));
    let sessions = row.get("n").and_then(|v| v.as_i64()).unwrap_or(0);
    Ok(json!({
        "ok": true,
        "sessions": sessions,
        "dbPath": db_path.to_string_lossy(),
    }))
}

/// `{ ok, sessions, dbPath }`.
#[tauri::command]
pub async fn get_health(state: State<'_, AppState>) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || get_health_inner(&db_path))
        .await
        .map_err(|e| format!("get_health failed: {e}"))?
}

// search_sessions

fn search_sessions_inner(
    db_path: &Path,
    filters: &SessionFilters,
    limit: i64,
    offset: i64,
) -> Result<Value, String> {
    let mut conn = open_db(db_path)?;
    let snapshot = conn.transaction().map_err(|e| e.to_string())?;
    let result = search_sessions_from_conn(&snapshot, filters, limit, offset, None)?;
    snapshot.commit().map_err(|e| e.to_string())?;
    Ok(result)
}

struct SessionQuery {
    base: String,
    params: Vec<SqlValue>,
    order_by: &'static str,
}

fn session_query(f: &SessionFilters) -> SessionQuery {
    let (q, agent, project, branch, since, sort, host) = (
        f.q.as_str(),
        f.agent.as_str(),
        f.project.as_str(),
        f.branch.as_str(),
        f.since.as_str(),
        f.sort.as_str(),
        f.host.as_str(),
    );
    let (models, tools, tags, starred) = (&f.models, &f.tools, &f.tags, f.starred);
    // ORDER BY uses the plain COALESCE expression so SQLite can use
    // session_activity_idx. Whitelisted — never interpolated from raw input.
    let order_by = match sort {
        "oldest" => "COALESCE(s.last_event_at,s.started_at) ASC, s.id ASC",
        "longest" => "duration_days DESC, COALESCE(s.last_event_at,s.started_at) DESC, s.id DESC",
        "messages" => "entry_count DESC, COALESCE(s.last_event_at,s.started_at) DESC, s.id DESC",
        _ => "COALESCE(s.last_event_at,s.started_at) DESC, s.id DESC",
    };

    let mut filters: Vec<String> = Vec::new();
    let mut params: Vec<SqlValue> = Vec::new();

    if starred {
        filters.push(
            "EXISTS (SELECT 1 FROM session_annotation n WHERE n.session_id=s.id AND n.starred=1)"
                .to_string(),
        );
    }
    for tag in tags {
        filters.push(
            "EXISTS (SELECT 1 FROM session_tag t WHERE t.session_id=s.id AND t.label=?)"
                .to_string(),
        );
        params.push(SqlValue::Text(tag.clone()));
    }
    // `@local` selects this machine's sessions; any other value is an SSH host.
    if host == LOCAL_HOST {
        filters.push("s.host IS NULL".to_string());
    } else if !host.is_empty() {
        filters.push("s.host = ?".to_string());
        params.push(SqlValue::Text(host.to_string()));
    }
    if !agent.is_empty() {
        filters.push("a.family = ?".to_string());
        params.push(SqlValue::Text(agent.to_string()));
    }
    if !project.is_empty() {
        filters.push("w.path = ?".to_string());
        params.push(SqlValue::Text(project.to_string()));
    }
    if !models.is_empty() {
        let placeholders = models.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        filters.push(format!(
            "EXISTS (SELECT 1 FROM session_context c JOIN model m ON m.id=c.model_id \
             WHERE c.session_id=s.id AND m.model_key IN ({placeholders}))"
        ));
        params.extend(models.iter().cloned().map(SqlValue::Text));
    }
    if !branch.is_empty() {
        filters.push(
            "EXISTS (SELECT 1 FROM session_context c WHERE c.session_id=s.id \
             AND c.branch LIKE ? ESCAPE '\\')"
                .to_string(),
        );
        params.push(SqlValue::Text(format!("%{}%", escape_like(branch))));
    }
    if !tools.is_empty() {
        let placeholders = tools.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        filters.push(format!(
            "EXISTS (SELECT 1 FROM tool_invocation t WHERE t.session_id=s.id \
             AND t.tool_name IN ({placeholders}))"
        ));
        params.extend(tools.iter().cloned().map(SqlValue::Text));
    }
    if let Some(cutoff) = sessions_since_cutoff(since) {
        // julianday() kept for correctness: plain string >= misorders
        // millisecond boundaries when stored/param formats mix.
        filters
            .push("julianday(COALESCE(s.last_event_at,s.started_at)) >= julianday(?)".to_string());
        params.push(SqlValue::Text(cutoff));
    }

    // Full-text search join (no LIKE fallback over entry.search_text).
    let mut search_join = String::new();
    if !q.trim().is_empty() {
        if let Some(expression) = fts_expression(q) {
            search_join = "JOIN (SELECT e.session_id, MIN(e.ordinal) AS rank FROM entry_fts \
                 JOIN entry e ON e.id=entry_fts.rowid WHERE entry_fts MATCH ? \
                 GROUP BY e.session_id) hits ON hits.session_id=s.id"
                .to_string();
            params.insert(0, SqlValue::Text(expression));
        } else {
            filters.push("(s.title LIKE ? ESCAPE '\\' OR s.cwd LIKE ? ESCAPE '\\')".to_string());
            let lit = format!("%{}%", escape_like(q));
            params.push(SqlValue::Text(lit.clone()));
            params.push(SqlValue::Text(lit));
        }
    }

    let where_clause = if filters.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", filters.join(" AND "))
    };
    let base = format!(
        "FROM session s JOIN agent a ON a.id=s.agent_id \
         LEFT JOIN workspace w ON w.id=s.workspace_id {search_join} {where_clause}"
    );
    SessionQuery {
        base,
        params,
        order_by,
    }
}

fn search_sessions_from_conn(
    conn: &Connection,
    f: &SessionFilters,
    limit: i64,
    offset: i64,
    job: Option<&ExportJob>,
) -> Result<Value, String> {
    let SessionQuery {
        base,
        params,
        order_by,
    } = session_query(f);
    let columns = "s.id,s.title,s.external_id,s.cwd,s.host,s.started_at,s.last_event_at,\
        a.family AS agent,a.product,w.path AS project,\
        (SELECT branch FROM session_context c WHERE c.session_id=s.id AND c.branch IS NOT NULL \
         ORDER BY c.id DESC LIMIT 1) AS branch,\
        (SELECT model_key FROM session_context c JOIN model m ON m.id=c.model_id \
         WHERE c.session_id=s.id ORDER BY c.id DESC LIMIT 1) AS model,\
        (SELECT substr(search_text,1,600) FROM entry e WHERE e.session_id=s.id \
         AND e.role IN ('user','assistant') AND search_text<>'' \
         ORDER BY e.ordinal DESC LIMIT 1) AS preview,\
        (SELECT COUNT(*) FROM entry e WHERE e.session_id=s.id) AS entry_count,\
        (julianday(COALESCE(s.last_event_at,s.started_at))-julianday(s.started_at)) AS duration_days";

    let mut list_params = params.clone();
    list_params.push(SqlValue::Integer(limit));
    list_params.push(SqlValue::Integer(offset));
    let mut sessions = query_all(
        conn,
        &format!("SELECT {columns} {base} ORDER BY {order_by} LIMIT ? OFFSET ?"),
        &list_params,
    )?;
    attach_annotations_batch(conn, &mut sessions, job)?;
    let total = query_one(conn, &format!("SELECT COUNT(*) n {base}"), &params)?
        .and_then(|row| row.get("n").and_then(|v| v.as_i64()))
        .unwrap_or(0);
    Ok(json!({ "sessions": sessions, "total": total }))
}

/// Flat args `{ q, agent, project, model, branch, tool, since, sort, limit,
/// offset }` where `model`/`tool` are comma-joined strings and
/// `limit`/`offset` arrive as strings from the frontend.
// Each parameter is a field of the frontend's IPC call.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn search_sessions(
    state: State<'_, AppState>,
    q: Option<String>,
    agent: Option<String>,
    project: Option<String>,
    model: Option<String>,
    branch: Option<String>,
    tool: Option<String>,
    since: Option<String>,
    sort: Option<String>,
    limit: Option<String>,
    offset: Option<String>,
    starred: Option<String>,
    tags: Option<Vec<String>>,
    host: Option<String>,
) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    let list = |value: &Option<String>| {
        opt_str(value)
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
    };
    let filters = SessionFilters {
        q: opt_str(&q).to_string(),
        agent: opt_str(&agent).to_string(),
        project: opt_str(&project).to_string(),
        models: list(&model),
        branch: opt_str(&branch).to_string(),
        tools: list(&tool),
        since: opt_str(&since).to_string(),
        sort: opt_str(&sort).to_string(),
        starred: opt_str(&starred) == "true",
        tags: tags.unwrap_or_default(),
        host: opt_str(&host).to_string(),
    };
    let (limit, offset) = (
        clamp_limit(limit.as_deref(), 20, 1, 100),
        parse_i64(opt_str(&offset), 0).max(0),
    );
    tauri::async_runtime::spawn_blocking(move || {
        search_sessions_inner(&db_path, &filters, limit, offset)
    })
    .await
    .map_err(|e| format!("search_sessions failed: {e}"))?
}

// get_session  (paginated transcript)

fn get_session_inner(db_path: &Path, id: &str, limit: i64, page: i64) -> Result<Value, String> {
    let session_id: i64 = id
        .trim()
        .parse()
        .map_err(|_| "Session not found".to_string())?;
    let mut offset = (page.max(1) - 1) * limit;
    let mut conn = open_db(db_path)?;
    let snapshot = conn.transaction().map_err(|e| e.to_string())?;
    let result = get_session_from_conn(&snapshot, session_id, limit, &mut offset)?;
    snapshot.commit().map_err(|e| e.to_string())?;
    Ok(result)
}

fn get_session_metadata(conn: &Connection, session_id: i64) -> Result<Value, String> {
    let mut session = query_one(
        conn,
        "SELECT s.*,a.family AS agent,a.product,w.path AS project,\
             (SELECT branch FROM session_context c WHERE c.session_id=s.id \
              AND c.branch IS NOT NULL ORDER BY c.id DESC LIMIT 1) AS branch,\
             (SELECT model_key FROM session_context c JOIN model m ON m.id=c.model_id \
              WHERE c.session_id=s.id ORDER BY c.id DESC LIMIT 1) AS model \
             FROM session s JOIN agent a ON a.id=s.agent_id \
             LEFT JOIN workspace w ON w.id=s.workspace_id \
             WHERE s.id=?",
        &[SqlValue::Integer(session_id)],
    )?
    .ok_or_else(|| "Session not found".to_string())?;

    attach_annotations(conn, &mut session)?;
    Ok(session)
}

fn get_session_from_conn(
    conn: &Connection,
    session_id: i64,
    limit: i64,
    offset: &mut i64,
) -> Result<Value, String> {
    let session = get_session_metadata(conn, session_id)?;
    let entry_counts = query_one(
        conn,
        "WITH classified AS (
           SELECT e.role, CASE WHEN e.role='tool'
             OR EXISTS (SELECT 1 FROM tool_invocation t WHERE t.entry_id=e.id)
             OR EXISTS (SELECT 1 FROM content_part cp WHERE cp.entry_id=e.id
                        AND cp.kind IN ('tool_call','tool_result'))
             THEN 1 ELSE 0 END AS has_tool
           FROM entry e WHERE e.session_id=?1
         ) SELECT COUNT(*) AS \"all\",
           COALESCE(SUM(CASE WHEN role='user' THEN 1 ELSE 0 END),0) AS user,
           COALESCE(SUM(CASE WHEN role='assistant' THEN 1 ELSE 0 END),0) AS assistant,
           COALESCE(SUM(has_tool),0) AS tool,
           COALESCE(SUM(CASE WHEN role IS NOT 'user' AND role IS NOT 'assistant'
                        AND has_tool=0 THEN 1 ELSE 0 END),0) AS event
           FROM classified",
        &[SqlValue::Integer(session_id)],
    )?
    .unwrap_or(json!({"all": 0, "user": 0, "assistant": 0, "tool": 0, "event": 0}));
    let total = entry_counts
        .get("all")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if *offset >= total && total > 0 {
        *offset = ((total + limit - 1) / limit - 1).max(0) * limit;
    }

    let entries = get_entries_from_conn(conn, session_id, limit, *offset)?;

    Ok(json!({
        "session": session,
        "entries": entries,
        "total": total,
        "entryCounts": entry_counts,
        "limit": limit,
        "offset": offset,
    }))
}

/// Load only the requested transcript page, using its entry IDs for attachments.
fn get_entries_from_conn(
    conn: &Connection,
    session_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<Value>, String> {
    let entries = query_all(
        conn,
        "SELECT * FROM entry WHERE session_id=? ORDER BY ordinal LIMIT ? OFFSET ?",
        &[
            SqlValue::Integer(session_id),
            SqlValue::Integer(limit),
            SqlValue::Integer(offset),
        ],
    )?;
    attach_entry_details(conn, entries)
}

fn get_entries_after(
    conn: &Connection,
    session_id: i64,
    after: Option<i64>,
) -> Result<Vec<Value>, String> {
    let entries = if let Some(ordinal) = after {
        query_all(
            conn,
            "SELECT * FROM entry WHERE session_id=? AND ordinal>? ORDER BY ordinal LIMIT 500",
            &[SqlValue::Integer(session_id), SqlValue::Integer(ordinal)],
        )?
    } else {
        query_all(
            conn,
            "SELECT * FROM entry WHERE session_id=? ORDER BY ordinal LIMIT 500",
            &[SqlValue::Integer(session_id)],
        )?
    };
    attach_entry_details(conn, entries)
}

fn attach_entry_details(conn: &Connection, entries: Vec<Value>) -> Result<Vec<Value>, String> {
    let entry_ids: Vec<i64> = entries
        .iter()
        .filter_map(|e| e.get("id").and_then(|v| v.as_i64()))
        .collect();

    let (mut parts_by_entry, mut tools_by_entry): (
        HashMap<i64, Vec<Value>>,
        HashMap<i64, Vec<Value>>,
    ) = (HashMap::new(), HashMap::new());
    if !entry_ids.is_empty() {
        let placeholders = entry_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let params: Vec<SqlValue> = entry_ids.into_iter().map(SqlValue::Integer).collect();
        for part in query_all(
            conn,
            &format!(
                "SELECT cp.*, e.ordinal AS e_ord FROM content_part cp \
                 JOIN entry e ON e.id=cp.entry_id \
                 WHERE cp.entry_id IN ({placeholders}) \
                 ORDER BY e_ord,cp.ordinal"
            ),
            &params,
        )? {
            if let Some(entry_id) = part.get("entry_id").and_then(|v| v.as_i64()) {
                parts_by_entry.entry(entry_id).or_default().push(part);
            }
        }
        let mut tool_params = params;
        let session_id = entries
            .first()
            .and_then(|entry| entry["session_id"].as_i64())
            .ok_or("Invalid transcript session")?;
        tool_params.insert(0, SqlValue::Integer(session_id));
        for tool in query_all(
            conn,
            &format!(
                "SELECT * FROM tool_invocation WHERE session_id=? AND entry_id IN ({placeholders}) ORDER BY id"
            ),
            &tool_params,
        )? {
            if let Some(entry_id) = tool.get("entry_id").and_then(|v| v.as_i64()) {
                tools_by_entry.entry(entry_id).or_default().push(tool);
            }
        }
    }

    let entries: Vec<Value> = entries
        .into_iter()
        .map(|mut entry| {
            let entry_id = entry.get("id").and_then(|v| v.as_i64()).unwrap_or(-1);
            entry["parts"] = Value::Array(parts_by_entry.remove(&entry_id).unwrap_or_default());
            entry["tools"] = Value::Array(tools_by_entry.remove(&entry_id).unwrap_or_default());
            entry
        })
        .collect();

    Ok(entries)
}

/// Flat args `{ id, limit, page }` (all strings from the frontend).
#[tauri::command]
pub async fn get_session(
    state: State<'_, AppState>,
    id: Option<String>,
    limit: Option<String>,
    page: Option<String>,
) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    let id = opt_str(&id).to_string();
    let limit = clamp_limit(limit.as_deref(), 100, 1, 500);
    let page = parse_i64(opt_str(&page), 1).max(1);
    tauri::async_runtime::spawn_blocking(move || get_session_inner(&db_path, &id, limit, page))
        .await
        .map_err(|e| format!("get_session failed: {e}"))?
}

// get_agents

fn get_agents_inner(db_path: &Path) -> Result<Value, String> {
    let conn = open_db(db_path)?;
    // Indexed rows (families already imported, with visible-session counts).
    let mut agents = query_all(
        &conn,
        "SELECT a.family,a.product,COUNT(s.id) AS sessions \
             FROM agent a LEFT JOIN session s ON s.agent_id=a.id \
             GROUP BY a.family ORDER BY a.product",
        &[],
    )?;
    // Union with filesystem-discovered agents: families whose sources exist on disk appear with `sessions: 0`
    // even before their first import.
    let mut seen: HashSet<String> = agents
        .iter()
        .filter_map(|row| {
            row.get("family")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .collect();
    for available in importer::get_available_agents() {
        if seen.contains(&available.family) {
            continue;
        }
        seen.insert(available.family.clone());
        agents.push(json!({
            "family": available.family,
            "product": available.product,
            "sessions": 0,
        }));
    }
    // Keep a stable product-sorted order after the union append.
    agents.sort_by(|a, b| {
        let pa = a.get("product").and_then(|v| v.as_str()).unwrap_or("");
        let pb = b.get("product").and_then(|v| v.as_str()).unwrap_or("");
        pa.cmp(pb).then_with(|| {
            let fa = a.get("family").and_then(|v| v.as_str()).unwrap_or("");
            let fb = b.get("family").and_then(|v| v.as_str()).unwrap_or("");
            fa.cmp(fb)
        })
    });
    Ok(Value::Array(agents))
}

#[tauri::command]
pub async fn get_agents(state: State<'_, AppState>) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || get_agents_inner(&db_path))
        .await
        .map_err(|e| format!("get_agents failed: {e}"))?
}

// get_projects

fn get_projects_inner(db_path: &Path) -> Result<Value, String> {
    let conn = open_db(db_path)?;
    let projects = query_all(
        &conn,
        "SELECT w.path,w.display_name,COUNT(s.id) AS sessions \
             FROM workspace w \
             LEFT JOIN session s ON s.workspace_id=w.id \
             GROUP BY w.id HAVING sessions>0 ORDER BY sessions DESC,w.path",
        &[],
    )?;
    Ok(Value::Array(projects))
}

#[tauri::command]
pub async fn get_projects(state: State<'_, AppState>) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || get_projects_inner(&db_path))
        .await
        .map_err(|e| format!("get_projects failed: {e}"))?
}

// get_facets

fn get_facets_inner(db_path: &Path) -> Result<Value, String> {
    let conn = open_db(db_path)?;
    let models = query_all(
        &conn,
        "SELECT DISTINCT m.model_key AS value,m.provider AS provider \
             FROM model m JOIN session_context c ON c.model_id=m.id \
             JOIN session s ON s.id=c.session_id \
             ORDER BY m.provider,m.model_key",
        &[],
    )?;
    let branches = query_all(
        &conn,
        "SELECT DISTINCT c.branch AS value FROM session_context c \
             JOIN session s ON s.id=c.session_id \
             WHERE c.branch IS NOT NULL AND c.branch<>'' \
             ORDER BY c.branch LIMIT 300",
        &[],
    )?
    .into_iter()
    .filter_map(|row| {
        row.get("value")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    })
    .map(Value::String)
    .collect::<Vec<_>>();
    let tools = query_all(
        &conn,
        "SELECT DISTINCT t.tool_name AS value FROM tool_invocation t \
             JOIN session s ON s.id=t.session_id \
             ORDER BY t.tool_name LIMIT 300",
        &[],
    )?
    .into_iter()
    .filter_map(|row| {
        row.get("value")
            .and_then(|v| v.as_str())
            .map(str::to_string)
    })
    .map(Value::String)
    .collect::<Vec<_>>();
    let tags = query_all(
        &conn,
        "SELECT DISTINCT label AS value FROM session_tag ORDER BY label",
        &[],
    )?
    .into_iter()
    .filter_map(|row| row["value"].as_str().map(str::to_string))
    .collect::<Vec<_>>();
    let hosts = query_all(&conn, "SELECT host AS value, COUNT(*) AS sessions FROM session WHERE host IS NOT NULL GROUP BY host ORDER BY host", &[])?;
    Ok(
        json!({ "models": models, "branches": branches, "tools": tools, "tags": tags, "hosts": hosts }),
    )
}

#[tauri::command]
pub async fn get_facets(state: State<'_, AppState>) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || get_facets_inner(&db_path))
        .await
        .map_err(|e| format!("get_facets failed: {e}"))?
}

// get_stats

fn get_stats_inner(db_path: &Path) -> Result<Value, String> {
    let conn = open_db(db_path)?;
    let count = |sql: &str| -> i64 {
        query_one(&conn, sql, &[])
            .ok()
            .flatten()
            .and_then(|row| row.get("n").and_then(|v| v.as_i64()))
            .unwrap_or(0)
    };
    Ok(json!({
        "sessions": count("SELECT COUNT(*) n FROM session"),
        "entries": count("SELECT COUNT(*) n FROM entry"),
        "agents": count("SELECT COUNT(DISTINCT agent_id) n FROM session"),
        "files": count("SELECT COUNT(DISTINCT path) n FROM file_activity"),
    }))
}

#[tauri::command]
pub async fn get_stats(state: State<'_, AppState>) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || get_stats_inner(&db_path))
        .await
        .map_err(|e| format!("get_stats failed: {e}"))?
}

// get_analytics

fn get_analytics_inner(
    db_path: &Path,
    timezone: &str,
    since: &str,
    until: &str,
    agent: &str,
    workspace: &str,
    model: &str,
) -> Result<Value, String> {
    let mut conn = open_db(db_path).map_err(sync::report_error)?;
    conn.pragma_update(None, "query_only", true)
        .map_err(|e| e.to_string())?;
    let snapshot = conn.transaction().map_err(|e| e.to_string())?;
    let analytics_state = analytics::shared_state();
    let filters = analytics::AnalyticsFilters {
        agent: agent.to_string(),
        model: model.to_string(),
        workspace: workspace.to_string(),
        timezone: if timezone.trim().is_empty() {
            "UTC".to_string()
        } else {
            timezone.trim().to_string()
        },
        since: since.trim().to_string(),
        until: until.trim().to_string(),
    };
    let mut report = analytics::get_full_analytics(&snapshot, &filters, &analytics_state)
        .map_err(sync::report_error)?;
    snapshot.commit().map_err(|e| e.to_string())?;
    let (refreshing, error) = analytics::refresh_status();
    report["refreshing"] = json!(refreshing);
    report["refreshError"] = json!(error);
    Ok(report)
}

/// Flat args `{ timezone, since, agent, workspace, model }`.
#[tauri::command]
pub async fn get_analytics(
    state: State<'_, AppState>,
    timezone: Option<String>,
    since: Option<String>,
    until: Option<String>,
    agent: Option<String>,
    workspace: Option<String>,
    model: Option<String>,
) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    let args = (
        opt_str(&timezone).to_string(),
        opt_str(&since).to_string(),
        opt_str(&agent).to_string(),
        opt_str(&workspace).to_string(),
        opt_str(&model).to_string(),
        opt_str(&until).to_string(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        get_analytics_inner(
            &db_path, &args.0, &args.1, &args.5, &args.2, &args.3, &args.4,
        )
    })
    .await
    .map_err(|e| format!("get_analytics failed: {e}"))?
}

// refresh_analytics

fn refresh_analytics_inner(db_path: &Path) -> Result<Value, String> {
    analytics::request_refresh(db_path, true);
    Ok(json!({ "ok": true, "message": "Analytics refresh scheduled" }))
}

#[tauri::command]
pub async fn refresh_analytics(state: State<'_, AppState>) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || refresh_analytics_inner(&db_path))
        .await
        .map_err(|e| format!("refresh_analytics failed: {e}"))?
}

// get_pricing_status

fn get_pricing_status_inner(db_path: &Path) -> Result<Value, String> {
    let conn = open_db(db_path)?;
    let mut pricing = pricing::PricingState::new();
    let status = pricing::get_pricing_status(&conn, &mut pricing)?;
    serde_json::to_value(&status).map_err(|e| format!("Could not encode pricing status: {e}"))
}

#[tauri::command]
pub async fn get_pricing_status(state: State<'_, AppState>) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || get_pricing_status_inner(&db_path))
        .await
        .map_err(|e| format!("get_pricing_status failed: {e}"))?
}

// sync_pricing

fn sync_pricing_inner(db_path: &Path, raw: &Value, updated_at: &str) -> Result<Value, String> {
    let _writer = db::user_write();
    let mut conn = open_db(db_path)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let mut pricing = pricing::PricingState::new();
    pricing::save_pricing_payload(&tx, &mut pricing, raw, updated_at)?;
    let shared = Arc::new(Mutex::new(pricing));
    analytics::queue_recalculation(&tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    analytics::prices_changed();
    analytics::request_refresh(db_path, false);
    let status = {
        let mut guard = shared
            .lock()
            .map_err(|_| "Pricing state lock was poisoned".to_string())?;
        pricing::get_pricing_status(&conn, &mut guard)?
    };
    let status_value =
        serde_json::to_value(&status).map_err(|e| format!("Could not encode pricing: {e}"))?;
    Ok(json!({ "ok": true, "message": "Model prices synced", "pricing": status_value }))
}

/// Explicit user sync: fetch models.dev (network), then cache + recalculate.
/// Returns an error on network/parse failures (no silent fallback).
#[tauri::command]
pub async fn sync_pricing(state: State<'_, AppState>) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    let raw = pricing::fetch_models_dev_payload().await?;
    let updated_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    tauri::async_runtime::spawn_blocking(move || sync_pricing_inner(&db_path, &raw, &updated_at))
        .await
        .map_err(|e| format!("sync_pricing failed: {e}"))?
}

// get_pricing_overrides

fn get_pricing_overrides_inner(db_path: &Path) -> Result<Value, String> {
    let conn = open_db(db_path)?;
    let mut pricing = pricing::PricingState::new();
    let overrides = pricing::list_price_overrides(&conn, &mut pricing)?;
    let value =
        serde_json::to_value(&overrides).map_err(|e| format!("Could not encode overrides: {e}"))?;
    Ok(json!({ "overrides": value }))
}

#[tauri::command]
pub async fn get_pricing_overrides(state: State<'_, AppState>) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || get_pricing_overrides_inner(&db_path))
        .await
        .map_err(|e| format!("get_pricing_overrides failed: {e}"))?
}

// save_pricing_override

#[allow(non_snake_case)]
fn save_pricing_override_inner(
    db_path: &Path,
    provider: &str,
    modelId: &str,
    displayName: &str,
    inPer1M: Option<f64>,
    outPer1M: Option<f64>,
    cachePer1M: Option<f64>,
) -> Result<Value, String> {
    let input = json!({
        "provider": provider,
        "modelId": modelId,
        "displayName": displayName,
        "inPer1M": inPer1M,
        "outPer1M": outPer1M,
        "cachePer1M": cachePer1M,
    });
    let _writer = db::user_write();
    let mut conn = open_db(db_path)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let mut pricing = pricing::PricingState::new();
    let saved = pricing::upsert_price_override(&tx, &mut pricing, &input)?;
    let shared = Arc::new(Mutex::new(pricing));
    analytics::queue_recalculation(&tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    analytics::prices_changed();
    analytics::request_refresh(db_path, false);
    let overrides = {
        let mut guard = shared
            .lock()
            .map_err(|_| "Pricing state lock was poisoned".to_string())?;
        pricing::list_price_overrides(&conn, &mut guard)?
    };
    let saved_value =
        serde_json::to_value(&saved).map_err(|e| format!("Could not encode override: {e}"))?;
    let overrides_value =
        serde_json::to_value(&overrides).map_err(|e| format!("Could not encode overrides: {e}"))?;
    Ok(json!({
        "ok": true,
        "message": "Price override saved",
        "override": saved_value,
        "overrides": overrides_value,
    }))
}

/// Flat args `{ provider, modelId, displayName, inPer1M, outPer1M,
/// cachePer1M }`. Validation failures surface as `Err("Invalid override:
/// …")`.
#[tauri::command]
#[allow(non_snake_case)]
pub async fn save_pricing_override(
    state: State<'_, AppState>,
    provider: Option<String>,
    modelId: Option<String>,
    displayName: Option<String>,
    inPer1M: Option<f64>,
    outPer1M: Option<f64>,
    cachePer1M: Option<f64>,
) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    let args = (
        opt_str(&provider).to_string(),
        opt_str(&modelId).to_string(),
        opt_str(&displayName).to_string(),
        inPer1M,
        outPer1M,
        cachePer1M,
    );
    tauri::async_runtime::spawn_blocking(move || {
        save_pricing_override_inner(&db_path, &args.0, &args.1, &args.2, args.3, args.4, args.5)
    })
    .await
    .map_err(|e| format!("save_pricing_override failed: {e}"))?
}

// delete_pricing_override

#[allow(non_snake_case)]
fn delete_pricing_override_inner(
    db_path: &Path,
    provider: &str,
    modelId: &str,
) -> Result<Value, String> {
    let scope = json!({ "provider": provider, "modelId": modelId });
    let _writer = db::user_write();
    let mut conn = open_db(db_path)?;
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    let mut pricing = pricing::PricingState::new();
    let deleted = pricing::delete_price_overrides(&tx, &mut pricing, &scope)?;
    let shared = Arc::new(Mutex::new(pricing));
    analytics::queue_recalculation(&tx)?;
    tx.commit().map_err(|e| e.to_string())?;
    analytics::prices_changed();
    analytics::request_refresh(db_path, false);
    let overrides = {
        let mut guard = shared
            .lock()
            .map_err(|_| "Pricing state lock was poisoned".to_string())?;
        pricing::list_price_overrides(&conn, &mut guard)?
    };
    let overrides_value =
        serde_json::to_value(&overrides).map_err(|e| format!("Could not encode overrides: {e}"))?;
    Ok(json!({
        "ok": true,
        "message": "Price override(s) reset",
        "deleted": deleted,
        "overrides": overrides_value,
    }))
}

/// Flat scope `{ provider?, modelId? }`: both → one row; `provider` →
/// whole provider; `modelId` → model everywhere; `{}` → all.
#[tauri::command]
#[allow(non_snake_case)]
pub async fn delete_pricing_override(
    state: State<'_, AppState>,
    provider: Option<String>,
    modelId: Option<String>,
) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    let args = (
        opt_str(&provider).to_string(),
        opt_str(&modelId).to_string(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        delete_pricing_override_inner(&db_path, &args.0, &args.1)
    })
    .await
    .map_err(|e| format!("delete_pricing_override failed: {e}"))?
}

// get_sync_status / get_sync_checkpoints / start_sync  (sync API)

/// Live status snapshot from the single-flight sync worker.
/// Shape matches what the frontend polls (`running`, camelCase progress
/// counters, `phase`, `errors`, `records`).
#[tauri::command]
pub async fn get_sync_status(_state: State<'_, AppState>) -> Result<Value, String> {
    let status = sync::get_status();
    Ok(json!({
        "running": status.running,
        "phase": status.phase,
        "filesDone": status.files_done,
        "filesTotal": status.files_total,
        "records": status.records,
        "errors": status.errors,
        "cancelled": status.phase == "cancelled",
    }))
}

/// Stop the active sync cooperatively; completed source imports are retained.
#[tauri::command]
pub fn stop_sync() -> bool {
    sync::request_stop()
}

fn get_sync_checkpoints_inner(db_path: &Path) -> Result<Value, String> {
    // Prefer the importer checkpoint helper. `load_checkpoints()` serves the
    // default DB path; for any other managed path use the same logic through
    // `get_checkpoints(&conn)` so custom `--db` locations stay correct.
    if db_path == db::db_path() {
        let rows = importer::load_checkpoints();
        if !rows.is_empty() {
            return serde_json::to_value(&rows)
                .map_err(|e| format!("Could not encode checkpoints: {e}"));
        }
    } else if let Ok(conn) = open_db(db_path) {
        if let Ok(rows) = importer::get_checkpoints(&conn) {
            if !rows.is_empty() {
                return serde_json::to_value(&rows)
                    .map_err(|e| format!("Could not encode checkpoints: {e}"));
            }
        }
    }
    // SQL fallback (same semantics as `importer::get_checkpoints`).
    let conn = open_db(db_path)?;
    let checkpoints = query_all(
        &conn,
        "SELECT a.family,
           COALESCE(sc.completed_at, MAX(ss.synced_at), MAX(s.imported_at)) AS completed_at,
           COALESCE(sc.source_cutoff, MAX(ss.synced_at), MAX(s.imported_at)) AS source_cutoff
         FROM agent a
         LEFT JOIN sync_checkpoint sc ON sc.family = a.family
         LEFT JOIN session s ON s.agent_id = a.id
         LEFT JOIN sync_state ss ON ss.source_uri = s.source_uri
         GROUP BY a.family
         ORDER BY a.family",
        &[],
    )?;
    Ok(Value::Array(checkpoints))
}

#[tauri::command]
pub async fn get_sync_checkpoints(state: State<'_, AppState>) -> Result<Value, String> {
    let db_path = db_path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || get_sync_checkpoints_inner(&db_path))
        .await
        .map_err(|e| format!("get_sync_checkpoints failed: {e}"))?
}

/// Validates `agent`/`mode`, then launches the
/// single-flight background worker. Returns `{ started: true, ...status }`
/// on a fresh launch, or `{ started: false, running: true, ...status }` when
/// a run is already active.
/// The frontend forces `running: true` on its copy and polls
/// `get_sync_status` either way.
#[tauri::command]
pub async fn start_sync(
    _state: State<'_, AppState>,
    agent: Option<String>,
    mode: Option<String>,
) -> Result<Value, String> {
    let family = opt_str(&agent).trim().to_string();
    let mode = {
        let raw = opt_str(&mode).trim();
        if raw.is_empty() {
            "incremental".to_string()
        } else {
            raw.to_string()
        }
    };
    if !family.is_empty() && !ALLOWED_FAMILIES.contains(&family.as_str()) {
        return Err("Unknown agent family.".to_string());
    }
    if mode != "incremental" && mode != "full" {
        return Err("Unknown sync mode.".to_string());
    }
    let filter = if family.is_empty() {
        None
    } else {
        Some(family.clone())
    };
    let started = sync::start_sync(filter, &mode);
    let status = sync::get_status();
    if !started && !status.running {
        return Err("Saved data is being managed. Start sync when it finishes.".into());
    }
    Ok(json!({
        "started": started,
        "alreadyRunning": !started,
        "running": true,
        "phase": status.phase,
        "filesDone": status.files_done,
        "filesTotal": status.files_total,
        "records": status.records,
        "errors": status.errors,
        "family": if family.is_empty() { Value::Null } else { Value::String(family) },
        "mode": mode,
    }))
}

fn attach_annotations(conn: &Connection, session: &mut Value) -> Result<(), String> {
    let id = session["id"].as_i64().ok_or("Invalid session ID")?;
    session["starred"] = json!(query_one(
        conn,
        "SELECT starred FROM session_annotation WHERE session_id=?",
        &[SqlValue::Integer(id)]
    )?
    .map(|r| r["starred"].as_i64() == Some(1))
    .unwrap_or(false));
    session["tags"] = json!(query_all(
        conn,
        "SELECT label FROM session_tag WHERE session_id=? ORDER BY label",
        &[SqlValue::Integer(id)]
    )?
    .into_iter()
    .filter_map(|r| r["label"].as_str().map(str::to_string))
    .collect::<Vec<_>>());
    Ok(())
}

fn attach_annotations_batch(
    conn: &Connection,
    sessions: &mut [Value],
    job: Option<&ExportJob>,
) -> Result<(), String> {
    // Keep the bind count bounded for whole-library exports.
    for chunk in sessions.chunks_mut(500) {
        if let Some(job) = job {
            job.check()?;
        }
        let mut positions = HashMap::with_capacity(chunk.len());
        let mut params = Vec::with_capacity(chunk.len());
        for (index, session) in chunk.iter_mut().enumerate() {
            let id = session["id"].as_i64().ok_or("Invalid session ID")?;
            positions.insert(id, index);
            params.push(SqlValue::Integer(id));
            session["starred"] = json!(false);
            session["tags"] = json!([]);
        }
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        for row in query_all(
            conn,
            &format!("SELECT session_id,starred FROM session_annotation WHERE session_id IN ({placeholders})"),
            &params,
        )? {
            if let Some(index) = row["session_id"].as_i64().and_then(|id| positions.get(&id)) {
                chunk[*index]["starred"] = json!(row["starred"].as_i64() == Some(1));
            }
        }
        for mut row in query_all(
            conn,
            &format!("SELECT session_id,label FROM session_tag WHERE session_id IN ({placeholders}) ORDER BY label"),
            &params,
        )? {
            if let Some(index) = row["session_id"].as_i64().and_then(|id| positions.get(&id)) {
                if let Some(tags) = chunk[*index]["tags"].as_array_mut() {
                    tags.push(row["label"].take());
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn update_session_annotations(
    state: State<'_, AppState>,
    id: i64,
    starred: Option<bool>,
    tags: Option<Vec<String>>,
) -> Result<Value, String> {
    let path = db_path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _writer = db::user_write();
        let mut conn = open_db(&path)?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
        let mut session = query_one(&tx, "SELECT id FROM session WHERE id=?", &[SqlValue::Integer(id)])?.ok_or("Session not found")?;
        if let Some(starred) = starred {
            tx.execute("INSERT INTO session_annotation(session_id,starred) VALUES (?1,?2) ON CONFLICT(session_id) DO UPDATE SET starred=excluded.starred", rusqlite::params![id, starred]).map_err(|e| e.to_string())?;
        }
        if let Some(tags) = tags {
            if tags.len() > 50 { return Err("Use at most 50 tags per session".to_string()); }
            tx.execute("DELETE FROM session_tag WHERE session_id=?", [id]).map_err(|e| e.to_string())?;
            for tag in tags {
                let tag = tag.trim();
                if tag.is_empty() { continue; }
                if tag.chars().count() > 80 || tag.chars().any(char::is_control) { return Err("Tags must be at most 80 characters with no control characters".to_string()); }
                tx.execute("INSERT OR IGNORE INTO session_tag(session_id,label) VALUES (?1,?2)", rusqlite::params![id, tag]).map_err(|e| e.to_string())?;
            }
        }
        attach_annotations(&tx, &mut session)?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(session)
    }).await.map_err(|e| e.to_string())?
}

/// Session list filters, shared by search and export.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SessionFilters {
    #[serde(default)]
    q: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    since: String,
    #[serde(default)]
    sort: String,
    #[serde(default)]
    starred: bool,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    host: String,
}

#[derive(Default)]
pub struct ExportState(Mutex<HashMap<String, Arc<ExportJob>>>);

#[derive(Default)]
struct ExportJob {
    cancelled: AtomicBool,
    interrupt: Mutex<Option<rusqlite::InterruptHandle>>,
    saved: Mutex<bool>,
}

impl ExportJob {
    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::SeqCst) {
            Err("Export stopped".to_string())
        } else {
            Ok(())
        }
    }
}

fn export_job(state: &ExportState, id: &str) -> Result<Arc<ExportJob>, String> {
    state
        .0
        .lock()
        .map_err(|_| "Could not access export state")?
        .get(id)
        .cloned()
        .ok_or_else(|| "Export no longer active".to_string())
}

#[tauri::command]
pub fn begin_export(state: State<'_, ExportState>, job_id: String) -> Result<(), String> {
    let mut jobs = state
        .0
        .lock()
        .map_err(|_| "Could not access export state")?;
    if jobs.contains_key(&job_id) {
        return Err("Export already active".to_string());
    }
    jobs.insert(job_id, Arc::new(ExportJob::default()));
    Ok(())
}

#[tauri::command]
pub fn cancel_export(state: State<'_, ExportState>, job_id: String) -> Result<bool, String> {
    let job = export_job(&state, &job_id)?;
    let saved = job.saved.lock().map_err(|_| "Could not stop export")?;
    if *saved {
        return Ok(false);
    }
    job.cancelled.store(true, Ordering::SeqCst);
    if let Some(handle) = job
        .interrupt
        .lock()
        .map_err(|_| "Could not stop export")?
        .as_ref()
    {
        handle.interrupt();
    }
    Ok(true)
}

#[tauri::command]
pub fn finish_export(state: State<'_, ExportState>, job_id: String) -> Result<(), String> {
    state
        .0
        .lock()
        .map_err(|_| "Could not access export state")?
        .remove(&job_id);
    Ok(())
}

struct ExportRequest {
    scope: String,
    session_id: Option<i64>,
    filters: SessionFilters,
    include_stats: bool,
    format: String,
}

struct ExportOutput<'a, W> {
    inner: W,
    job: &'a ExportJob,
}

impl<W: Write> Write for ExportOutput<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.job.check().map_err(std::io::Error::other)?;
        self.inner.write(&bytes[..bytes.len().min(64 * 1024)])
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.job.check().map_err(std::io::Error::other)?;
        self.inner.flush()
    }
}

fn export_ids(conn: &Connection, request: &ExportRequest) -> Result<Vec<i64>, String> {
    if request.scope == "session" {
        return Ok(vec![request
            .session_id
            .ok_or("Select a session to export")?]);
    }
    let defaults = SessionFilters::default();
    let filters = if request.scope == "library" {
        &defaults
    } else {
        &request.filters
    };
    let SessionQuery {
        base,
        params,
        order_by,
    } = session_query(filters);
    let columns = match filters.sort.as_str() {
        "longest" => "s.id,(julianday(COALESCE(s.last_event_at,s.started_at))-julianday(s.started_at)) AS duration_days",
        "messages" => "s.id,(SELECT COUNT(*) FROM entry e WHERE e.session_id=s.id) AS entry_count",
        _ => "s.id",
    };
    let mut statement = conn
        .prepare_cached(&format!("SELECT {columns} {base} ORDER BY {order_by}"))
        .map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(rusqlite::params_from_iter(params), |row| row.get(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn write_export(
    conn: &Connection,
    request: &ExportRequest,
    output: impl Write,
    job: &ExportJob,
    progress: impl Fn(usize, usize, usize),
) -> Result<usize, String> {
    if !["session", "library", "filtered"].contains(&request.scope.as_str()) {
        return Err("Invalid export scope".into());
    }
    if !["json", "markdown"].contains(&request.format.as_str()) {
        return Err("Invalid export format".into());
    }
    job.check()?;
    let ids = export_ids(conn, request)?;
    let total = ids.len();
    let pricing = Arc::new(Mutex::new(pricing::PricingState::new()));
    if request.include_stats {
        let mut state = pricing.lock().map_err(|_| "Could not lock pricing")?;
        pricing::load_pricing_cache(conn, &mut state)?;
    }
    let mut writer = crate::export_format::ExportWriter::new(output, &request.format);
    writer
        .begin(&request.scope, &chrono::Utc::now().to_rfc3339(), total)
        .map_err(|e| e.to_string())?;
    progress(0, total, 0);
    for (index, id) in ids.into_iter().enumerate() {
        job.check()?;
        let mut session = get_session_metadata(conn, id)?;
        let object = session.as_object_mut().ok_or("Invalid session")?;
        for key in [
            "raw_metadata_json",
            "metrics_json",
            "token_usage_json",
            "source_uri",
            "source_size",
            "source_mtime",
        ] {
            object.remove(key);
        }
        let stats = if request.include_stats {
            let stats = analytics::calculate_session(conn, &pricing, id)?;
            Some(json!({
                "duration_seconds": stats.duration_seconds, "model_name": stats.model_name,
                "input_tokens": stats.input_tokens, "output_tokens": stats.output_tokens,
                "cached_tokens": stats.cached_tokens, "total_tokens": stats.total_tokens,
                "estimated_cost": stats.estimated_cost, "user_turns": stats.user_turns,
                "assistant_turns": stats.assistant_turns, "total_turns": stats.total_turns,
                "tool_count": stats.tool_count, "tool_error_count": stats.tool_error_count,
            }))
        } else {
            None
        };
        writer
            .session(&session, stats.as_ref())
            .map_err(|e| e.to_string())?;
        let mut after = None;
        let mut written = 0;
        loop {
            job.check()?;
            let page = get_entries_after(conn, id, after)?;
            if page.is_empty() {
                break;
            }
            for entry in &page {
                job.check()?;
                writer.entry(entry).map_err(|e| e.to_string())?;
            }
            after = Some(
                page.last()
                    .and_then(|entry| entry["ordinal"].as_i64())
                    .ok_or("Invalid transcript ordinal")?,
            );
            written += page.len();
            progress(index, total, written);
        }
        writer.unlinked_tools().map_err(|e| e.to_string())?;
        let mut after = None;
        loop {
            job.check()?;
            let tools = if let Some(after) = after {
                query_all(conn,"SELECT * FROM tool_invocation WHERE session_id=? AND entry_id IS NULL AND id>? ORDER BY id LIMIT 500",&[SqlValue::Integer(id),SqlValue::Integer(after)])?
            } else {
                query_all(conn,"SELECT * FROM tool_invocation WHERE session_id=? AND entry_id IS NULL ORDER BY id LIMIT 500",&[SqlValue::Integer(id)])?
            };
            if tools.is_empty() {
                break;
            }
            for tool in &tools {
                job.check()?;
                writer.unlinked_tool(tool).map_err(|e| e.to_string())?;
            }
            after = Some(
                tools
                    .last()
                    .and_then(|tool| tool["id"].as_i64())
                    .ok_or("Invalid tool ID")?,
            );
        }
        writer.end_session().map_err(|e| e.to_string())?;
        progress(index + 1, total, written);
    }
    writer.finish().map_err(|e| e.to_string())?;
    job.check()?;
    Ok(total)
}

fn save_streamed_export(
    database: &Path,
    target: &Path,
    request: &ExportRequest,
    job: &ExportJob,
    progress: impl Fn(usize, usize, usize),
) -> Result<usize, String> {
    job.check()?;
    let parent = target
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let database = std::fs::canonicalize(database).map_err(|e| e.to_string())?;
    let candidate = if target.exists() {
        std::fs::canonicalize(target).map_err(|e| e.to_string())?
    } else {
        std::fs::canonicalize(parent)
            .map_err(|e| e.to_string())?
            .join(target.file_name().ok_or("Invalid export path")?)
    };
    if candidate == database
        || candidate.as_os_str() == format!("{}-wal", database.display()).as_str()
        || candidate.as_os_str() == format!("{}-shm", database.display()).as_str()
    {
        return Err(
            "Choose an export file separate from the active database and its journal files.".into(),
        );
    }
    let mut conn = open_db(&database)?;
    *job.interrupt
        .lock()
        .map_err(|_| "Could not access export state")? = Some(conn.get_interrupt_handle());
    conn.pragma_update(None, "query_only", true)
        .map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut temporary = tempfile::Builder::new()
        .prefix(&format!(".{}-export-", crate::brand::SLUG))
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|e| e.to_string())?;
    let count = {
        let mut output = std::io::BufWriter::with_capacity(
            64 * 1024,
            ExportOutput {
                inner: temporary.as_file_mut(),
                job,
            },
        );
        let count = write_export(&tx, request, &mut output, job, progress)?;
        output.flush().map_err(|e| e.to_string())?;
        count
    };
    tx.commit().map_err(|e| e.to_string())?;
    temporary.as_file().sync_all().map_err(|e| e.to_string())?;
    let mut saved = job
        .saved
        .lock()
        .map_err(|_| "Could not access export state")?;
    job.check()?;
    temporary
        .persist(target)
        .map_err(|e| format!("Could not save export: {e}"))?;
    *saved = true;
    Ok(count)
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn export_sessions(
    state: State<'_, AppState>,
    exports: State<'_, ExportState>,
    app: tauri::AppHandle,
    job_id: String,
    path: String,
    scope: String,
    id: Option<i64>,
    filters: SessionFilters,
    include_stats: bool,
    format: String,
) -> Result<Value, String> {
    let database = db_path_of(&state)?;
    let job = export_job(&exports, &job_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        let request = ExportRequest {
            scope,
            session_id: id,
            filters,
            include_stats,
            format,
        };
        let count = save_streamed_export(
            &database,
            Path::new(&path),
            &request,
            &job,
            |done, total, entries| {
                let _ = app.emit(
                    "export-progress",
                    json!({"jobId":job_id,"done":done,"total":total,"entries":entries}),
                );
            },
        )?;
        Ok(json!({"sessions":count,"path":path}))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "preview_fixtures.rs"]
mod preview_fixtures;

#[cfg(test)]
#[path = "query_tests.rs"]
mod query_tests;
