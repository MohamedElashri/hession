//! Append normalization for Claude/Codex only. Parsed rows are staged on disk
//! outside the library write lock, then streamed into one atomic transaction.
use super::*;
use hession_core::{indexing, jsonl};
use serde_json::{json, Map};
use std::io::{self, Write};

// Old summaries lack durable entry metrics (and could contain a fork's parent
// identity). Rebuild once; session IDs, bookmarks and tags survive the rebuild.
const PARSER_VERSION: u32 = 3;
const MAX_STAGE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_SUMMARY_BYTES: usize = 16 * 1024 * 1024;
const SUMMARY_LIMIT: &str = "Source metadata exceeds the 16 MiB limit; previous history preserved";

fn json_bytes(value: &impl Serialize) -> Result<usize, String> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|n| *n <= MAX_SUMMARY_BYTES)
                .ok_or_else(|| io::Error::other(SUMMARY_LIMIT))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_err(|_| SUMMARY_LIMIT.to_string())?;
    Ok(counter.0)
}

/// Exact compact-JSON sizes, caching only fields that remain unchanged between
/// ordinary events. Large recorded metadata must not be serialized per event.
struct SummaryBudget {
    metadata: usize,
    cwd: usize,
    title: usize,
}
impl SummaryBudget {
    fn new(summary: &Summary) -> Result<Self, String> {
        let budget = Self {
            metadata: json_bytes(&summary.metadata)?,
            cwd: json_bytes(&summary.cwd)?,
            title: json_bytes(&summary.title)?,
        };
        budget.check(summary)?;
        Ok(budget)
    }
    fn check(&self, summary: &Summary) -> Result<usize, String> {
        let size = b"{\"entry_metrics\":,\"metadata\":,\"cwd\":,\"title\":,\"start\":,\"end\":}"
            .len()
            + self.metadata
            + self.cwd
            + self.title
            + json_bytes(&summary.entry_metrics)?
            + json_bytes(&summary.start)?
            + json_bytes(&summary.end)?;
        if size > MAX_SUMMARY_BYTES {
            Err(SUMMARY_LIMIT.into())
        } else {
            Ok(size)
        }
    }
    fn observe(
        &mut self,
        summary: &mut Summary,
        family: &str,
        metadata: &Value,
        record: &NormalizedRecord,
    ) -> Result<usize, String> {
        let cwd_missing = summary.cwd.is_none();
        let title_missing = summary.title.is_none();
        summary.observe(family, metadata, record)?;
        if metadata.as_object().is_some_and(|m| !m.is_empty()) {
            self.metadata = json_bytes(&summary.metadata)?;
        }
        if cwd_missing && summary.cwd.is_some() {
            self.cwd = json_bytes(&summary.cwd)?;
        }
        if title_missing && summary.title.is_some() {
            self.title = json_bytes(&summary.title)?;
        }
        self.check(summary)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct EntryMetrics {
    total_turns: i64,
    user_turns: i64,
    assistant_turns: i64,
    user_chars: i64,
    assistant_chars: i64,
    tool_chars: i64,
}
impl EntryMetrics {
    fn observe(&mut self, record: &NormalizedRecord) -> Result<(), String> {
        fn add(total: &mut i64, value: i64) -> Result<(), String> {
            *total = total
                .checked_add(value)
                .ok_or("Session metrics exceed the supported count")?;
            Ok(())
        }
        // Match SQLite length(TEXT): Unicode scalar values, stopping at NUL.
        let length = record
            .body
            .split('\0')
            .next()
            .unwrap_or_default()
            .chars()
            .count() as i64;
        add(&mut self.total_turns, 1)?;
        match record.role.as_deref() {
            Some("user") => {
                add(&mut self.user_turns, 1)?;
                add(&mut self.user_chars, length)?;
            }
            Some("assistant") => {
                add(&mut self.assistant_turns, 1)?;
                add(&mut self.assistant_chars, length)?;
            }
            _ => add(&mut self.tool_chars, length)?,
        }
        Ok(())
    }
    fn valid(&self, records: u64) -> bool {
        u64::try_from(self.total_turns).ok() == Some(records)
            && [
                self.user_turns,
                self.assistant_turns,
                self.user_chars,
                self.assistant_chars,
                self.tool_chars,
            ]
            .iter()
            .all(|n| *n >= 0)
            && self
                .user_turns
                .checked_add(self.assistant_turns)
                .is_some_and(|n| n <= self.total_turns)
    }
    fn resolved(&self, conn: &Connection, id: i64) -> Result<Value, String> {
        let mut value = serde_json::to_value(self).map_err(|e| e.to_string())?;
        let (count, errors) =
            hession_core::sessions::tool_counts(conn, id).map_err(|e| e.to_string())?;
        value["tool_count"] = json!(count);
        value["tool_error_count"] = json!(errors);
        Ok(value)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Summary {
    entry_metrics: EntryMetrics,
    metadata: Map<String, Value>,
    cwd: Option<String>,
    title: Option<String>,
    start: Option<String>,
    end: Option<String>,
}

impl Summary {
    fn observe(
        &mut self,
        family: &str,
        metadata: &Value,
        record: &NormalizedRecord,
    ) -> Result<(), String> {
        self.entry_metrics.observe(record)?;
        if let Some(map) = metadata.as_object() {
            formats::merge_session_metadata(family, &mut self.metadata, map);
        }
        if self.cwd.is_none() {
            self.cwd = record.context.cwd.clone();
        }
        if self.title.is_none() && record.role.as_deref() == Some("user") {
            let title = record.body.split_whitespace().collect::<Vec<_>>().join(" ");
            if !title.is_empty()
                && !title.to_lowercase().contains("environment context")
                && !title.contains("<environment_context>")
            {
                self.title = Some(title.chars().take(130).collect());
            }
        }
        if let Some(time) = &record.occurred_at {
            if self.start.as_ref().is_none_or(|t| time < t) {
                self.start = Some(time.clone());
            }
            if self.end.as_ref().is_none_or(|t| time > t) {
                self.end = Some(time.clone());
            }
        }
        Ok(())
    }

    fn resolved(&self) -> Value {
        let mut meta = Value::Object(self.metadata.clone());
        for (key, aliases, fallback) in [
            ("cwd", &["cwd", "directory", "git_repo_root"][..], &self.cwd),
            ("title", &["title"][..], &self.title),
            (
                "started_at",
                &["started_at", "time_created", "timestamp"][..],
                &self.start,
            ),
            (
                "last_activity_at",
                &["last_activity_at", "ended_at", "time_updated"][..],
                &self.end,
            ),
        ] {
            // Use the same parsing rules as the legacy session upsert.
            let present = if key.ends_with("_at") {
                first_iso(&meta, aliases)
            } else {
                first_str(&meta, aliases)
            };
            if present.is_none() {
                if let Some(value) = fallback {
                    meta[key] = json!(value);
                }
            }
        }
        meta
    }
}

fn merge_usage(conn: &Connection, session: i64, key: &str, value: &Value) -> Result<(), String> {
    conn.execute("INSERT INTO index_usage(session_id,message_key,input,output,cached) VALUES(?1,?2,?3,?4,?5)
        ON CONFLICT(session_id,message_key) DO UPDATE SET
        input=CASE WHEN ?3 IS NULL THEN input WHEN input IS NULL THEN ?3 ELSE MAX(input,?3) END,
        output=CASE WHEN ?4 IS NULL THEN output WHEN output IS NULL THEN ?4 ELSE MAX(output,?4) END,
        cached=CASE WHEN ?5 IS NULL THEN cached WHEN cached IS NULL THEN ?5 ELSE MAX(cached,?5) END",
        params![session,key,value["inputTokens"].as_i64(),value["outputTokens"].as_i64(),value["cachedTokens"].as_i64()])
        .map_err(|e|e.to_string())?;
    Ok(())
}

fn total_usage(conn: &Connection, session: i64) -> Result<Option<Value>, String> {
    let mut stmt = conn
        .prepare("SELECT input,output,cached FROM index_usage WHERE session_id=?")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([session]).map_err(|e| e.to_string())?;
    let mut totals: [Option<i64>; 3] = [None; 3];
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if stop_requested() {
            return Err("Import stopped".into());
        }
        for (i, total) in totals.iter_mut().enumerate() {
            if let Some(n) = row.get::<_, Option<i64>>(i).map_err(|e| e.to_string())? {
                *total = Some(total.unwrap_or(0).saturating_add(n));
            }
        }
    }
    let mut value = Map::new();
    for (key, total) in ["inputTokens", "outputTokens", "cachedTokens"]
        .into_iter()
        .zip(totals)
    {
        if let Some(n) = total {
            value.insert(key.into(), json!(n));
        }
    }
    Ok((!value.is_empty()).then_some(Value::Object(value)))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn import(
    conn: &mut Connection,
    family: &str,
    source_uri: &str,
    file: &Path,
    product: &str,
    root: &str,
    force: bool,
    pricing: &mut ImportPricing,
) -> Result<usize, String> {
    let epoch = pricing
        .epoch
        .unwrap_or(indexing::epoch(conn).map_err(|e| e.to_string())?);
    let previous = indexing::checkpoint(conn, source_uri).map_err(|e| e.to_string())?;
    let mut cursor = previous
        .as_ref()
        .and_then(|(_, cursor, _)| serde_json::from_str::<jsonl::Cursor>(cursor).ok());
    let previous_summary = previous
        .as_ref()
        .and_then(|(_, _, summary)| serde_json::from_str::<Summary>(summary).ok())
        .and_then(|summary| {
            SummaryBudget::new(&summary)
                .ok()
                .map(|budget| (summary, budget))
        });
    if let Some(saved) = &cursor {
        if previous_summary
            .as_ref()
            .is_none_or(|(summary, _)| !summary.entry_metrics.valid(saved.records))
        {
            cursor = None;
        }
    }
    if let (Some(cursor_value), Some((session, _, _))) = (&cursor, &previous) {
        let (count, max): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*),COALESCE(MAX(ordinal),-1) FROM entry WHERE session_id=?",
                [session],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        if u64::try_from(count).ok() != Some(cursor_value.records)
            || max.checked_add(1).and_then(|n| u64::try_from(n).ok()) != u64::try_from(count).ok()
        {
            cursor = None;
        }
    }
    let mut reader = match jsonl::Reader::open(
        file,
        if force { None } else { cursor.as_ref() },
        PARSER_VERSION,
        jsonl::Validation::Content,
        &stop_requested,
    )
    .map_err(|e| e.to_string())?
    {
        Some(reader) => reader,
        None => return Ok(0),
    };
    let append = reader.mode() == jsonl::Mode::Append;
    let old_count = if append {
        cursor.as_ref().map_or(0, |c| c.records)
    } else {
        0
    };
    let (mut summary, mut summary_budget) = if append {
        previous_summary.ok_or("Missing append summary")?
    } else {
        let summary = Summary::default();
        let budget = SummaryBudget::new(&summary)?;
        (summary, budget)
    };
    // tempfile creates private unpredictable paths and unlinks on every normal
    // error/cancellation. No transcript staging is placed in the native store.
    // NamedTempFile retains private 0600 permissions on macOS and Linux.
    let stage_file = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    let stage = Connection::open(stage_file.path()).map_err(|e| e.to_string())?;
    stage.execute_batch("PRAGMA journal_mode=OFF; PRAGMA synchronous=OFF; PRAGMA temp_store=FILE;
        PRAGMA cache_size=-2048;
        CREATE TABLE record(ordinal INTEGER PRIMARY KEY,json TEXT NOT NULL,usage_key TEXT,usage_json TEXT);
        CREATE TABLE affected_call(call_id TEXT PRIMARY KEY) WITHOUT ROWID;
        BEGIN;").map_err(|e|e.to_string())?;
    let mut count = 0usize;
    let mut bytes = 0u64;
    while let Some(value) = reader.next_record().map_err(|e| e.to_string())? {
        let data = formats::session_data(family, source_uri, file, vec![value.clone()], 0, 0.0)?;
        let record = data.records.first().ok_or("Expected a normalized record")?;
        summary_budget.observe(&mut summary, family, &data.metadata, record)?;
        let ordinal = old_count
            .checked_add(count as u64)
            .and_then(|n| i64::try_from(n).ok())
            .ok_or("Too many source records")?;
        let key = if family == "codex" {
            "cumulative".into()
        } else {
            first_str(&value["message"], &["id"])
                .or_else(|| first_str(&value, &["id", "uuid"]))
                .unwrap_or_else(|| ordinal.to_string())
        };
        let text = serde_json::to_string(record).map_err(|e| e.to_string())?;
        bytes = bytes.saturating_add(text.len() as u64);
        for call in record
            .tools
            .iter()
            .filter_map(|tool| tool.call_id.as_deref())
            .chain(
                record
                    .results
                    .iter()
                    .filter_map(|result| result.call_id.as_deref()),
            )
        {
            let added = stage
                .prepare_cached("INSERT OR IGNORE INTO affected_call VALUES(?)")
                .map_err(|e| e.to_string())?
                .execute([call])
                .map_err(|e| e.to_string())?;
            if added > 0 {
                bytes = bytes.saturating_add(call.len() as u64);
            }
        }
        if bytes > MAX_STAGE_BYTES {
            return Err(
                "Source exceeds the 4 GiB import staging limit; previous history preserved".into(),
            );
        }
        stage
            .execute(
                "INSERT INTO record VALUES(?,?,?,?)",
                params![ordinal, text, key, data.usage.map(|u| u.to_string())],
            )
            .map_err(|e| e.to_string())?;
        count += 1;
    }
    stage.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    let next = reader.finish().map_err(|e| e.to_string())?;
    if append && cursor.as_ref() == Some(&next) {
        return Ok(0);
    }
    if !append && next.offset < next.fingerprint.size {
        let existing: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session WHERE source_uri=?)",
                [source_uri],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if existing {
            return Err(
                "Source has an incomplete final line; previous import preserved until it completes"
                    .into(),
            );
        }
    }
    if next.records == 0 {
        let existing: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session WHERE source_uri=?)",
                [source_uri],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if existing {
            return Err("Source contains no complete records; previous import preserved".into());
        }
        return Ok(0);
    }
    let _writer = crate::db::background_write(conn).map_err(|e| e.to_string())?;
    indexing::check_epoch(conn, epoch)?;
    if stop_requested() {
        return Err("Import stopped".into());
    }
    pricing.refresh(conn)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| e.to_string())?;
    if pricing.automatic && indexing::excluded(&tx, source_uri).map_err(|e| e.to_string())? {
        return Ok(0);
    }
    if !pricing.automatic {
        indexing::include(&tx, source_uri).map_err(|e| e.to_string())?;
    }
    // Another source importer holds the source lock across preparation. A
    // non-import writer may still have invalidated this cursor meanwhile.
    if append && indexing::checkpoint(&tx, source_uri).map_err(|e| e.to_string())? != previous {
        return Err("History changed while preparing append; retry indexing".into());
    }
    let data = formats::SessionData {
        uri: source_uri.into(),
        fallback_id: file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session")
            .into(),
        format: "jsonl".into(),
        metadata: Value::Object(summary.metadata.clone()),
        records: Vec::new(),
        usage: None,
        size: i64::try_from(next.fingerprint.size).map_err(|_| "Source exceeds supported size")?,
        mtime: next.fingerprint.modified_secs as f64 * 1000.0
            + next.fingerprint.modified_nanos as f64 / 1_000_000.0,
    };
    let resolved = summary.resolved();
    let id = upsert_session(&tx, family, product, root, &data, Some(&resolved))?;
    if !append {
        for table in [
            "index_usage",
            "index_result",
            "file_activity",
            "tool_invocation",
            "session_context",
            "entry",
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE session_id=?"), [id])
                .map_err(|e| e.to_string())?;
        }
        tx.execute(
            "INSERT INTO index_change(session_id,reset) VALUES(?,1)",
            [id],
        )
        .map_err(|e| e.to_string())?;
    }
    let mut stmt = stage
        .prepare("SELECT ordinal,json,usage_key,usage_json FROM record ORDER BY ordinal")
        .map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        if stop_requested() {
            return Err("Import stopped".into());
        }
        let record: NormalizedRecord =
            serde_json::from_str(&row.get::<_, String>(1).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        insert_record(&tx, id, row.get(0).map_err(|e| e.to_string())?, &record)?;
        if let Some(usage) = row.get::<_, Option<String>>(3).map_err(|e| e.to_string())? {
            merge_usage(
                &tx,
                id,
                &row.get::<_, String>(2).map_err(|e| e.to_string())?,
                &serde_json::from_str(&usage).map_err(|e| e.to_string())?,
            )?;
        }
        for result in &record.results {
            if let Some(call) = &result.call_id {
                tx.execute("INSERT INTO index_result VALUES(?1,?2,?3,?4) ON CONFLICT(session_id,call_id) DO UPDATE SET result_json=?3,occurred_at=?4",
                params![id,call,serde_json::to_string(result).map_err(|e|e.to_string())?,record.occurred_at]).map_err(|e|e.to_string())?;
            }
        }
    }
    // Includes previously seen results whose calls arrived in this append.
    let mut result_stmt = stage
        .prepare("SELECT call_id FROM affected_call ORDER BY call_id")
        .map_err(|e| e.to_string())?;
    let mut results = result_stmt.query([]).map_err(|e| e.to_string())?;
    while let Some(row) = results.next().map_err(|e| e.to_string())? {
        if stop_requested() {
            return Err("Import stopped".into());
        }
        let call: String = row.get(0).map_err(|e| e.to_string())?;
        let recorded: Option<(String, Option<String>)> = tx
            .prepare_cached(
                "SELECT result_json,occurred_at FROM index_result WHERE session_id=? AND call_id=?",
            )
            .map_err(|e| e.to_string())?
            .query_row(params![id, call], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((result, occurred_at)) = recorded else {
            continue;
        };
        let result: ToolResult = serde_json::from_str(&result).map_err(|e| e.to_string())?;
        tx.prepare_cached("UPDATE tool_invocation SET result_text=?1,status=?2,exit_code=?3,ended_at=?4,
            duration_ms=CASE WHEN started_at IS NOT NULL AND ?4 IS NOT NULL THEN MAX(0,ROUND((julianday(?4)-julianday(started_at))*86400000)) ELSE duration_ms END
            WHERE session_id=?5 AND call_id=?6 AND (result_text IS NOT ?1 OR status IS NOT ?2 OR exit_code IS NOT ?3 OR ended_at IS NOT ?4)")
            .map_err(|e|e.to_string())?.execute(params![result.text,result.status,result.exit_code,occurred_at,id,call]).map_err(|e|e.to_string())?;
    }
    drop(results);
    drop(result_stmt);
    let usage = total_usage(&tx, id)?;
    let metrics = summary.entry_metrics.resolved(&tx, id)?;
    tx.execute(
        "UPDATE session SET token_usage_json=?,metrics_json=? WHERE id=?",
        params![usage.map(|u| u.to_string()), metrics.to_string(), id],
    )
    .map_err(|e| e.to_string())?;
    crate::analytics::process_session(&tx, &pricing.state, id)?;
    set_file_state(&tx, source_uri, data.size, data.mtime).map_err(|e| e.to_string())?;
    indexing::save_checkpoint(
        &tx,
        source_uri,
        id,
        &serde_json::to_string(&next).map_err(|e| e.to_string())?,
        &serde_json::to_string(&summary).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if stop_requested() {
        return Err("Import stopped".into());
    }
    tx.commit()
        .map_err(|e| format!("Commit incremental import: {e}"))?;
    crate::analytics::shared_state().clear_cache();
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{Duration, Instant};

    fn summary_record() -> NormalizedRecord {
        normalize_record("codex", &json!({
            "type":"response_item", "timestamp":"2026-11-18T10:00:00Z",
            "payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello 🌍\n\"quoted\"\u{0} tail"}]}
        }), 0).unwrap()
    }

    #[test]
    fn summary_budget_matches_serialized_bytes_and_enforces_exact_limit() {
        let mut summary = Summary::default();
        let mut budget = SummaryBudget::new(&summary).unwrap();
        assert_eq!(
            budget.check(&summary).unwrap(),
            serde_json::to_vec(&summary).unwrap().len()
        );
        let mut record = summary_record();
        record.context.cwd = Some("/workspace/日本語/\"quoted\"".into());
        for metadata in [
            json!({"id":"child","instructions":"\\\n\"🌍\u{0}"}),
            json!({}),
            json!({"model":"test","nested":{"line":"\t\r\n"}}),
        ] {
            let size = budget
                .observe(&mut summary, "codex", &metadata, &record)
                .unwrap();
            assert_eq!(size, serde_json::to_vec(&summary).unwrap().len());
            record.context.cwd = Some("/ignored/after-first-value".into());
            record.occurred_at = Some("2026-11-18T09:00:00Z".into());
        }
        summary.metadata.clear();
        summary.metadata.insert("padding".into(), json!(""));
        let base = serde_json::to_vec(&summary).unwrap().len();
        summary.metadata.insert(
            "padding".into(),
            json!("x".repeat(MAX_SUMMARY_BYTES - base)),
        );
        let budget = SummaryBudget::new(&summary).unwrap();
        assert_eq!(budget.check(&summary).unwrap(), MAX_SUMMARY_BYTES);
        assert_eq!(
            serde_json::to_vec(&summary).unwrap().len(),
            MAX_SUMMARY_BYTES
        );
        summary.metadata.insert("extra".into(), json!(true));
        assert!(matches!(SummaryBudget::new(&summary), Err(error) if error == SUMMARY_LIMIT));
        assert_eq!(
            json_bytes(&"x".repeat(MAX_SUMMARY_BYTES)).unwrap_err(),
            SUMMARY_LIMIT
        );
    }

    #[test]
    #[ignore = "release-mode performance probe; synthetic metadata only"]
    fn summary_budget_performance() {
        let record = summary_record();
        let mut results = Vec::new();
        for bytes in [4096, 32768, 1048576] {
            let mut summary = Summary::default();
            summary
                .metadata
                .insert("instructions".into(), json!("x".repeat(bytes)));
            summary.observe("codex", &Value::Null, &record).unwrap();
            let mut budget = SummaryBudget::new(&summary).unwrap();
            let mut before = Vec::new();
            let mut after = Vec::new();
            for round in 0..11 {
                // Same summary, records, serialization format, and byte limit.
                // Alternate order so the reference does not always run first.
                for cached in if round % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let start = Instant::now();
                    for _ in 0..1000 {
                        let size = if cached {
                            budget
                                .observe(&mut summary, "codex", &Value::Null, &record)
                                .unwrap()
                        } else {
                            summary.observe("codex", &Value::Null, &record).unwrap();
                            serde_json::to_vec(&summary).unwrap().len()
                        };
                        assert!(std::hint::black_box(size) <= MAX_SUMMARY_BYTES);
                    }
                    if round >= 3 {
                        if cached {
                            after.push(start.elapsed().as_secs_f64() * 1000.0)
                        } else {
                            before.push(start.elapsed().as_secs_f64() * 1000.0)
                        }
                    }
                }
            }
            assert_eq!(
                budget.check(&summary).unwrap(),
                serde_json::to_vec(&summary).unwrap().len()
            );
            results.push(json!({"metadataBytes":bytes,"recordsPerSample":1000,"beforeMs":before,"afterMs":after}));
        }
        let report = json!({"probe":"isolated exact summary-size guard; old serializer vs cached field sizes","cases":results});
        if let Ok(path) = std::env::var("HESSION_SUMMARY_BENCH_OUTPUT") {
            std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
        }
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, Connection, ImportPricing) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("source.jsonl");
        let conn = crate::db::open_db(&dir.path().join("history.sqlite")).unwrap();
        (dir, file, conn, ImportPricing::new())
    }
    fn run(
        conn: &mut Connection,
        file: &Path,
        family: &str,
        pricing: &mut ImportPricing,
    ) -> Result<usize, String> {
        // The write-priority gate is process-wide, including other tests' private
        // libraries. Large sync fixtures can legitimately occupy it past a single
        // background-write deadline on a busy host. These tests check import
        // reconciliation; dedicated gate tests cover timeout/rejection behavior.
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let result = import(
                conn,
                family,
                &format!("{family}://{}", file.display()),
                file,
                family,
                "/synthetic",
                false,
                pricing,
            );
            if matches!(&result, Err(error) if error.contains("Library write queue is busy"))
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(25));
                continue;
            }
            return result;
        }
    }
    fn append(file: &Path, value: Value) {
        writeln!(
            fs::OpenOptions::new().append(true).open(file).unwrap(),
            "{value}"
        )
        .unwrap();
    }
    fn scalar(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn incremental_metrics_match_full_sql_for_unicode_nul_and_late_tool_status_changes() {
        let (dir, file, mut conn, mut pricing) = fixture();
        fs::write(
            &file,
            format!(
                "{}\n",
                json!({"type":"session_meta","payload":{"id":"metrics"}})
            ),
        )
        .unwrap();
        run(&mut conn, &file, "codex", &mut pricing).unwrap();
        for record in [
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"界🦀\u{0000}ignored by SQLite length"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"A Unicode reply élan"}]}}),
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"late","name":"Read","arguments":"{}"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"late","exit_code":2,"output":"failed 🦀"}}),
            json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"late","exit_code":0,"output":"recovered 界"}}),
        ] {
            append(&file, record);
            assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 1);
            let stored: String = conn
                .query_row("SELECT metrics_json FROM session", [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&stored).unwrap(),
                formats::session_metrics(&conn, scalar(&conn, "SELECT id FROM session")).unwrap()
            );
            drop(conn);
            conn = crate::db::open_db(&dir.path().join("history.sqlite")).unwrap();
        }
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM tool_invocation WHERE status='completed' AND exit_code=0"
            ),
            1
        );
    }

    #[test]
    fn corrupt_summary_rebuilds_automatically_without_losing_annotations() {
        let (_dir, file, mut conn, mut pricing) = fixture();
        fs::write(
            &file,
            include_str!("../tests/fixtures/codex-concurrency.jsonl"),
        )
        .unwrap();
        run(&mut conn, &file, "codex", &mut pricing).unwrap();
        let id = scalar(&conn, "SELECT id FROM session");
        conn.execute(
            "INSERT INTO session_annotation(session_id,starred) VALUES(?,1)",
            [id],
        )
        .unwrap();
        for raw in ["not-json", "{}"] {
            conn.execute("UPDATE index_checkpoint SET summary_json=?", [raw])
                .unwrap();
            assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 4);
            assert_eq!(scalar(&conn, "SELECT id FROM session"), id);
            assert_eq!(scalar(&conn, "SELECT starred FROM session_annotation"), 1);
            assert_eq!(scalar(&conn, "SELECT count(*) FROM entry"), 4);
            assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 0);
        }
    }

    #[test]
    fn codex_fork_keeps_own_metadata_across_inherited_headers_and_restart() {
        let (dir, file, mut conn, mut pricing) = fixture();
        let child = json!({"type":"session_meta","timestamp":"2026-11-17T11:00:00Z","payload":{
            "id":"child","timestamp":"2026-11-17T11:00:00Z","cwd":"/child",
            "source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}},
            "forked_from_id":"parent","agent_nickname":"Ada"
        }});
        let parent = json!({"type":"session_meta","timestamp":"2026-11-17T10:00:00Z","payload":{
            "id":"parent","timestamp":"2026-11-17T10:00:00Z","cwd":"/parent","source":"cli"
        }});
        fs::write(&file, format!("{child}\n{parent}\n")).unwrap();
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 2);
        let session = scalar(&conn, "SELECT id FROM session");
        let check = |conn: &Connection| {
            let (id, start, cwd, raw): (String, String, String, String) = conn
                .query_row(
                    "SELECT external_id,started_at,cwd,raw_metadata_json FROM session",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap();
            assert_eq!(id, "child");
            assert_eq!(start, "2026-11-17T11:00:00.000Z");
            assert_eq!(cwd, "/child");
            let meta: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(meta["source"], child["payload"]["source"]);
            assert_eq!(meta["agent_nickname"], "Ada");
        };
        check(&conn);
        let data = formats::session_data(
            "codex",
            "test://fork",
            &file,
            vec![child.clone(), parent.clone()],
            0,
            0.0,
        )
        .unwrap();
        assert_eq!(data.metadata["id"], "child");
        assert_eq!(data.metadata["cwd"], "/child");
        assert_eq!(data.records.len(), 2);

        conn.execute(
            "INSERT INTO session_annotation(session_id,starred) VALUES(?,1)",
            [session],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_tag(session_id,label) VALUES(?,'retained')",
            [session],
        )
        .unwrap();
        drop(conn);
        append(&file, parent);
        let mut conn = crate::db::open_db(&dir.path().join("history.sqlite")).unwrap();
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 1);
        check(&conn);
        assert_eq!(scalar(&conn, "SELECT id FROM session"), session);
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM entry"), 3);
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 0);

        // Simulate a library indexed by the old parser. A quiet source must be
        // rebuilt on upgrade, correcting metadata without losing annotations.
        conn.execute("UPDATE session SET external_id='parent'", [])
            .unwrap();
        conn.execute("UPDATE index_checkpoint SET cursor_json=json_set(cursor_json,'$.parserVersion',1),invalidated=0", []).unwrap();
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 3);
        check(&conn);
        assert_eq!(scalar(&conn, "SELECT id FROM session"), session);
        assert_eq!(scalar(&conn, "SELECT starred FROM session_annotation"), 1);
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM session_tag WHERE label='retained'"
            ),
            1
        );
    }

    #[test]
    fn codex_append_preserves_entry_ids_and_late_results_and_fts() {
        let (_dir, file, mut conn, mut pricing) = fixture();
        fs::write(
            &file,
            include_str!("../tests/fixtures/codex-concurrency.jsonl"),
        )
        .unwrap();
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 4);
        let first = scalar(&conn, "SELECT id FROM entry WHERE ordinal=0");
        let session = scalar(&conn, "SELECT id FROM session");
        conn.execute(
            "INSERT INTO session_annotation(session_id,starred) VALUES(?,1)",
            [session],
        )
        .unwrap();
        append(
            &file,
            json!({"timestamp":"2026-11-17T10:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"fixture-call","output":"lateneedle"}}),
        );
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 1);
        assert_eq!(scalar(&conn, "SELECT id FROM entry WHERE ordinal=0"), first);
        assert_eq!(scalar(&conn, "SELECT count(*) FROM entry"), 5);
        assert_eq!(scalar(&conn, "SELECT starred FROM session_annotation"), 1);
        assert_eq!(
            scalar(
                &conn,
                "SELECT count(*) FROM tool_invocation WHERE result_text='lateneedle'"
            ),
            1
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT count(*) FROM entry_fts WHERE entry_fts MATCH 'lateneedle'"
            ),
            1
        );
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 0);
        assert_eq!(
            scalar(
                &conn,
                "SELECT count(*) FROM index_checkpoint WHERE invalidated=0"
            ),
            1
        );
        assert_eq!(
            conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
    }

    #[test]
    fn claude_usage_deduplicates_repeated_messages_across_restart_and_keeps_zero() {
        let (_dir, file, mut conn, mut pricing) = fixture();
        let line = |id: &str, input: i64, output: i64| json!({"type":"assistant","sessionId":"native","timestamp":"2026-11-17T10:00:00Z","message":{"id":id,"role":"assistant","content":"hello","usage":{"input_tokens":input,"output_tokens":output,"cache_read_input_tokens":0}}});
        fs::write(&file, format!("{}\n", line("a", 10, 2))).unwrap();
        run(&mut conn, &file, "claude", &mut pricing).unwrap();
        append(&file, line("a", 10, 5));
        append(&file, line("b", 20, 3));
        drop(conn);
        let mut conn = crate::db::open_db(&_dir.path().join("history.sqlite")).unwrap();
        assert_eq!(run(&mut conn, &file, "claude", &mut pricing).unwrap(), 2);
        let usage: String = conn
            .query_row("SELECT token_usage_json FROM session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&usage).unwrap(),
            json!({"inputTokens":30,"outputTokens":8,"cachedTokens":0})
        );
        let values = parse_jsonl_file(&file).unwrap();
        let expected =
            formats::session_data("claude", "test://parity", &file, values, 0, 0.0).unwrap();
        assert_eq!(expected.usage, Some(serde_json::from_str(&usage).unwrap()));
    }

    #[test]
    fn partial_tail_and_failed_append_do_not_advance_checkpoint_or_lose_history() {
        let (_dir, file, mut conn, mut pricing) = fixture();
        fs::write(
            &file,
            include_str!("../tests/fixtures/codex-concurrency.jsonl"),
        )
        .unwrap();
        run(&mut conn, &file, "codex", &mut pricing).unwrap();
        let checkpoint: String = conn
            .query_row("SELECT cursor_json FROM index_checkpoint", [], |r| r.get(0))
            .unwrap();
        let mut stream = fs::OpenOptions::new().append(true).open(&file).unwrap();
        stream.write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"pending").unwrap();
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 0);
        assert_eq!(scalar(&conn, "SELECT count(*) FROM entry"), 4);
        stream.write_all(b" completed\"}}\n").unwrap();
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 1);
        let committed: String = conn
            .query_row("SELECT cursor_json FROM index_checkpoint", [], |r| r.get(0))
            .unwrap();
        assert_ne!(checkpoint, committed);
        stream.write_all(b"{bad}\n").unwrap();
        assert!(run(&mut conn, &file, "codex", &mut pricing).is_err());
        assert_eq!(scalar(&conn, "SELECT count(*) FROM entry"), 5);
        assert_eq!(
            conn.query_row("SELECT cursor_json FROM index_checkpoint", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            committed
        );
    }

    #[test]
    fn result_before_call_rewrite_and_external_edit_are_reconciled() {
        let (_dir, file, mut conn, mut pricing) = fixture();
        fs::write(&file,format!("{}\n",json!({"type":"response_item","payload":{"type":"function_call_output","call_id":"future","output":"alreadyseen"}}))).unwrap();
        run(&mut conn, &file, "codex", &mut pricing).unwrap();
        append(
            &file,
            json!({"type":"response_item","payload":{"type":"function_call","call_id":"future","name":"shell_command","arguments":"{}"}}),
        );
        run(&mut conn, &file, "codex", &mut pricing).unwrap();
        assert_eq!(
            scalar(
                &conn,
                "SELECT count(*) FROM tool_invocation WHERE result_text='alreadyseen'"
            ),
            1
        );
        conn.execute(
            "UPDATE entry SET search_text='external edit' WHERE ordinal=0",
            [],
        )
        .unwrap();
        assert_eq!(scalar(&conn, "SELECT invalidated FROM index_checkpoint"), 1);
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 2);
        assert_eq!(
            scalar(
                &conn,
                "SELECT count(*) FROM entry WHERE search_text='external edit'"
            ),
            0
        );
        let id = scalar(&conn, "SELECT id FROM session");
        fs::write(
            &file,
            include_str!("../tests/fixtures/codex-concurrency.jsonl"),
        )
        .unwrap();
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 4);
        assert_eq!(scalar(&conn, "SELECT id FROM session"), id);
        assert_eq!(
            scalar(
                &conn,
                "SELECT count(*) FROM tool_invocation WHERE call_id='future'"
            ),
            0
        );
    }

    #[test]
    fn checkpoint_failure_rolls_back_entries_tool_results_analytics_and_source_state() {
        let (_dir, file, mut conn, mut pricing) = fixture();
        fs::write(
            &file,
            include_str!("../tests/fixtures/codex-concurrency.jsonl"),
        )
        .unwrap();
        run(&mut conn, &file, "codex", &mut pricing).unwrap();
        let checkpoint: String = conn
            .query_row("SELECT cursor_json FROM index_checkpoint", [], |r| r.get(0))
            .unwrap();
        conn.execute_batch("CREATE TRIGGER fail_checkpoint BEFORE UPDATE ON index_checkpoint WHEN NEW.invalidated=0 BEGIN SELECT RAISE(ABORT,'fixture failure'); END;").unwrap();
        append(
            &file,
            json!({"timestamp":"2026-11-17T10:00:04Z","type":"response_item","payload":{"type":"function_call_output","call_id":"fixture-call","output":"rollbackneedle"}}),
        );
        assert!(run(&mut conn, &file, "codex", &mut pricing)
            .unwrap_err()
            .contains("fixture failure"));
        assert_eq!(scalar(&conn, "SELECT count(*) FROM entry"), 4);
        assert_eq!(
            scalar(
                &conn,
                "SELECT count(*) FROM entry_fts WHERE entry_fts MATCH 'rollbackneedle'"
            ),
            0
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT count(*) FROM tool_invocation WHERE result_text='rollbackneedle'"
            ),
            0
        );
        assert_eq!(
            conn.query_row("SELECT cursor_json FROM index_checkpoint", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            checkpoint
        );
        assert_eq!(scalar(&conn, "SELECT invalidated FROM index_checkpoint"), 0);
        conn.execute_batch("DROP TRIGGER fail_checkpoint").unwrap();
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 1);
    }

    #[test]
    fn automatic_exclusions_stale_jobs_and_corrupt_counts_recover_without_native_writes() {
        let (_dir, file, mut conn, mut pricing) = fixture();
        let initial = include_str!("../tests/fixtures/codex-concurrency.jsonl");
        fs::write(&file, initial).unwrap();
        let uri = format!("codex://{}", file.display());
        indexing::exclude(&conn, &uri).unwrap();
        pricing.automatic = true;
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 0);
        assert_eq!(scalar(&conn, "SELECT count(*) FROM session"), 0);
        pricing.automatic = false;
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 4);
        assert!(!indexing::excluded(&conn, &uri).unwrap());
        pricing.epoch = Some(indexing::epoch(&conn).unwrap());
        indexing::maintenance(&conn).unwrap();
        append(
            &file,
            json!({"type":"event_msg","payload":{"type":"user_message","message":"new work"}}),
        );
        assert!(run(&mut conn, &file, "codex", &mut pricing)
            .unwrap_err()
            .contains("Saved history changed"));
        assert_eq!(scalar(&conn, "SELECT count(*) FROM entry"), 4);
        pricing.epoch = None;
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 5);
        conn.execute(
            "UPDATE index_checkpoint SET cursor_json=json_set(cursor_json,'$.records',1)",
            [],
        )
        .unwrap();
        assert_eq!(run(&mut conn, &file, "codex", &mut pricing).unwrap(), 5);
        assert_eq!(scalar(&conn, "SELECT count(*) FROM entry"), 5);
        assert_eq!(scalar(&conn, "SELECT user_turns FROM session_analytics"), 2);
        let raw: String = conn
            .query_row("SELECT raw_metadata_json FROM session", [], |r| r.get(0))
            .unwrap();
        assert!(serde_json::from_str::<Value>(&raw)
            .unwrap()
            .get("started_at")
            .is_none());
        assert!(fs::read_to_string(&file).unwrap().starts_with(initial));
    }
}
