//! Portable SQLite backups and transactional library maintenance.
//! Incoming databases are read as data only; their SQL, triggers and IDs are never installed.
use crate::{db, sync, AppState};
use rusqlite::{
    params, params_from_iter, types::Value, Connection, OpenFlags, OptionalExtension,
    TransactionBehavior,
};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use std::{
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::Duration,
};
use tauri::State;

const BACKUP_ID: i64 = 0x53424f58; // SBOX
const BACKUP_VERSION: i64 = 1;
use crate::library_revision::{DATA_TABLES as TABLES, SETTINGS_TABLES};
type Row = HashMap<String, Value>;
type IdMaps = HashMap<&'static str, HashMap<i64, i64>>;

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct DataFilters {
    query: String,
    agent: String,
    workspace: String,
    since: String,
    until: String,
    starred: bool,
    tag: String,
}

impl DataFilters {
    fn is_empty(&self) -> bool {
        self.query.is_empty()
            && self.agent.is_empty()
            && self.workspace.is_empty()
            && self.since.is_empty()
            && self.until.is_empty()
            && !self.starred
            && self.tag.is_empty()
    }
}

fn path_of(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .db_path
        .lock()
        .map(|p| p.clone())
        .map_err(|_| "Could not access the database path".into())
}
fn open(path: &Path) -> Result<Connection, String> {
    let conn = db::open_db(path).map_err(|e| e.to_string())?;
    crate::library_revision::mark_managed(&conn).map_err(|e| e.to_string())?;
    Ok(conn)
}
fn sql<T>(result: rusqlite::Result<T>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<String>, String> {
    let mut stmt = sql(conn.prepare(&format!("PRAGMA table_info(\"{table}\")")))?;
    let rows = sql(stmt.query_map([], |r| r.get(1)))?;
    sql(rows.collect())
}
fn rows(conn: &Connection, table: &str, condition: &str) -> Result<Vec<Row>, String> {
    let mut stmt = sql(conn.prepare(&format!("SELECT * FROM \"{table}\" {condition}")))?;
    let names: Vec<String> = stmt.column_names().iter().map(|n| n.to_string()).collect();
    let mapped = sql(stmt.query_map([], |r| {
        names
            .iter()
            .enumerate()
            .map(|(i, name)| Ok((name.clone(), r.get(i)?)))
            .collect()
    }))?;
    sql(mapped.collect())
}
fn integer(row: &Row, key: &str) -> Result<i64, String> {
    match row.get(key) {
        Some(Value::Integer(n)) => Ok(*n),
        _ => Err(format!("Invalid {key} in backup")),
    }
}
fn text<'a>(row: &'a Row, key: &str) -> Result<&'a str, String> {
    match row.get(key) {
        Some(Value::Text(s)) => Ok(s),
        _ => Err(format!("Invalid {key} in backup")),
    }
}
fn insert(conn: &Connection, table: &str, row: &Row) -> Result<i64, String> {
    // Column names always originate from our schema validation, never user SQL.
    let mut names: Vec<&String> = row.keys().collect();
    names.sort();
    let fields = names
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(",");
    let placeholders = vec!["?"; names.len()].join(",");
    sql(conn.execute(
        &format!("INSERT INTO \"{table}\" ({fields}) VALUES ({placeholders})"),
        params_from_iter(names.iter().map(|n| &row[*n])),
    ))?;
    Ok(conn.last_insert_rowid())
}

fn selection(conn: &Connection, f: &DataFilters) -> Result<Vec<i64>, String> {
    let parse_boundary = |value: &str| {
        if value.is_empty() {
            Ok(None)
        } else {
            chrono::DateTime::parse_from_rfc3339(value)
                .map(Some)
                .map_err(|_| "Invalid date filter")
        }
    };
    if let (Some(since), Some(until)) = (parse_boundary(&f.since)?, parse_boundary(&f.until)?) {
        if since >= until {
            return Err("The end date must be on or after the start date.".into());
        }
    }
    let mut conditions = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    for (value, condition) in [
        (&f.agent, "a.family=?"),
        (&f.workspace, "w.path=?"),
        (
            &f.since,
            "julianday(COALESCE(s.last_event_at,s.started_at))>=julianday(?)",
        ),
        (
            &f.until,
            "julianday(COALESCE(s.last_event_at,s.started_at))<julianday(?)",
        ),
        (
            &f.tag,
            "EXISTS(SELECT 1 FROM session_tag t WHERE t.session_id=s.id AND t.label=?)",
        ),
    ] {
        if !value.is_empty() {
            conditions.push(condition);
            values.push(Value::Text(value.clone()));
        }
    }
    if f.starred {
        conditions.push(
            "EXISTS(SELECT 1 FROM session_annotation n WHERE n.session_id=s.id AND n.starred=1)",
        );
    }
    if !f.query.trim().is_empty() {
        conditions.push("(s.title LIKE ? ESCAPE '\\' OR s.external_id LIKE ? ESCAPE '\\' OR EXISTS(SELECT 1 FROM entry e WHERE e.session_id=s.id AND e.search_text LIKE ? ESCAPE '\\'))");
        let escaped = f
            .query
            .trim()
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        values.extend(std::iter::repeat_n(Value::Text(format!("%{escaped}%")), 3));
    }
    let condition = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };
    let mut stmt = sql(conn.prepare(&format!("SELECT s.id FROM session s JOIN agent a ON a.id=s.agent_id LEFT JOIN workspace w ON w.id=s.workspace_id {condition} ORDER BY s.id")))?;
    let results = sql(stmt.query_map(params_from_iter(values), |r| r.get(0)))?;
    sql(results.collect())
}
fn select_ids(conn: &Connection, ids: &[i64]) -> Result<(), String> {
    sql(conn.execute_batch("CREATE TEMP TABLE selected_session(id INTEGER PRIMARY KEY)"))?;
    let mut stmt = sql(conn.prepare("INSERT INTO selected_session VALUES (?)"))?;
    for id in ids {
        sql(stmt.execute([id]))?;
    }
    Ok(())
}
fn token(conn: &Connection, ids: &[i64]) -> Result<String, String> {
    if let Some(token) = sql(crate::library_revision::managed_token(conn, ids))? {
        return Ok(token);
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    ids.hash(&mut hash);
    // Hash logical input data in a stable order within the caller's snapshot.
    // Counts/max IDs miss edits to transcripts, dimensions, tags and prices.
    // Exclude derived analytics: its background refresh does not change what
    // the user is exporting, importing or deleting. Stream rows to bound RAM.
    for table in TABLES.iter().chain(SETTINGS_TABLES) {
        table.hash(&mut hash);
        let mut info = sql(conn.prepare(&format!("PRAGMA table_info(\"{table}\")")))?;
        let keys = sql(info.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(5)?))))?;
        let mut keys: Vec<(String, i64)> = sql(keys.collect())?;
        keys.retain(|(_, order)| *order > 0);
        keys.sort_by_key(|(_, order)| *order);
        if keys.is_empty() {
            return Err(format!("Missing primary key in {table}"));
        }
        let order = keys
            .iter()
            .map(|(name, _)| format!("\"{}\"", name.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(",");
        let mut stmt = sql(conn.prepare(&format!("SELECT * FROM \"{table}\" ORDER BY {order}")))?;
        let names = stmt.column_names();
        names.hash(&mut hash);
        let count = stmt.column_count();
        let mut rows = sql(stmt.query([]))?;
        while let Some(row) = sql(rows.next())? {
            0xffu8.hash(&mut hash);
            for column in 0..count {
                use rusqlite::types::ValueRef;
                match sql(row.get_ref(column))? {
                    ValueRef::Null => 0u8.hash(&mut hash),
                    ValueRef::Integer(n) => {
                        1u8.hash(&mut hash);
                        n.hash(&mut hash);
                    }
                    ValueRef::Real(n) => {
                        2u8.hash(&mut hash);
                        n.to_bits().hash(&mut hash);
                    }
                    ValueRef::Text(s) => {
                        3u8.hash(&mut hash);
                        s.hash(&mut hash);
                    }
                    ValueRef::Blob(b) => {
                        4u8.hash(&mut hash);
                        b.hash(&mut hash);
                    }
                }
            }
        }
    }
    Ok(format!("{:016x}", hash.finish()))
}
fn verify_token(actual: &str, expected: &str) -> Result<(), String> {
    if actual != expected {
        Err("The data changed after the preview. Refresh the preview and try again.".into())
    } else {
        Ok(())
    }
}
fn summary_counts(conn: &Connection, ids: &[i64]) -> Result<Json, String> {
    let entries: i64 = sql(conn.query_row(
        "SELECT COUNT(*) FROM entry WHERE session_id IN (SELECT id FROM selected_session)",
        [],
        |r| r.get(0),
    ))?;
    let tools: i64 = sql(conn.query_row("SELECT COUNT(*) FROM tool_invocation WHERE session_id IN (SELECT id FROM selected_session)", [], |r| r.get(0)))?;
    Ok(json!({"sessions":ids.len(), "entries":entries, "tools":tools}))
}
fn summary(conn: &Connection, ids: &[i64]) -> Result<Json, String> {
    let mut result = summary_counts(conn, ids)?;
    result["token"] = json!(token(conn, ids)?);
    Ok(result)
}
fn options(conn: &Connection) -> Result<Json, String> {
    let strings = |query: &str| -> Result<Vec<String>, String> {
        let mut stmt = sql(conn.prepare(query))?;
        let values = sql(stmt.query_map([], |r| r.get(0)))?;
        sql(values.collect())
    };
    Ok(json!({
        "agents": strings("SELECT DISTINCT a.family FROM agent a JOIN session s ON s.agent_id=a.id ORDER BY a.family")?,
        "workspaces": strings("SELECT DISTINCT w.path FROM workspace w JOIN session s ON s.workspace_id=w.id ORDER BY w.path")?,
        "tags": strings("SELECT DISTINCT label FROM session_tag ORDER BY label COLLATE NOCASE")?
    }))
}
fn prune(conn: &Connection) -> Result<(), String> {
    sql(conn.execute_batch("DELETE FROM sync_state WHERE source_uri NOT IN (SELECT source_uri FROM session);
        DELETE FROM agent WHERE id NOT IN (SELECT agent_id FROM session);
        DELETE FROM workspace WHERE id NOT IN (SELECT workspace_id FROM session WHERE workspace_id IS NOT NULL);
        DELETE FROM model WHERE id NOT IN (SELECT model_id FROM session_context WHERE model_id IS NOT NULL);
        DELETE FROM repository WHERE id NOT IN (SELECT repository_id FROM session_context WHERE repository_id IS NOT NULL);"))
}
fn different_file(file: &Path, database: &Path) -> Result<(), String> {
    let existing_db = std::fs::canonicalize(database).map_err(|e| e.to_string())?;
    let candidate = if file.exists() {
        std::fs::canonicalize(file).map_err(|e| e.to_string())?
    } else {
        std::fs::canonicalize(file.parent().ok_or("Invalid file path")?)
            .map_err(|e| e.to_string())?
            .join(file.file_name().ok_or("Invalid file path")?)
    };
    if candidate == existing_db
        || candidate.as_os_str() == format!("{}-wal", existing_db.display()).as_str()
        || candidate.as_os_str() == format!("{}-shm", existing_db.display()).as_str()
    {
        return Err(
            "Choose a backup file separate from the active database and its journal files.".into(),
        );
    }
    Ok(())
}
fn open_backup(path: &Path, destination: &Connection) -> Result<Connection, String> {
    let source = sql(Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ))?;
    sql(source.busy_timeout(Duration::from_secs(5)))?;
    // READ_ONLY protects the source database while permitting our own TEMP tables.
    sql(source.execute_batch("PRAGMA trusted_schema=OFF; BEGIN"))?;
    let app_id: i64 = sql(source.query_row("PRAGMA application_id", [], |r| r.get(0)))?;
    let version: i64 = sql(source.query_row("PRAGMA user_version", [], |r| r.get(0)))?;
    if app_id != BACKUP_ID || version != BACKUP_VERSION {
        return Err(format!(
            "Choose a SQLite backup created by {}’s Manage saved data export.",
            crate::brand::NAME
        ));
    }
    for table in TABLES
        .iter()
        .copied()
        .chain(SETTINGS_TABLES.iter().copied())
    {
        let kind: Option<String> = sql(source
            .query_row(
                "SELECT type FROM sqlite_schema WHERE name=?",
                [table],
                |r| r.get(0),
            )
            .optional())?;
        if kind.as_deref() != Some("table")
            || columns(&source, table)? != columns(destination, table)?
        {
            return Err(format!(
                "The backup has an incompatible {table} table. No data was changed."
            ));
        }
    }
    let check: String = sql(source.query_row("PRAGMA quick_check", [], |r| r.get(0)))?;
    if check != "ok" {
        return Err(format!("The backup failed its integrity check: {check}"));
    }
    let invalid: bool = sql(source.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check)",
        [],
        |r| r.get(0),
    ))?;
    if invalid {
        return Err("The backup contains broken data references. No data was changed.".into());
    }
    Ok(source)
}

#[tauri::command]
pub async fn preview_saved_data(
    state: State<'_, AppState>,
    filters: DataFilters,
) -> Result<Json, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut conn = open(&path)?;
        let tx = sql(conn.transaction())?;
        let ids = selection(&tx, &filters)?;
        select_ids(&tx, &ids)?;
        let mut result = summary(&tx, &ids)?;
        result["options"] = options(&tx)?;
        result["databasePath"] = json!(path);
        result["databaseBytes"] = json!(
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
                + std::fs::metadata(format!("{}-wal", path.display()))
                    .map(|m| m.len())
                    .unwrap_or(0)
        );
        result["totalSessions"] = json!(sql(tx.query_row(
            "SELECT COUNT(*) FROM session",
            [],
            |r| r.get::<_, i64>(0)
        ))?);
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn export_saved_data(
    state: State<'_, AppState>,
    filters: DataFilters,
    path: String,
    expected_token: String,
) -> Result<Json, String> {
    let database = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = sync::acquire_data_management()?;
        let _writer = crate::db::user_write();
        let conn = open(&database)?;
        let destination = PathBuf::from(path);
        different_file(&destination, &database)?;
        let parent = destination.parent().ok_or("Invalid backup path")?;
        let temp = tempfile::Builder::new()
            .prefix(&format!(".{}-backup-", crate::brand::SLUG))
            .suffix(".sqlite")
            .tempfile_in(parent)
            .map_err(|e| e.to_string())?
            .into_temp_path();
        sql(conn.backup(rusqlite::MAIN_DB, &temp, None))?;
        let mut backup = sql(Connection::open(&temp))?;
        sql(backup.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL",
        ))?;
        let tx = sql(backup.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let ids = selection(&tx, &filters)?;
        let actual = if expected_token.starts_with("r1:") {
            sql(crate::library_revision::snapshot_token(&tx, &ids))?
                .ok_or("The library revision changed. Refresh the preview.")?
        } else {
            token(&tx, &ids)?
        };
        verify_token(&actual, &expected_token)?;
        select_ids(&tx, &ids)?;
        let total: i64 = sql(tx.query_row("SELECT COUNT(*) FROM session", [], |r| r.get(0)))?;
        sql(tx.execute(
            "DELETE FROM session WHERE id NOT IN (SELECT id FROM selected_session)",
            [],
        ))?;
        if ids.len() as i64 != total {
            sql(tx.execute("DELETE FROM sync_checkpoint", []))?;
            // Remove excluded text from FTS segments as well as normal tables.
            sql(tx.execute("INSERT INTO entry_fts(entry_fts) VALUES ('rebuild')", []))?;
        }
        prune(&tx)?;
        sql(tx.pragma_update(None, "application_id", BACKUP_ID))?;
        sql(tx.pragma_update(None, "user_version", BACKUP_VERSION))?;
        sql(tx.commit())?;
        sql(backup.execute_batch("DROP TABLE selected_session; VACUUM"))?;
        drop(backup);
        std::fs::File::open(&temp)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        temp.persist(&destination)
            .map_err(|e| format!("Could not save backup: {e}"))?;
        Ok(json!({"sessions":ids.len(),"path":destination}))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Source URI is authoritative. Portable copies can also match by family and external ID.
fn matching_session(
    destination: &Connection,
    source: &Connection,
    row: &Row,
) -> Result<Option<i64>, String> {
    let by_uri = sql(destination
        .query_row(
            "SELECT id FROM session WHERE source_uri=?",
            [text(row, "source_uri")?],
            |r| r.get(0),
        )
        .optional())?;
    if by_uri.is_some() {
        return Ok(by_uri);
    }
    let external = match row.get("external_id") {
        Some(Value::Text(s)) if !s.trim().is_empty() => s,
        _ => return Ok(None),
    };
    let family: String = sql(source.query_row(
        "SELECT family FROM agent WHERE id=?",
        [integer(row, "agent_id")?],
        |r| r.get(0),
    ))?;
    let mut stmt = sql(destination.prepare("SELECT s.id FROM session s JOIN agent a ON a.id=s.agent_id WHERE a.family=? AND s.external_id=? ORDER BY s.id"))?;
    let results = sql(stmt.query_map(params![family, external], |r| r.get::<_, i64>(0)))?;
    let matches: Vec<i64> = sql(results.collect())?;
    match matches.as_slice() {
        [] => Ok(None), [id] => Ok(Some(*id)), _ => Err(format!("More than one saved session matches {external}. Resolve the duplicate before importing.")),
    }
}
/// Incoming rows paired with the saved session they replace, plus a token
/// that detects changes between preview and import.
type ImportPlan = (Vec<(Row, Option<i64>)>, String);

fn import_plan(
    destination: &Connection,
    source: &Connection,
    ids: &[i64],
) -> Result<ImportPlan, String> {
    let selected = rows(
        source,
        "session",
        "WHERE id IN (SELECT id FROM selected_session) ORDER BY id",
    )?;
    let mut plan = Vec::new();
    let mut seen = HashSet::new();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    token(source, ids)?.hash(&mut hash);
    token(destination, &[])?.hash(&mut hash);
    for row in selected {
        let matched = matching_session(destination, source, &row)?;
        if let Some(id) = matched {
            if !seen.insert(id) {
                return Err("Multiple incoming sessions match the same saved session. Import them separately.".into());
            }
        }
        matched.hash(&mut hash);
        text(&row, "source_uri")?.hash(&mut hash);
        plan.push((row, matched));
    }
    Ok((plan, format!("{:016x}", hash.finish())))
}

#[tauri::command]
pub async fn preview_data_import(
    state: State<'_, AppState>,
    path: String,
    filters: DataFilters,
) -> Result<Json, String> {
    let database = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut destination = open(&database)?;
        different_file(Path::new(&path), &database)?;
        let tx = sql(destination.transaction())?;
        let source = open_backup(Path::new(&path), &tx)?;
        let ids = selection(&source, &filters)?;
        select_ids(&source, &ids)?;
        let (plan, import_token) = import_plan(&tx, &source, &ids)?;
        let matches = plan.iter().filter(|(_, id)| id.is_some()).count();
        let mut result = summary_counts(&source, &ids)?;
        result["token"] = json!(import_token);
        result["matchingSessions"] = json!(matches);
        result["newSessions"] = json!(ids.len() - matches);
        result["options"] = options(&source)?;
        result["pricingOverrides"] = json!(sql(source.query_row(
            "SELECT COUNT(*) FROM model_price_overrides",
            [],
            |r| r.get::<_, i64>(0)
        ))?);
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Reuse dimension rows by their natural keys, including SQL NULL equivalence.
fn copy_dimensions(destination: &Connection, source: &Connection) -> Result<IdMaps, String> {
    let mut maps = HashMap::new();
    for (table, keys, condition) in [
        ("agent", vec!["family", "install_root"], "WHERE id IN (SELECT agent_id FROM session WHERE id IN (SELECT id FROM selected_session))"),
        ("workspace", vec!["path"], "WHERE id IN (SELECT workspace_id FROM session WHERE id IN (SELECT id FROM selected_session))"),
        ("model", vec!["provider", "model_key"], "WHERE id IN (SELECT model_id FROM session_context WHERE session_id IN (SELECT id FROM selected_session))"),
        ("repository", vec!["common_dir", "remote_url"], "WHERE id IN (SELECT repository_id FROM session_context WHERE session_id IN (SELECT id FROM selected_session))"),
    ] {
        let mut map = HashMap::new();
        for mut row in rows(source, table, condition)? {
            let old = integer(&row, "id")?;
            let predicate = keys.iter().map(|k| format!("\"{k}\" IS ?")).collect::<Vec<_>>().join(" AND ");
            let existing: Option<i64> = sql(destination.query_row(&format!("SELECT id FROM {table} WHERE {predicate} ORDER BY id LIMIT 1"), params_from_iter(keys.iter().map(|k| &row[*k])), |r| r.get(0)).optional())?;
            row.remove("id");
            let new = match existing { Some(id) => id, None => insert(destination, table, &row)? };
            map.insert(old, new);
        }
        maps.insert(table, map);
    }
    Ok(maps)
}
fn remap(row: &mut Row, key: &str, table: &'static str, maps: &IdMaps) -> Result<(), String> {
    if matches!(row.get(key), Some(Value::Null)) {
        return Ok(());
    }
    let old = integer(row, key)?;
    let new = maps
        .get(table)
        .and_then(|m| m.get(&old))
        .ok_or_else(|| format!("Invalid {table} reference in backup"))?;
    row.insert(key.into(), Value::Integer(*new));
    Ok(())
}

#[tauri::command]
pub async fn import_saved_data(
    state: State<'_, AppState>,
    path: String,
    filters: DataFilters,
    mode: String,
    include_pricing: bool,
    expected_token: String,
) -> Result<Json, String> {
    let database = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = sync::acquire_data_management()?;
        let _writer = crate::db::user_write();
        if mode != "keep" && mode != "replace" { return Err("Invalid import mode".into()); }
        let mut destination = open(&database)?;
        different_file(Path::new(&path), &database)?;
        let tx = sql(destination.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let source = open_backup(Path::new(&path), &tx)?;
        let ids = selection(&source, &filters)?;
        select_ids(&source, &ids)?;
        let (plan, import_token) = import_plan(&tx, &source, &ids)?;
        verify_token(&import_token, &expected_token)?;
        let mut inserted = 0;
        let mut replaced = 0;
        let mut skipped = 0;
        // Only copy dimensions belonging to sessions that will actually be imported.
        for (row, matched) in &plan {
            if mode == "keep" && matched.is_some() { sql(source.execute("DELETE FROM selected_session WHERE id=?", [integer(row, "id")?]))?; }
        }
        let mut maps = copy_dimensions(&tx, &source)?;
        let mut session_map = HashMap::new();
        let mut changed_families = HashSet::new();
        for (mut row, matched) in plan {
            if mode == "keep" && matched.is_some() { skipped += 1; continue; }
            let old = integer(&row, "id")?;
            if let Some(id) = matched {
                let (old_uri, old_family): (String, String) = sql(tx.query_row("SELECT s.source_uri,a.family FROM session s JOIN agent a ON a.id=s.agent_id WHERE s.id=?", [id], |r| Ok((r.get(0)?, r.get(1)?))))?;
                changed_families.insert(old_family);
                sql(tx.execute("DELETE FROM sync_state WHERE source_uri=?", [old_uri]))?;
                sql(tx.execute("DELETE FROM session WHERE id=?", [id]))?;
                replaced += 1;
            } else { inserted += 1; }
            remap(&mut row, "agent_id", "agent", &maps)?;
            remap(&mut row, "workspace_id", "workspace", &maps)?;
            let family: String = sql(tx.query_row("SELECT family FROM agent WHERE id=?", [integer(&row, "agent_id")?], |r| r.get(0)))?;
            changed_families.insert(family);
            // Imported checkpoints must never cause local logs to be skipped.
            sql(tx.execute("DELETE FROM sync_state WHERE source_uri=?", [text(&row, "source_uri")?]))?;
            row.remove("id");
            if let Some(id) = matched { row.insert("id".into(), Value::Integer(id)); }
            let new = insert(&tx, "session", &row)?;
            session_map.insert(old, new);
        }
        maps.insert("session", session_map);
        for (table, condition, references) in [
            ("entry", "WHERE session_id IN (SELECT id FROM selected_session) ORDER BY id", vec![("session_id", "session")]),
            ("content_part", "WHERE entry_id IN (SELECT id FROM entry WHERE session_id IN (SELECT id FROM selected_session)) ORDER BY id", vec![("entry_id", "entry")]),
            ("session_context", "WHERE session_id IN (SELECT id FROM selected_session) ORDER BY id", vec![("session_id", "session"), ("entry_id", "entry"), ("repository_id", "repository"), ("model_id", "model")]),
            ("tool_invocation", "WHERE session_id IN (SELECT id FROM selected_session) ORDER BY id", vec![("session_id", "session"), ("entry_id", "entry")]),
            ("file_activity", "WHERE session_id IN (SELECT id FROM selected_session) ORDER BY id", vec![("session_id", "session"), ("entry_id", "entry"), ("tool_invocation_id", "tool_invocation")]),
            ("session_annotation", "WHERE session_id IN (SELECT id FROM selected_session)", vec![("session_id", "session")]),
            ("session_tag", "WHERE session_id IN (SELECT id FROM selected_session)", vec![("session_id", "session")]),
        ] {
            let mut map = HashMap::new();
            for mut row in rows(&source, table, condition)? {
                let old = row.get("id").and_then(|v| if let Value::Integer(n) = v { Some(*n) } else { None });
                for (key, parent) in &references { remap(&mut row, key, parent, &maps)?; }
                row.remove("id");
                let new = insert(&tx, table, &row)?;
                if let Some(id) = old { map.insert(id, new); }
            }
            maps.insert(table, map);
        }
        for family in changed_families { sql(tx.execute("DELETE FROM sync_checkpoint WHERE family=?", [family]))?; }
        let mut pricing_count = 0;
        if include_pricing {
            for row in rows(&source, "model_price_overrides", "")? {
                let exists: bool = sql(tx.query_row("SELECT EXISTS(SELECT 1 FROM model_price_overrides WHERE provider=? AND model_id=?)", params![text(&row, "provider")?, text(&row, "model_id")?], |r| r.get(0)))?;
                if exists && mode == "keep" { continue; }
                sql(tx.execute("DELETE FROM model_price_overrides WHERE provider=? AND model_id=?", params![text(&row, "provider")?, text(&row, "model_id")?]))?;
                insert(&tx, "model_price_overrides", &row)?;
                pricing_count += 1;
            }
            if pricing_count > 0 { crate::analytics::queue_recalculation(&tx)?; }
        }
        prune(&tx)?;
        sql(tx.commit())?;
        crate::analytics::shared_state().clear_cache();
        if pricing_count > 0 {
            crate::analytics::prices_changed();
            crate::analytics::request_refresh(&database, false);
        }
        Ok(json!({"inserted":inserted,"replaced":replaced,"skipped":skipped,"pricingOverrides":pricing_count}))
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn delete_saved_data(
    state: State<'_, AppState>,
    filters: DataFilters,
    expected_token: String,
    confirmation: String,
    reset_database: bool,
) -> Result<Json, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = sync::acquire_data_management()?;
        let _writer = crate::db::user_write();
        if confirmation != if reset_database { "RESET" } else { "DELETE" } { return Err("Type the confirmation word before deleting data.".into()); }
        let mut conn = open(&path)?;
        let tx = sql(conn.transaction_with_behavior(TransactionBehavior::Immediate))?;
        let ids = selection(&tx, &filters)?;
        verify_token(&token(&tx, &ids)?, &expected_token)?;
        select_ids(&tx, &ids)?;
        if reset_database {
            if !filters.is_empty() { return Err("Clear all filters before resetting the database.".into()); }
            let total: i64 = sql(tx.query_row("SELECT COUNT(*) FROM session", [], |r| r.get(0)))?;
            if total != ids.len() as i64 { return Err("Clear all filters before resetting the database.".into()); }
        }
        // Clearing family checkpoints allows deleted local sources to return on an explicit sync.
        sql(tx.execute("DELETE FROM sync_checkpoint WHERE family IN (SELECT a.family FROM agent a JOIN session s ON s.agent_id=a.id JOIN selected_session x ON x.id=s.id)", []))?;
        sql(tx.execute("DELETE FROM sync_state WHERE source_uri IN (SELECT source_uri FROM session WHERE id IN (SELECT id FROM selected_session))", []))?;
        sql(tx.execute("DELETE FROM session WHERE id IN (SELECT id FROM selected_session)", []))?;
        sql(tx.execute("INSERT INTO entry_fts(entry_fts) VALUES ('rebuild')", []))?;
        prune(&tx)?;
        if reset_database { sql(tx.execute_batch("DELETE FROM sync_state; DELETE FROM sync_checkpoint; DELETE FROM model_pricing_cache; DELETE FROM model_price_overrides;"))?; }
        sql(tx.commit())?;
        crate::analytics::shared_state().clear_cache();
        if reset_database { crate::analytics::prices_changed(); }
        // The deletion is already committed. Report a compaction failure as a warning, never as a failed deletion.
        let warning = conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE)").err().map(|e| format!("Data was deleted, but disk space could not be fully reclaimed: {e}"));
        Ok(json!({"deleted":ids.len(),"reset":reset_database,"warning":warning}))
    }).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session(id,agent_id,source_uri,title) VALUES(1,1,'test://1','Original')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO entry(id,session_id,ordinal,kind,search_text) VALUES(1,1,0,'message','Original text')",[]).unwrap();
        conn
    }

    #[test]
    fn incoming_backups_hash_content_even_when_revision_numbers_match() {
        let directory = tempfile::tempdir().unwrap();
        let source = library();
        let first_path = directory.path().join("first.sqlite");
        let second_path = directory.path().join("second.sqlite");
        source.backup(rusqlite::MAIN_DB, &first_path, None).unwrap();
        source
            .backup(rusqlite::MAIN_DB, &second_path, None)
            .unwrap();
        let first = Connection::open(first_path).unwrap();
        let second = Connection::open(second_path).unwrap();
        first
            .execute(
                "UPDATE entry SET search_text='First changed text' WHERE id=1",
                [],
            )
            .unwrap();
        second
            .execute(
                "UPDATE entry SET search_text='Other changed text' WHERE id=1",
                [],
            )
            .unwrap();
        assert_eq!(
            crate::library_revision::snapshot_token(&first, &[1]).unwrap(),
            crate::library_revision::snapshot_token(&second, &[1]).unwrap()
        );
        let first_token = token(&first, &[1]).unwrap();
        let second_token = token(&second, &[1]).unwrap();
        assert_ne!(first_token, second_token);
        assert!(verify_token(&first_token, &second_token).is_err());
        assert!(!first_token.starts_with("r1:"));
    }

    #[test]
    fn managed_tokens_cover_text_tags_prices_and_fall_back_if_tracking_is_removed() {
        let conn = library();
        crate::library_revision::mark_managed(&conn).unwrap();
        let mut previous = token(&conn, &[1]).unwrap();
        assert!(previous.starts_with("r1:"));
        for statement in [
            "UPDATE entry SET search_text='Updated' WHERE id=1",
            "INSERT INTO session_tag VALUES(1,'tag')",
            "INSERT INTO session_annotation VALUES(1,1)",
            "INSERT INTO model_price_overrides(provider,model_id,in_per_1m,out_per_1m,cache_per_1m) VALUES('','model',1,2,0.1)",
        ] {
            conn.execute(statement,[]).unwrap();
            let actual=token(&conn,&[1]).unwrap();
            assert_ne!(actual,previous);
            assert!(verify_token(&actual,&previous).is_err());
            previous=actual;
        }
        conn.execute_batch("DROP TRIGGER library_revision_entry_update")
            .unwrap();
        let before = token(&conn, &[1]).unwrap();
        assert!(!before.starts_with("r1:"));
        conn.execute(
            "UPDATE entry SET search_text='Changed after removing the trigger'",
            [],
        )
        .unwrap();
        assert_ne!(token(&conn, &[1]).unwrap(), before);
    }
}
