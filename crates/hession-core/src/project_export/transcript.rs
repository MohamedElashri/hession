use super::{privacy::Sanitizer, ExportError, Work};
use crate::{search::VISIBLE_CONVERSATION_KINDS, transcript_export::ExportWriter};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

/// Fixed schema expressions only; callers cannot select identifiers or limits.
/// The direct-column octet_length check is lazy before any TEXT read/substring.
#[derive(Clone, Copy)]
pub(super) enum Metadata {
    Family,
    Product,
    PortableId,
    CallId,
    ToolName,
    Status,
    Tag,
    Branch,
    EntryKind,
    Role,
    Occurred,
    PartKind,
    Action,
    Evidence,
    Observed,
    EntryOrdinal,
    PartOrdinal,
    ExitCode,
    FileToolId,
    RepositoryId,
}
impl Metadata {
    pub(super) fn sql(self) -> String {
        let (column, limit) = match self {
            Self::Family => ("a.family", 1024),
            Self::Product => ("a.product", 1024),
            Self::PortableId => ("portable_id", 4096),
            Self::CallId => ("t.call_id", 4096),
            Self::ToolName => ("t.tool_name", 1024),
            Self::Status => ("t.status", 128),
            Self::Tag => ("label", 1024),
            Self::Branch => ("branch", 4096),
            Self::EntryKind => ("e.kind", 128),
            Self::Role => ("e.role", 128),
            Self::Occurred => ("e.occurred_at", 128),
            Self::PartKind => ("p.kind", 128),
            Self::Action => ("f.action", 128),
            Self::Evidence => ("f.evidence", 512),
            Self::Observed => ("c.observed_at", 128),
            Self::EntryOrdinal => ("e.ordinal", 128),
            Self::PartOrdinal => ("p.ordinal", 128),
            Self::ExitCode => ("t.exit_code", 128),
            Self::FileToolId => ("f.tool_invocation_id", 128),
            Self::RepositoryId => ("c.repository_id", 128),
        };
        format!("CASE WHEN coalesce(octet_length({column}),0)<={limit} THEN {column} ELSE _hession_export_metadata_limit_v1() END")
    }
}

pub(super) fn digest(parts: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part);
    }
    format!("{:x}", hash.finalize())
}
pub(super) fn cell(
    work: &Work<'_>,
    table: &str,
    column: &str,
    id: i64,
    budget: usize,
) -> Result<Option<String>, ExportError> {
    work.check()?;
    let null = work
        .conn
        .prepare_cached(&format!(
            "SELECT {column} IS NULL FROM {table} WHERE rowid=?"
        ))?
        .query_row([id], |r| r.get::<_, bool>(0))?;
    if null {
        return Ok(None);
    }
    // Incremental SQLite BLOB I/O also supports TEXT columns and checks byte size before allocation.
    let mut blob = work.conn.blob_open("main", table, column, id, true)?;
    if blob.len() > budget.min(work.limits.max_cell_bytes) {
        return Err(ExportError::Limit("individual field bytes"));
    }
    let mut data = Vec::with_capacity(blob.len());
    blob.read_to_end(&mut data)?;
    work.check()?;
    Ok(Some(String::from_utf8(data).map_err(|_| {
        ExportError::Invalid("Invalid UTF-8 in recorded text".into())
    })?))
}
fn small(text: String) -> Result<String, ExportError> {
    if text.len() > 4096 {
        Err(ExportError::Limit("metadata bytes"))
    } else {
        Ok(text)
    }
}
pub(super) fn logical_id(work: &Work<'_>, id: i64) -> Result<(String, &'static str), ExportError> {
    let mut basis = "native_or_replication";
    let source = cell(
        work,
        "session",
        "source_uri",
        id,
        work.limits.max_cell_bytes,
    )?
    .unwrap_or_default();
    let portable = if let Some(rest) = source.strip_prefix("hession-sync://") {
        let portable = rest.split('/').next().unwrap_or_default();
        if portable.is_empty() {
            basis = "source_digest";
            digest(&[b"hession-export-source-identity-v1", source.as_bytes()])
        } else {
            portable.to_owned()
        }
    } else {
        let existing:bool=work.conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='replication_identity')",[],|r|r.get(0))?;
        let mapped = if existing {
            work.conn
                .query_row(
                    &format!(
                        "SELECT {} FROM replication_identity WHERE local_session_id=? LIMIT 1",
                        Metadata::PortableId.sql()
                    ),
                    [id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
        } else {
            None
        };
        if let Some(mapped) = mapped {
            if mapped.len() > 4096 {
                return Err(ExportError::Limit("portable identity"));
            }
            mapped
        } else if let Some(native) = cell(
            work,
            "session",
            "external_id",
            id,
            work.limits.max_cell_bytes,
        )?
        .filter(|s| !s.is_empty())
        {
            let family = work.conn.query_row(
                &format!(
                    "SELECT {} FROM session s CROSS JOIN agent a ON a.id=s.agent_id WHERE s.id=?",
                    Metadata::Family.sql()
                ),
                [id],
                |r| r.get::<_, String>(0),
            )?;
            let started = cell(work, "session", "started_at", id, 4096)?.unwrap_or_default();
            digest(&[
                b"hession-session-v1",
                small(family)?.as_bytes(),
                native.as_bytes(),
                started.as_bytes(),
            ])
        } else {
            basis = "source_digest";
            digest(&[b"hession-export-source-identity-v1", source.as_bytes()])
        }
    };
    Ok((
        format!(
            "session-{}",
            digest(&[b"hession-export-session-v1", portable.as_bytes()])
        ),
        basis,
    ))
}
pub(super) fn entry_id(session: &str, ordinal: i64) -> String {
    format!("{session}:entry:{ordinal}")
}
pub(super) fn tool_id(
    conn: &Connection,
    session: &str,
    session_id: i64,
    id: i64,
) -> Result<Option<String>, ExportError> {
    let row=conn.query_row(&format!("SELECT coalesce({},''),coalesce({},-1),{},(SELECT count(*) FROM tool_invocation other INDEXED BY tool_entry_idx WHERE other.session_id=t.session_id AND other.entry_id IS t.entry_id AND other.id<t.id) FROM tool_invocation t LEFT JOIN entry e ON e.id=t.entry_id AND e.session_id=t.session_id WHERE t.id=?1 AND t.session_id=?2",Metadata::CallId.sql(),Metadata::EntryOrdinal.sql(),Metadata::ToolName.sql()),params![id,session_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?))).optional()?;
    row.map(|(call, ordinal, name, rank)| {
        if call.len() > 4096 {
            return Err(ExportError::Limit("tool call identity"));
        }
        Ok(format!(
            "tool-{}",
            digest(&[
                session.as_bytes(),
                call.as_bytes(),
                ordinal.to_string().as_bytes(),
                small(name)?.as_bytes(),
                rank.to_string().as_bytes()
            ])
        ))
    })
    .transpose()
}
fn metadata(
    work: &Work<'_>,
    id: i64,
    portable: &str,
    privacy: &mut Sanitizer<'_>,
) -> Result<Value, ExportError> {
    let (family,product,starred):(String,String,bool)=work.conn.query_row(&format!("SELECT {},{},coalesce((SELECT starred FROM session_annotation WHERE session_id=s.id),0) FROM session s CROSS JOIN agent a ON a.id=s.agent_id WHERE s.id=?",Metadata::Family.sql(),Metadata::Product.sql()),[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let mut value =
        json!({"id":portable,"agent":small(family)?,"product":small(product)?,"starred":starred});
    for name in ["title", "started_at", "last_event_at"] {
        value[name] = cell(work, "session", name, id, work.limits.max_cell_bytes)?
            .map(Value::String)
            .unwrap_or(Value::Null);
    }
    let mut tags=work.conn.prepare(&format!("SELECT {} AS export_label FROM session_tag NOT INDEXED WHERE session_id=? ORDER BY export_label COLLATE BINARY LIMIT 101",Metadata::Tag.sql()))?;
    let tags = tags
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if tags.len() > 100 {
        return Err(ExportError::Limit("session tag count"));
    }
    value["tags"] = serde_json::to_value(tags)?;
    if privacy.options.include_absolute_paths {
        for name in ["cwd", "host"] {
            value[name] = cell(work, "session", name, id, work.limits.max_cell_bytes)?
                .map(Value::String)
                .unwrap_or(Value::Null);
        }
    }
    if privacy.options.include_git_metadata {
        value["branch"]=work.conn.query_row(&format!("SELECT {} FROM session_context INDEXED BY context_session_latest_idx WHERE session_id=? AND branch IS NOT NULL ORDER BY id DESC LIMIT 1",Metadata::Branch.sql()),[id],|r|r.get::<_,String>(0)).optional()?.map(Value::String).unwrap_or(Value::Null);
    }
    privacy.value(&mut value)?;
    Ok(value)
}
fn tool(
    work: &Work<'_>,
    session_id: i64,
    portable: &str,
    id: i64,
    privacy: &mut Sanitizer<'_>,
    remaining: &mut usize,
) -> Result<Value, ExportError> {
    let (name,status,exit,ordinal):(String,Option<String>,Option<i64>,Option<i64>)=work.conn.query_row(&format!("SELECT {},{},{},{} FROM tool_invocation t LEFT JOIN entry e ON e.id=t.entry_id AND e.session_id=t.session_id WHERE t.session_id=?1 AND t.id=?2",Metadata::ToolName.sql(),Metadata::Status.sql(),Metadata::ExitCode.sql(),Metadata::EntryOrdinal.sql()),params![session_id,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let mut value = json!({"id":tool_id(work.conn,portable,session_id,id)?,"entry_id":ordinal.map(|o|entry_id(portable,o)),"tool_name":small(name)?,"status":status,"exit_code":exit});
    privacy.value(&mut value)?;
    if privacy.options.include_tool_bodies {
        for column in ["command_text", "arguments_json", "result_text"] {
            if let Some(text) = cell(work, "tool_invocation", column, id, *remaining)? {
                let sanitized = privacy.structured_text(&text)?;
                *remaining = remaining
                    .checked_sub(text.len().max(sanitized.len()))
                    .ok_or(ExportError::Limit("entry bytes"))?;
                value[column] = Value::String(sanitized);
            }
        }
    }
    Ok(value)
}
#[derive(Default)]
pub(super) struct TranscriptCounts {
    pub scanned: u64,
    pub exported: u64,
}
pub(super) struct TranscriptStyle<'a> {
    pub format: &'a str,
    pub exported_at: &'a str,
}
pub(super) fn write_transcript(
    work: &Work<'_>,
    session_id: i64,
    portable: &str,
    style: TranscriptStyle<'_>,
    output: &mut dyn Write,
    privacy: &mut Sanitizer<'_>,
    statistics: Option<&Value>,
) -> Result<TranscriptCounts, ExportError> {
    let options = privacy.options;
    let mut writer = ExportWriter::new(output, style.format);
    writer.begin("project", style.exported_at, 1)?;
    let mut stats = statistics.cloned();
    if let Some(stats) = &mut stats {
        privacy.value(stats)?;
    }
    writer.session(
        &metadata(work, session_id, portable, privacy)?,
        stats.as_ref(),
    )?;
    let mut after = None;
    let mut counts = TranscriptCounts::default();
    loop {
        work.check()?;
        // Separate first/continuation queries let SQLite seek on both index columns.
        // A nullable OR would rescan the session prefix for every page.
        let predicate = if after.is_some() {
            " AND e.ordinal>?2"
        } else {
            ""
        };
        let mut stmt = work.conn.prepare_cached(&format!(
            "SELECT e.id,{},{},{},{} FROM entry e INDEXED BY entry_session_order_idx \
             WHERE e.session_id=?1{predicate} ORDER BY e.ordinal LIMIT 50",
            Metadata::EntryOrdinal.sql(),
            Metadata::EntryKind.sql(),
            Metadata::Role.sql(),
            Metadata::Occurred.sql()
        ))?;
        let parameters: &[&dyn rusqlite::ToSql] = match &after {
            Some(ordinal) => &[&session_id, ordinal],
            None => &[&session_id],
        };
        let rows = stmt
            .query_map(parameters, |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if rows.is_empty() {
            break;
        }
        for (id, ordinal, kind, role, occurred_at) in rows {
            after = Some(ordinal);
            counts.scanned += 1;
            if counts.scanned > work.limits.max_entries {
                return Err(ExportError::Limit("entry count"));
            }
            work.check()?;
            if role.as_deref().is_some_and(|role| {
                role.eq_ignore_ascii_case("system") || role.eq_ignore_ascii_case("developer")
            }) || matches!(
                kind.to_ascii_lowercase().as_str(),
                "reasoning" | "thinking" | "analysis" | "redacted_thinking"
            ) {
                continue;
            }
            let conversation = matches!(role.as_deref(), Some("user" | "assistant"))
                && VISIBLE_CONVERSATION_KINDS.contains(&kind.as_str());
            let recorded_tool = options.include_tool_bodies
                && (role.as_deref() == Some("tool")
                    || matches!(
                        kind.as_str(),
                        "tool" | "tool_call" | "tool_result" | "tool_use" | "command_execution"
                    ));
            if !conversation && !recorded_tool {
                continue;
            }
            let mut remaining = work.limits.max_entry_bytes;
            let mut parts = Vec::new();
            let mut part_stmt=work.conn.prepare_cached(&format!("SELECT p.id,{},{} FROM content_part p INDEXED BY sqlite_autoindex_content_part_1 WHERE p.entry_id=? ORDER BY p.ordinal LIMIT 1001",Metadata::PartKind.sql(),Metadata::PartOrdinal.sql()))?;
            let source_parts = part_stmt
                .query_map([id], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            if source_parts.len() > 1000 {
                return Err(ExportError::Limit("entry parts"));
            }
            for (part_id, part_kind, part_ordinal) in source_parts {
                if part_kind != "text"
                    && !(options.include_tool_bodies
                        && matches!(
                            part_kind.as_str(),
                            "tool_call" | "tool_result" | "tool_use" | "tool_output"
                        ))
                {
                    continue;
                }
                let mut part = json!({"kind":part_kind,"ordinal":part_ordinal});
                for column in if part_kind == "text" {
                    &["text"][..]
                } else {
                    &["text", "data_json"][..]
                } {
                    if let Some(text) = cell(work, "content_part", column, part_id, remaining)? {
                        let sanitized = if *column == "data_json" {
                            privacy.structured_text(&text)?
                        } else {
                            privacy.text(&text)
                        };
                        remaining = remaining
                            .checked_sub(text.len().max(sanitized.len()))
                            .ok_or(ExportError::Limit("entry bytes"))?;
                        part[column] = Value::String(sanitized);
                    }
                }
                parts.push(part);
            }
            let mut tool_stmt=work.conn.prepare_cached("SELECT id FROM tool_invocation INDEXED BY tool_entry_idx WHERE session_id=?1 AND entry_id=?2 ORDER BY id LIMIT 101")?;
            let ids = tool_stmt
                .query_map(params![session_id, id], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            if ids.len() > 100 {
                return Err(ExportError::Limit("tools per entry"));
            }
            let mut tools = Vec::new();
            for id in ids {
                tools.push(tool(
                    work,
                    session_id,
                    portable,
                    id,
                    privacy,
                    &mut remaining,
                )?);
            }
            if parts.is_empty() && tools.is_empty() {
                continue;
            }
            let entry = json!({"id":entry_id(portable,ordinal),"ordinal":ordinal,"kind":privacy.text(&kind),"role":role.map(|s|privacy.text(&s)),"occurred_at":occurred_at.map(|s|privacy.text(&s)),"parts":parts,"tools":tools});
            // Never expose search_text or payload_json as the legacy formatter's fallback.
            writer.entry(&entry)?;
            counts.exported += 1;
        }
    }
    writer.unlinked_tools()?;
    let mut after = 0;
    loop {
        work.check()?;
        let mut stmt=work.conn.prepare("SELECT id FROM tool_invocation INDEXED BY tool_session_idx WHERE session_id=?1 AND entry_id IS NULL AND id>?2 ORDER BY id LIMIT 100")?;
        let ids = stmt
            .query_map(params![session_id, after], |r| r.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if ids.is_empty() {
            break;
        }
        for id in ids {
            after = id;
            let mut remaining = work.limits.max_entry_bytes;
            writer.unlinked_tool(&tool(
                work,
                session_id,
                portable,
                id,
                privacy,
                &mut remaining,
            )?)?;
            counts.scanned += 1;
            if counts.scanned > work.limits.max_entries {
                return Err(ExportError::Limit("entry and unlinked tool count"));
            }
        }
    }
    writer.end_session()?;
    writer.finish()?;
    Ok(counts)
}
pub(super) fn session_stats(conn: &Connection, id: i64) -> Result<Option<Value>, ExportError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM session_analytics WHERE session_id=?)",
        [id],
        |r| r.get(0),
    )?;
    if !exists {
        return Ok(None);
    }
    let fields = [
        "duration_seconds",
        "input_tokens",
        "output_tokens",
        "cached_tokens",
        "total_tokens",
        "estimated_cost",
        "user_turns",
        "assistant_turns",
        "tool_count",
        "tool_error_count",
    ];
    // REAL affinity can force a TEXT load even inside typeof(column). A
    // read-only incremental handle inspects the record type without loading
    // the cell. TEXT/BLOB succeeds, so reject it; numeric/null is reported by
    // SQLite's fixed builtin diagnostic. Propagate every other database error.
    // The caller holds one read snapshot across these probes and the SELECT.
    for field in fields {
        match conn.blob_open("main", "session_analytics", field, id, true) {
            Ok(_) => return Err(ExportError::Limit("metadata bytes")),
            Err(rusqlite::Error::SqliteFailure(error, Some(message)))
                if error.extended_code == rusqlite::ffi::SQLITE_ERROR
                    && matches!(
                        message.as_str(),
                        "cannot open value of type integer"
                            | "cannot open value of type real"
                            | "cannot open value of type null"
                    ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(conn.query_row("SELECT duration_seconds,input_tokens,output_tokens,cached_tokens,total_tokens,estimated_cost,user_turns,assistant_turns,tool_count,tool_error_count FROM session_analytics WHERE session_id=?",[id],|r|Ok(json!({"duration_seconds":r.get::<_,Option<f64>>(0)?,"input_tokens":r.get::<_,i64>(1)?,"output_tokens":r.get::<_,i64>(2)?,"cached_tokens":r.get::<_,i64>(3)?,"total_tokens":r.get::<_,i64>(4)?,"estimated_cost":r.get::<_,Option<f64>>(5)?,"user_turns":r.get::<_,i64>(6)?,"assistant_turns":r.get::<_,i64>(7)?,"tool_count":r.get::<_,i64>(8)?,"tool_error_count":r.get::<_,i64>(9)?}))).optional()?)
}
