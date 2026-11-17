//! Foreground local observation used by desktop and the explicit headless watch
//! command. Observation never refreshes SSH mirrors or starts relay sync.
use crate::{
    db,
    importer::{self, LocalCatalog},
};
use hession_core::{
    indexing,
    library_revision::{self, LibraryRevision},
    locking::{LibraryLock, LockKind},
    observation::Pending,
};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_secs(15);
const AUDIT: Duration = Duration::from_secs(600);
const MAX_DIRECTORIES: usize = 4096;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub phase: String,
    pub roots: Vec<PathBuf>,
    pub unavailable_roots: Vec<PathBuf>,
    pub watched_directories: usize,
    pub pending_sources: usize,
    pub active_source: Option<String>,
    pub last_success: Option<String>,
    pub last_import_ms: Option<u64>,
    pub errors: Vec<String>,
    pub overflow_count: u64,
    pub may_be_stale: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryChanged {
    pub revision: LibraryRevision,
    pub session_ids: Vec<i64>,
    pub source_kind: &'static str,
}

struct Control {
    shutdown: AtomicBool,
    paused: AtomicBool,
    cancel: Arc<AtomicBool>,
    status: Mutex<Status>,
}
impl Control {
    fn update(&self, task: impl FnOnce(&mut Status)) {
        task(&mut self.status.lock().unwrap_or_else(|e| e.into_inner()));
    }
    fn error(&self, error: impl AsRef<str>) {
        self.update(|s| {
            s.may_be_stale = true;
            if s.errors.len() == 25 {
                s.errors.remove(0);
            }
            s.errors.push(error.as_ref().chars().take(300).collect());
        });
    }
}

pub struct Observer {
    control: Arc<Control>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Observer {
    pub(crate) fn start(
        database: PathBuf,
        catalog: LocalCatalog,
        on_change: Arc<dyn Fn(LibraryChanged) + Send + Sync>,
    ) -> Result<Self, String> {
        db::open_db(&database).map_err(|e| e.to_string())?;
        // Admission is checked before starting a thread and has kernel lifetime.
        let leader = LibraryLock::acquire(&database, LockKind::WatchLeader, Duration::ZERO)
            .map_err(|e| {
                format!("Another watcher may own this library. Stop it before watching here: {e}")
            })?;
        let control = Arc::new(Control {
            shutdown: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancel: Arc::new(AtomicBool::new(false)),
            status: Mutex::new(Status {
                phase: "starting".into(),
                roots: catalog.roots(),
                may_be_stale: true,
                ..Status::default()
            }),
        });
        let worker_control = control.clone();
        let worker = thread::Builder::new()
            .name("history-observer".into())
            .spawn(move || {
                let _leader = leader;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(&database, catalog, &worker_control, &*on_change)
                }));
                if result.is_err() {
                    worker_control
                        .error("Indexing worker stopped unexpectedly; restart observation");
                }
                worker_control.update(|s| {
                    s.phase = "stopped".into();
                    s.active_source = None;
                    s.may_be_stale = true;
                });
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            control,
            worker: Some(worker),
        })
    }
    pub fn status(&self) -> Status {
        self.control
            .status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn pause(&self, paused: bool) {
        self.control.paused.store(paused, Ordering::SeqCst);
        if paused {
            self.control.cancel.store(true, Ordering::SeqCst);
        }
        self.control.update(|s| {
            s.phase = if paused { "paused" } else { "resuming" }.into();
            s.may_be_stale = true;
        });
    }
    pub fn interrupt(&self) {
        self.control.cancel.store(true, Ordering::SeqCst);
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        self.control.shutdown.store(true, Ordering::SeqCst);
        self.interrupt();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn directories(roots: &[PathBuf]) -> (BTreeSet<PathBuf>, bool) {
    let mut found = BTreeSet::new();
    let mut stack: Vec<_> = roots.iter().cloned().map(|p| (p, 0)).collect();
    let mut truncated = false;
    let mut examined = 0usize;
    while let Some((path, depth)) = stack.pop() {
        if found.len() >= MAX_DIRECTORIES {
            truncated = true;
            break;
        }
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        // A configured root can deliberately be a directory symlink. Never
        // descend through symlinks discovered inside it.
        let directory =
            meta.is_dir() || (depth == 0 && meta.file_type().is_symlink() && path.is_dir());
        if !directory || (depth > 0 && meta.file_type().is_symlink()) {
            continue;
        }
        if !found.insert(path.clone()) {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&path) {
            for (n, entry) in entries.take(100_001).enumerate() {
                examined += 1;
                if examined > 100_000 {
                    truncated = true;
                    break;
                }
                if n == 100_000 {
                    truncated = true;
                    break;
                }
                let Ok(entry) = entry else {
                    continue;
                };
                if entry.file_type().is_ok_and(|t| t.is_dir()) {
                    if depth < 24 && stack.len() < MAX_DIRECTORIES {
                        stack.push((entry.path(), depth + 1));
                    } else {
                        truncated = true;
                    }
                }
            }
        } else {
            truncated = true;
        }
        if examined > 100_000 {
            break;
        }
    }
    (found, truncated)
}

fn affected(source: &Path, event: &Path) -> bool {
    source==event || source.starts_with(event)
        || source.parent()==event.parent() // WAL and companion manifests.
        || event.to_string_lossy().strip_suffix("-wal").is_some_and(|p|Path::new(p)==source)
}

fn run(
    database: &Path,
    catalog: LocalCatalog,
    control: &Control,
    on_change: &dyn Fn(LibraryChanged),
) {
    let mut conn = match db::open_db(database) {
        Ok(conn) => conn,
        Err(e) => {
            control.error(e.to_string());
            return;
        }
    };
    if let Err(error) = db::configure_bulk_writer(&conn) {
        control.error(error.to_string());
        return;
    }
    let (sender, receiver) = mpsc::sync_channel::<PathBuf>(256);
    // Keep timeout-based polling asleep when the native backend cannot start.
    let _keepalive = sender.clone();
    let overflow = Arc::new(AtomicBool::new(false));
    let callback_overflow = overflow.clone();
    let mut watcher =
        match notify::recommended_watcher(move |result: notify::Result<Event>| match result {
            Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                if event.need_rescan() || event.paths.len() > 32 {
                    callback_overflow.store(true, Ordering::SeqCst);
                }
                for path in event.paths.into_iter().take(32) {
                    if sender.try_send(path).is_err() {
                        callback_overflow.store(true, Ordering::SeqCst);
                    }
                }
            }
            Err(_) => {
                callback_overflow.store(true, Ordering::SeqCst);
            }
            _ => {}
        }) {
            Ok(watcher) => Some(watcher),
            Err(e) => {
                control.error(format!(
                    "Native observation unavailable; polling every 15 seconds: {e}"
                ));
                None
            }
        };
    let roots = catalog.roots();
    let mut watched = BTreeSet::new();
    let mut seen_roots = BTreeSet::new();
    let mut pending = Pending::default();
    let mut fingerprints = BTreeMap::<(String, PathBuf), String>::new();
    let mut last_poll = Instant::now();
    let mut last_audit = Instant::now();
    let mut scan = true;
    let mut was_suspended = false;
    let mut last_revision = library_revision::current(&conn).ok();
    let mut changed = BTreeSet::new();
    let mut last_emit = Instant::now();
    while !control.shutdown.load(Ordering::SeqCst) {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(path) => pending.push(&path, Instant::now()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
        for _ in 0..256 {
            match receiver.try_recv() {
                Ok(path) => pending.push(&path, Instant::now()),
                Err(_) => break,
            }
        }
        if overflow.swap(false, Ordering::SeqCst) {
            pending.overflow();
            control.update(|s| s.overflow_count = s.overflow_count.saturating_add(1));
            control.error("Observation queue overflow or native gap; reconciling known roots");
        }
        let suspended = control.paused.load(Ordering::SeqCst)
            || crate::sync::is_managing_data()
            || crate::sync::get_status().running;
        if suspended {
            was_suspended = true;
            control.update(|s| {
                s.phase = if control.paused.load(Ordering::SeqCst) {
                    "paused"
                } else {
                    "waiting"
                }
                .into();
                s.may_be_stale = true;
            });
            continue;
        }
        if was_suspended {
            scan = true;
            was_suspended = false;
        }
        control.cancel.store(false, Ordering::SeqCst);
        let now = Instant::now();
        let (events, recovery) = pending.take_ready(now);
        let audit = now.duration_since(last_audit) >= AUDIT;
        let poll = now.duration_since(last_poll) >= POLL;
        let do_scan = scan || recovery || audit || poll || !events.is_empty();
        control.update(|s| s.pending_sources = pending.len());
        if do_scan {
            let full_audit = scan || recovery || audit;
            scan = false;
            last_poll = now;
            if audit {
                last_audit = now;
            }
            let (desired, truncated) = directories(&roots);
            if truncated {
                control.error("Directory observation is incomplete; some sources may be stale. Check source permissions or narrow the configured roots.");
            }
            if let Some(watcher) = watcher.as_mut() {
                for path in watched.difference(&desired) {
                    let _ = watcher.unwatch(path);
                }
                let mut actual = watched
                    .intersection(&desired)
                    .cloned()
                    .collect::<BTreeSet<_>>();
                for path in desired.difference(&watched) {
                    match watcher.watch(path, RecursiveMode::NonRecursive) {
                        Ok(()) => {
                            actual.insert(path.clone());
                        }
                        Err(e) => {
                            control.error(format!("Cannot watch {}; polling: {e}", path.display()))
                        }
                    }
                }
                watched = actual;
            }
            let unavailable = roots
                .iter()
                .filter(|root| !root.is_dir())
                .cloned()
                .collect::<Vec<_>>();
            let missing_previously_available =
                unavailable.iter().any(|root| seen_roots.contains(root));
            seen_roots.extend(roots.iter().filter(|root| root.is_dir()).cloned());
            let observation_incomplete = watcher.is_none() || watched != desired;
            control.update(|s| {
                s.unavailable_roots = unavailable;
                s.watched_directories = watched.len();
                s.phase = if !observation_incomplete && !truncated {
                    "active"
                } else {
                    "degraded"
                }
                .into();
            });
            let epoch = match indexing::epoch(&conn) {
                Ok(epoch) => epoch,
                Err(e) => {
                    control.error(e.to_string());
                    continue;
                }
            };
            let files = importer::with_cancellation(control.cancel.clone(), || catalog.files());
            let mut live = BTreeSet::new();
            let mut pass_failed = false;
            for (family, path) in files {
                if control.cancel.load(Ordering::SeqCst)
                    || control.paused.load(Ordering::SeqCst)
                    || control.shutdown.load(Ordering::SeqCst)
                {
                    scan = true;
                    break;
                }
                let key = (family.clone(), path.clone());
                live.insert(key.clone());
                let stamp = match catalog.fingerprint(&family, &path) {
                    Ok(stamp) => stamp,
                    Err(e) => {
                        control.error(e);
                        pass_failed = true;
                        continue;
                    }
                };
                if !full_audit
                    && fingerprints.get(&key) == Some(&stamp)
                    && !events.iter().any(|event| affected(&path, event))
                {
                    continue;
                }
                control.update(|s| {
                    s.active_source = Some(
                        path.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned(),
                    );
                    s.pending_sources = s.pending_sources.saturating_add(1);
                });
                let before = conn
                    .query_row("SELECT COALESCE(MAX(seq),0) FROM index_change", [], |r| {
                        r.get::<_, i64>(0)
                    })
                    .unwrap_or(0);
                let import_started = Instant::now();
                let result = importer::with_cancellation(control.cancel.clone(), || {
                    catalog.import(&mut conn, &family, &path, epoch)
                });
                match result {
                    Ok(_) => {
                        fingerprints.insert(key, stamp);
                        control.update(|s| {
                            s.last_success = Some(chrono::Utc::now().to_rfc3339());
                            s.last_import_ms =
                                Some(import_started.elapsed().as_millis().min(u64::MAX as u128)
                                    as u64);
                        });
                    }
                    Err(e) if !control.cancel.load(Ordering::SeqCst) => {
                        control.error(e);
                        pass_failed = true;
                    }
                    Err(_) => {
                        scan = true;
                    }
                }
                if let Ok(mut stmt) = conn
                    .prepare("SELECT DISTINCT session_id FROM index_change WHERE seq>? LIMIT 128")
                {
                    if let Ok(ids) = stmt.query_map([before], |r| r.get::<_, i64>(0)) {
                        for id in ids.flatten() {
                            if changed.len() < 128 {
                                changed.insert(id);
                            }
                        }
                    }
                }
                if last_emit.elapsed() >= Duration::from_millis(250) {
                    emit(&conn, &mut last_revision, &mut changed, on_change);
                    last_emit = Instant::now();
                }
            }
            // Never erase cache entries after a cancelled partial discovery.
            if !control.cancel.load(Ordering::SeqCst) {
                fingerprints.retain(|key, _| live.contains(key));
            }
            control.update(|s| {
                s.active_source = None;
                s.pending_sources = pending.len();
                s.may_be_stale = pass_failed
                    || truncated
                    || observation_incomplete
                    || missing_previously_available;
            });
        }
        emit(&conn, &mut last_revision, &mut changed, on_change);
    }
}

fn emit(
    conn: &rusqlite::Connection,
    previous: &mut Option<LibraryRevision>,
    changed: &mut BTreeSet<i64>,
    callback: &dyn Fn(LibraryChanged),
) {
    if let Ok(revision) = library_revision::current(conn) {
        if previous.as_ref() != Some(&revision) {
            callback(LibraryChanged {
                revision: revision.clone(),
                session_ids: changed.iter().copied().collect(),
                source_kind: "library",
            });
            *previous = Some(revision);
        }
    }
    changed.clear();
}

fn desktop() -> &'static Mutex<Option<Observer>> {
    static CELL: OnceLock<Mutex<Option<Observer>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}
pub(crate) fn interrupt_for_manual() {
    if let Some(observer) = desktop().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        observer.interrupt();
    }
}

#[tauri::command]
pub async fn start_indexing(
    state: tauri::State<'_, crate::AppState>,
    app: tauri::AppHandle,
) -> Result<Status, String> {
    let database = state.db_path.lock().map_err(|e| e.to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut slot = desktop()
            .lock()
            .map_err(|_| "Could not access observer state")?;
        if slot
            .as_ref()
            .is_none_or(|observer| observer.status().phase == "stopped")
        {
            use tauri::Emitter;
            *slot = Some(Observer::start(
                database,
                LocalCatalog::capture(None),
                Arc::new(move |change| {
                    let _ = app.emit("library-changed", change);
                }),
            )?);
        }
        Ok(slot.as_ref().ok_or("Observer did not start")?.status())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn indexing_status() -> Status {
    desktop()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map_or_else(
            || Status {
                phase: "stopped".into(),
                may_be_stale: true,
                ..Status::default()
            },
            Observer::status,
        )
}

#[tauri::command]
pub fn pause_indexing(paused: bool) -> Result<Status, String> {
    let slot = desktop()
        .lock()
        .map_err(|_| "Could not access observer state")?;
    let observer = slot
        .as_ref()
        .ok_or("Start observation before changing pause state")?;
    observer.pause(paused);
    Ok(observer.status())
}

#[tauri::command]
pub async fn session_delta(
    state: tauri::State<'_, crate::AppState>,
    session_id: i64,
    cursor: Option<hession_core::indexing::DeltaCursor>,
    limit: Option<u32>,
) -> Result<serde_json::Value, String> {
    let path = state.db_path.lock().map_err(|e| e.to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        hession_core::service::Library::new(path)
            .session_delta(session_id, cursor, limit.unwrap_or(100))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn wait_until(mut check: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while !check() {
            assert!(
                Instant::now() < end,
                "observer did not converge before deadline"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
    #[test]
    fn native_observation_partial_append_pause_recovery_and_single_leader() {
        let dir = tempfile::tempdir().unwrap();
        let source_root = dir.path().join("codex");
        let sessions = source_root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let file = sessions.join("fixture.jsonl");
        let initial = include_str!("../tests/fixtures/codex-concurrency.jsonl");
        std::fs::write(&file, initial).unwrap();
        let path = dir.path().join("library/history.sqlite");
        let conn = db::open_db(&path).unwrap();
        let (sender, receiver) = mpsc::channel();
        let observer = Observer::start(
            path.clone(),
            LocalCatalog::fixture(source_root.clone()),
            Arc::new(move |change| {
                let _ = sender.send(change);
            }),
        )
        .unwrap();
        assert!(Observer::start(
            path.clone(),
            LocalCatalog::fixture(source_root.clone()),
            Arc::new(|_| {})
        )
        .is_err());
        let count = || {
            conn.query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        wait_until(|| count() == 4);
        let first = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
        let notice = serde_json::to_value(first).unwrap();
        assert!(notice.get("entries").is_none());
        assert!(notice.get("sourceUri").is_none());
        let mut native = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        native.write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"partial").unwrap();
        thread::sleep(Duration::from_millis(700));
        assert_eq!(count(), 4);
        let appended = Instant::now();
        native.write_all(b" complete\"}}\n").unwrap();
        wait_until(|| count() == 5);
        eprintln!(
            "native JSONL complete-write-to-commit: {} ms",
            appended.elapsed().as_millis()
        );
        observer.pause(true);
        wait_until(|| observer.status().active_source.is_none());
        writeln!(native,"{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"while paused\"}}}}").unwrap();
        thread::sleep(Duration::from_millis(700));
        assert_eq!(count(), 5);
        observer.pause(false);
        wait_until(|| count() == 6);
        drop(observer);
        assert_eq!(
            conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert!(std::fs::read_to_string(&file).unwrap().starts_with(initial));
        // Kernel leadership releases on orderly shutdown and can be reacquired.
        let replacement =
            Observer::start(path, LocalCatalog::fixture(source_root), Arc::new(|_| {})).unwrap();
        drop(replacement);
    }
    #[test]
    fn directory_walk_is_bounded_and_does_not_follow_cycles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("nested/cycle")).unwrap();
        let (found, truncated) = directories(&[dir.path().into()]);
        assert_eq!(found.len(), 2);
        assert!(!truncated);
        assert!(affected(
            Path::new("/store/state.db"),
            Path::new("/store/state.db-wal")
        ));
    }
}
