//! Incremental session analytics with timezone-aware calendar bucketing,
//! cache-efficiency math, tool metrics, and productivity pulse.
//!
//! Wire format: the assembled JSON uses snake_case keys
//! (`session_leaderboards`, `by_model`, `daily_trend`, …) expected by the UI;
//! the pricing-shaped sub-objects serialize as camelCase (`pricingSource`,
//! `inPer1M`, …).
//!
//! Implementation notes:
//! - Dates are parsed with chrono (RFC3339 + `YYYY-MM-DD HH:MM:SS` +
//!   date-only), covering the stored `started_at` formats.
//! - Calendar bucketing uses IANA timezone rules, with fixed UTC/GMT offsets
//!   supported for older callers.
//! - 10s in-memory cache is a `Mutex<HashMap<String, (Instant, Value)>>`
//!   keyed by the JSON-encoded filters.
//! - Analytics sync batches 20 sessions per `BEGIN IMMEDIATE`/`COMMIT` to
//!   limit fsyncs.
//! - Query params use `&[&dyn ToSql]` (owned `String`/`f64`), since rusqlite
//!   does not implement `ToSql` for `serde_json::Value`.

use crate::pricing::{
    calculate_cost, estimate_tokens_from_chars, get_model_pricing, get_pricing_reference,
    get_pricing_status, load_pricing_cache, PricingState, TOKEN_CHARS_PER_TOKEN,
};
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, TimeDelta, Timelike, Utc};
use rusqlite::functions::{Context, FunctionFlags};
use rusqlite::{Connection, Row, Statement, types::ValueRef};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Cooperative cancellation probe shared by report calculations.
fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::SeqCst) {
        Err("Cancelled".into())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Serialize structs (camelCase wire format)
// ---------------------------------------------------------------------------

/// Query filters: `agent/model/workspace/timezone/since`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsFilters {
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub timezone: String,
    #[serde(default)]
    pub since: String,
    #[serde(default)]
    pub until: String,
}

/// Cache status snapshot embedded in the methodology note.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Methodology {
    pub pricing_source: String,
    pub pricing_source_url: String,
    pub pricing_updated_at: Option<String>,
    pub pricing_model_count: usize,
    pub custom_override_count: usize,
    pub token_note: String,
}

/// Bounded report cache; each calculation loads pricing from its own snapshot.
pub struct AnalyticsState {
    pub cache: Mutex<HashMap<String, (Instant, Value)>>,
    revision: AtomicU64,
}

impl AnalyticsState {
    pub fn new() -> Self {
        Self { cache: Mutex::new(HashMap::new()), revision: AtomicU64::new(0) }
    }

    pub fn clear_cache(&self) {
        self.revision.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }
}

/// One bounded report cache for the application, rather than one per request.
pub fn shared_state() -> Arc<AnalyticsState> {
    static STATE: OnceLock<Arc<AnalyticsState>> = OnceLock::new();
    STATE.get_or_init(|| Arc::new(AnalyticsState::new())).clone()
}

static PRICE_REVISION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn pricing_revision() -> u64 {
    PRICE_REVISION.load(Ordering::SeqCst)
}

pub(crate) fn queue_recalculation(conn: &Connection) -> Result<(), String> {
    conn.execute("INSERT OR IGNORE INTO analytics_refresh_queue SELECT id FROM session", [])
        .map_err(|e| sqlite_error("Could not queue analytics recalculation", e))?;
    Ok(())
}

pub(crate) fn prices_changed() {
    PRICE_REVISION.fetch_add(1, Ordering::SeqCst);
    shared_state().clear_cache();
}

#[derive(Default)]
struct RefreshState {
    running: bool,
    requested: bool,
    force: bool,
    error: Option<String>,
}

fn refresh_state() -> &'static Mutex<RefreshState> {
    static STATE: OnceLock<Mutex<RefreshState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(RefreshState::default()))
}

pub fn refresh_status() -> (bool, Option<String>) {
    let state = refresh_state().lock().unwrap_or_else(|e| e.into_inner());
    (state.running || state.requested, state.error.clone())
}

/// Coalesce recalculations and keep them off the report read path. Imports
/// already save fresh metrics atomically; this fills older/backup rows and
/// recalculates prices. Sync and maintenance get priority between batches.
pub fn request_refresh(path: &std::path::Path, force: bool) {
    let mut state = refresh_state().lock().unwrap_or_else(|e| e.into_inner());
    state.requested = true;
    state.force |= force;
    state.error = None;
    if state.running || crate::sync::is_running() || crate::sync::is_managing_data() {
        return;
    }
    state.running = true;
    let path = path.to_path_buf();
    let worker = std::thread::Builder::new()
        .name("analytics-refresh".into())
        .spawn(move || {
            loop {
                let force = {
                    let mut state = refresh_state().lock().unwrap_or_else(|e| e.into_inner());
                    if !state.requested {
                        state.running = false;
                        return;
                    }
                    state.requested = false;
                    std::mem::take(&mut state.force)
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let conn = crate::db::open_db(&path).map_err(|e| e.to_string())?;
                    crate::db::configure_bulk_writer(&conn).map_err(|e| e.to_string())?;
                    let pricing = Arc::new(Mutex::new(PricingState::new()));
                    sync_analytics(&conn, force, &pricing)
                }))
                .unwrap_or_else(|_| {
                    Err(
                        "Analytics calculation stopped unexpectedly. Refresh analytics to retry."
                            .into(),
                    )
                });
                shared_state().clear_cache();
                if let Err(error) = result {
                    let mut state = refresh_state().lock().unwrap_or_else(|e| e.into_inner());
                    let paused = error == crate::sync::ANALYTICS_PAUSED_MESSAGE;
                    if paused {
                        state.running = false;
                        state.requested = true;
                        state.force |= force;
                    } else {
                        state.error = Some(error);
                        // A request coalesced during this run still needs a
                        // worker. Keep single-flight ownership while retrying.
                        if state.requested {
                            continue;
                        }
                        state.running = false;
                    }
                    drop(state);
                    // A sync may have finished between the pause and this lock.
                    // Recheck after publishing the pending request, without polling.
                    if paused {
                        request_refresh(&path, false);
                    }
                    return;
                }
                refresh_state()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .error = None;
            }
        });
    if let Err(error) = worker {
        state.running = false;
        state.requested = false;
        state.error = Some(format!("Could not start analytics calculation: {error}"));
    }
}

// ---------------------------------------------------------------------------
// Calendar (timezone-aware bucketing)
// ---------------------------------------------------------------------------

fn has_timezone_suffix(text: &str) -> bool {
    if text.ends_with('Z') || text.ends_with('z') {
        return true;
    }
    let b = text.as_bytes();
    let is_digit = |c: u8| c.is_ascii_digit();
    if b.len() >= 6 {
        let s = b[b.len() - 6];
        if (s == b'+' || s == b'-')
            && is_digit(b[b.len() - 5])
            && is_digit(b[b.len() - 4])
            && b[b.len() - 3] == b':'
            && is_digit(b[b.len() - 2])
            && is_digit(b[b.len() - 1])
        {
            return true;
        }
    }
    if b.len() >= 5 {
        let s = b[b.len() - 5];
        if (s == b'+' || s == b'-')
            && is_digit(b[b.len() - 4])
            && is_digit(b[b.len() - 3])
            && is_digit(b[b.len() - 2])
            && is_digit(b[b.len() - 1])
        {
            return true;
        }
    }
    false
}

fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    let text = value.trim();
    if text.is_empty() {
        return None;
    }
    let text = text.replacen(' ', "T", 1);
    let text = if has_timezone_suffix(&text) { text } else { format!("{text}Z") };
    DateTime::parse_from_rfc3339(&text)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|| {
            NaiveDateTime::parse_from_str(&text, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|dt| dt.and_utc())
        })
        .or_else(|| {
            NaiveDate::parse_from_str(&text, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0).map(|t| t.and_utc()))
        })
}

struct Calendar {
    date: String,
    hour: i64,
    day: i64,
}

/// Fixed-offset minutes parsed from the `timezone` filter (`UTC`, `UTC+5:30`,
/// `GMT-8`, …). IANA names are handled separately by `calendar_in`.
fn timezone_offset_minutes(tz: &str) -> i32 {
    let t = tz.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("utc") || t.eq_ignore_ascii_case("gmt") || t.eq_ignore_ascii_case("z") {
        return 0;
    }
    let upper = t.to_ascii_uppercase();
    let rest = upper.strip_prefix("UTC").or_else(|| upper.strip_prefix("GMT")).unwrap_or(upper.as_str());
    let rest = rest.trim();
    if rest.is_empty() {
        return 0;
    }
    let (sign, digits) = match rest.strip_prefix('+') {
        Some(d) => (1, d),
        None => match rest.strip_prefix('-') {
            Some(d) => (-1, d),
            None => return 0,
        },
    };
    let digits = digits.trim();
    // Accept HH, HHMM, HH:MM.
    let (h, m) = if let Some((a, b)) = digits.split_once(':') {
        (a.trim().parse::<i32>().unwrap_or(0), b.trim().parse::<i32>().unwrap_or(0))
    } else if digits.len() > 2 && !digits.contains(' ') {
        let (a, b) = digits.split_at(digits.len() - 2);
        (a.parse::<i32>().unwrap_or(0), b.parse::<i32>().unwrap_or(0))
    } else {
        (digits.parse::<i32>().unwrap_or(0), 0)
    };
    (sign * (h * 60 + m)).clamp(-14 * 60, 14 * 60)
}

fn calendar_in(value: &str, timezone: &str) -> Option<Calendar> {
    let parsed = parse_date(value)?;
    let naive = if let Ok(tz) = timezone.parse::<chrono_tz::Tz>() {
        parsed.with_timezone(&tz).naive_local()
    } else {
        (parsed + TimeDelta::minutes(timezone_offset_minutes(timezone) as i64)).naive_utc()
    };
    Some(Calendar {
        date: naive.format("%Y-%m-%d").to_string(),
        hour: naive.hour() as i64,
        day: naive.weekday().number_from_sunday() as i64 - 1,
    })
}

fn register_calendar_functions(db: &Connection, timezone: &str) -> Result<(), String> {
    let tz = timezone.to_string();
    db.create_scalar_function(
        "user_date",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx: &Context| {
            let v: Option<String> = ctx.get(0)?;
            Ok(v.as_deref().and_then(|t| calendar_in(t, &tz)).map(|c| c.date))
        },
    )
    .map_err(|e| format!("Could not register user_date: {e}"))?;
    let tz = timezone.to_string();
    db.create_scalar_function(
        "user_hour",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx: &Context| {
            let v: Option<String> = ctx.get(0)?;
            Ok(v.as_deref().and_then(|t| calendar_in(t, &tz)).map(|c| c.hour))
        },
    )
    .map_err(|e| format!("Could not register user_hour: {e}"))?;
    let tz = timezone.to_string();
    db.create_scalar_function(
        "user_weekday",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx: &Context| {
            let v: Option<String> = ctx.get(0)?;
            Ok(v.as_deref().and_then(|t| calendar_in(t, &tz)).map(|c| c.day))
        },
    )
    .map_err(|e| format!("Could not register user_weekday: {e}"))?;
    Ok(())
}

fn register_cache_discount(db: &Connection, pricing: Arc<Mutex<PricingState>>) -> Result<(), String> {
    db.create_scalar_function(
        "cache_discount",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |ctx: &Context| {
            let model: Option<String> = ctx.get(0).unwrap_or(None);
            let mut st = pricing.lock().map_err(|_| rusqlite::Error::InvalidQuery)?;
            let p = get_model_pricing(&mut st, &model.unwrap_or_default());
            Ok((p.in_per_1m - p.cache_per_1m).max(0.0) / 1_000_000.0)
        },
    )
    .map_err(|e| format!("Could not register cache_discount: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// sync_analytics
// ---------------------------------------------------------------------------

fn sqlite_error(ctx: &str, e: rusqlite::Error) -> String {
    format!("{ctx}: {e}")
}

#[derive(Serialize)]
pub(crate) struct SessionAnalytics {
    session_id: i64,
    agent_id: i64,
    workspace_id: Option<i64>,
    started_at: Option<String>,
    last_event_at: Option<String>,
    pub duration_seconds: f64,
    pub model_name: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cached_tokens: i64,
    pub total_tokens: i64,
    pub estimated_cost: f64,
    pub user_turns: i64,
    pub assistant_turns: i64,
    pub total_turns: i64,
    pub tool_count: i64,
    pub tool_error_count: i64,
}

/// Read-only calculation, also used for exports without claiming the writer.
pub(crate) fn calculate_session(db: &Connection, pricing: &Arc<Mutex<PricingState>>, id: i64) -> Result<SessionAnalytics, String> {
    let (agent_id, workspace_id, started_at, last_event_at, metrics_json, token_usage_json, model_name, duration_seconds_opt): (
        i64, Option<i64>, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<f64>,
    ) = db
        .query_row(
            "SELECT s.agent_id, s.workspace_id, s.started_at, s.last_event_at,
                    s.metrics_json, s.token_usage_json,
                    (SELECT m.model_key FROM session_context c JOIN model m ON m.id=c.model_id
                      WHERE c.session_id=s.id ORDER BY c.id DESC LIMIT 1) AS model_name,
                    COALESCE(MAX(0, ROUND((julianday(COALESCE(s.last_event_at,s.started_at))-julianday(s.started_at))*86400)),0) AS duration_seconds
             FROM session s WHERE s.id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?)),
        )
        .map_err(|e| sqlite_error("Could not read the analytics session", e))?;
    // NULL/missing/NaN duration -> 0.
    let duration_seconds: f64 = duration_seconds_opt.filter(|v| v.is_finite()).unwrap_or(0.0);

    // Use cached metrics only when every required count is present. Older
    // imports contain partial blobs and must fall back to the actual rows.
    let (total_turns, user_turns, assistant_turns, user_chars, assistant_chars, tool_chars): (i64, i64, i64, i64, i64, i64) = {
        let metrics: Option<Value> = metrics_json.as_deref().and_then(|s| serde_json::from_str(s).ok());
        match metrics.filter(|m| ["total_turns", "user_turns", "assistant_turns", "user_chars", "assistant_chars", "tool_chars"]
            .iter().all(|key| m.get(*key).and_then(Value::as_i64).is_some_and(|n| n >= 0))) {
            Some(m) => (
                m.get("total_turns").and_then(|v| v.as_i64()).unwrap_or(0),
                m.get("user_turns").and_then(|v| v.as_i64()).unwrap_or(0),
                m.get("assistant_turns").and_then(|v| v.as_i64()).unwrap_or(0),
                m.get("user_chars").and_then(|v| v.as_i64()).unwrap_or(0),
                m.get("assistant_chars").and_then(|v| v.as_i64()).unwrap_or(0),
                m.get("tool_chars").and_then(|v| v.as_i64()).unwrap_or(0),
            ),
            None => db
                .query_row(
                    "SELECT COUNT(*),
                        COALESCE(SUM(role='user'),0), COALESCE(SUM(role='assistant'),0),
                        COALESCE(SUM(CASE WHEN role='user' THEN length(search_text) ELSE 0 END),0),
                        COALESCE(SUM(CASE WHEN role='assistant' THEN length(search_text) ELSE 0 END),0),
                        COALESCE(SUM(CASE WHEN role IS NOT 'user' AND role IS NOT 'assistant' THEN length(search_text) ELSE 0 END),0)
                     FROM entry WHERE session_id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
                )
                .map_err(|e| sqlite_error("Could not read the analytics entries", e))?,
        }
    };

    let (tool_count, tool_error_count): (i64, i64) = {
        let metrics: Option<Value> = metrics_json.as_deref().and_then(|s| serde_json::from_str(s).ok());
        match metrics.filter(|m| ["tool_count", "tool_error_count"].iter()
            .all(|key| m.get(*key).and_then(Value::as_i64).is_some_and(|n| n >= 0))) {
            Some(m) => (
                m.get("tool_count").and_then(|v| v.as_i64()).unwrap_or(0),
                m.get("tool_error_count").and_then(|v| v.as_i64()).unwrap_or(0),
            ),
            None => db
                .query_row(
                    "SELECT COUNT(*),
                        COALESCE(SUM(status='error' OR (exit_code IS NOT NULL AND exit_code!=0)),0)
                     FROM tool_invocation WHERE session_id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| sqlite_error("Could not read the analytics tools", e))?,
        }
    };

    // Token counts prefer actual reported usage; otherwise estimate from text
    // length at TOKEN_CHARS_PER_TOKEN chars per token.
    let usage: Option<Value> = token_usage_json.as_deref().and_then(|s| serde_json::from_str(s).ok());
    let usage_num = |key: &str| {
        usage.as_ref().and_then(|u| u.get(key)).filter(|v| !v.is_null()).and_then(|v| v.as_f64()).filter(|n| n.is_finite() && *n >= 0.0)
    };
    let (est_in, est_out) = estimate_tokens_from_chars(user_chars, assistant_chars, tool_chars);
    let input = usage_num("inputTokens").map(|v| v as i64).unwrap_or(est_in);
    let output = usage_num("outputTokens").map(|v| v as i64).unwrap_or(est_out);
    let cached = usage_num("cachedTokens").map(|v| v as i64).unwrap_or(0);
    let total_tokens = input.checked_add(output)
        .ok_or("Session token counts exceed the supported range")?;

    let estimated_cost = {
        let mut st = pricing.lock().map_err(|_| "Could not lock the pricing state".to_string())?;
        calculate_cost(&mut st, &model_name.clone().unwrap_or_default(), input, output, cached)
    };

    Ok(SessionAnalytics {
        session_id: id, agent_id, workspace_id, started_at, last_event_at,
        duration_seconds, model_name, input_tokens: input, output_tokens: output,
        cached_tokens: cached, total_tokens, estimated_cost,
        user_turns, assistant_turns, total_turns, tool_count, tool_error_count,
    })
}

pub(crate) fn process_session(db: &Connection, pricing: &Arc<Mutex<PricingState>>, id: i64) -> Result<(), String> {
    let row = calculate_session(db, pricing, id)?;
    db.prepare_cached(
        "INSERT OR REPLACE INTO session_analytics (
           session_id, agent_id, workspace_id, started_at, last_event_at, duration_seconds, model_name,
           input_tokens, output_tokens, cached_tokens, total_tokens, estimated_cost,
           user_turns, assistant_turns, total_turns, tool_count, tool_error_count, calculated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,CURRENT_TIMESTAMP)"
    ).and_then(|mut statement| statement.execute(rusqlite::params![
        row.session_id, row.agent_id, row.workspace_id, row.started_at, row.last_event_at,
        row.duration_seconds, row.model_name, row.input_tokens, row.output_tokens,
        row.cached_tokens, row.total_tokens, row.estimated_cost, row.user_turns,
        row.assistant_turns, row.total_turns, row.tool_count, row.tool_error_count,
    ])).map_err(|e| sqlite_error("Could not save the session analytics", e))?;
    db.prepare_cached("DELETE FROM analytics_refresh_queue WHERE session_id=?")
        .and_then(|mut statement| statement.execute([id]))
        .map_err(|e| sqlite_error("Could not complete analytics recalculation", e))?;
    Ok(())
}

/// Recompute per-session analytics rows. Returns sessions processed.
pub fn sync_analytics(
    db: &Connection,
    force: bool,
    pricing: &Arc<Mutex<PricingState>>,
) -> Result<usize, String> {
    sync_analytics_cancellable(db, force, pricing, &std::sync::atomic::AtomicBool::new(false))
}

pub fn sync_analytics_cancellable(
    db: &Connection,
    force: bool,
    pricing: &Arc<Mutex<PricingState>>,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<usize, String> {
    crate::sync::ensure_analytics_writer_available()?;
    check_cancelled(cancelled)?;
    {
        let mut st = pricing
            .lock()
            .map_err(|_| "Could not lock the pricing state".to_string())?;
        load_pricing_cache(db, &mut st)?;
    }
    if force {
        let _writer = crate::db::background_write();
        crate::sync::ensure_analytics_writer_available()?;
        queue_recalculation(db)?;
    }
    let ids: Vec<i64> = {
        let mut stmt = db
            .prepare(
                "SELECT s.id FROM session s
                 LEFT JOIN session_analytics sa ON sa.session_id=s.id
                 WHERE sa.session_id IS NULL
                    OR s.id IN (SELECT session_id FROM analytics_refresh_queue)
                 ORDER BY s.id",
            )
            .map_err(|e| sqlite_error("Could not read the pending sessions", e))?;
        let mapped = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| sqlite_error("Could not read the pending sessions", e))?;
        let mut out: Vec<i64> = Vec::new();
        for r in mapped {
            out.push(r.map_err(|e| sqlite_error("Could not decode a pending session", e))?);
        }
        out
    };
    let mut processed = 0;
    let mut failures = 0;
    let mut first_error = None;
    // Group into batches of 20 per transaction to avoid an fsync per session.
    for chunk in ids.chunks(20) {
        crate::sync::ensure_analytics_writer_available()?;
        check_cancelled(cancelled)?;
        let _writer = crate::db::background_write();
        crate::sync::ensure_analytics_writer_available()?;
        {
            let mut state = pricing.lock().map_err(|_| "Could not lock pricing")?;
            load_pricing_cache(db, &mut state)?;
        }
        let tx = rusqlite::Transaction::new_unchecked(db, rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| sqlite_error("Could not start the analytics batch", e))?;
        for id in chunk {
            let exists: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM session WHERE id=?)",
                    [id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?;
            if !exists {
                continue;
            }
            check_cancelled(cancelled)?;
            tx.execute_batch("SAVEPOINT analytics_session")
                .map_err(|e| sqlite_error("Could not start the session calculation", e))?;
            match process_session(&tx, pricing, *id) {
                Ok(()) => {
                    tx.execute_batch("RELEASE analytics_session")
                        .map_err(|e| sqlite_error("Could not finish the session calculation", e))?;
                    processed += 1;
                }
                Err(error) => {
                    tx.execute_batch("ROLLBACK TO analytics_session; RELEASE analytics_session")
                        .map_err(|e| {
                            sqlite_error("Could not roll back the session calculation", e)
                        })?;
                    failures += 1;
                    if first_error.is_none() {
                        first_error = Some(format!("session {id}: {error}"));
                    }
                }
            }
        }
        tx.commit()
            .map_err(|e| sqlite_error("Could not commit the analytics batch", e))?;
        shared_state().clear_cache();
    }
    if let Some(error) = first_error {
        return Err(format!("Analytics updated {processed} sessions; {failures} failed and remain pending. First failure: {error}"));
    }
    Ok(processed)
}

// ---------------------------------------------------------------------------
// Query helpers
// ---------------------------------------------------------------------------

fn ref_to_json(v: ValueRef) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => json!(n),
        ValueRef::Real(n) => json!(n),
        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => json!(String::from_utf8_lossy(b)),
    }
}

fn rows_to_json(stmt: &mut Statement, params: &[&dyn rusqlite::types::ToSql]) -> Result<Vec<Value>, rusqlite::Error> {
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mapped = stmt.query_map(params, {
        let cols = cols.clone();
        move |row: &Row| {
            let mut m = Map::new();
            for (i, name) in cols.iter().enumerate() {
                m.insert(name.clone(), ref_to_json(row.get_ref(i)?));
            }
            Ok(Value::Object(m))
        }
    })?;
    let mut out: Vec<Value> = Vec::new();
    for r in mapped {
        out.push(r?);
    }
    Ok(out)
}

fn one_to_json(stmt: &mut Statement, params: &[&dyn rusqlite::types::ToSql]) -> Result<Option<Value>, rusqlite::Error> {
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt.query(params)?;
    match rows.next()? {
        Some(row) => {
            let mut m = Map::new();
            for (i, name) in cols.iter().enumerate() {
                m.insert(name.clone(), ref_to_json(row.get_ref(i)?));
            }
            Ok(Some(Value::Object(m)))
        }
        None => Ok(None),
    }
}

fn build_filters(q: &AnalyticsFilters) -> (String, Vec<String>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    if !q.agent.trim().is_empty() {
        clauses.push("a.family = ?".to_string());
        params.push(q.agent.trim().to_string());
    }
    if !q.workspace.trim().is_empty() {
        clauses.push("(w.path = ? OR w.display_name = ?)".to_string());
        params.push(q.workspace.trim().to_string());
        params.push(q.workspace.trim().to_string());
    }
    if !q.model.trim().is_empty() {
        clauses.push("sa.model_name = ?".to_string());
        params.push(q.model.trim().to_string());
    }
    if !q.since.trim().is_empty() {
        // Keep julianday() here: plain string >= misorders millisecond
        // boundaries when stored/param formats mix.
        let days: Option<i64> = match q.since.trim() {
            "day" => Some(1),
            "week" => Some(7),
            "month" => Some(30),
            "3months" => Some(90),
            "year" => Some(365),
            _ => None,
        };
        if let Some(days) = days {
            let cutoff = Utc::now() - TimeDelta::milliseconds(days * 86_400_000);
            clauses.push("julianday(COALESCE(sa.started_at, s.started_at)) >= julianday(?)".to_string());
            params.push(cutoff.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        } else if let Some(parsed) = parse_date(q.since.trim()) {
            clauses.push("julianday(COALESCE(sa.started_at, s.started_at)) >= julianday(?)".to_string());
            params.push(parsed.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        }
    }
    if !q.until.trim().is_empty() {
        clauses.push(if q.since.trim().is_empty() {
            "(sa.started_at IS NULL OR julianday(sa.started_at) IS NULL OR julianday(sa.started_at) < julianday(?))".to_string()
        } else {
            "julianday(COALESCE(sa.started_at, s.started_at)) < julianday(?)".to_string()
        });
        params.push(q.until.trim().to_string());
    }
    // Guard against bogus future years from nanosecond timestamps.
    clauses.push("(sa.started_at IS NULL OR julianday(sa.started_at) IS NULL OR julianday(sa.started_at) <= julianday('2030-01-01'))".to_string());
    (format!("WHERE {}", clauses.join(" AND ")), params)
}

const BASE_JOIN: &str = "
    FROM session_analytics sa
    JOIN session s ON s.id = sa.session_id
    JOIN agent a ON a.id = sa.agent_id
    LEFT JOIN workspace w ON w.id = sa.workspace_id
";

const CACHE_COLUMNS: &str = "
    COALESCE(SUM(MIN(MAX(sa.cached_tokens, 0), MAX(sa.input_tokens, 0))), 0) AS cache_hit_tokens,
    COALESCE(SUM(MIN(MAX(sa.cached_tokens, 0), MAX(sa.input_tokens, 0)) * cache_discount(sa.model_name)), 0) AS cache_savings,
    COALESCE(SUM(MAX(0, sa.input_tokens - MAX(sa.cached_tokens, 0)) * cache_discount(sa.model_name)), 0) AS potential_cache_savings";

fn apply_efficiency(row: &mut Value) {
    let Some(o) = row.as_object_mut() else { return };
    let f = |k: &str| o.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let (input, hits, turns, cost, dur, days) =
        (f("input_tokens"), f("cache_hit_tokens"), f("total_turns"), f("estimated_cost"), f("total_duration_seconds"), f("active_days"));
    o.insert("cache_hit_pct".into(), json!(if input > 0.0 { hits / input * 100.0 } else { 0.0 }));
    o.insert("cost_per_turn".into(), if turns > 0.0 { json!(cost / turns) } else { Value::Null });
    o.insert("cost_per_hour".into(), if dur > 0.0 { json!(cost * 3600.0 / dur) } else { Value::Null });
    o.insert("cost_per_day".into(), if days > 0.0 { json!(cost / days) } else { Value::Null });
}

fn round_to(v: f64, decimals: u32) -> f64 {
    let f = 10f64.powi(decimals as i32);
    (v * f).round() / f
}

fn shift_day(date: &str, days: i64) -> String {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.checked_add_signed(TimeDelta::days(days)))
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| date.to_string())
}

fn empty_day(date: &str) -> Value {
    json!({ "date": date, "sessions": 0, "input_tokens": 0, "output_tokens": 0,
        "total_tokens": 0, "estimated_cost": 0, "turns": 0, "user_turns": 0,
        "assistant_turns": 0, "tool_calls": 0, "tool_errors": 0 })
}

fn fill_days(rows: Vec<Value>, start: Option<String>, end: Option<String>) -> Vec<Value> {
    let (Some(start), Some(end)) = (start, end) else { return rows };
    let mut map: HashMap<String, Value> = HashMap::new();
    for r in rows {
        if let Some(d) = r.get("date").and_then(|v| v.as_str()) {
            map.insert(d.to_string(), r);
        }
    }
    let mut out = Vec::new();
    let mut date = start;
    while date <= end {
        let next = shift_day(&date, 1);
        if next <= date {
            break;
        }
        out.push(map.remove(&date).unwrap_or_else(|| empty_day(&date)));
        date = next;
    }
    out
}

fn month_end(today: &str) -> String {
    let year: i32 = today.get(..4).and_then(|s| s.parse().ok()).unwrap_or(2024);
    let month: u32 = today.get(5..7).and_then(|s| s.parse().ok()).unwrap_or(1);
    let first_next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    first_next
        .and_then(|d| d.pred_opt())
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| today.to_string())
}

// ---------------------------------------------------------------------------
// get_full_analytics
// ---------------------------------------------------------------------------

/// Full analytics payload (KPIs, breakdowns, trends, tool metrics, pulse).
/// Returns `serde_json::Value` with snake_case keys.
pub fn get_full_analytics(
    db: &Connection,
    filters: &AnalyticsFilters,
    state: &AnalyticsState,
) -> Result<Value, String> {
    let revision = state.revision.load(Ordering::SeqCst);
    let cache_key = json!({
        "revision": revision, "agent": filters.agent, "model": filters.model,
        "workspace": filters.workspace, "timezone": filters.timezone, "since": filters.since, "until": filters.until,
    })
    .to_string();
    {
        let cache = state.cache.lock().map_err(|_| "Could not lock the analytics cache".to_string())?;
        if let Some((at, data)) = cache.get(&cache_key) {
            if at.elapsed() < Duration::from_secs(10) {
                return Ok(data.clone());
            }
        }
    }

    let report_pricing = Arc::new(Mutex::new(PricingState::new()));
    let timezone = filters.timezone.clone();
    register_calendar_functions(db, &timezone)?;

    let pricing_meta = {
        let mut p = report_pricing.lock().map_err(|_| "Could not lock the pricing state".to_string())?;
        get_pricing_status(db, &mut p)?
    };
    let pricing_meta_json = serde_json::to_value(&pricing_meta).unwrap_or(json!({}));

    register_cache_discount(db, report_pricing.clone())?;

    let (clause, owned) = build_filters(filters);
    let refs: Vec<&dyn rusqlite::types::ToSql> = owned.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();

    // 1. Overall KPIs
    let summary_row: Value = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT COUNT(sa.session_id) as total_sessions,
                    COALESCE(SUM(sa.input_tokens),0) as input_tokens,
                    COALESCE(SUM(sa.output_tokens),0) as output_tokens,
                    COALESCE(SUM(sa.cached_tokens),0) as cached_tokens,
                    COALESCE(SUM(sa.total_tokens),0) as total_tokens,
                    COALESCE(SUM(sa.estimated_cost),0) as estimated_cost,
                    COALESCE(SUM(sa.total_turns),0) as total_turns,
                    COALESCE(SUM(sa.user_turns),0) as user_turns,
                    COALESCE(SUM(sa.assistant_turns),0) as assistant_turns,
                    COALESCE(SUM(sa.tool_count),0) as total_tool_calls,
                    COALESCE(SUM(sa.tool_error_count),0) as tool_error_count,
                    COALESCE(SUM(sa.duration_seconds),0) as total_duration_seconds,
                    COALESCE(AVG(sa.duration_seconds),0) as avg_duration_seconds,
                    COUNT(DISTINCT user_date(sa.started_at)) AS active_days,
                    {CACHE_COLUMNS} {BASE_JOIN} {clause}"
            ))
            .map_err(|e| sqlite_error("Could not prepare the KPI query", e))?;
        one_to_json(&mut stmt, &refs)
            .map_err(|e| sqlite_error("Could not run the KPI query", e))?
            .ok_or_else(|| "No KPI row returned.".to_string())?
    };
    let f = |k: &str| summary_row.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let total_sessions = summary_row.get("total_sessions").and_then(|v| v.as_i64()).unwrap_or(0);
    let summary_cost = f("estimated_cost");
    let avg_cost = if total_sessions > 0 { summary_cost / total_sessions as f64 } else { 0.0 };
    let avg_turns = if total_sessions > 0 { f("total_turns") / total_sessions as f64 } else { 0.0 };
    let tool_error_rate = if f("total_tool_calls") > 0.0 { f("tool_error_count") / f("total_tool_calls") * 100.0 } else { 0.0 };

    // 2. By model
    let mut by_model = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT sa.model_name, COUNT(sa.session_id) as sessions,
                    SUM(sa.input_tokens) as input_tokens, SUM(sa.output_tokens) as output_tokens,
                    SUM(sa.cached_tokens) as cached_tokens, SUM(sa.total_tokens) as total_tokens,
                    SUM(sa.estimated_cost) as estimated_cost, SUM(sa.total_turns) AS total_turns,
                    COALESCE(SUM(sa.duration_seconds),0) AS total_duration_seconds,
                    COUNT(DISTINCT user_date(sa.started_at)) AS active_days,
                    {CACHE_COLUMNS} {BASE_JOIN} {clause}
                 GROUP BY sa.model_name ORDER BY estimated_cost DESC, total_tokens DESC"
            ))
            .map_err(|e| sqlite_error("Could not prepare the by-model query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the by-model query", e))?
    };
    for row in by_model.iter_mut() {
        apply_efficiency(row);
        let cost = row.get("estimated_cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let name = row.get("model_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let pricing = {
            let mut p = report_pricing.lock().map_err(|_| "Could not lock the pricing state".to_string())?;
            get_model_pricing(&mut p, &name)
        };
        if let Some(o) = row.as_object_mut() {
            o.insert("cost_pct".into(), json!(if summary_cost > 0.0 { cost / summary_cost * 100.0 } else { 0.0 }));
            o.insert("pricing".into(), serde_json::to_value(&pricing).unwrap_or(Value::Null));
        }
    }

    // 3. By agent
    let mut by_agent = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT a.family, a.product, COUNT(sa.session_id) as sessions,
                    SUM(sa.input_tokens) as input_tokens, SUM(sa.output_tokens) as output_tokens,
                    SUM(sa.cached_tokens) as cached_tokens, SUM(sa.total_tokens) as total_tokens,
                    SUM(sa.estimated_cost) as estimated_cost, SUM(sa.total_turns) as total_turns,
                    SUM(sa.tool_count) as tool_count, SUM(sa.tool_error_count) as tool_error_count,
                    COALESCE(SUM(sa.duration_seconds),0) AS total_duration_seconds,
                    COUNT(DISTINCT user_date(sa.started_at)) AS active_days,
                    {CACHE_COLUMNS} {BASE_JOIN} {clause}
                 GROUP BY a.id ORDER BY estimated_cost DESC, sessions DESC"
            ))
            .map_err(|e| sqlite_error("Could not prepare the by-agent query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the by-agent query", e))?
    };
    for row in by_agent.iter_mut() {
        apply_efficiency(row);
        let (cost, tc, te) = (
            row.get("estimated_cost").and_then(|v| v.as_f64()).unwrap_or(0.0),
            row.get("tool_count").and_then(|v| v.as_f64()).unwrap_or(0.0),
            row.get("tool_error_count").and_then(|v| v.as_f64()).unwrap_or(0.0),
        );
        if let Some(o) = row.as_object_mut() {
            o.insert("cost_pct".into(), json!(if summary_cost > 0.0 { cost / summary_cost * 100.0 } else { 0.0 }));
            o.insert("error_rate".into(), json!(if tc > 0.0 { te / tc * 100.0 } else { 0.0 }));
        }
    }

    // 4. By workspace
    let mut by_workspace = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT COALESCE(w.display_name, w.path, 'Other') as name, w.path,
                    COUNT(sa.session_id) as sessions, SUM(sa.total_tokens) as total_tokens,
                    SUM(sa.input_tokens) as input_tokens, SUM(sa.output_tokens) as output_tokens,
                    SUM(sa.estimated_cost) as estimated_cost, SUM(sa.total_turns) as total_turns,
                    COALESCE(SUM(sa.duration_seconds),0) AS total_duration_seconds,
                    COUNT(DISTINCT user_date(sa.started_at)) AS active_days,
                    {CACHE_COLUMNS} {BASE_JOIN} {clause}
                 GROUP BY sa.workspace_id ORDER BY estimated_cost DESC, total_tokens DESC"
            ))
            .map_err(|e| sqlite_error("Could not prepare the by-workspace query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the by-workspace query", e))?
    };
    for row in by_workspace.iter_mut() {
        apply_efficiency(row);
        let cost = row.get("estimated_cost").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if let Some(o) = row.as_object_mut() {
            o.insert("cost_pct".into(), json!(if summary_cost > 0.0 { cost / summary_cost * 100.0 } else { 0.0 }));
        }
    }

    // 5. Daily trend + quiet-day fill + 7d moving average
    let daily_rows = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT user_date(sa.started_at) as date, COUNT(sa.session_id) as sessions,
                    SUM(sa.input_tokens) as input_tokens, SUM(sa.output_tokens) as output_tokens,
                    SUM(sa.total_tokens) as total_tokens, SUM(sa.estimated_cost) as estimated_cost,
                    SUM(sa.total_turns) as turns, SUM(sa.user_turns) AS user_turns,
                    SUM(sa.assistant_turns) AS assistant_turns, SUM(sa.tool_count) as tool_calls,
                    SUM(sa.tool_error_count) as tool_errors
                 {BASE_JOIN} {clause}
                 GROUP BY date HAVING date IS NOT NULL AND date >= '2024-01-01' AND date <= '2030-01-01'
                 ORDER BY date ASC"
            ))
            .map_err(|e| sqlite_error("Could not prepare the daily trend query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the daily trend query", e))?
    };
    let now_iso = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let today = calendar_in(&now_iso, &timezone).map(|c| c.date).unwrap_or_else(|| Utc::now().date_naive().format("%Y-%m-%d").to_string());
    let preset_days: Option<i64> = match filters.since.trim() {
        "day" => Some(1),
        "week" => Some(7),
        "month" => Some(30),
        "3months" => Some(90),
        "year" => Some(365),
        _ => None,
    };
    let requested_start = preset_days
        .and_then(|d| calendar_in(&(Utc::now() - TimeDelta::milliseconds(d * 86_400_000)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true), &timezone))
        .map(|c| c.date)
        .or_else(|| if filters.since.trim().is_empty() { None } else { calendar_in(filters.since.trim(), &timezone).map(|c| c.date) });
    let trend_start = requested_start.or_else(|| daily_rows.first().and_then(|r| r.get("date").and_then(|d| d.as_str()).map(str::to_string)));
    let trend_start = match trend_start {
        Some(s) if s.as_str() < "2024-01-01" => Some("2024-01-01".to_string()),
        Some(s) => Some(s),
        None => None,
    };
    let mut daily_trend = fill_days(daily_rows, trend_start, Some(today.clone()));
    for i in 0..daily_trend.len() {
        let lo = i.saturating_sub(6);
        let window: Vec<Value> = daily_trend[lo..=i].to_vec();
        let mut ma = Map::new();
        for key in ["estimated_cost", "total_tokens", "sessions", "turns"] {
            let sum: f64 = window.iter().map(|d| d.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0)).sum();
            ma.insert(key.to_string(), json!(sum / window.len() as f64));
        }
        if let Some(o) = daily_trend[i].as_object_mut() {
            o.insert("moving_average".into(), Value::Object(ma));
            let (u, a) = (
                o.get("user_turns").and_then(|v| v.as_f64()).unwrap_or(0.0),
                o.get("assistant_turns").and_then(|v| v.as_f64()).unwrap_or(0.0),
            );
            o.insert("user_assistant_ratio".into(), if a > 0.0 { json!(u / a) } else { Value::Null });
        }
    }

    // Spend outlook (full calendar weeks + month-to-date).
    let month_start = format!("{}-01", &today[..7]);
    let prior_week_start = shift_day(&today, -14);
    let comparison_start = if month_start < prior_week_start { month_start.clone() } else { prior_week_start.clone() };
    let comp_filters = AnalyticsFilters {
        agent: filters.agent.clone(),
        model: filters.model.clone(),
        workspace: filters.workspace.clone(),
        timezone: filters.timezone.clone(),
        since: format!("{}T00:00:00Z", shift_day(&comparison_start, -2)),
        until: filters.until.clone(),
    };
    let (comp_clause, comp_owned) = build_filters(&comp_filters);
    let comparison_rows = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT user_date(sa.started_at) AS date, COUNT(*) AS sessions,
                    SUM(sa.total_tokens) AS total_tokens, SUM(sa.total_turns) AS turns,
                    SUM(sa.estimated_cost) AS estimated_cost
                 {BASE_JOIN} {comp_clause}
                   AND user_date(sa.started_at) >= ? AND user_date(sa.started_at) <= ?
                 GROUP BY date ORDER BY date"
            ))
            .map_err(|e| sqlite_error("Could not prepare the comparison query", e))?;
        let mut all_owned = comp_owned.clone();
        all_owned.push(comparison_start.clone());
        all_owned.push(today.clone());
        let all_refs: Vec<&dyn rusqlite::types::ToSql> = all_owned.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
        rows_to_json(&mut stmt, &all_refs).map_err(|e| sqlite_error("Could not run the comparison query", e))?
    };
    let comparison_days = fill_days(comparison_rows, Some(comparison_start.clone()), Some(today.clone()));
    let sum_days = |start: &str, end: &str, key: &str| -> f64 {
        comparison_days.iter()
            .filter(|d| { let dt = d.get("date").and_then(|v| v.as_str()).unwrap_or(""); dt >= start && dt <= end })
            .map(|d| d.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0)).sum()
    };
    let recent_start = shift_day(&today, -7);
    let recent_end = shift_day(&today, -1);
    let mut weekly_deltas = Map::new();
    for key in ["estimated_cost", "total_tokens", "sessions", "turns"] {
        let cur = sum_days(&recent_start, &recent_end, key);
        let prev = sum_days(&prior_week_start, &shift_day(&today, -8), key);
        let pct = if prev > 0.0 { (cur - prev) / prev * 100.0 } else if cur == 0.0 { 0.0 } else { f64::NAN };
        weekly_deltas.insert(key.to_string(), json!({ "current": cur, "previous": prev, "pct_change": pct }));
    }
    let month_end_date = month_end(&today);
    let remaining: f64 = month_end_date[8..].parse::<f64>().unwrap_or(0.0) - today[8..].parse::<f64>().unwrap_or(0.0);
    let daily_avg = weekly_deltas.get("estimated_cost").and_then(|v| v.get("current")).and_then(|v| v.as_f64()).unwrap_or(0.0) / 7.0;
    let mtd = sum_days(&month_start, &today, "estimated_cost");
    let spend_outlook = json!({
        "weekly_deltas": weekly_deltas, "recent_start": recent_start, "recent_end": recent_end,
        "prior_start": prior_week_start, "prior_end": shift_day(&today, -8),
        "daily_average_cost": daily_avg, "month_to_date_cost": mtd,
        "forecast_month_end_cost": mtd + daily_avg * remaining, "month_end": month_end_date,
    });

    // Session leaderboards
    let mut leaderboards = Map::new();
    for metric in ["estimated_cost", "total_tokens", "duration_seconds", "total_turns"] {
        let mut stmt = db
            .prepare(&format!(
                "SELECT sa.session_id, s.title AS session_title, a.family, sa.model_name,
                    COALESCE(w.display_name, w.path, 'Other') AS workspace, sa.started_at,
                    sa.estimated_cost, sa.total_tokens, sa.duration_seconds, sa.total_turns
                 {BASE_JOIN} {clause} ORDER BY sa.{metric} DESC, sa.session_id DESC LIMIT 5"
            ))
            .map_err(|e| sqlite_error("Could not prepare the leaderboard query", e))?;
        let rows = rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the leaderboard query", e))?;
        leaderboards.insert(metric.to_string(), Value::Array(rows));
    }

    // 6. Tool metrics
    let top_failing_bash = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT COALESCE(t.command_text,
                        CASE WHEN json_valid(t.arguments_json) THEN COALESCE(json_extract(t.arguments_json, '$.command'), json_extract(t.arguments_json, '$.cmd')) END,
                        t.tool_name) as command, t.tool_name, COUNT(*) as fail_count,
                    MAX(t.started_at) as last_failed_at, MAX(COALESCE(t.exit_code, 1)) as exit_code,
                    SUBSTR(t.result_text, 1, 140) as sample_error
                 FROM tool_invocation t INDEXED BY tool_failures_idx
                 JOIN session s ON s.id = t.session_id JOIN agent a ON a.id = s.agent_id
                 LEFT JOIN workspace w ON w.id = s.workspace_id
                 JOIN session_analytics sa ON sa.session_id = s.id
                 {clause} AND (t.status = 'error' OR (t.exit_code IS NOT NULL AND t.exit_code != 0))
                   AND (t.tool_name IN ('bash','exec','CommandExecution','terminal','shell') OR t.command_text IS NOT NULL)
                 GROUP BY command HAVING command IS NOT NULL AND command != ''
                 ORDER BY fail_count DESC LIMIT 15"
            ))
            .map_err(|e| sqlite_error("Could not prepare the failing bash query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the failing bash query", e))?
    };
    let mut tool_error_rates = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT t.tool_name, COUNT(*) as total_calls,
                    SUM(CASE WHEN t.status = 'error' OR (t.exit_code IS NOT NULL AND t.exit_code != 0) THEN 1 ELSE 0 END) as error_calls,
                    SUM(CASE WHEN t.status = 'completed' AND (t.exit_code IS NULL OR t.exit_code = 0) THEN 1 ELSE 0 END) as success_calls,
                    ROUND(AVG(CASE WHEN t.duration_ms > 0 AND t.duration_ms < 600000 THEN t.duration_ms END)) as avg_duration_ms
                 FROM tool_invocation t INDEXED BY tool_metrics_name_idx
                 JOIN session s ON s.id = t.session_id JOIN agent a ON a.id = s.agent_id
                 LEFT JOIN workspace w ON w.id = s.workspace_id
                 JOIN session_analytics sa ON sa.session_id = s.id
                 {clause} GROUP BY t.tool_name HAVING total_calls >= 3
                 ORDER BY error_calls DESC, total_calls DESC LIMIT 16"
            ))
            .map_err(|e| sqlite_error("Could not prepare the tool error query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the tool error query", e))?
    };
    for row in tool_error_rates.iter_mut() {
        let (t, e) = (
            row.get("total_calls").and_then(|v| v.as_f64()).unwrap_or(0.0),
            row.get("error_calls").and_then(|v| v.as_f64()).unwrap_or(0.0),
        );
        if let Some(o) = row.as_object_mut() {
            o.insert("error_rate".into(), json!(if t > 0.0 { round_to(e / t * 100.0, 1) } else { 0.0 }));
        }
    }
    let slowest_tools = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT t.id, t.tool_name,
                    COALESCE(t.command_text,
                        CASE WHEN json_valid(t.arguments_json) THEN COALESCE(json_extract(t.arguments_json, '$.command'), json_extract(t.arguments_json, '$.cmd'), json_extract(t.arguments_json, '$.filePath'), json_extract(t.arguments_json, '$.path')) END,
                        t.tool_name) as command,
                    t.status, t.session_id, s.title as session_title, t.duration_ms, t.started_at
                 FROM tool_invocation t
                 JOIN session s ON s.id = t.session_id JOIN agent a ON a.id = s.agent_id
                 LEFT JOIN workspace w ON w.id = s.workspace_id
                 JOIN session_analytics sa ON sa.session_id = s.id
                 {clause} AND t.duration_ms > 50 AND t.duration_ms < 600000
                 ORDER BY t.duration_ms DESC LIMIT 15"
            ))
            .map_err(|e| sqlite_error("Could not prepare the slowest tools query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the slowest tools query", e))?
    };
    // Nearest-rank p50/p95 across all recorded nonnegative durations.
    let tool_latency = {
        let mut stmt = db
            .prepare(&format!(
                "WITH ranked AS (
                    SELECT t.tool_name, t.duration_ms,
                        ROW_NUMBER() OVER (PARTITION BY t.tool_name ORDER BY t.duration_ms) AS rank,
                        COUNT(*) OVER (PARTITION BY t.tool_name) AS samples
                     FROM tool_invocation t
                     JOIN session s ON s.id = t.session_id JOIN agent a ON a.id = s.agent_id
                     LEFT JOIN workspace w ON w.id = s.workspace_id
                     JOIN session_analytics sa ON sa.session_id = s.id
                     {clause} AND t.duration_ms IS NOT NULL AND t.duration_ms >= 0
                 ) SELECT tool_name, MAX(samples) AS samples, AVG(duration_ms) AS avg_duration_ms,
                    MIN(CASE WHEN rank >= (samples + 1) / 2 THEN duration_ms END) AS p50_duration_ms,
                    MIN(CASE WHEN rank >= (samples * 95 + 99) / 100 THEN duration_ms END) AS p95_duration_ms,
                    MAX(duration_ms) AS max_duration_ms
                 FROM ranked GROUP BY tool_name ORDER BY p95_duration_ms DESC, samples DESC"
            ))
            .map_err(|e| sqlite_error("Could not prepare the latency query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the latency query", e))?
    };
    let tool_statuses = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT COALESCE(t.status, 'unspecified') as status, COUNT(*) as count
                 FROM tool_invocation t INDEXED BY tool_metrics_status_idx
                 JOIN session s ON s.id = t.session_id JOIN agent a ON a.id = s.agent_id
                 LEFT JOIN workspace w ON w.id = s.workspace_id
                 JOIN session_analytics sa ON sa.session_id = s.id
                 {clause} GROUP BY status ORDER BY count DESC"
            ))
            .map_err(|e| sqlite_error("Could not prepare the tool status query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the tool status query", e))?
    };

    // 7. Productivity pulse
    let heatmap_rows = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT user_weekday(sa.started_at) as day_of_week, user_hour(sa.started_at) as hour_of_day,
                    COUNT(*) as count, SUM(sa.total_turns) as turns
                 {BASE_JOIN} {clause}
                   AND sa.started_at IS NOT NULL AND user_weekday(sa.started_at) IS NOT NULL
                 GROUP BY day_of_week, hour_of_day"
            ))
            .map_err(|e| sqlite_error("Could not prepare the heatmap query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the heatmap query", e))?
    };
    let mut peak_point = json!({ "day_of_week": 1, "hour_of_day": 14, "count": 0 });
    for p in &heatmap_rows {
        if p.get("count").and_then(|v| v.as_i64()).unwrap_or(0)
            > peak_point.get("count").and_then(|v| v.as_i64()).unwrap_or(0)
        {
            peak_point = p.clone();
        }
    }
    let mut duration_distribution = Vec::new();
    for (label, min, max) in [
        ("< 2 mins", 0.0, 120.0),
        ("2–10 mins", 120.0, 600.0),
        ("10–30 mins", 600.0, 1800.0),
        ("30–60 mins", 1800.0, 3600.0),
        ("1–2 hours", 3600.0, 7200.0),
        ("> 2 hours", 7200.0, 9_999_999.0),
    ] {
        let mut stmt = db
            .prepare(&format!(
                "SELECT COUNT(*) as count, COALESCE(AVG(sa.total_turns), 0) as avg_turns
                 {BASE_JOIN} {clause} AND sa.duration_seconds >= ? AND sa.duration_seconds < ?"
            ))
            .map_err(|e| sqlite_error("Could not prepare the duration query", e))?;
        let mut all_owned = owned.clone();
        let min_s = min.to_string();
        let max_s = max.to_string();
        // Params are strings here; SQLite compares numerically for REAL column.
        all_owned.push(min_s);
        all_owned.push(max_s);
        let all_refs: Vec<&dyn rusqlite::types::ToSql> = all_owned.iter().map(|s| s as &dyn rusqlite::types::ToSql).collect();
        let row = one_to_json(&mut stmt, &all_refs)
            .map_err(|e| sqlite_error("Could not run the duration query", e))?
            .unwrap_or(json!({}));
        let count = row.get("count").and_then(|v| v.as_f64()).unwrap_or(0.0);
        duration_distribution.push(json!({
            "label": label, "count": count as i64,
            "avg_turns": round_to(row.get("avg_turns").and_then(|v| v.as_f64()).unwrap_or(0.0), 1),
            "pct": if total_sessions > 0 { round_to(count / total_sessions as f64 * 100.0, 1) } else { 0.0 },
        }));
    }
    let turn_depth_counts = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT CASE WHEN sa.total_turns = 0 THEN '0' WHEN sa.total_turns = 1 THEN '1'
                    WHEN sa.total_turns <= 5 THEN '2–5' WHEN sa.total_turns <= 20 THEN '6–20' ELSE '20+' END AS label,
                    COUNT(*) AS count {BASE_JOIN} {clause} GROUP BY label"
            ))
            .map_err(|e| sqlite_error("Could not prepare the turn depth query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the turn depth query", e))?
    };
    let turn_depth_distribution: Vec<Value> = ["0", "1", "2–5", "6–20", "20+"]
        .iter()
        .map(|label| {
            let count = turn_depth_counts.iter()
                .find(|r| r.get("label").and_then(|v| v.as_str()) == Some(*label))
                .and_then(|r| r.get("count").and_then(|v| v.as_f64())).unwrap_or(0.0);
            json!({ "label": label, "count": count as i64,
                "pct": if total_sessions > 0 { count / total_sessions as f64 * 100.0 } else { 0.0 } })
        })
        .collect();
    let turns_by_project = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT COALESCE(w.display_name, w.path, 'Other') as name, w.path,
                    COUNT(sa.session_id) as sessions, SUM(sa.total_turns) as total_turns,
                    SUM(sa.user_turns) as user_turns, SUM(sa.assistant_turns) as assistant_turns,
                    ROUND(AVG(sa.total_turns), 1) as avg_turns_per_session,
                    ROUND(COALESCE(SUM(sa.duration_seconds),0) / 3600.0, 1) as total_hours
                 {BASE_JOIN} {clause}
                 GROUP BY sa.workspace_id HAVING total_turns > 0 ORDER BY total_turns DESC LIMIT 16"
            ))
            .map_err(|e| sqlite_error("Could not prepare the turns-by-project query", e))?;
        rows_to_json(&mut stmt, &refs).map_err(|e| sqlite_error("Could not run the turns-by-project query", e))?
    };
    let active_days_count: i64 = {
        let mut stmt = db
            .prepare(&format!(
                "SELECT COUNT(DISTINCT user_date(sa.started_at)) as count
                 {BASE_JOIN} {clause} AND sa.started_at IS NOT NULL"
            ))
            .map_err(|e| sqlite_error("Could not prepare the active days query", e))?;
        one_to_json(&mut stmt, &refs)
            .map_err(|e| sqlite_error("Could not run the active days query", e))?
            .and_then(|r| r.get("count").and_then(|v| v.as_i64()))
            .unwrap_or(0)
    };

    let mut kpis = summary_row.clone();
    apply_efficiency(&mut kpis);
    if let Some(o) = kpis.as_object_mut() {
        o.insert("total_sessions".into(), json!(total_sessions));
        o.insert("estimated_cost".into(), json!(round_to(summary_cost, 2)));
        o.insert("avg_cost_per_session".into(), json!(round_to(avg_cost, 3)));
        o.insert("avg_turns_per_session".into(), json!(round_to(avg_turns, 1)));
        o.insert("tool_error_rate".into(), json!(round_to(tool_error_rate, 1)));
        o.insert("total_duration_hours".into(), json!(round_to(f("total_duration_seconds") / 3600.0, 1)));
        o.insert("avg_duration_minutes".into(), json!(round_to(f("avg_duration_seconds") / 60.0, 1)));
        o.insert("active_days".into(), json!(active_days_count));
        o.insert("peak_day_of_week".into(), peak_point.get("day_of_week").cloned().unwrap_or(Value::Null));
        o.insert("peak_hour_of_day".into(), peak_point.get("hour_of_day").cloned().unwrap_or(Value::Null));
    }

    let peak_day = match peak_point.get("day_of_week").and_then(|v| v.as_i64()).unwrap_or(0) {
        0 => "Sunday", 1 => "Monday", 2 => "Tuesday", 3 => "Wednesday",
        4 => "Thursday", 5 => "Friday", _ => "Saturday",
    };
    let peak_hour = format!("{}:00", peak_point.get("hour_of_day").and_then(|v| v.as_i64()).unwrap_or(12));

    let pricing_reference = {
        let p = report_pricing.lock().map_err(|_| "Could not lock the pricing state".to_string())?;
        serde_json::to_value(get_pricing_reference(&p)).unwrap_or(Value::Null)
    };
    let methodology = Methodology {
        pricing_source: pricing_meta.source.clone(),
        pricing_source_url: pricing_meta.source_url.clone(),
        pricing_updated_at: pricing_meta.updated_at.clone(),
        pricing_model_count: pricing_meta.model_count,
        custom_override_count: pricing_meta.override_count,
        token_note: format!(
            "Token counts use actual reported usage when available; otherwise estimated from text length at {TOKEN_CHARS_PER_TOKEN} chars per token."
        ),
    };

    let result = json!({
        "session_leaderboards": leaderboards,
        "kpis": kpis,
        "tokens_and_costs": {
            "by_model": by_model, "by_agent": by_agent, "by_workspace": by_workspace,
            "daily_trend": daily_trend, "spend_outlook": spend_outlook,
            "pricing_reference": pricing_reference, "pricing_meta": pricing_meta_json,
            "methodology": serde_json::to_value(&methodology).unwrap_or(Value::Null),
        },
        "tool_metrics": {
            "top_failing_bash": top_failing_bash, "tool_error_rates": tool_error_rates,
            "slowest_tools": slowest_tools, "latency_distribution": tool_latency,
            "tool_statuses": tool_statuses,
        },
        "productivity_pulse": {
            "heatmap": heatmap_rows, "duration_distribution": duration_distribution,
            "turn_depth_distribution": turn_depth_distribution,
            "turn_ratio_trend": daily_trend.iter().map(|r| json!({
                "date": r.get("date").cloned().unwrap_or(Value::Null),
                "user_turns": r.get("user_turns").cloned().unwrap_or(Value::Null),
                "assistant_turns": r.get("assistant_turns").cloned().unwrap_or(Value::Null),
                "user_assistant_ratio": r.get("user_assistant_ratio").cloned().unwrap_or(Value::Null),
            })).collect::<Vec<Value>>(),
            "turns_by_project": turns_by_project, "peak_day": peak_day, "peak_hour": peak_hour,
        },
    });

    if let Ok(mut cache) = state.cache.lock() {
        if cache.len() >= 32 { cache.clear(); }
        if state.revision.load(Ordering::SeqCst) == revision {
            cache.insert(cache_key, (Instant::now(), result.clone()));
        }
    }
    Ok(result)
}
