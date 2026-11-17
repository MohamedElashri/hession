//! Transcript queries preserving desktop response shapes and snapshot boundaries.
use crate::sql::{query_all, query_one};
use rusqlite::{types::Value as SqlValue, Connection};
use serde_json::{json, Value};
use std::collections::HashMap;

pub fn get_session_metadata(conn: &Connection, session_id: i64) -> Result<Value, String> {
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

pub fn get_session_from_conn(
    conn: &Connection,
    session_id: i64,
    limit: i64,
    offset: &mut i64,
) -> Result<Value, String> {
    if !(1..=500).contains(&limit) || *offset < 0 {
        return Err("Use a transcript limit of 1–500 and a nonnegative offset".into());
    }
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
pub fn get_entries_from_conn(
    conn: &Connection,
    session_id: i64,
    limit: i64,
    offset: i64,
) -> Result<Vec<Value>, String> {
    if !(1..=500).contains(&limit) || offset < 0 {
        return Err("Use a transcript limit of 1–500 and a nonnegative offset".into());
    }
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

pub fn get_entries_after(
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

pub(crate) fn attach_entry_details(
    conn: &Connection,
    entries: Vec<Value>,
) -> Result<Vec<Value>, String> {
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

pub fn attach_annotations(conn: &Connection, session: &mut Value) -> Result<(), String> {
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
