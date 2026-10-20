//! Simplified port of `server/sync-controller.js` + `server/sync-worker.js`:
//! single-flight guard plus the progress-shaped status snapshot.
//!
//! The worker thread is a plain `std::thread` (no async runtime needed):
//! [`start_sync`] returns `false` immediately when a run is already active,
//! otherwise it flips the state to `scanning` and runs
//! [`crate::importer::sync_all_with_progress`] in the background, folding
//! progress into the shared [`SyncState`].
//!
//! Only deps: std + serde (+ chrono for timestamps).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::{Deserialize, Serialize};

/// Progress snapshot (mirrors the controller `status` shape, simplified).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    pub running: bool,
    pub phase: String,
    pub files_done: usize,
    pub files_total: usize,
    pub records: usize,
    pub errors: Vec<String>,
}

impl Default for SyncState {
    fn default() -> Self {
        Self {
            running: false,
            phase: "idle".to_string(),
            files_done: 0,
            files_total: 0,
            records: 0,
            errors: Vec::new(),
        }
    }
}

fn mutex() -> &'static Mutex<SyncState> {
    static CELL: OnceLock<Mutex<SyncState>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(SyncState::default()))
}

fn lock_state() -> MutexGuard<'static, SyncState> {
    mutex().lock().unwrap_or_else(|e| e.into_inner())
}

static MANAGING_DATA: AtomicBool = AtomicBool::new(false);

/// Shares the sync launch lock so maintenance and sync cannot start together.
pub struct DataManagementGuard(());

pub fn acquire_data_management() -> Result<DataManagementGuard, String> {
    let state = lock_state();
    if state.running {
        return Err("Stop session sync before managing saved data.".into());
    }
    if MANAGING_DATA.swap(true, Ordering::SeqCst) {
        return Err("Another data operation is in progress. Please wait.".into());
    }
    Ok(DataManagementGuard(()))
}

impl Drop for DataManagementGuard {
    fn drop(&mut self) {
        MANAGING_DATA.store(false, Ordering::SeqCst);
        crate::analytics::request_refresh(&crate::db::db_path(), false);
    }
}

pub fn is_managing_data() -> bool {
    MANAGING_DATA.load(Ordering::SeqCst)
}

/// Clone of the current status (mirrors `getSyncStatus`).
pub fn get_status() -> SyncState {
    lock_state().clone()
}

/// Start a background sync. Returns `false` when a run is already active
/// (single-flight guard, mirrors `sync-controller.js` returning the pending
/// promise); `true` when a new run was launched.
pub fn start_sync(agent_filter: Option<String>, mode: &str) -> bool {
    {
        let mut state = lock_state();
        if state.running || is_managing_data() {
            return false;
        }
        crate::importer::reset_stop();
        *state = SyncState {
            running: true,
            phase: "scanning".to_string(),
            files_done: 0,
            files_total: 0,
            records: 0,
            errors: Vec::new(),
        };
    }
    let mode_owned = mode.to_string();
    let worker = std::thread::Builder::new()
        .name("session-sync".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::importer::sync_all_with_progress(
                    agent_filter.as_deref(),
                    &mode_owned,
                    &|done, total, records| {
                        let mut state = lock_state();
                        state.files_done = done;
                        state.files_total = total;
                        state.records = records;
                        if state.phase == "scanning" && total > 0 {
                            state.phase = "importing".to_string();
                        }
                    },
                )
            }));
            let mut state = lock_state();
            state.running = false;
            match result {
                Ok(summary) => {
                    state.phase = if summary.cancelled {
                        "cancelled".to_string()
                    } else if summary.errors.iter().any(|e| !e.file.starts_with(crate::importer::HOST_ERROR_PREFIX)) {
                        // An unreachable host keeps its last mirror; not a failed sync.
                        "failed".to_string()
                    } else {
                        "complete".to_string()
                    };
                    state.files_done = summary.files_done;
                    state.files_total = summary.files_total;
                    state.records = summary.records;
                    state.errors = summary
                        .errors
                        .into_iter()
                        .map(|e| format!("{}: {}", e.file, e.error))
                        .collect();
                }
                Err(_) => {
                    state.phase = "failed".into();
                    state
                        .errors
                        .push("Session sync stopped unexpectedly. Retry sync to continue.".into());
                }
            }
            drop(state);
            crate::analytics::shared_state().clear_cache();
            crate::analytics::request_refresh(&crate::db::db_path(), false);
        });
    if let Err(error) = worker {
        let mut state = lock_state();
        state.running = false;
        state.phase = "failed".into();
        state
            .errors
            .push(format!("Could not start session sync: {error}"));
        return false;
    }
    true
}

/// Request cooperative cancellation while holding the single-flight lock.
pub fn request_stop() -> bool {
    let mut state = lock_state();
    if !state.running {
        return false;
    }
    crate::importer::request_stop();
    state.phase = "stopping".to_string();
    true
}

/// Whether a sync run is currently active.
pub fn is_running() -> bool {
    lock_state().running
}

pub const ANALYTICS_PAUSED_MESSAGE: &str = "Analytics calculation deferred while session data is being updated.";

/// Background metric writes yield to imports/maintenance; report reads continue.
pub fn ensure_analytics_writer_available() -> Result<(), String> {
    if is_running() || is_managing_data() {
        Err(ANALYTICS_PAUSED_MESSAGE.into())
    } else {
        Ok(())
    }
}

pub fn report_error(error: String) -> String {
    if error.contains("database is locked") || error.contains("database table is locked") {
        "The database is busy. Please try again in a moment.".into()
    } else {
        error
    }
}
