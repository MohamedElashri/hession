//! Lossless content extraction and read-only adapters for agent SQLite sources.

use super::{first_str, iso_time, normalize_record, stop_requested, NormalizedRecord};
use rusqlite::{params, types::ValueRef, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentPart {
    pub kind: String,
    pub text: Option<String>,
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: Option<String>,
    pub name: String,
    pub arguments: Value,
    pub command: Option<String>,
    pub result: Option<String>,
    pub status: String,
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: Option<String>,
    pub text: String,
    pub status: String,
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecordContext {
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub commit: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub agent: Option<String>,
    pub permission: Option<String>,
}

pub(super) struct SessionData {
    pub uri: String,
    pub fallback_id: String,
    pub format: String,
    pub metadata: Value,
    pub records: Vec<NormalizedRecord>,
    pub usage: Option<Value>,
    pub size: i64,
    pub mtime: f64,
}

fn value_text(v: &Value) -> String {
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}

fn arguments(v: Option<&Value>) -> Value {
    match v {
        Some(Value::String(s)) => {
            serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.clone()))
        }
        Some(v) => v.clone(),
        None => json!({}),
    }
}

fn content_parts(value: &Value, parts: &mut Vec<ContentPart>) {
    match value {
        Value::Null => (),
        Value::String(s) => {
            if !s.is_empty() {
                parts.push(ContentPart {
                    kind: "text".into(),
                    text: Some(s.clone()),
                    data: None,
                });
            }
        }
        Value::Array(items) => {
            for item in items {
                content_parts(item, parts);
            }
        }
        Value::Object(_) => {
            let typ = first_str(value, &["type", "kind"])
                .unwrap_or_default()
                .to_lowercase();
            let kind = match typ.as_str() {
                "tool_use" | "tool_call" | "function_call" | "toolcall" | "tool" => "tool_call",
                "tool_result"
                | "function_call_output"
                | "custom_tool_call_output"
                | "toolresult" => "tool_result",
                "text" | "input_text" | "output_text" => "text",
                _ => typ.as_str(),
            };
            if kind == "tool_call" {
                parts.push(ContentPart {
                    kind: kind.into(),
                    text: first_str(value, &["name", "toolName", "tool_name", "tool"])
                        .or_else(|| first_str(&value["function"], &["name"])),
                    data: Some(value.clone()),
                });
            } else if kind == "tool_result" {
                let output = value
                    .get("content")
                    .or_else(|| value.get("output"))
                    .or_else(|| value.get("result"))
                    .or_else(|| value.get("text"));
                parts.push(ContentPart {
                    kind: kind.into(),
                    text: output.map(value_text),
                    data: Some(value.clone()),
                });
            } else if kind == "text" || typ.is_empty() {
                if let Some(s) = value.get("text").and_then(Value::as_str) {
                    parts.push(ContentPart {
                        kind: "text".into(),
                        text: Some(s.into()),
                        data: Some(value.clone()),
                    });
                } else if let Some(nested) = value.get("content").or_else(|| value.get("output")) {
                    content_parts(nested, parts);
                } else {
                    parts.push(ContentPart {
                        kind: "event".into(),
                        text: None,
                        data: Some(value.clone()),
                    });
                }
            } else {
                // Keep reasoning, media, and unfamiliar content types intact.
                parts.push(ContentPart {
                    kind: kind.into(),
                    text: first_str(value, &["text", "thinking"]),
                    data: Some(value.clone()),
                });
            }
        }
        _ => parts.push(ContentPart {
            kind: "event".into(),
            text: None,
            data: Some(value.clone()),
        }),
    }
}

fn has_error(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn tool_call(value: &Value) -> Option<ToolCall> {
    let name = first_str(value, &["name", "toolName", "tool_name", "tool"])
        .or_else(|| first_str(&value["function"], &["name"]))?;
    let args = arguments(
        value
            .get("input")
            .or_else(|| value.get("arguments"))
            .or_else(|| value["function"].get("arguments"))
            .or_else(|| value["state"].get("input")),
    );
    let state = value.get("state").unwrap_or(value);
    let exit_code = state
        .get("exit_code")
        .or_else(|| state.get("exitCode"))
        .or_else(|| state["metadata"].get("exitCode"))
        .or_else(|| state["metadata"].get("exit_code"))
        .and_then(Value::as_i64);
    let error = state
        .get("is_error")
        .or_else(|| state.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || state.get("error").is_some_and(has_error)
        || exit_code.is_some_and(|code| code != 0);
    let result = state
        .get("output")
        .or_else(|| state.get("aggregated_output"))
        .or_else(|| state.get("error"))
        .filter(|v| !v.is_null())
        .map(value_text);
    Some(ToolCall {
        call_id: first_str(value, &["call_id", "callId", "callID", "toolCallId", "id"]),
        name,
        command: first_str(&args, &["command", "cmd"]).or_else(|| first_str(value, &["command"])),
        arguments: args,
        status: if error {
            "error".into()
        } else {
            first_str(state, &["status"]).unwrap_or_else(|| {
                if result.is_some() {
                    "completed"
                } else {
                    "invoked"
                }
                .into()
            })
        },
        result,
        exit_code,
    })
}

pub(super) fn record_details(
    family: &str,
    obj: &Value,
    content: &Value,
    role: &str,
) -> (
    Vec<ContentPart>,
    Vec<ToolCall>,
    Vec<ToolResult>,
    RecordContext,
) {
    let source = match family {
        "codex" | "muse" => obj.get("payload").unwrap_or(obj),
        "claude" | "pi" | "commandcode" => {
            obj.get("message").filter(|v| v.is_object()).unwrap_or(obj)
        }
        _ => obj,
    };
    let mut parts = Vec::new();
    let source_type = first_str(source, &["type"]).unwrap_or_default();
    if source_type.ends_with("_call_output") {
        let mut result = source.clone();
        result["type"] = json!("tool_result");
        content_parts(&result, &mut parts);
    } else {
        content_parts(content, &mut parts);
    }
    let item = source.get("item").unwrap_or(source);
    let item_type = first_str(item, &["type"])
        .unwrap_or_default()
        .to_lowercase();
    if matches!(
        item_type.as_str(),
        "function_call" | "custom_tool_call" | "tool_call" | "tool_use" | "toolcall"
    ) {
        if !parts.iter().any(|p| p.kind == "tool_call") {
            let mut call = item.clone();
            // Codex custom tools use a string input; preserve it verbatim.
            call["type"] = json!("tool_call");
            content_parts(&call, &mut parts);
        }
    }
    if let Some(calls) = source.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            let mut call = call.clone();
            call["type"] = json!("tool_call");
            content_parts(&call, &mut parts);
        }
    }
    if matches!(role, "tool" | "toolResult" | "tool_result")
        && !parts.iter().any(|p| p.kind == "tool_result")
    {
        parts.clear();
        let mut result = source.clone();
        result["type"] = json!("tool_result");
        result["content"] = content.clone();
        content_parts(&result, &mut parts);
    }
    let mut tools = parts
        .iter()
        .filter(|p| p.kind == "tool_call")
        .filter_map(|p| p.data.as_ref().and_then(tool_call))
        .collect::<Vec<_>>();
    if !matches!(role, "tool" | "toolResult" | "tool_result")
        && (item.get("command").is_some()
            || source.get("toolName").is_some()
            || matches!(
                item_type.as_str(),
                "commandexecution" | "filechange" | "mcptoolcall"
            ))
    {
        let mut call = item.clone();
        if first_str(&call, &["name", "toolName", "tool_name"]).is_none() {
            call["name"] = json!(first_str(item, &["type"]).unwrap_or_else(|| "shell".into()));
        }
        if call.get("input").is_none() && call.get("arguments").is_none() {
            call["input"] = json!({"command": item.get("command")});
        }
        if let Some(tool) = tool_call(&call) {
            if !tools
                .iter()
                .any(|t| t.call_id == tool.call_id && t.name == tool.name)
            {
                parts.push(ContentPart {
                    kind: "tool_call".into(),
                    text: Some(tool.name.clone()),
                    data: Some(call),
                });
                tools.push(tool);
            }
        }
    }
    let results = parts
        .iter()
        .filter(|p| p.kind == "tool_result")
        .map(|p| {
            let value = p.data.as_ref().unwrap_or(&Value::Null);
            let exit_code = value
                .get("exit_code")
                .or_else(|| value.get("exitCode"))
                .and_then(Value::as_i64);
            let is_error = value
                .get("is_error")
                .or_else(|| value.get("isError"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
                || exit_code.is_some_and(|code| code != 0);
            ToolResult {
                call_id: first_str(
                    value,
                    &["tool_use_id", "tool_call_id", "call_id", "toolCallId"],
                ),
                text: p.text.clone().unwrap_or_default(),
                exit_code,
                status: if is_error { "error" } else { "completed" }.into(),
            }
        })
        .collect();
    if parts.is_empty() {
        parts.push(ContentPart {
            kind: "event".into(),
            text: None,
            data: Some(obj.clone()),
        });
    }
    let model_value = source.get("model").or_else(|| obj.get("model"));
    let context = RecordContext {
        cwd: first_str(source, &["cwd", "directory"])
            .or_else(|| first_str(obj, &["cwd", "directory"])),
        branch: first_str(source, &["gitBranch", "branch"])
            .or_else(|| first_str(obj, &["gitBranch", "branch"]))
            .or_else(|| first_str(&obj["git"], &["branch"])),
        commit: first_str(source, &["commit", "commit_sha"])
            .or_else(|| first_str(&source["git"], &["commit"])),
        model: first_str(source, &["model", "model_id", "modelID"])
            .or_else(|| first_str(obj, &["model", "model_id", "modelID"]))
            .or_else(|| model_value.and_then(|m| first_str(m, &["id", "modelID", "model_id"]))),
        provider: first_str(source, &["provider", "providerID", "billing_provider"])
            .or_else(|| first_str(obj, &["provider", "providerID", "billing_provider"]))
            .or_else(|| model_value.and_then(|m| first_str(m, &["providerID", "provider"]))),
        agent: first_str(source, &["agent", "agent_name", "agentType"]),
        permission: first_str(source, &["permissionMode", "permission_mode"])
            .or_else(|| first_str(obj, &["permissionMode", "permission_mode"])),
    };
    (parts, tools, results, context)
}

fn count(v: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_i64).filter(|n| *n >= 0))
}

/// Muse batches its first records inside `retained_frame` wrappers whose real
/// records live in `children[].record_json` (escaped JSON strings). Expand one
/// JSONL line into its inner records so usage and timestamps see every event.
fn expand_muse_values(values: &[Value]) -> Result<Vec<Value>, String> {
    let mut out = Vec::with_capacity(values.len());
    for value in values {
        if value.get("retained_frame").is_some() {
            if let Some(children) = value.get("children").and_then(Value::as_array) {
                let mut expanded = false;
                for child in children {
                    if let Some(s) = child.get("record_json").and_then(Value::as_str) {
                        if s.trim().is_empty() {
                            continue;
                        }
                        let inner: Value = serde_json::from_str(s)
                            .map_err(|e| format!("Invalid muse record_json: {e}"))?;
                        out.push(inner);
                        expanded = true;
                    }
                }
                if expanded {
                    continue;
                }
            }
        }
        out.push(value.clone());
    }
    Ok(out)
}

/// Preserve reported zeros, but do not manufacture zeros for missing fields.
fn reported_usage(family: &str, values: &[Value]) -> Option<Value> {
    // Antigravity transcripts carry no token fields at all (only
    // step_index/source/type/content/thinking/tool_calls), so there is nothing
    // to sum. Return None so analytics falls back to text estimates with
    // zero cached tokens rather than recording a bogus zero-usage row.
    if family == "antigravity" {
        return None;
    }
    let mut totals: [Option<i64>; 3] = [None; 3];
    let mut per_message = std::collections::HashMap::<String, [Option<i64>; 3]>::new();
    for (index, value) in values.iter().enumerate() {
        // Muse mirrors every billable call as a `goal_usage_attribution`
        // copy; only `model_completed` carries the authoritative per-call
        // usage, so skip everything else to avoid double counting.
        if family == "muse"
            && value
                .get("payload")
                .and_then(|p| p.get("event"))
                .and_then(|e| e.get("kind"))
                .and_then(Value::as_str)
                != Some("model_completed")
        {
            continue;
        }
        let (u, cumulative) = match family {
            "codex" => (&value["payload"]["info"]["total_token_usage"], true),
            "claude" => (&value["message"]["usage"], false),
            "pi" => (&value["message"]["usage"], false),
            // CommandCode reports usage as a top-level sibling of `message`
            // (pi nests it inside `message`); accept both, prefer top-level.
            "commandcode" => {
                if value.get("usage").is_some_and(|v| v.is_object()) {
                    (&value["usage"], false)
                } else {
                    (&value["message"]["usage"], false)
                }
            }
            "muse" => (&value["payload"]["event"]["usage"], false),
            "opencode" => (&value["tokens"], false),
            _ => (&Value::Null, false),
        };
        if !u.is_object() {
            continue;
        }
        let input = count(u, &["input_tokens", "inputTokens", "input"]);
        let output = count(u, &["output_tokens", "outputTokens", "output"]);
        let cached = count(
            u,
            &[
                "cached_input_tokens",
                "cache_read_input_tokens",
                "cachedTokens",
                "cached_tokens",
                "cacheRead",
                "cacheReadTokens",
                "cache_read_tokens",
            ],
        )
        .or_else(|| count(&u["cache"], &["read"]));
        let created = count(
            u,
            &[
                "cache_creation_input_tokens",
                "cacheWrite",
                "cacheWriteTokens",
                "cache_write_tokens",
            ],
        )
        .or_else(|| count(&u["cache"], &["write"]));
        // `input` is already the billed total for codex (cumulative),
        // commandcode (`inputTokens`) and muse (`input_tokens`); adding the
        // cache subset again would double count. Claude (`input_tokens`) and
        // pi (`input`) report the non-cached slice, so the total is
        // input + cache-read + cache-write.
        let input_is_total = family == "codex"
            || family == "commandcode"
            || family == "muse"
            || u.get("inputTokens").is_some();
        let fields = [
            if input_is_total {
                input
            } else {
                input.map(|n| {
                    n.saturating_add(cached.unwrap_or(0))
                        .saturating_add(created.unwrap_or(0))
                })
            },
            output,
            cached,
        ];
        if cumulative {
            for i in 0..3 {
                if let Some(n) = fields[i] {
                    totals[i] = Some(totals[i].unwrap_or(0).max(n));
                }
            }
        } else {
            let id = first_str(&value["message"], &["id"])
                .or_else(|| first_str(value, &["id", "uuid"]))
                .unwrap_or_else(|| index.to_string());
            let previous = per_message.entry(id).or_insert([None; 3]);
            for i in 0..3 {
                if let Some(n) = fields[i] {
                    previous[i] = Some(previous[i].unwrap_or(0).max(n));
                }
            }
        }
    }
    for fields in per_message.values() {
        for i in 0..3 {
            if let Some(n) = fields[i] {
                totals[i] = Some(totals[i].unwrap_or(0).saturating_add(n));
            }
        }
    }
    let mut usage = Map::new();
    for (key, n) in ["inputTokens", "outputTokens", "cachedTokens"]
        .into_iter()
        .zip(totals)
    {
        if let Some(n) = n {
            usage.insert(key.into(), json!(n));
        }
    }
    if usage.is_empty() {
        None
    } else {
        Some(Value::Object(usage))
    }
}

/// Resolve identity from the conversation directory, never a transcript name.
/// The summary database is optional; older/local exports can use prompt titles.
fn antigravity_metadata(file: &Path) -> Result<Map<String, Value>, String> {
    let mut metadata = Map::new();
    let Some(conversation) = file.ancestors().find(|path| {
        path.parent().and_then(Path::file_name).is_some_and(|name| name == "brain")
    }) else {
        return Ok(metadata);
    };
    let id = conversation.file_name().and_then(|name| name.to_str())
        .ok_or("Invalid Antigravity conversation directory")?;
    metadata.insert("id".into(), json!(id));
    let root = conversation.parent().and_then(Path::parent)
        .ok_or("Invalid Antigravity source root")?;
    let db = root.join("conversation_summaries.db");
    if !db.is_file() {
        return Ok(metadata);
    }
    let source = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open Antigravity summaries: {e}"))?;
    source.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let summary: Option<(String, String)> = source.query_row(
        "SELECT title,workspace_uris FROM conversation_summaries WHERE conversation_id=?1",
        [id], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(|e| format!("read Antigravity title: {e}"))?;
    if let Some((title, workspaces)) = summary {
        if !title.trim().is_empty() {
            metadata.insert("title".into(), json!(title));
        }
        let uris: Vec<String> = serde_json::from_str(&workspaces)
            .map_err(|e| format!("read Antigravity workspaces: {e}"))?;
        if let Some(path) = uris.iter().find_map(|uri| {
            reqwest::Url::parse(uri).ok()?.to_file_path().ok()
        }) {
            metadata.insert("cwd".into(), json!(path.to_string_lossy()));
        }
    }
    Ok(metadata)
}

pub(super) fn session_data(
    family: &str,
    uri: &str,
    file: &Path,
    values: Vec<Value>,
    size: i64,
    mtime: f64,
) -> Result<SessionData, String> {
    // Unwrap muse `retained_frame` batches once so the record loop and the
    // usage rollup below both see the same inner events.
    let values = if family == "muse" {
        expand_muse_values(&values)?
    } else {
        values
    };
    let mut metadata = Map::new();
    let mut records = Vec::with_capacity(values.len());
    for (i, value) in values.iter().enumerate() {
        if stop_requested() {
            return Err("Import stopped".into());
        }
        let meta = if family == "codex" && value["type"] == "session_meta" {
            &value["payload"]
        } else {
            value
        };
        // Message IDs must not overwrite the session identity.
        for key in [
            "cwd",
            "directory",
            "title",
            "gitBranch",
            "model",
            "provider",
            "version",
            "started_at",
            "ended_at",
            "last_activity_at",
        ] {
            if let Some(v) = meta.get(key).filter(|v| !v.is_null()) {
                metadata.insert(key.into(), v.clone());
            }
        }
        if family == "codex" && value["type"] == "session_meta"
            || matches!(family, "pi" | "commandcode") && value["type"] == "session"
        {
            if let Some(map) = meta.as_object() {
                metadata.extend(map.clone());
            }
        } else if let Some(id) = value.get("sessionId").or_else(|| value.get("session_id")) {
            metadata.insert("id".into(), id.clone());
        }
        if value["type"] == "ai-title" {
            if let Some(title) = value.get("aiTitle") {
                metadata.insert("title".into(), title.clone());
            }
        }
        let record =
            normalize_record(family, value, i as i64).ok_or("Expected an object record")?;
        records.push(record);
    }
    if family == "antigravity" {
        metadata.extend(antigravity_metadata(file)?);
    }
    let usage = reported_usage(family, &values);
    Ok(SessionData {
        uri: uri.into(),
        fallback_id: file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session")
            .into(),
        format: if family == "muse" {
            "muse-session-envelope-v1"
        } else {
            "jsonl"
        }
        .into(),
        metadata: Value::Object(metadata),
        records,
        usage,
        size,
        mtime,
    })
}

pub(super) fn insert_details(
    conn: &Connection,
    session_id: i64,
    entry_id: i64,
    record: &NormalizedRecord,
) -> Result<(), String> {
    for (ordinal, part) in record.parts.iter().enumerate() {
        conn.prepare_cached("INSERT INTO content_part(entry_id,ordinal,kind,text,data_json) VALUES (?1,?2,?3,?4,?5)")
            .map_err(|e| e.to_string())?.execute(params![entry_id,ordinal as i64,part.kind,part.text,part.data.as_ref().map(Value::to_string)]).map_err(|e| e.to_string())?;
    }
    let context = &record.context;
    let model_id = match &context.model {
        Some(model) => super::super::db::get_or_create_model(
            conn,
            context.provider.as_deref().unwrap_or(""),
            model,
        )
        .map_err(|e| e.to_string())?,
        None => None,
    };
    if context != &RecordContext::default() {
        conn.prepare_cached("INSERT INTO session_context(session_id,entry_id,observed_at,cwd,branch,commit_sha,model_id,agent_role,permission_mode,raw_json)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)")
            .map_err(|e| e.to_string())?.execute(params![session_id,entry_id,record.occurred_at,context.cwd,context.branch,context.commit,model_id,context.agent,context.permission,
                serde_json::to_string(context).map_err(|e| e.to_string())?]).map_err(|e| e.to_string())?;
    }
    for tool in &record.tools {
        conn.prepare_cached("INSERT INTO tool_invocation(session_id,entry_id,call_id,tool_name,command_text,arguments_json,result_text,status,exit_code,started_at,ended_at)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)")
            .map_err(|e| e.to_string())?.execute(params![session_id,entry_id,tool.call_id,tool.name,tool.command,tool.arguments.to_string(),tool.result,tool.status,tool.exit_code,
                record.occurred_at, if tool.result.is_some() { record.occurred_at.as_deref() } else { None }]).map_err(|e| e.to_string())?;
        let tool_id = conn.last_insert_rowid();
        insert_paths(
            conn,
            session_id,
            entry_id,
            tool_id,
            &tool.arguments,
            &tool.name,
        )?;
    }
    Ok(())
}

fn insert_paths(
    conn: &Connection,
    session_id: i64,
    entry_id: i64,
    tool_id: i64,
    value: &Value,
    tool_name: &str,
) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if [
                    "file_path",
                    "filepath",
                    "path",
                    "target_file",
                    "targetfile",
                    "filename",
                ]
                .contains(&key.to_lowercase().as_str())
                {
                    if let Some(path) = value.as_str().filter(|s| !s.is_empty()) {
                        let name = tool_name.to_lowercase();
                        let action = if ["write", "edit", "patch", "delete", "create_file", "str_replace"].iter().any(|part| name.contains(part)) {
                            "write"
                        } else if ["read", "view", "open_file"].iter().any(|part| name.contains(part)) {
                            "read"
                        } else {
                            "unknown"
                        };
                        conn.prepare_cached("INSERT INTO file_activity(session_id,entry_id,tool_invocation_id,path,action,evidence,confidence)
                        VALUES (?1,?2,?3,?4,?5,'tool arguments','reported')").map_err(|e| e.to_string())?
                            .execute(params![session_id,entry_id,tool_id,path,action]).map_err(|e| e.to_string())?;
                    }
                }
                insert_paths(conn, session_id, entry_id, tool_id, value, tool_name)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                insert_paths(conn, session_id, entry_id, tool_id, item, tool_name)?;
            }
        }
        Value::String(patch) if tool_name.to_lowercase().contains("patch") && patch.contains("*** Begin Patch") => {
            // Patch text supplies explicit targets even without a structured path argument.
            for line in patch.lines() {
                for prefix in ["*** Update File: ", "*** Add File: ", "*** Delete File: ", "*** Move to: "] {
                    if let Some(path) = line.strip_prefix(prefix).map(str::trim).filter(|p| !p.is_empty()) {
                        conn.prepare_cached("INSERT INTO file_activity(session_id,entry_id,tool_invocation_id,path,action,evidence,confidence)
                            VALUES (?1,?2,?3,?4,'write','patch target','reported')").map_err(|e| e.to_string())?
                            .execute(params![session_id,entry_id,tool_id,path]).map_err(|e| e.to_string())?;
                    }
                }
            }
        }
        _ => (),
    }
    Ok(())
}

pub(super) fn session_metrics(conn: &Connection, id: i64) -> Result<Value, String> {
    let mut metrics = conn.query_row(
        "SELECT COUNT(*),COALESCE(SUM(role='user'),0),COALESCE(SUM(role='assistant'),0),
         COALESCE(SUM(CASE WHEN role='user' THEN length(search_text) ELSE 0 END),0),
         COALESCE(SUM(CASE WHEN role='assistant' THEN length(search_text) ELSE 0 END),0),
         COALESCE(SUM(CASE WHEN role IS NOT 'user' AND role IS NOT 'assistant' THEN length(search_text) ELSE 0 END),0)
         FROM entry WHERE session_id=?", [id], |row| {
            Ok(json!({"total_turns":row.get::<_,i64>(0)?,"user_turns":row.get::<_,i64>(1)?,"assistant_turns":row.get::<_,i64>(2)?,
                "user_chars":row.get::<_,i64>(3)?,"assistant_chars":row.get::<_,i64>(4)?,"tool_chars":row.get::<_,i64>(5)?}))
        }).map_err(|e| e.to_string())?;
    let (count, errors): (i64, i64) = conn.query_row(
        "SELECT COUNT(*),COALESCE(SUM(status='error' OR (exit_code IS NOT NULL AND exit_code!=0)),0) FROM tool_invocation WHERE session_id=?",
        [id], |row| Ok((row.get(0)?,row.get(1)?))).map_err(|e| e.to_string())?;
    metrics["tool_count"] = json!(count);
    metrics["tool_error_count"] = json!(errors);
    Ok(metrics)
}

/// Convert SQLite rows dynamically so optional columns can vary by version.
/// SQL identifiers are fixed by the adapters, never supplied by a source row.
pub(super) fn rows(conn: &Connection, sql: &str, session: Option<&str>) -> Result<Vec<Value>, String> {
    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| format!("read source schema: {e}"))?;
    let names = stmt
        .column_names()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    let mut rows = match session {
        Some(id) => stmt.query([id]),
        None => stmt.query([]),
    }
    .map_err(|e| e.to_string())?;
    let mut values = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if stop_requested() {
            return Err("Import stopped".into());
        }
        let mut value = Map::new();
        for (i, name) in names.iter().enumerate() {
            let v = match row.get_ref(i).map_err(|e| e.to_string())? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(n) => json!(n),
                ValueRef::Real(n) => json!(n),
                ValueRef::Text(s) => Value::String(
                    std::str::from_utf8(s)
                        .map_err(|e| format!("source text encoding: {e}"))?
                        .into(),
                ),
                // Hermes stores binary display identities alongside transcript
                // text. Keep every byte in raw records with an explicit tag;
                // binary metadata must not prevent importing the database.
                ValueRef::Blob(bytes) => json!({"$sqlite_blob": bytes}),
            };
            value.insert(name.clone(), v);
        }
        values.push(Value::Object(value));
    }
    Ok(values)
}

fn source_json(value: &Value, column: &str, default: Value) -> Result<Value, String> {
    match value.get(column) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::String(s)) if s.is_empty() => Ok(default),
        Some(Value::String(s)) => {
            serde_json::from_str(s).map_err(|e| format!("Invalid JSON in source {column}: {e}"))
        }
        Some(v) => Ok(v.clone()),
    }
}

fn source_id(value: &Value) -> Result<String, String> {
    first_str(value, &["id"]).ok_or_else(|| "Source row has no ID".into())
}

/// Size + mtime fingerprint of a source database file, folding in `-wal` and
/// `-journal` sidecars so updates not yet checkpointed into the main file
/// still invalidate the fast path below.
fn db_fingerprint(file: &Path) -> Result<(i64, f64), String> {
    let stat = std::fs::metadata(file).map_err(|e| format!("stat {}: {e}", file.display()))?;
    if !stat.is_file() {
        return Err("Source is not a file".to_string());
    }
    let mut size = stat.len() as i64;
    let mut mtime = super::source_mtime(&stat);
    let name = file.as_os_str().to_owned();
    for suffix in ["-wal", "-journal"] {
        let mut sidecar = name.clone();
        sidecar.push(suffix);
        if let Ok(st) = std::fs::metadata(Path::new(&sidecar)) {
            if st.is_file() {
                size = size.saturating_add(st.len() as i64);
                mtime = mtime.max(super::source_mtime(&st));
            }
        }
    }
    Ok((size, mtime))
}

/// Cheap per-session change check without reading the transcript: message
/// count plus the session timestamp that `opencode_session` / `hermes_session`
/// store as the sync-state size/mtime. No content is fetched or hashed.
fn session_fresh(
    snapshot: &rusqlite::Transaction<'_>,
    dest: &Connection,
    uri: &str,
    family: &str,
    session: &Value,
    id: &str,
) -> Result<bool, String> {
    let count: i64 = snapshot
        .query_row(
            if family == "opencode" {
                "SELECT COUNT(*) FROM message WHERE session_id=?"
            } else {
                "SELECT COUNT(*) FROM messages WHERE session_id=?"
            },
            [id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    let stamp = if family == "opencode" {
        session
            .get("time_updated")
            .and_then(Value::as_f64)
            .or_else(|| session.get("time_created").and_then(Value::as_f64))
            .unwrap_or(0.0)
    } else {
        session
            .get("last_activity_at")
            .and_then(Value::as_f64)
            .or_else(|| session.get("ended_at").and_then(Value::as_f64))
            .or_else(|| session.get("started_at").and_then(Value::as_f64))
            .unwrap_or(0.0)
            * 1000.0
    };
    Ok(super::get_file_state(dest, &format!("{uri}#{id}"))
        .map_err(|e| e.to_string())?
        .is_some_and(|st| st.size == count && st.mtime == stamp && st.has_metrics))
}

pub(super) fn import_database(
    dest: &mut Connection,
    uri: &str,
    family: &str,
    file: &Path,
    product: &str,
    root: &str,
    force: bool,
    mut on_session: Option<&mut dyn FnMut(usize)>,
    pricing: &mut super::ImportPricing,
) -> Result<usize, String> {
    if !matches!(family, "opencode" | "hermes") {
        return Err(format!("Unsupported database source: {family}"));
    }
    // File-level fast path: an unchanged database file is skipped without
    // opening it at all.
    let (size, mtime) = db_fingerprint(file)?;
    if !force {
        if let Some(st) = super::get_file_state(dest, uri).map_err(|e| e.to_string())? {
            if st.size == size && st.mtime == mtime {
                return Ok(0);
            }
        }
    }
    let mut source = Connection::open_with_flags(file, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("open source database: {e}"))?;
    source
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    // A read transaction reads a consistent set of session/message/part rows.
    let snapshot = source.transaction().map_err(|e| e.to_string())?;
    let sessions = rows(
        &snapshot,
        if family == "opencode" {
            "SELECT * FROM session ORDER BY time_created,id"
        } else {
            "SELECT * FROM sessions ORDER BY started_at,id"
        },
        None,
    )?;
    let mut total = 0;
    let mut batch = Vec::new();
    let mut batch_records = 0;
    let mut batch_start = std::time::Instant::now();
    for session in sessions {
        if stop_requested() {
            return Err("Import stopped".into());
        }
        let id = source_id(&session)?;
        if !force && session_fresh(&snapshot, dest, uri, family, &session, &id)? {
            if let Some(report) = on_session.as_deref_mut() {
                report(0);
            }
            continue;
        }
        let messages = rows(&snapshot, if family == "opencode" {
            "SELECT * FROM message WHERE session_id=? ORDER BY time_created,id"
        } else {
            "SELECT * FROM messages WHERE session_id=? ORDER BY id"
        }, Some(&id))?;
        let parts = if family == "opencode" {
            rows(&snapshot,
                "SELECT p.* FROM part p JOIN message m ON m.id=p.message_id
                 WHERE m.session_id=? ORDER BY p.time_created,p.id", Some(&id))?
        } else { Vec::new() };
        let data = if family == "opencode" {
            let mut by_message: HashMap<String, Vec<Value>> = HashMap::new();
            for part in parts {
                let message_id = first_str(&part, &["message_id"]).ok_or("Invalid OpenCode part message ID")?;
                by_message.entry(message_id).or_default().push(part);
            }
            opencode_session(uri, session, &id, messages, by_message)?
        } else {
            hermes_session(uri, session, &id, messages)?
        };
        batch_records += data.records.len();
        batch.push(data);
        // Parse sources outside the writer gate. Bound both memory and time;
        // waiting interactive writes also trigger an early batch boundary.
        if batch.len() >= 16 || batch_records >= 5000
            || batch_start.elapsed() >= std::time::Duration::from_millis(40)
            || crate::db::user_write_pending()
        {
            for count in super::commit_batch(dest, family, product, root, pricing, std::mem::take(&mut batch))? {
                total += count;
                if let Some(report) = on_session.as_deref_mut() { report(count); }
            }
            batch_records = 0;
            batch_start = std::time::Instant::now();
        }
    }
    if stop_requested() {
        return Err("Import stopped".into());
    }
    snapshot.commit().map_err(|e| e.to_string())?;
    if !batch.is_empty() {
        for count in super::commit_batch(dest, family, product, root, pricing, batch)? {
            total += count;
            if let Some(report) = on_session.as_deref_mut() { report(count); }
        }
    }
    let _writer = crate::db::background_write();
    if stop_requested() { return Err("Import stopped".into()); }
    super::set_file_state(dest, uri, size, mtime).map_err(|e| e.to_string())?;
    Ok(total)
}

fn milliseconds_time(value: &Value) -> Option<String> {
    if let Some(ms) = value.as_i64() {
        chrono::DateTime::from_timestamp_millis(ms)
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    } else {
        iso_time(value)
    }
}

fn seconds_time(value: &Value) -> Option<String> {
    if let Some(seconds) = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
        .filter(|n| n.is_finite())
    {
        chrono::DateTime::from_timestamp_millis((seconds * 1000.0) as i64)
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    } else {
        iso_time(value)
    }
}

fn opencode_session(
    uri: &str,
    mut metadata: Value,
    id: &str,
    messages: Vec<Value>,
    mut parts: HashMap<String, Vec<Value>>,
) -> Result<SessionData, String> {
    if let Some(raw_model) = metadata["model"].as_str() {
        if let Ok(model) = serde_json::from_str::<Value>(raw_model) {
            if model.is_object() {
                metadata["model"] = model;
            }
        }
    }
    let mut records = Vec::with_capacity(messages.len());
    let mut usage_values = Vec::new();
    for message in &messages {
        let message_id = source_id(message)?;
        let data = source_json(message, "data", Value::Null)?;
        if !data.is_object() {
            return Err("OpenCode message data must be an object".into());
        }
        let raw_parts = parts.remove(&message_id).unwrap_or_default();
        let mut content = Vec::new();
        for part in &raw_parts {
            let data = source_json(part, "data", Value::Null)?;
            if !data.is_object() {
                return Err("OpenCode part data must be an object".into());
            }
            content.push(data);
        }
        let mut normalized = data.clone();
        normalized["id"] = json!(message_id);
        normalized["type"] = json!("message");
        let timestamp = data["time"]
            .get("created")
            .cloned()
            .unwrap_or_else(|| message["time_created"].clone());
        normalized["timestamp"] = milliseconds_time(&timestamp)
            .map(Value::String)
            .unwrap_or(Value::Null);
        normalized["parent_id"] = data.get("parentID").cloned().unwrap_or(Value::Null);
        normalized["model"] = data
            .get("model")
            .or_else(|| data.get("modelID"))
            .cloned()
            .unwrap_or_else(|| metadata["model"].clone());
        normalized["provider"] = data.get("providerID").cloned().unwrap_or(Value::Null);
        normalized["content"] = json!(content);
        normalized["cwd"] = metadata.get("directory").cloned().unwrap_or(Value::Null);
        let mut record = normalize_record("opencode", &normalized, records.len() as i64)
            .ok_or("Invalid OpenCode message")?;
        record.payload = json!({"message": message, "parts": raw_parts});
        records.push(record);
        normalized["id"] = json!(message_id);
        usage_values.push(normalized);
    }
    metadata["cwd"] = metadata.get("directory").cloned().unwrap_or(Value::Null);
    let mtime = metadata["time_updated"]
        .as_f64()
        .or_else(|| metadata["time_created"].as_f64())
        .unwrap_or(0.0);
    let mut usage = reported_usage("opencode", &usage_values).unwrap_or_else(|| json!({}));
    // Newer databases also report authoritative session totals.
    if let Some(input) = count(&metadata, &["tokens_input"]) {
        usage["inputTokens"] = json!(input
            .saturating_add(count(&metadata, &["tokens_cache_read"]).unwrap_or(0))
            .saturating_add(count(&metadata, &["tokens_cache_write"]).unwrap_or(0)));
    }
    for (key, column) in [
        ("outputTokens", "tokens_output"),
        ("cachedTokens", "tokens_cache_read"),
    ] {
        if let Some(n) = count(&metadata, &[column]) {
            usage[key] = json!(n);
        }
    }
    // Session timestamps in OpenCode are milliseconds.
    metadata["started_at"] = milliseconds_time(&metadata["time_created"])
        .map(Value::String)
        .unwrap_or(Value::Null);
    metadata["last_activity_at"] = milliseconds_time(&metadata["time_updated"])
        .map(Value::String)
        .unwrap_or(Value::Null);
    Ok(SessionData {
        uri: format!("{uri}#{id}"),
        fallback_id: id.into(),
        format: "opencode-db".into(),
        usage: if usage.as_object().is_some_and(|m| !m.is_empty()) {
            Some(usage)
        } else {
            None
        },
        metadata,
        size: records.len() as i64,
        mtime,
        records,
    })
}

fn hermes_session(
    uri: &str,
    mut metadata: Value,
    id: &str,
    messages: Vec<Value>,
) -> Result<SessionData, String> {
    let mut records = Vec::with_capacity(messages.len());
    let cwd = first_str(&metadata, &["cwd", "git_repo_root"])
        .map(Value::String)
        .unwrap_or(Value::Null);
    for message in &messages {
        let mut normalized = message.clone();
        normalized["type"] = json!("message");
        normalized["cwd"] = cwd.clone();
        normalized["model"] = metadata.get("model").cloned().unwrap_or(Value::Null);
        normalized["provider"] = metadata
            .get("billing_provider")
            .cloned()
            .unwrap_or(Value::Null);
        normalized["branch"] = metadata.get("git_branch").cloned().unwrap_or(Value::Null);
        let calls = source_json(message, "tool_calls", json!([]))?;
        if !calls.is_array() {
            return Err("Hermes tool_calls must be an array".into());
        }
        normalized["tool_calls"] = calls;
        normalized["timestamp"] = seconds_time(&message["timestamp"])
            .map(Value::String)
            .unwrap_or(Value::Null);
        let mut record = normalize_record("hermes", &normalized, records.len() as i64)
            .ok_or("Invalid Hermes message")?;
        record.payload = message.clone();
        // Tool results are linked by call ID without dropping their original entry.
        if record.role.as_deref() == Some("tool") {
            if let Some(result) = record.results.first_mut() {
                if let Some(text) = message["content"].as_str() {
                    if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                        result.exit_code = parsed["exit_code"].as_i64();
                        if result.exit_code.is_some_and(|n| n != 0)
                            || parsed.get("error").is_some_and(has_error)
                        {
                            result.status = "error".into();
                        }
                    }
                }
            }
        }
        records.push(record);
    }
    metadata["cwd"] = cwd;
    let mut usage = Map::new();
    for (output, input) in [
        ("inputTokens", "input_tokens"),
        ("outputTokens", "output_tokens"),
        ("cachedTokens", "cache_read_tokens"),
    ] {
        if let Some(n) = count(&metadata, &[input]) {
            usage.insert(output.into(), json!(n));
        }
    }
    let mtime = metadata["last_activity_at"]
        .as_f64()
        .or_else(|| metadata["ended_at"].as_f64())
        .or_else(|| metadata["started_at"].as_f64())
        .unwrap_or(0.0)
        * 1000.0;
    for key in ["started_at", "ended_at", "last_activity_at"] {
        metadata[key] = seconds_time(&metadata[key])
            .map(Value::String)
            .unwrap_or(Value::Null);
    }
    Ok(SessionData {
        uri: format!("{uri}#{id}"),
        fallback_id: id.into(),
        format: "hermes-db".into(),
        usage: if usage.is_empty() {
            None
        } else {
            Some(Value::Object(usage))
        },
        metadata,
        size: records.len() as i64,
        mtime,
        records,
    })
}
