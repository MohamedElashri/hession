//! Read-only Cline task and SDK session artifacts. Companion files participate
//! in the fingerprint so metadata and usage updates are imported together.

use super::formats::{self, SessionData};
use super::{
    env_path, first_str, home_dir, normalize_record, source_mtime, stop_requested, MAX_FILES,
};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub(super) fn source_roots(home: &Path) -> Vec<PathBuf> {
    let config = env_path("CLINE_DIR", home.join(".cline"));
    let data = env_path("CLINE_DATA_DIR", config.join("data"));
    let mut roots = vec![
        env_path("CLINE_SESSION_DATA_DIR", data.join("sessions")),
        data.join("tasks"),
    ];
    // Explicit custom extension storage, separate from Cline's CLI settings.
    if let Ok(storage) = std::env::var("CLINE_STORAGE_DIR") {
        if !storage.trim().is_empty() {
            roots.push(PathBuf::from(storage.trim()).join("tasks"));
        }
    }
    #[cfg(target_os = "macos")]
    let editors = home.join("Library/Application Support");
    #[cfg(windows)]
    let editors = env_path("APPDATA", home.join("AppData/Roaming"));
    #[cfg(not(any(target_os = "macos", windows)))]
    let editors = env_path("XDG_CONFIG_HOME", home.join(".config"));
    for editor in [
        "Code",
        "Code - Insiders",
        "Code - OSS",
        "VSCodium",
        "Cursor",
        "Windsurf",
    ] {
        let user = editors.join(editor).join("User");
        roots.push(user.join("globalStorage/saoudrizwan.claude-dev/tasks"));
        if let Ok(profiles) = fs::read_dir(user.join("profiles")) {
            for profile in profiles.flatten() {
                roots.push(
                    profile
                        .path()
                        .join("globalStorage/saoudrizwan.claude-dev/tasks"),
                );
            }
        }
    }
    for server in [".vscode-server", ".vscode-server-insiders"] {
        roots.push(
            home.join(server)
                .join("data/User/globalStorage/saoudrizwan.claude-dev/tasks"),
        );
    }
    roots.sort();
    roots.dedup();
    roots
}

pub(super) fn discover(root: &Path, files: &mut Vec<PathBuf>) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if stop_requested() || files.len() >= MAX_FILES {
            return;
        }
        // One canonical source per task; UI history is a fallback, not a
        // second copy of the same conversation.
        let api = dir.join("api_conversation_history.json");
        let ui = dir.join("ui_messages.json");
        if api.is_file() {
            files.push(api);
        } else if ui.is_file() {
            files.push(ui);
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if stop_requested() || files.len() >= MAX_FILES {
                return;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && stack.len() < MAX_FILES {
                stack.push(entry.path());
            } else if kind.is_file()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".messages.json")
            {
                files.push(entry.path());
            }
        }
    }
}

fn modern(file: &Path) -> bool {
    file.file_name()
        .is_some_and(|n| n.to_string_lossy().ends_with(".messages.json"))
}

fn session_root(file: &Path) -> PathBuf {
    source_roots(&home_dir())
        .into_iter()
        .filter(|root| file.starts_with(root))
        .max_by_key(|root| root.components().count())
        .or_else(|| {
            file.ancestors()
                .find(|p| p.file_name().is_some_and(|n| n == "sessions"))
                .map(Path::to_path_buf)
        })
        .unwrap_or_else(|| {
            file.parent()
                .and_then(Path::parent)
                .unwrap_or(Path::new("."))
                .into()
        })
}

pub(super) fn resume_environment(file: &Path) -> Vec<(String, String)> {
    let root = session_root(file);
    let mut env = vec![
        (
            "CLINE_SESSION_DATA_DIR".into(),
            root.to_string_lossy().into_owned(),
        ),
        (
            "CLINE_DB_DATA_DIR".into(),
            database(file)
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
    ];
    if root.file_name().is_some_and(|n| n == "sessions") {
        if let Some(data) = root.parent() {
            env.push(("CLINE_DATA_DIR".into(), data.to_string_lossy().into_owned()));
            if data.file_name().is_some_and(|n| n == "data") {
                if let Some(config) = data.parent() {
                    env.push(("CLINE_DIR".into(), config.to_string_lossy().into_owned()));
                }
            }
        }
    }
    env
}

fn database(file: &Path) -> PathBuf {
    let data = env_path(
        "CLINE_DATA_DIR",
        env_path("CLINE_DIR", home_dir().join(".cline")).join("data"),
    );
    let root = session_root(file);
    let default = if root.file_name().is_some_and(|n| n == "sessions") {
        root.parent().unwrap_or(&data).join("db")
    } else {
        data.join("db")
    };
    env_path("CLINE_DB_DATA_DIR", default).join("sessions.db")
}

fn companions(file: &Path) -> Vec<PathBuf> {
    let dir = file.parent().unwrap_or(Path::new("."));
    if modern(file) {
        let manifest = file
            .to_string_lossy()
            .trim_end_matches(".messages.json")
            .to_owned()
            + ".json";
        let db = database(file);
        vec![
            file.into(),
            manifest.into(),
            db.clone(),
            PathBuf::from(format!("{}-wal", db.display())),
            PathBuf::from(format!("{}-journal", db.display())),
        ]
    } else {
        let tasks = dir.parent().unwrap_or(dir);
        let storage = tasks.parent().unwrap_or(tasks);
        vec![
            dir.join("api_conversation_history.json"),
            dir.join("ui_messages.json"),
            dir.join("task_metadata.json"),
            tasks.join("taskHistory.json"),
            storage.join("state/taskHistory.json"),
        ]
    }
}

pub(super) fn fingerprint(file: &Path) -> Result<(i64, f64), String> {
    let mut size = 0_i64;
    let mut mtime = 0_f64;
    for path in companions(file) {
        match fs::metadata(&path) {
            Ok(stat) if stat.is_file() => {
                size = size.saturating_add(stat.len() as i64);
                mtime += source_mtime(&stat);
            }
            Ok(_) => return Err(format!("Cline source is not a file: {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && path != file => (),
            Err(e) => return Err(format!("stat {}: {e}", path.display())),
        }
    }
    Ok((size, mtime))
}

fn read_json(file: &Path) -> Result<Option<Value>, String> {
    if stop_requested() {
        return Err("Import stopped".into());
    }
    match fs::read(file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("Invalid Cline JSON {}: {e}", file.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", file.display())),
    }
}

fn array(value: &Value) -> Result<&Vec<Value>, String> {
    value
        .as_array()
        .ok_or_else(|| "Cline history must be a JSON array".into())
}

fn ui_tool_event(message: &&Value) -> bool {
    message["ask"] == "tool"
        || message["say"] == "tool"
        || message["ask"] == "command"
        || message["say"] == "command"
}

pub(super) fn session_data(
    uri: &str,
    file: &Path,
    size: i64,
    mtime: f64,
) -> Result<SessionData, String> {
    let source = read_json(file)?.ok_or("Cline source disappeared; retry sync")?;
    let dir = file.parent().ok_or("Cline source has no directory")?;
    let mut metadata = json!({});
    let mut ui = Vec::new();
    let id;
    let values;
    if modern(file) {
        id = first_str(&source, &["sessionId"]).ok_or("Cline messages have no sessionId")?;
        values = array(
            source
                .get("messages")
                .ok_or("Cline source has no messages")?,
        )?
        .clone();
        metadata = source.clone();
        metadata
            .as_object_mut()
            .ok_or("Invalid Cline messages envelope")?
            .remove("messages");
        let manifest = PathBuf::from(
            file.to_string_lossy()
                .trim_end_matches(".messages.json")
                .to_owned()
                + ".json",
        );
        if let Some(value) = read_json(&manifest)? {
            metadata
                .as_object_mut()
                .ok_or("Invalid Cline messages envelope")?
                .extend(value.as_object().ok_or("Invalid Cline manifest")?.clone());
        }
        let db = database(file);
        if db.is_file() {
            let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|e| e.to_string())?;
            conn.busy_timeout(std::time::Duration::from_secs(5))
                .map_err(|e| e.to_string())?;
            if let Some(row) = formats::rows(
                &conn,
                "SELECT * FROM sessions WHERE session_id=?",
                Some(&id),
            )?
            .pop()
            {
                metadata
                    .as_object_mut()
                    .ok_or("Invalid Cline messages envelope")?
                    .extend(row.as_object().ok_or("Invalid Cline session row")?.clone());
            }
        }
        metadata["last_activity_at"] = [&source["updated_at"], &metadata["updated_at"]]
            .into_iter()
            .filter_map(super::iso_time)
            .max()
            .map(Value::String)
            .unwrap_or(Value::Null);
        if let Some(title) = first_str(&metadata, &["prompt"]) {
            metadata["title"] = json!(title);
        }
    } else {
        id = dir
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("Cline task has no ID")?
            .into();
        values = array(&source)?.clone();
        let ui_path = dir.join("ui_messages.json");
        ui = if file == ui_path {
            values.clone()
        } else {
            read_json(&ui_path)?
                .map(|v| array(&v).cloned())
                .transpose()?
                .unwrap_or_default()
        };
        let tasks = dir.parent().ok_or("Cline task has no storage directory")?;
        for path in [
            tasks.join("taskHistory.json"),
            tasks
                .parent()
                .unwrap_or(tasks)
                .join("state/taskHistory.json"),
        ] {
            if let Some(history) = read_json(&path)? {
                if let Some(task) = array(&history)?
                    .iter()
                    .find(|task| first_str(task, &["id"]).as_deref() == Some(&id))
                {
                    metadata
                        .as_object_mut()
                        .ok_or("Invalid Cline task metadata")?
                        .extend(
                            task.as_object()
                                .ok_or("Invalid Cline task metadata")?
                                .clone(),
                        );
                    break;
                }
            }
        }
        if let Some(details) = read_json(&dir.join("task_metadata.json"))? {
            metadata["task_metadata"] = details.clone();
            if let Some(model) = details["model_usage"].as_array().and_then(|a| a.last()) {
                metadata["model"] = model["model_id"].clone();
                metadata["provider"] = model["model_provider_id"].clone();
            }
        }
        metadata["title"] = metadata.get("task").cloned().unwrap_or(Value::Null);
        metadata["cwd"] = metadata
            .get("cwdOnTaskInitialization")
            .cloned()
            .unwrap_or(Value::Null);
        if metadata["model"].is_null() {
            metadata["model"] = metadata["modelId"].clone();
        }
        if metadata["provider"].is_null() {
            metadata["provider"] = metadata["apiProvider"].clone();
        }
        metadata["started_at"] = ui.first().map(|m| m["ts"].clone()).unwrap_or(Value::Null);
        metadata["last_activity_at"] = ui.last().map(|m| m["ts"].clone()).unwrap_or(Value::Null);
    }
    metadata["id"] = json!(id);
    let mut records = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        if stop_requested() {
            return Err("Import stopped".into());
        }
        let mut record =
            normalize_record("cline", value, index as i64).ok_or("Invalid Cline message")?;
        record.context.cwd = first_str(&metadata, &["cwd", "workspace_root"]);
        record.context.model = record
            .context
            .model
            .or_else(|| first_str(&metadata, &["model"]));
        record.context.provider = record
            .context
            .provider
            .or_else(|| first_str(&metadata, &["provider"]));
        if record.occurred_at.is_none() {
            record.occurred_at = ui
                .iter()
                .find(|m| m["conversationHistoryIndex"].as_u64() == Some(index as u64))
                .and_then(|m| super::iso_time(&m["ts"]));
        }
        records.push(record);
    }
    if records.is_empty() {
        return Err("Cline source contains no messages; previous import preserved".into());
    }
    // The API transcript owns message text. UI-only tool activity supplements
    // XML tool-calling tasks without importing a second transcript copy.
    if file
        .file_name()
        .is_some_and(|n| n == "api_conversation_history.json")
        && !records.iter().any(|r| !r.tools.is_empty())
    {
        for value in ui.iter().filter(ui_tool_event) {
            let record = normalize_record("cline", value, records.len() as i64)
                .ok_or("Invalid Cline tool event")?;
            records.push(record);
        }
        // Keep the API message order. A UI event's conversation index refers
        // to the API row it accompanied, so attach its tools to that row.
        let extras = records.split_off(values.len());
        let tool_events = ui.iter().filter(ui_tool_event);
        for (event, extra) in tool_events.zip(extras) {
            if let Some(target) = event["conversationHistoryIndex"]
                .as_u64()
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < values.len())
                .and_then(|index| records.get_mut(index))
            {
                target.parts.extend(extra.parts);
                target.body.push('\n');
                target.body.push_str(&extra.body);
                target.tools.extend(extra.tools);
                let events = target
                    .payload
                    .as_object_mut()
                    .ok_or("Invalid Cline UI tool event")?
                    .entry("cline_ui_tool_events")
                    .or_insert_with(|| json!([]));
                events
                    .as_array_mut()
                    .ok_or("Invalid Cline UI tool event list")?
                    .push(event.clone());
            } else {
                records.push(extra);
            }
        }
    }
    let usage = if modern(file) {
        usage(values.iter().filter_map(|v| v.get("metrics")), true)
    } else {
        let mut requests: Vec<Value> = Vec::new();
        for event in ui
            .iter()
            .filter(|v| v["say"] == "api_req_started" || v["say"] == "api_req_finished")
        {
            let value: Value = serde_json::from_str(event["text"].as_str().unwrap_or("{}"))
                .map_err(|e| format!("Invalid Cline request usage: {e}"))?;
            if event["say"] == "api_req_finished" {
                if let Some(request) = requests.last_mut() {
                    request
                        .as_object_mut()
                        .ok_or("Invalid Cline API request")?
                        .extend(value.as_object().ok_or("Invalid Cline API usage")?.clone());
                    continue;
                }
            }
            requests.push(value);
        }
        usage(requests.iter(), false).or_else(|| usage(std::iter::once(&metadata), false))
    };
    Ok(SessionData {
        uri: uri.into(),
        fallback_id: id,
        format: if modern(file) {
            "cline-messages-v1"
        } else {
            "cline-task"
        }
        .into(),
        metadata,
        records,
        usage,
        size,
        mtime,
    })
}

fn usage<'a>(values: impl Iterator<Item = &'a Value>, input_is_total: bool) -> Option<Value> {
    let mut input = 0_i64;
    let mut output = 0_i64;
    let mut cached = 0_i64;
    let mut reported = false;
    for value in values {
        let count = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| value[*key].as_i64())
                .map(|n| n.max(0))
        };
        let incoming = count(&["inputTokens", "tokensIn"]);
        let outgoing = count(&["outputTokens", "tokensOut"]);
        let read = count(&["cacheReadTokens", "cacheReads"]).unwrap_or(0);
        let write = count(&["cacheWriteTokens", "cacheWrites"]).unwrap_or(0);
        reported |= incoming.is_some() || outgoing.is_some();
        input = input.saturating_add(incoming.unwrap_or(0));
        if !input_is_total {
            input = input.saturating_add(read).saturating_add(write);
        }
        output = output.saturating_add(outgoing.unwrap_or(0));
        cached = cached.saturating_add(read);
    }
    reported.then(|| json!({"inputTokens": input, "outputTokens": output, "cachedTokens": cached}))
}
