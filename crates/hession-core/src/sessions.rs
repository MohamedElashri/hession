//! Transcript queries preserving desktop response shapes and snapshot boundaries.
use crate::sql::{query_all, query_one};
use rusqlite::{types::Value as SqlValue, Connection};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Count tool calls using narrow indexes instead of loading historical bodies
/// just to evaluate an error aggregate.
pub fn tool_counts(conn: &Connection, session_id: i64) -> rusqlite::Result<(i64, i64)> {
    conn.query_row(
        "SELECT (SELECT COUNT(*) FROM tool_invocation WHERE session_id=?1),
         (SELECT COUNT(*) FROM tool_invocation WHERE session_id=?1
          AND (status='error' OR (exit_code IS NOT NULL AND exit_code!=0)))",
        [session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

pub fn get_session_metadata(conn: &Connection, session_id: i64) -> Result<Value, String> {
    get_session_metadata_projected(conn, session_id, false)
}

fn get_session_metadata_projected(
    conn: &Connection,
    session_id: i64,
    reading: bool,
) -> Result<Value, String> {
    let columns = if reading {
        "s.id,s.agent_id,s.workspace_id,s.external_id,s.source_uri,s.source_format,
         s.source_version,s.agent_version,s.cwd,s.title,s.started_at,s.last_event_at,
         s.imported_at,s.source_size,s.source_mtime,NULL AS raw_metadata_json,
         s.token_usage_json,s.metrics_json,s.host,s.is_subagent"
    } else {
        "s.*"
    };
    let mut session = query_one(
        conn,
        &format!(
            "SELECT {columns},a.family AS agent,a.product,w.path AS project,\
             (SELECT branch FROM session_context c WHERE c.session_id=s.id \
              AND c.branch IS NOT NULL ORDER BY c.id DESC LIMIT 1) AS branch,\
             (SELECT model_key FROM session_context c JOIN model m ON m.id=c.model_id \
              WHERE c.session_id=s.id ORDER BY c.id DESC LIMIT 1) AS model \
             FROM session s JOIN agent a ON a.id=s.agent_id \
             LEFT JOIN workspace w ON w.id=s.workspace_id \
             WHERE s.id=?"
        ),
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
    get_session_projected(conn, session_id, limit, offset, false)
}

/// Desktop reading excludes duplicate search/native text already represented by
/// display parts or tools. Full exports and the existing full-fidelity query
/// retain their original payloads. Bookkeeping-only records keep their fallback.
pub fn get_reading_session_from_conn(
    conn: &Connection,
    session_id: i64,
    limit: i64,
    offset: &mut i64,
) -> Result<Value, String> {
    get_session_projected(conn, session_id, limit, offset, true)
}

fn get_session_projected(
    conn: &Connection,
    session_id: i64,
    limit: i64,
    offset: &mut i64,
    reading: bool,
) -> Result<Value, String> {
    if !(1..=500).contains(&limit) || *offset < 0 {
        return Err("Use a transcript limit of 1–500 and a nonnegative offset".into());
    }
    let session = get_session_metadata_projected(conn, session_id, reading)?;
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
    if *offset >= total && (reading || total > 0) {
        *offset = (total - 1).max(0) / limit * limit;
    }

    let entries = if reading {
        let entries = query_all(
            conn,
            "SELECT e.id,e.session_id,e.ordinal,e.external_entry_id,e.parent_entry_id,
             e.turn_id,e.occurred_at,e.kind,e.role,NULL AS search_text,
             CASE WHEN EXISTS(SELECT 1 FROM content_part cp WHERE cp.entry_id=e.id
                     AND ((cp.kind IN ('text','tool_result') AND octet_length(cp.text)>0)
                         OR (cp.kind<>'text' AND octet_length(cp.data_json)>0)))
               OR EXISTS(SELECT 1 FROM tool_invocation t WHERE t.entry_id=e.id)
               THEN NULL ELSE e.payload_json END AS payload_json
             FROM entry e WHERE e.session_id=? ORDER BY e.ordinal LIMIT ? OFFSET ?",
            &[
                SqlValue::Integer(session_id),
                SqlValue::Integer(limit),
                SqlValue::Integer(*offset),
            ],
        )?;
        attach_entry_details_projected(conn, entries, true)?
    } else {
        get_entries_from_conn(conn, session_id, limit, *offset)?
    };

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
    attach_entry_details_projected(conn, entries, false)
}

fn attach_entry_details_projected(
    conn: &Connection,
    entries: Vec<Value>,
    reading: bool,
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
        let numbered = params
            .iter()
            .enumerate()
            .map(|(index, _)| format!("?{}", index + 1))
            .collect::<Vec<_>>()
            .join(",");
        let part_scope = if reading { &numbered } else { &placeholders };
        let part_columns = if reading {
            let reference = format!("CASE WHEN cp.kind='tool_result' AND json_valid(cp.data_json) THEN
                (SELECT t.id FROM tool_invocation t WHERE t.session_id=e.session_id
                 AND t.entry_id IN ({numbered}) AND t.result_text IS NOT NULL
                 AND t.call_id=COALESCE(json_extract(cp.data_json,'$.tool_use_id'),
                     json_extract(cp.data_json,'$.call_id'),json_extract(cp.data_json,'$.tool_call_id'),
                     json_extract(cp.data_json,'$.toolCallId'))
                 AND t.result_text=cp.text ORDER BY t.id DESC LIMIT 1) END");
            format!("cp.id,cp.entry_id,cp.ordinal,cp.kind,
             CASE WHEN ({reference}) IS NOT NULL THEN NULL ELSE cp.text END AS text,
             ({reference}) AS tool_result_id,
             CASE WHEN cp.kind='text' AND octet_length(cp.text)>0 THEN NULL
               WHEN cp.kind='tool_call' AND EXISTS(SELECT 1 FROM tool_invocation t WHERE t.entry_id=cp.entry_id) THEN NULL
               WHEN cp.kind='tool_result' AND octet_length(cp.text)>0 AND json_valid(cp.data_json)
                 THEN json_remove(cp.data_json,'$.content','$.output','$.result','$.text')
               ELSE cp.data_json END AS data_json")
        } else {
            "cp.*".into()
        };
        for part in query_all(
            conn,
            &format!(
                "SELECT {part_columns}, e.ordinal AS e_ord FROM content_part cp \
                 JOIN entry e ON e.id=cp.entry_id \
                 WHERE cp.entry_id IN ({part_scope}) \
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_omits_duplicate_payloads_without_losing_display_or_export_content() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
            INSERT INTO session(id,agent_id,source_uri,external_id,title) VALUES(1,1,'fixture://reading','reading','Reading');
            INSERT INTO entry(id,session_id,ordinal,kind,role,payload_json) VALUES
             (1,1,0,'message','assistant','{}'),(2,1,1,'event',NULL,'{\"bookkeeping\":true}'),
             (3,1,2,'message','user','{\"fallback\":true}'),(4,1,3,'tool_call','tool','{}');
            INSERT INTO content_part(id,entry_id,ordinal,kind,text,data_json) VALUES
             (1,1,0,'text','text','{}'),(2,3,0,'text','','{\"empty\":true}');
            INSERT INTO tool_invocation(id,session_id,entry_id,call_id,tool_name,arguments_json,result_text)
             VALUES(1,1,4,'call','shell','{\"command\":\"echo hello\"}','hello');").unwrap();
        let text = "Visible Unicode text: café. ".repeat(20_000);
        let raw = json!({"type":"native","text":text}).to_string();
        let part = json!({"type":"text","text":text}).to_string();
        conn.execute(
            "UPDATE entry SET search_text=?1,payload_json=?2 WHERE id=1",
            rusqlite::params![text, raw],
        )
        .unwrap();
        conn.execute(
            "UPDATE content_part SET text=?1,data_json=?2 WHERE id=1",
            rusqlite::params![text, part],
        )
        .unwrap();
        let full = get_session_from_conn(&conn, 1, 500, &mut 0).unwrap();
        let reading = get_reading_session_from_conn(&conn, 1, 500, &mut 0).unwrap();
        assert_eq!(reading["total"], full["total"]);
        assert_eq!(reading["entryCounts"], full["entryCounts"]);
        assert_eq!(reading["session"], full["session"]);
        let rows = reading["entries"].as_array().unwrap();
        assert!(rows[0]["search_text"].is_null());
        assert!(rows[0]["payload_json"].is_null());
        assert!(rows[0]["parts"][0]["data_json"].is_null());
        assert_eq!(rows[0]["parts"][0]["text"], text);
        assert_eq!(rows[1]["payload_json"], "{\"bookkeeping\":true}");
        assert_eq!(rows[2]["payload_json"], "{\"fallback\":true}");
        assert_eq!(rows[2]["parts"][0]["data_json"], "{\"empty\":true}");
        assert!(rows[3]["payload_json"].is_null());
        assert_eq!(rows[3]["tools"], full["entries"][3]["tools"]);
        assert_eq!(full["entries"][0]["payload_json"], raw);
        assert_eq!(full["entries"][0]["search_text"], text);
        assert_eq!(full["entries"][0]["parts"][0]["data_json"], part);
        assert!(
            serde_json::to_vec(&reading).unwrap().len() * 3
                < serde_json::to_vec(&full).unwrap().len()
        );
        let unchanged = get_session_from_conn(&conn, 1, 500, &mut 0).unwrap();
        assert_eq!(
            full, unchanged,
            "reading must not alter stored/full-fidelity history"
        );
        for kind in [
            "user_message",
            "agent_message",
            "user",
            "assistant",
            "GENERIC",
            "custom_native_kind",
        ] {
            conn.execute("UPDATE entry SET kind=? WHERE id=1", [kind])
                .unwrap();
            let page = get_reading_session_from_conn(&conn, 1, 500, &mut 0).unwrap();
            assert!(
                page["entries"][0]["payload_json"].is_null(),
                "displayed native kind {kind} must not retain duplicate text"
            );
            assert_eq!(page["entries"][0]["parts"][0]["text"], text);
            assert_eq!(
                get_session_from_conn(&conn, 1, 500, &mut 0).unwrap()["entries"][0]["payload_json"],
                raw
            );
        }
    }
    #[test]
    fn reading_reuses_tool_outputs_and_preserves_cross_page_and_export_bodies() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
            INSERT INTO session(id,agent_id,external_id,source_uri,title) VALUES(1,1,'outputs','fixture://outputs','Outputs');
            INSERT INTO entry(id,session_id,ordinal,kind,role) VALUES
              (1,1,0,'function_call','assistant'),(2,1,1,'function_call_output','tool'),(3,1,2,'session_meta',NULL);
            INSERT INTO tool_invocation(id,session_id,entry_id,call_id,tool_name,arguments_json)
              VALUES(1,1,1,'large-output','shell','{\"cmd\":\"echo synthetic\"}');").unwrap();
        let text = "Complete output: café. ".repeat(30_000);
        let raw_result =
            json!({"type":"function_call_output","call_id":"large-output","output":text})
                .to_string();
        let header = json!({"instructions":text}).to_string();
        conn.execute(
            "UPDATE tool_invocation SET result_text=? WHERE id=1",
            [&text],
        )
        .unwrap();
        conn.execute("UPDATE entry SET payload_json=? WHERE id=2", [&raw_result])
            .unwrap();
        conn.execute("UPDATE entry SET payload_json=? WHERE id=3", [&header])
            .unwrap();
        conn.execute(
            "UPDATE session SET raw_metadata_json=? WHERE id=1",
            [&header],
        )
        .unwrap();
        conn.execute("INSERT INTO content_part(entry_id,ordinal,kind,text,data_json) VALUES(1,0,'tool_call','shell','{\"arguments\":{\"cmd\":\"echo synthetic\"}}')",[]).unwrap();
        conn.execute("INSERT INTO content_part(entry_id,ordinal,kind,text,data_json) VALUES(2,0,'tool_result',?1,?2)", rusqlite::params![text,raw_result]).unwrap();
        conn.execute(
            "INSERT INTO content_part(entry_id,ordinal,kind,data_json) VALUES(3,0,'event',?)",
            [&header],
        )
        .unwrap();
        let full = get_session_from_conn(&conn, 1, 100, &mut 0).unwrap();
        let reading = get_reading_session_from_conn(&conn, 1, 100, &mut 0).unwrap();
        assert!(reading["session"]["raw_metadata_json"].is_null());
        assert_eq!(reading["entries"][0]["tools"][0]["result_text"], text);
        assert!(reading["entries"][0]["parts"][0]["data_json"].is_null());
        let result = &reading["entries"][1];
        assert!(result["payload_json"].is_null());
        assert!(result["parts"][0]["text"].is_null());
        assert_eq!(result["parts"][0]["tool_result_id"], 1);
        let metadata: Value =
            serde_json::from_str(result["parts"][0]["data_json"].as_str().unwrap()).unwrap();
        assert_eq!(metadata["call_id"], "large-output");
        assert!(metadata.get("output").is_none());
        assert!(reading["entries"][2]["payload_json"].is_null());
        assert_eq!(reading["entries"][2]["parts"][0]["data_json"], header);
        assert!(
            serde_json::to_vec(&reading).unwrap().len() * 3
                < serde_json::to_vec(&full).unwrap().len()
        );
        assert_eq!(get_session_from_conn(&conn, 1, 100, &mut 0).unwrap(), full);
        // An isolated result page needs its own complete text, not an unresolved reference.
        let isolated = get_reading_session_from_conn(&conn, 1, 1, &mut 1).unwrap();
        assert_eq!(isolated["entries"][0]["parts"][0]["text"], text);
        assert!(isolated["entries"][0]["parts"][0]["tool_result_id"].is_null());
        // A newer call result must not replace an earlier result event's body.
        conn.execute(
            "UPDATE tool_invocation SET result_text='Later result' WHERE id=1",
            [],
        )
        .unwrap();
        let later = get_reading_session_from_conn(&conn, 1, 100, &mut 0).unwrap();
        assert_eq!(later["entries"][1]["parts"][0]["text"], text);
        assert!(later["entries"][1]["parts"][0]["tool_result_id"].is_null());
    }
}
