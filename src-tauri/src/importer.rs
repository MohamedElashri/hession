//! Source discovery and transactional imports for JSONL and SQLite agent logs.
//! Imports preserve raw records and structured content. Parse/read failures and
//! cancellation roll back the active batch and preserve completed imports.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[path = "import_formats.rs"]
mod formats;
#[path = "import_cline.rs"]
mod cline;
use formats::{ContentPart, RecordContext, ToolCall, ToolResult};

pub(crate) fn cline_resume_environment(file: &Path) -> Vec<(String, String)> {
    cline::resume_environment(file)
}

/// `SyncError::file` prefix for a remote host that could not be fetched.
pub const HOST_ERROR_PREFIX: &str = "host ";
/// Max errors retained per run.
const MAX_ERRORS: usize = 25;
/// Cap on files collected during directory walks (robustness guard).
const MAX_FILES: usize = 100_000;
/// Cooperative cancellation flag (`requestSyncStop` equivalent).
static STOP: AtomicBool = AtomicBool::new(false);

/// Ask a running sync to stop at the next cancellation check.
pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
}

/// Clear cancellation before publishing a new run as active.
pub fn reset_stop() {
    STOP.store(false, Ordering::SeqCst);
}

fn stop_requested() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Cancellation check for long-running helpers outside this module.
pub(crate) fn is_stop_requested() -> bool {
    stop_requested()
}

// ---------------------------------------------------------------------------
// Public shapes
// ---------------------------------------------------------------------------

/// One importable agent family.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableAgent {
    pub family: String,
    pub product: String,
}

/// One checkpoint row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub family: String,
    pub completed_at: Option<String>,
    pub source_cutoff: Option<String>,
}

/// Lossless normalized record, with structured content and source metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedRecord {
    pub kind: String,
    pub role: Option<String>,
    pub body: String,
    pub occurred_at: Option<String>,
    pub external_id: Option<String>,
    pub parent: Option<String>,
    pub turn_id: Option<String>,
    pub parts: Vec<ContentPart>,
    pub tools: Vec<ToolCall>,
    pub results: Vec<ToolResult>,
    pub context: RecordContext,
    pub payload: Value,
}

/// One import failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncError {
    pub file: String,
    pub error: String,
}

/// Run totals.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncSummary {
    pub files_done: usize,
    pub files_total: usize,
    pub records: usize,
    pub skipped: usize,
    pub errors: Vec<SyncError>,
    pub cancelled: bool,
}

// ---------------------------------------------------------------------------
// Source discovery (+ env overrides)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    /// Recursive `*.jsonl` scan of `root/sub`.
    Jsonl,
    /// Single `opencode.db` file under `root`.
    OpenCodeDb,
    /// `state.db` + `profiles/*/state.db` + `sessions/**/*.jsonl`.
    Hermes,
    /// One complete transcript per conversation under `root/brain`.
    Antigravity,
    /// Cline task folders or SDK session artifacts.
    Cline,
}

struct Source {
    family: &'static str,
    product: &'static str,
    root: PathBuf,
    sub: &'static str,
    kind: SourceKind,
}

fn home_dir() -> PathBuf {
    for key in ["HOME", "USERPROFILE"] {
        if let Ok(v) = std::env::var(key) {
            if !v.trim().is_empty() {
                return PathBuf::from(v.trim());
            }
        }
    }
    PathBuf::from(".")
}

fn env_path(key: &str, default: PathBuf) -> PathBuf {
    match std::env::var(key) {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
        _ => default,
    }
}

fn hermes_home(home: &Path) -> PathBuf {
    #[cfg(windows)]
    let base = env_path("LOCALAPPDATA", home.join("AppData").join("Local"));
    #[cfg(not(windows))]
    let base = home.to_path_buf();

    let suffix = std::env::var("HERMES_DATA_DIR_SUFFIX").unwrap_or_default();
    let directory = if cfg!(windows) { "hermes" } else { ".hermes" };
    env_path("HERMES_HOME", base.join(format!("{directory}{suffix}")))
}

/// Local sources followed by mirrored remote-host sources (see `remote`).
fn all_sources() -> Vec<Source> {
    let mut out = local_sources();
    for (family, kind, root, sub) in crate::remote::mirrored_roots() {
        let Some(local) = out.iter().find(|s| s.family == family) else { continue };
        let (family, product) = (local.family, local.product);
        let kind = match kind.as_str() {
            "jsonl" => SourceKind::Jsonl,
            "opencode" => SourceKind::OpenCodeDb,
            "hermes" => SourceKind::Hermes,
            "antigravity" => SourceKind::Antigravity,
            "cline" => SourceKind::Cline,
            _ => continue,
        };
        let sub: &'static str = match sub.as_str() {
            "" => "",
            "sessions" => "sessions",
            "projects" => "projects",
            "brain" | "opencode.db" => "",
            _ => continue,
        };
        out.push(Source { family, product, root, sub, kind });
    }
    out
}

/// All known local sources with env overrides applied:
/// `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `XDG_DATA_HOME`, `PI_CODING_AGENT_DIR`,
/// `COMMANDCODE_HOME`, `HERMES_HOME`, `ANTIGRAVITY_HOME`.
fn local_sources() -> Vec<Source> {
    let home = home_dir();
    let xdg = env_path("XDG_DATA_HOME", home.join(".local/share"));
    let mut out = vec![
        Source {
            family: "codex",
            product: "Codex",
            root: env_path("CODEX_HOME", home.join(".codex")),
            sub: "sessions",
            kind: SourceKind::Jsonl,
        },
        Source {
            family: "claude",
            product: "Claude Code",
            root: env_path("CLAUDE_CONFIG_DIR", home.join(".claude")),
            sub: "projects",
            kind: SourceKind::Jsonl,
        },
        Source {
            family: "opencode",
            product: "OpenCode",
            root: xdg.join("opencode"),
            sub: "",
            kind: SourceKind::OpenCodeDb,
        },
        Source {
            family: "pi",
            product: "Pi",
            root: env_path("PI_CODING_AGENT_DIR", home.join(".pi/agent")),
            sub: "sessions",
            kind: SourceKind::Jsonl,
        },
        Source {
            family: "commandcode",
            product: "Command Code",
            root: env_path("COMMANDCODE_HOME", home.join(".commandcode")),
            sub: "projects",
            kind: SourceKind::Jsonl,
        },
        Source {
            family: "muse",
            product: "Muse",
            root: xdg.join("muse"),
            sub: "sessions",
            kind: SourceKind::Jsonl,
        },
        Source {
            family: "hermes",
            product: "Hermes Agent",
            root: hermes_home(&home),
            sub: "",
            kind: SourceKind::Hermes,
        },
    ];
    for root in cline::source_roots(&home) {
        out.push(Source {
            family: "cline",
            product: "Cline",
            root,
            sub: "",
            kind: SourceKind::Cline,
        });
    }
    match std::env::var("ANTIGRAVITY_HOME") {
        Ok(v) if !v.trim().is_empty() => out.push(Source {
            family: "antigravity",
            product: "Antigravity",
            root: PathBuf::from(v.trim()),
            sub: "",
            kind: SourceKind::Antigravity,
        }),
        _ => {
            for leaf in ["antigravity-cli", "antigravity"] {
                out.push(Source {
                    family: "antigravity",
                    product: "Antigravity",
                    root: home.join(".gemini").join(leaf),
                    sub: "",
                    kind: SourceKind::Antigravity,
                });
            }
        }
    }
    out
}

fn source_present(src: &Source) -> bool {
    match src.kind {
        SourceKind::Jsonl => src.root.join(src.sub).is_dir(),
        SourceKind::OpenCodeDb => src.root.join("opencode.db").is_file(),
        SourceKind::Hermes => src.root.is_dir(),
        SourceKind::Antigravity => src.root.join("brain").is_dir(),
        SourceKind::Cline => src.root.is_dir(),
    }
}

/// Families whose source paths exist.
pub fn get_available_agents() -> Vec<AvailableAgent> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for src in all_sources() {
        if seen.contains(src.family) || !source_present(&src) {
            continue;
        }
        seen.insert(src.family);
        out.push(AvailableAgent {
            family: src.family.to_string(),
            product: src.product.to_string(),
        });
    }
    out
}

fn walk_jsonl(dir: &Path, acc: &mut Vec<PathBuf>) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        if acc.len() >= MAX_FILES || stop_requested() {
            return;
        }
        let entries = match fs::read_dir(&current) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if stop_requested() {
                return;
            }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                acc.push(path);
                if acc.len() >= MAX_FILES {
                    return;
                }
            }
        }
    }
}

fn sort_by_mtime_desc(paths: &mut [PathBuf]) {
    paths.sort_by_cached_key(|p| {
        std::cmp::Reverse(fs::metadata(p).and_then(|s| s.modified()).unwrap_or(UNIX_EPOCH))
    });
}

/// All candidate files for one family.
pub fn discover_source_files(family: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for src in all_sources().iter().filter(|s| s.family == family) {
        match src.kind {
            SourceKind::Cline => cline::discover(&src.root, &mut out),
            SourceKind::Jsonl => walk_jsonl(&src.root.join(src.sub), &mut out),
            SourceKind::OpenCodeDb => {
                let f = src.root.join("opencode.db");
                if f.is_file() {
                    out.push(f);
                }
            }
            SourceKind::Hermes => {
                let db = src.root.join("state.db");
                if db.is_file() {
                    out.push(db);
                }
                if let Ok(rd) = fs::read_dir(src.root.join("profiles")) {
                    for entry in rd.flatten() {
                        let p = entry.path().join("state.db");
                        if p.is_file() {
                            out.push(p);
                        }
                    }
                }
                walk_jsonl(&src.root.join("sessions"), &mut out);
            }
            SourceKind::Antigravity => {
                if let Ok(entries) = fs::read_dir(src.root.join("brain")) {
                    for entry in entries.flatten() {
                        if stop_requested() || out.len() >= MAX_FILES {
                            break;
                        }
                        let logs = entry.path().join(".system_generated/logs");
                        // Full transcripts retain structured tool arguments. The
                        // display transcript and chunks are copies, not sessions.
                        for name in ["transcript_full.jsonl", "transcript.jsonl"] {
                            let file = logs.join(name);
                            if file.is_file() {
                                out.push(file);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    sort_by_mtime_desc(&mut out);
    out
}

// ---------------------------------------------------------------------------
// JSONL parsing + minimal normalization
// ---------------------------------------------------------------------------

/// Read a stable, complete JSONL snapshot. Never silently discard bad records.
pub fn parse_jsonl_file(path: &Path) -> Result<Vec<Value>, String> {
    let file = fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let before = file.metadata().map_err(|e| format!("stat source: {e}"))?;
    let mut reader = BufReader::new((&file).take(before.len()));
    let mut records = Vec::new();
    let mut line = String::new();
    let mut bytes = 0u64;
    let mut line_number = 0;
    loop {
        if stop_requested() {
            return Err("Import stopped".to_string());
        }
        line.clear();
        let n = reader
            .read_line(&mut line)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        line_number += 1;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line)
            .map_err(|e| format!("{} line {line_number}: {e}", path.display()))?;
        if !value.is_object() {
            return Err(format!(
                "{} line {line_number}: expected an object",
                path.display()
            ));
        }
        records.push(value);
    }
    let after = file.metadata().map_err(|e| format!("stat source: {e}"))?;
    let current = fs::metadata(path).map_err(|e| format!("stat source path: {e}"))?;
    if bytes != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || before.len() != current.len()
        || before.modified().ok() != current.modified().ok()
    {
        return Err(format!(
            "{} changed while reading; retry sync",
            path.display()
        ));
    }
    Ok(records)
}

fn iso_time(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Number(n) => {
            let f = n.as_f64()?;
            if !f.is_finite() {
                return None;
            }
            // Values < 1e12 are seconds, otherwise milliseconds.
            // Muse `recorded_at` is microseconds (>= 1e14 can never be ms:
            // 1e14 ms is year 5138), with nanoseconds above 1e17.
            let a = f.abs();
            let (secs, nsec) = if a < 1e12 {
                let ms = f * 1000.0;
                let secs = (ms / 1000.0).floor() as i64;
                let nsec = ((ms - secs as f64 * 1000.0) * 1_000_000.0).round() as i64;
                (secs, nsec)
            } else if a < 1e14 {
                let secs = (f / 1000.0).floor() as i64;
                let nsec = ((f - secs as f64 * 1000.0) * 1_000_000.0).round() as i64;
                (secs, nsec)
            } else if a < 1e17 {
                let secs = (f / 1_000_000.0).floor() as i64;
                let nsec = ((f - secs as f64 * 1_000_000.0) * 1000.0).round() as i64;
                (secs, nsec)
            } else {
                let secs = (f / 1_000_000_000.0).floor() as i64;
                let nsec = (f - secs as f64 * 1_000_000_000.0).round() as i64;
                (secs, nsec)
            };
            DateTime::from_timestamp(secs, nsec.clamp(0, 999_999_999) as u32)
                .map(|d| d.to_rfc3339_opts(SecondsFormat::Millis, true))
        }
        serde_json::Value::String(s) => parse_time_str(s),
        _ => None,
    }
}

/// Lenient timestamp parse, normalized to ISO.
fn parse_time_str(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let fmt = |dt: DateTime<chrono::FixedOffset>| {
        dt.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::Millis, true)
    };
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(fmt(dt));
    }
    let mut owned = s.to_string();
    if owned.contains('T') {
        let has_tz = owned.ends_with('Z')
            || owned.contains('+')
            || owned.get(10..).is_some_and(|tail| tail.contains('-'));
        if !has_tz {
            owned.push('Z');
        }
    } else {
        owned = owned.replace(' ', "T") + "Z";
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(&owned) {
        return Some(fmt(dt));
    }
    None
}

fn first_str(obj: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let m = obj.as_object()?;
    for k in keys {
        match m.get(*k) {
            Some(serde_json::Value::String(s)) if !s.is_empty() => return Some(s.clone()),
            Some(serde_json::Value::Number(_) | serde_json::Value::Bool(_)) => {
                return m.get(*k).map(|v| v.to_string())
            }
            _ => {}
        }
    }
    None
}

fn first_iso(obj: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let m = obj.as_object()?;
    for k in keys {
        if let Some(v) = m.get(*k) {
            if let Some(s) = iso_time(v) {
                return Some(s);
            }
        }
    }
    if let Some(created) = m.get("time").and_then(|t| t.get("created")) {
        if let Some(s) = iso_time(created) {
            return Some(s);
        }
    }
    None
}

/// Normalize an agent record without truncating transcript content.
/// Returns `None` for non-object lines.
pub fn normalize_record(
    family: &str,
    obj: &serde_json::Value,
    _ordinal: i64,
) -> Option<NormalizedRecord> {
    let m = obj.as_object()?;
    let typ = first_str(obj, &["type"]).unwrap_or_else(|| "event".to_string());
    let null = serde_json::Value::Null;

    let (kind, role, content, occurred, external, parent, turn): (
        String,
        String,
        serde_json::Value,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = match family {
        "codex" => {
            let p = m.get("payload").unwrap_or(obj);
            let (k, r, c) = match typ.as_str() {
                "response_item" => (
                    first_str(p, &["type"]).unwrap_or_else(|| "response_item".to_string()),
                    first_str(p, &["role"]).unwrap_or_else(|| {
                        if first_str(p, &["type"]).is_some_and(|t| t.ends_with("_call_output")) {
                            "tool"
                        } else {
                            "assistant"
                        }
                        .to_string()
                    }),
                    p.get("content").cloned().unwrap_or(null.clone()),
                ),
                "event_msg" => {
                    let r = match first_str(p, &["type"]).as_deref() {
                        Some("user_message") => "user".to_string(),
                        Some("agent_message") => "assistant".to_string(),
                        _ => first_str(p, &["role"]).unwrap_or_default(),
                    };
                    let c = p
                        .get("message")
                        .or_else(|| p.get("item"))
                        .or_else(|| p.get("last_agent_message"))
                        .cloned()
                        .unwrap_or(null.clone());
                    (
                        first_str(p, &["type"]).unwrap_or_else(|| "event".to_string()),
                        r,
                        c,
                    )
                }
                _ => (
                    first_str(p, &["type"]).unwrap_or_else(|| typ.clone()),
                    first_str(p, &["role"]).unwrap_or_default(),
                    p.get("message")
                        .or_else(|| p.get("summary"))
                        .or_else(|| p.get("text"))
                        .cloned()
                        .unwrap_or(null.clone()),
                ),
            };
            let occ = first_iso(obj, &["timestamp"])
                .or_else(|| first_iso(p, &["timestamp", "completed_at", "created_at"]));
            let ext = first_str(p, &["id", "event_id"])
                .or_else(|| first_str(obj, &["id", "uuid", "event_id"]));
            (
                k,
                r,
                c,
                occ,
                ext,
                first_str(obj, &["parentId", "parentUuid", "parent_id"]),
                first_str(p, &["turn_id"]).or_else(|| first_str(obj, &["turn_id", "turnId"])),
            )
        }
        "claude" => {
            let msg = m
                .get("message")
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or(null.clone());
            let r = first_str(&msg, &["role"])
                .or_else(|| first_str(obj, &["type"]))
                .unwrap_or_default();
            let c = match m.get("message") {
                Some(serde_json::Value::String(s)) => serde_json::Value::String(s.clone()),
                _ => msg
                    .get("content")
                    .or_else(|| m.get("content"))
                    .cloned()
                    .unwrap_or(null.clone()),
            };
            let k = first_str(obj, &["type"]).unwrap_or_else(|| r.clone());
            let occ = first_iso(obj, &["timestamp"]).or_else(|| first_iso(&msg, &["timestamp"]));
            let ext = first_str(obj, &["uuid"])
                .or_else(|| first_str(&msg, &["id"]))
                .or_else(|| first_str(obj, &["id", "event_id"]));
            (
                if k.is_empty() { "event".to_string() } else { k },
                r,
                c,
                occ,
                ext,
                first_str(obj, &["parentUuid", "parentId", "parent_id"]),
                first_str(obj, &["turn_id", "turnId"]),
            )
        }
        "pi" | "commandcode" => {
            let msg = m
                .get("message")
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or(null.clone());
            let r = first_str(&msg, &["role"])
                .or_else(|| first_str(obj, &["role"]))
                .unwrap_or_default();
            let mut c = msg
                .get("content")
                .or_else(|| msg.get("output"))
                .or_else(|| m.get("content"))
                .or_else(|| m.get("summary"))
                .cloned()
                .unwrap_or(null.clone());
            let empty = matches!(&c, serde_json::Value::Null)
                | matches!(&c, serde_json::Value::String(s) if s.is_empty());
            if empty {
                if let Some(cmd) = first_str(&msg, &["command"]) {
                    let out = first_str(&msg, &["output"]).unwrap_or_default();
                    c = serde_json::Value::String(format!("$ {cmd}\n{out}"));
                }
            }
            let k = first_str(obj, &["type"]).unwrap_or_else(|| "message".to_string());
            let occ = first_iso(obj, &["timestamp"]).or_else(|| first_iso(&msg, &["timestamp"]));
            (
                k,
                r,
                c,
                occ,
                first_str(obj, &["id", "uuid", "event_id"]),
                first_str(obj, &["parentId", "parentUuid", "parent_id"]),
                first_str(&msg, &["turnId"]).or_else(|| first_str(obj, &["turn_id", "turnId"])),
            )
        }
        "muse" => {
            let src = m
                .get("payload")
                .filter(|v| v.is_object())
                .cloned()
                .unwrap_or_else(|| obj.clone());
            let k = first_str(obj, &["payload_type", "record_type"])
                .or_else(|| first_str(&src, &["type"]))
                .or_else(|| first_str(obj, &["type"]))
                .unwrap_or_else(|| "event".to_string());
            let r = first_str(&src, &["role"]).unwrap_or_default();
            let c = ["content", "text", "output", "summary"]
                .iter()
                .find_map(|k| src.get(*k))
                .or_else(|| src.get("message").and_then(|v| v.get("content")))
                .cloned()
                .unwrap_or(null.clone());
            let occ = first_iso(obj, &["recorded_at", "timestamp"])
                .or_else(|| first_iso(&src, &["timestamp", "created_at"]));
            let ext = first_str(obj, &["id"]).or_else(|| first_str(&src, &["id"]));
            (
                k,
                r,
                c,
                occ,
                ext,
                first_str(obj, &["parent_id"]).or_else(|| first_str(&src, &["parent_id"])),
                None,
            )
        }
        "cline" => {
            let event = first_str(obj, &["say", "ask"]).unwrap_or_else(|| "message".into());
            let mut role = first_str(obj, &["role"]).unwrap_or_else(|| {
                match event.as_str() {
                    "task" | "user_feedback" | "user_feedback_diff" => "user",
                    "text" | "reasoning" | "completion_result" | "followup" | "plan_mode_respond" | "tool" | "command" => "assistant",
                    _ => "",
                }.into()
            });
            let mut content = m.get("content").or_else(|| m.get("text")).cloned().unwrap_or(null.clone());
            if role == "user" && content.as_array().is_some_and(|items| !items.is_empty()
                && items.iter().all(|part| part["type"] == "tool_result"))
            {
                role = "tool".into();
            }
            if matches!(event.as_str(), "tool" | "command") && m.get("role").is_none() {
                let input = if event == "command" {
                    serde_json::json!({"command": content})
                } else {
                    content.as_str().and_then(|s| serde_json::from_str::<Value>(s).ok())
                        .unwrap_or_else(|| serde_json::json!({"text": content}))
                };
                let name = if event == "command" { "execute_command".into() }
                    else { first_str(&input, &["tool", "name"]).unwrap_or_else(|| "tool".into()) };
                content = serde_json::json!([{"type": "tool_use", "name": name, "input": input}]);
            }
            (event, role, content, first_iso(obj, &["ts", "timestamp"]),
                first_str(obj, &["id", "ts"]), None, None)
        }
        "antigravity" => {
            let role = if typ == "USER_INPUT" {
                "user".to_string()
            } else if first_str(obj, &["source"]).as_deref() == Some("MODEL") {
                "assistant".to_string()
            } else {
                first_str(obj, &["role"]).unwrap_or_default()
            };
            (
                typ,
                role,
                m.get("content").cloned().unwrap_or(null.clone()),
                first_iso(obj, &["created_at", "timestamp"]),
                m.get("step_index").and_then(Value::as_i64).map(|n| n.to_string()),
                None,
                None,
            )
        }
        _ => {
            let r = first_str(obj, &["role"]).unwrap_or_default();
            let c = m
                .get("content")
                .or_else(|| m.get("text"))
                .cloned()
                .unwrap_or(null.clone());
            (
                typ,
                r,
                c,
                first_iso(
                    obj,
                    &["timestamp", "recorded_at", "createdAt", "created_at"],
                ),
                first_str(obj, &["id", "uuid", "event_id"]),
                first_str(obj, &["parent_id", "parentID", "parentId", "parentUuid"]),
                first_str(obj, &["turn_id", "turnId"]),
            )
        }
    };

    let (parts, tools, results, mut context) = formats::record_details(family, obj, &content, &role);
    if family == "cline" {
        context.model = first_str(&obj["modelInfo"], &["id", "modelId"]).or(context.model);
        context.provider = first_str(&obj["modelInfo"], &["provider", "providerId"]).or(context.provider);
    }
    let mut searchable = parts
        .iter()
        .filter_map(|p| p.text.clone())
        .collect::<Vec<_>>();
    for tool in &tools {
        searchable.push(tool.arguments.to_string());
        if let Some(command) = &tool.command {
            searchable.push(command.clone());
        }
        if let Some(result) = &tool.result {
            searchable.push(result.clone());
        }
    }
    let body = searchable.join("\n");
    let role = match role.as_str() {
        "toolResult" | "tool_result" => Some("tool".to_string()),
        r if r.is_empty() => None,
        r => Some(r.to_string()),
    };
    Some(NormalizedRecord {
        kind,
        role,
        body,
        occurred_at: occurred,
        external_id: external,
        parent,
        turn_id: turn,
        parts,
        tools,
        results,
        context,
        payload: obj.clone(),
    })
}

// ---------------------------------------------------------------------------
// sync_state / sync_checkpoint helpers
// ---------------------------------------------------------------------------

/// Incremental sync is driven by file size + modification time only: a source
/// whose current size and mtime still match the last successful import is
/// skipped without being read. No content hashes, no parser versions.
struct FileState {
    size: i64,
    mtime: f64,
    has_metrics: bool,
}

fn get_file_state(conn: &Connection, source_uri: &str) -> rusqlite::Result<Option<FileState>> {
    conn.prepare_cached(
        "SELECT ss.size, ss.mtime, (s.metrics_json IS NOT NULL)
         FROM sync_state ss LEFT JOIN session s ON s.source_uri = ss.source_uri
         WHERE ss.source_uri = ?1",
    )?.query_row(params![source_uri],
        |row| {
            Ok(FileState {
                size: row.get(0)?,
                mtime: row.get(1)?,
                has_metrics: row.get::<_, Option<i64>>(2)?.unwrap_or(0) != 0,
            })
        },
    )
    .optional()
}

fn set_file_state(
    conn: &Connection,
    source_uri: &str,
    size: i64,
    mtime: f64,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO sync_state(source_uri, size, mtime, synced_at)
         VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP)
         ON CONFLICT(source_uri) DO UPDATE SET size = excluded.size,
           mtime = excluded.mtime, synced_at = CURRENT_TIMESTAMP",
        params![source_uri, size, mtime],
    )?;
    Ok(())
}

/// Checkpoint rows for every known agent family.
pub fn get_checkpoints(conn: &Connection) -> Result<Vec<Checkpoint>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT a.family,
               COALESCE(sc.completed_at, MAX(ss.synced_at), MAX(s.imported_at)) AS completed_at,
               COALESCE(sc.source_cutoff, MAX(ss.synced_at), MAX(s.imported_at)) AS source_cutoff
             FROM agent a
             LEFT JOIN sync_checkpoint sc ON sc.family = a.family
             LEFT JOIN session s ON s.agent_id = a.id
             LEFT JOIN sync_state ss ON ss.source_uri = s.source_uri
             GROUP BY a.family ORDER BY a.family",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Checkpoint {
                family: row.get(0)?,
                completed_at: row.get(1)?,
                source_cutoff: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

fn put_checkpoint(
    conn: &Connection,
    family: &str,
    completed_at: &str,
    source_cutoff: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO sync_checkpoint(family, completed_at, source_cutoff)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(family) DO UPDATE SET completed_at = excluded.completed_at,
           source_cutoff = excluded.source_cutoff",
        params![family, completed_at, source_cutoff],
    )?;
    Ok(())
}

/// Checkpoints via the default database path (empty vec when unavailable).
pub fn load_checkpoints() -> Vec<Checkpoint> {
    let conn = match crate::db::open_db(&crate::db::db_path()) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    get_checkpoints(&conn).unwrap_or_default()
}

fn split_source_uri(source_uri: &str) -> Result<(&str, PathBuf), String> {
    let pos = source_uri
        .find("://")
        .ok_or_else(|| format!("bad source_uri (want family:///path): {source_uri}"))?;
    let family = &source_uri[..pos];
    let path = PathBuf::from(&source_uri[pos + 3..]);
    if family.is_empty() || path.as_os_str().is_empty() {
        return Err(format!("bad source_uri: {source_uri}"));
    }
    Ok((family, path))
}

// ---------------------------------------------------------------------------
// Atomic source imports: session, transcript, derived activity, and sync state
// ---------------------------------------------------------------------------

/// Reuse the pricing index for the whole sync; reload only after a user
/// changes prices, while holding the same writer slot as that change.
struct ImportPricing {
    state: Arc<Mutex<crate::pricing::PricingState>>,
    revision: Option<u64>,
}

impl ImportPricing {
    fn new() -> Self {
        Self { state: Arc::new(Mutex::new(crate::pricing::PricingState::new())), revision: None }
    }

    fn refresh(&mut self, conn: &Connection) -> Result<(), String> {
        let revision = crate::analytics::pricing_revision();
        if self.revision != Some(revision) {
            let mut pricing = self.state.lock().map_err(|_| "Could not lock import pricing")?;
            crate::pricing::load_pricing_cache(conn, &mut pricing)?;
            self.revision = Some(revision);
        }
        Ok(())
    }
}

/// A bounded batch amortizes WAL/FTS commit costs. Each session and its
/// statistics are atomic; cancellation rolls back the currently active batch.
fn commit_batch(
    conn: &mut Connection,
    family: &str,
    product: &str,
    root: &str,
    pricing: &mut ImportPricing,
    sessions: Vec<formats::SessionData>,
) -> Result<Vec<usize>, String> {
    let _writer = crate::db::background_write();
    if stop_requested() { return Err("Import stopped".into()); }
    pricing.refresh(conn)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e| e.to_string())?;
    let mut counts = Vec::with_capacity(sessions.len());
    for data in sessions {
        counts.push(commit_session(&tx, family, product, root, data, &pricing.state)?);
    }
    if stop_requested() { return Err("Import stopped".into()); }
    tx.commit().map_err(|e| format!("commit import batch: {e}"))?;
    crate::analytics::shared_state().clear_cache();
    Ok(counts)
}

fn import_file_with_conn(
    conn: &mut Connection,
    source_uri: &str,
    force: bool,
    on_session: Option<&mut dyn FnMut(usize)>,
    pricing: &mut ImportPricing,
) -> Result<usize, String> {
    let (family, file) = split_source_uri(source_uri)?;
    let (product, agent_root) = local_sources()
        .iter()
        .find(|s| s.family == family)
        .map(|s| (s.product.to_string(), s.root.to_string_lossy().to_string()))
        .ok_or_else(|| format!("unknown family: {family}"))?;

    if file.extension().and_then(|e| e.to_str()) == Some("db") {
        return formats::import_database(
            conn,
            source_uri,
            family,
            &file,
            &product,
            &agent_root,
            force,
            on_session,
            pricing,
        );
    }
    if family == "cline" {
        let (size, mtime) = cline::fingerprint(&file)?;
        if !force && get_file_state(conn, source_uri).map_err(|e| e.to_string())?
            .is_some_and(|st| st.size == size && st.mtime == mtime && st.has_metrics)
        {
            return Ok(0);
        }
        let data = cline::session_data(source_uri, &file, size, mtime)?;
        if cline::fingerprint(&file)? != (size, mtime) {
            return Err("Cline source changed while reading; retry sync".into());
        }
        return Ok(commit_batch(conn, family, &product, &agent_root, pricing, vec![data])?.into_iter().sum());
    }
    let stat = fs::metadata(&file).map_err(|e| format!("stat {}: {e}", file.display()))?;
    if !stat.is_file() {
        return Err("Source is not a file".to_string());
    }
    let size = stat.len() as i64;
    let mtime = source_mtime(&stat);
    if !force {
        if let Some(st) = get_file_state(conn, source_uri).map_err(|e| e.to_string())? {
            if st.size == size && st.mtime == mtime && st.has_metrics {
                return Ok(0);
            }
        }
    }
    let values = parse_jsonl_file(&file)?;
    let after = fs::metadata(&file).map_err(|e| format!("stat source: {e}"))?;
    if after.len() != stat.len() || after.modified().ok() != stat.modified().ok() {
        return Err("Source changed while reading; retry sync".to_string());
    }
    if values.is_empty() {
        let imported: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session WHERE source_uri=?)",
                [source_uri],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if imported {
            return Err("Source contains no records; previous import preserved".to_string());
        }
        return Ok(0);
    }
    let data = formats::session_data(family, source_uri, &file, values, size, mtime)?;
    Ok(commit_batch(conn, family, &product, &agent_root, pricing, vec![data])?.into_iter().sum())
}

fn source_mtime(stat: &fs::Metadata) -> f64 {
    stat.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// All derived rows and the checkpoint commit together, preserving session IDs
/// (and therefore bookmarks/tags) during a reimport. A source whose size and
/// mtime changed is always rebuilt in full: no content hashing, no
/// prefix-append shortcut.
fn commit_session(
    conn: &Connection,
    family: &str,
    product: &str,
    agent_root: &str,
    data: formats::SessionData,
    pricing: &Arc<Mutex<crate::pricing::PricingState>>,
) -> Result<usize, String> {
    if stop_requested() {
        return Err("Import stopped".to_string());
    }
    let agent_id = crate::db::get_or_create_agent(conn, family, product, Some(agent_root))
        .map_err(|e| e.to_string())?;
    let cwd = first_str(&data.metadata, &["cwd", "directory", "git_repo_root"])
        .or_else(|| data.records.iter().find_map(|r| r.context.cwd.clone()));
    // Remote sessions record host paths; qualify their workspace as
    // `host:/path` so identical paths on different machines stay distinct.
    let host = Path::new(data.uri.split_once("://").map_or("", |(_, p)| p))
        .ancestors()
        .find_map(crate::remote::from_mirror)
        .map(|(host, _)| host);
    let workspace_id = match (cwd.as_deref(), host.as_deref()) {
        (Some(c), Some(h)) => crate::db::get_or_create_workspace(conn, &format!("{h}:{c}")).map_err(|e| e.to_string())?,
        (Some(c), None) => crate::db::get_or_create_workspace(conn, c).map_err(|e| e.to_string())?,
        (None, _) => None,
    };
    let title = first_str(&data.metadata, &["title"]).or_else(|| {
        data.records
            .iter()
            .filter(|r| r.role.as_deref() == Some("user"))
            .map(|r| {
                let text = if family == "antigravity" {
                    r.body.split_once("<USER_REQUEST>")
                        .and_then(|(_, tail)| tail.split_once("</USER_REQUEST>"))
                        .map(|(request, _)| request)
                        .unwrap_or(&r.body)
                } else {
                    &r.body
                };
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            })
            .find(|s| {
                !s.is_empty()
                    && !s.to_lowercase().contains("environment context")
                    && !s.contains("<environment_context>")
            })
            .map(|s| s.chars().take(130).collect::<String>())
    });
    let start =
        first_iso(&data.metadata, &["started_at", "time_created", "timestamp"]).or_else(|| {
            data.records
                .iter()
                .filter_map(|r| r.occurred_at.clone())
                .min()
        });
    let end = first_iso(
        &data.metadata,
        &["last_activity_at", "ended_at", "time_updated"],
    )
    .or_else(|| {
        data.records
            .iter()
            .filter_map(|r| r.occurred_at.clone())
            .max()
    })
    .or_else(|| start.clone());
    let external =
        first_str(&data.metadata, &["id", "session_id", "sessionId"]).unwrap_or(data.fallback_id);
    conn.execute(
        "INSERT INTO session(agent_id,workspace_id,external_id,source_uri,source_format,cwd,title,started_at,last_event_at,
          source_size,source_mtime,raw_metadata_json,token_usage_json,metrics_json,host)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,NULL,?14)
         ON CONFLICT(source_uri) DO UPDATE SET agent_id=excluded.agent_id,workspace_id=excluded.workspace_id,host=excluded.host,
          external_id=excluded.external_id,source_format=excluded.source_format,cwd=excluded.cwd,title=excluded.title,
          started_at=excluded.started_at,last_event_at=excluded.last_event_at,source_size=excluded.source_size,
          source_mtime=excluded.source_mtime,raw_metadata_json=excluded.raw_metadata_json,
          token_usage_json=excluded.token_usage_json,metrics_json=NULL,imported_at=CURRENT_TIMESTAMP",
        params![agent_id,workspace_id,external,data.uri,data.format,cwd,title,start,end,data.size,data.mtime,
            data.metadata.to_string(), data.usage.as_ref().map(Value::to_string), host],
    ).map_err(|e| format!("upsert session: {e}"))?;
    let session_id: i64 = conn
        .query_row(
            "SELECT id FROM session WHERE source_uri=?",
            [&data.uri],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    for table in ["file_activity", "tool_invocation", "session_context", "entry"] {
        conn.prepare_cached(&format!("DELETE FROM {table} WHERE session_id=?"))
            .map_err(|e| e.to_string())?.execute([session_id]).map_err(|e| e.to_string())?;
    }
    for (ordinal, record) in data.records.iter().enumerate() {
        if stop_requested() {
            return Err("Import stopped".to_string());
        }
        conn.prepare_cached(
            "INSERT INTO entry(session_id,ordinal,external_entry_id,parent_entry_id,turn_id,occurred_at,kind,role,search_text,payload_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        ).map_err(|e| e.to_string())?.execute(params![session_id,ordinal as i64,record.external_id,record.parent,record.turn_id,record.occurred_at,
                record.kind,record.role,record.body,record.payload.to_string()],
        ).map_err(|e| format!("insert entry: {e}"))?;
        let entry_id = conn.last_insert_rowid();
        formats::insert_details(conn, session_id, entry_id, record)?;
    }
    // Results can precede calls in a log; correlate after all calls are inserted.
    for record in &data.records {
        for result in &record.results {
            if let Some(call_id) = &result.call_id {
                conn.prepare_cached(
                    "UPDATE tool_invocation SET result_text=?1,status=?2,exit_code=?3,ended_at=?4,
                     duration_ms=CASE WHEN started_at IS NOT NULL AND ?4 IS NOT NULL
                       THEN MAX(0,ROUND((julianday(?4)-julianday(started_at))*86400000)) ELSE duration_ms END
                     WHERE session_id=?5 AND call_id=?6
                       AND (result_text IS NOT ?1 OR status IS NOT ?2 OR exit_code IS NOT ?3 OR ended_at IS NOT ?4)",
                ).map_err(|e| e.to_string())?.execute(params![result.text,result.status,result.exit_code,record.occurred_at,session_id,call_id],
                ).map_err(|e| e.to_string())?;
            }
        }
    }
    let metrics = formats::session_metrics(conn, session_id)?;
    conn.execute(
        "UPDATE session SET metrics_json=?1 WHERE id=?2",
        params![metrics.to_string(), session_id],
    )
    .map_err(|e| e.to_string())?;
    crate::analytics::process_session(conn, pricing, session_id)?;
    set_file_state(conn, &data.uri, data.size, data.mtime).map_err(|e| e.to_string())?;
    Ok(data.records.len())
}

// ---------------------------------------------------------------------------
// syncAll (incremental / full)
// ---------------------------------------------------------------------------

fn push_error(summary: &mut SyncSummary, file: &str, error: String) {
    if summary.errors.len() >= MAX_ERRORS {
        summary.errors.remove(0);
    }
    summary.errors.push(SyncError {
        file: file.to_string(),
        error: error.chars().take(200).collect(),
    });
}

/// Mirror remote hosts, then import all (or one family's) sources. `mode` is
/// `"incremental"` or `"full"`; `progress` receives `(done, total, records)`.
/// The caller resets cancellation before making the run available to stop.
pub fn sync_all_with_progress(
    agent_filter: Option<&str>,
    mode: &str,
    progress: &dyn Fn(usize, usize, usize),
) -> SyncSummary {
    let run_start = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut summary = SyncSummary::default();
    let db_path = crate::db::db_path();
    let force = mode == "full";

    let mut families: Vec<String> = match agent_filter {
        Some(f) if !f.is_empty() => vec![f.to_string()],
        _ => all_sources().iter().map(|s| s.family.to_string()).collect(),
    };
    families.sort();
    families.dedup();

    // Refresh remote mirrors first; a host that fails keeps its last mirror,
    // so its already-fetched sessions still import below.
    for (host, error) in crate::remote::mirror_enabled_hosts(&|_| {}) {
        if stop_requested() {
            summary.cancelled = true;
            return summary;
        }
        push_error(&mut summary, &format!("{HOST_ERROR_PREFIX}{host}"), error);
    }

    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for fam in &families {
        if stop_requested() {
            summary.cancelled = true;
            return summary;
        }
        for path in discover_source_files(fam) {
            files.push((fam.clone(), path));
        }
    }
    if stop_requested() {
        summary.cancelled = true;
        return summary;
    }
    sort_by_mtime_desc_vec(&mut files);
    // Progress units: a JSONL/transcript file imports atomically (one unit),
    // while a SQLite database fans out into one unit per session row. Without
    // this a single `opencode.db` holding hundreds of sessions sits at `0/1`
    // until every session commits at once.
    let mut units: Vec<usize> = Vec::with_capacity(files.len());
    for (fam, path) in &files {
        if stop_requested() {
            summary.cancelled = true;
            return summary;
        }
        units.push(if is_database_file(path) {
            count_db_sessions(fam, path).unwrap_or(1)
        } else {
            1
        });
    }
    if stop_requested() {
        summary.cancelled = true;
        return summary;
    }
    summary.files_total = units.iter().sum();
    progress(0, summary.files_total, 0);

    let mut conn = match crate::db::open_db(&db_path) {
        Ok(c) => c,
        Err(e) => {
            push_error(&mut summary, "sync", e.to_string());
            return summary;
        }
    };
    if let Err(error) = crate::db::configure_bulk_writer(&conn) {
        push_error(&mut summary, "sync", error.to_string());
        return summary;
    }
    let mut pricing = ImportPricing::new();
    let mut failed: HashSet<String> = HashSet::new();
    for (index, (fam, path)) in files.iter().enumerate() {
        if stop_requested() {
            summary.cancelled = true;
            break;
        }
        let uri = format!("{}://{}", fam, path.display());
        if is_database_file(path) {
            // Progress counts committed batches; the current batch rolls back
            // on a source error or cancellation, and checkpoints do not advance.
            let expected = units.get(index).copied().unwrap_or(1);
            let mut sessions_reported = 0usize;
            let outcome = {
                let mut tick = |records_added: usize| {
                    sessions_reported += 1;
                    summary.files_done += 1;
                    summary.records += records_added;
                    if summary.files_done > summary.files_total {
                        summary.files_total = summary.files_done;
                    }
                    progress(summary.files_done, summary.files_total, summary.records);
                };
                import_file_with_conn(&mut conn, &uri, force, Some(&mut tick), &mut pricing)
            };
            match outcome {
                Ok(n) => {
                    if n == 0 {
                        summary.skipped += 1;
                    }
                    // Records were already folded per session above; only
                    // account units the import never iterated (file-level fast
                    // path, empty database). Never top up a stopped run.
                    if !stop_requested() {
                        top_up_units(&mut summary, expected, sessions_reported, progress);
                    }
                }
                Err(e) => {
                    if stop_requested() {
                        summary.cancelled = true;
                        break;
                    }
                    failed.insert(fam.clone());
                    push_error(
                        &mut summary,
                        &path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&uri)
                            .to_string(),
                        e,
                    );
                    top_up_units(&mut summary, expected, sessions_reported, progress);
                }
            }
        } else {
            match import_file_with_conn(&mut conn, &uri, force, None, &mut pricing) {
                Ok(n) => {
                    summary.records += n;
                    if n == 0 {
                        summary.skipped += 1;
                    }
                }
                Err(e) => {
                    if stop_requested() {
                        summary.cancelled = true;
                        break;
                    }
                    failed.insert(fam.clone());
                    push_error(
                        &mut summary,
                        &path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&uri)
                            .to_string(),
                        e,
                    );
                }
            }
            summary.files_done += 1;
            progress(summary.files_done, summary.files_total, summary.records);
        }
        if stop_requested() {
            summary.cancelled = true;
            break;
        }
    }

    summary.cancelled |= stop_requested();
    if !summary.cancelled {
        let _writer = crate::db::background_write();
        let done_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        for fam in &families {
            if !failed.contains(fam) {
                if let Err(e) = put_checkpoint(&conn, fam, &done_at, &run_start) {
                    push_error(&mut summary, fam, format!("save checkpoint: {e}"));
                }
            }
        }
    }
    summary
}

/// Whether this source file takes the SQLite path in [`import_file_with_conn`].
fn is_database_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("db")
}

/// Number of progress units in one SQLite source: one per session row.
/// `None` when the database cannot be read (the caller counts the file as a
/// single unit and the import itself reports the error).
fn count_db_sessions(family: &str, file: &Path) -> Option<usize> {
    // Table names are fixed by the adapters, never supplied by a source row.
    let table = match family {
        "opencode" => "session",
        "hermes" => "sessions",
        _ => return None,
    };
    let conn =
        Connection::open_with_flags(file, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    conn.busy_timeout(std::time::Duration::from_secs(5)).ok()?;
    let sql = format!("SELECT COUNT(*) FROM {table}");
    conn.query_row(&sql, [], |row| row.get::<_, i64>(0))
        .ok()
        .and_then(|n| usize::try_from(n).ok())
}

/// Mark the remaining units of a database file done when its import returned
/// without iterating them (unchanged-file fast path, empty database, or a
/// mid-file error which still rolls the whole source back).
fn top_up_units(
    summary: &mut SyncSummary,
    expected: usize,
    reported: usize,
    progress: &dyn Fn(usize, usize, usize),
) {
    let missing = expected.saturating_sub(reported);
    if missing > 0 {
        summary.files_done += missing;
        progress(summary.files_done, summary.files_total, summary.records);
    }
}

fn sort_by_mtime_desc_vec(files: &mut [(String, PathBuf)]) {
    files.sort_by_cached_key(|(_, path)| {
        std::cmp::Reverse(fs::metadata(path).and_then(|s| s.modified()).unwrap_or(UNIX_EPOCH))
    });
}
