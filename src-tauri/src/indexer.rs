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
        mpsc, Arc, Mutex, OnceLock, RwLock,
    },
    thread,
    time::{Duration, Instant},
};

const POLL: Duration = Duration::from_secs(15);
const AUDIT: Duration = Duration::from_secs(600);
// Library-change notifications coalesce before the final desktop reads.
// Leave those reads time to finish before returning their freed buffers too.
const IDLE_RECLAIM: Duration = crate::memory::QUIET;
const MAX_DIRECTORIES: usize = 4096;
#[cfg(not(test))]
const NATIVE_RETRY: Duration = Duration::from_secs(30);
#[cfg(test)]
const NATIVE_RETRY: Duration = Duration::from_millis(500);

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
    pub wakeups: u64,
    pub discovery_passes: u64,
    pub fingerprint_checks: u64,
    pub import_attempts: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryChanged {
    pub revision: LibraryRevision,
    pub session_ids: Vec<i64>,
    pub source_kind: &'static str,
}

enum Notice {
    Paths {
        paths: Vec<PathBuf>,
        structural: bool,
    },
    Wake,
}

type SourceKey = (String, PathBuf);

struct Control {
    wake: mpsc::SyncSender<Notice>,
    shutdown: AtomicBool,
    paused: AtomicBool,
    cancel: Arc<AtomicBool>,
    status: Mutex<Status>,
    native_error: Arc<Mutex<Option<String>>>,
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
        let (sender, receiver) = mpsc::sync_channel(256);
        let control = Arc::new(Control {
            wake: sender,
            shutdown: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancel: Arc::new(AtomicBool::new(false)),
            status: Mutex::new(Status {
                phase: "starting".into(),
                roots: catalog.roots(),
                may_be_stale: true,
                ..Status::default()
            }),
            native_error: Arc::new(Mutex::new(None)),
        });
        let worker_control = control.clone();
        let worker = thread::Builder::new()
            .name("history-observer".into())
            .spawn(move || {
                let _leader = leader;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(&database, catalog, &worker_control, receiver, &*on_change)
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
        let _ = self.control.wake.try_send(Notice::Wake);
    }
    pub fn interrupt(&self) {
        self.control.cancel.store(true, Ordering::SeqCst);
        let _ = self.control.wake.try_send(Notice::Wake);
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        #[cfg(test)]
        if std::thread::panicking() {
            eprintln!("Observer status at failed assertion: {:?}", self.status());
        }
        self.control.shutdown.store(true, Ordering::SeqCst);
        self.interrupt();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
fn directories(roots: &[PathBuf]) -> (BTreeSet<PathBuf>, bool) {
    watch_directories(
        &roots
            .iter()
            .cloned()
            .map(|path| (path, 24))
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
fn watch_directories(roots: &[(PathBuf, usize)]) -> (BTreeSet<PathBuf>, bool) {
    let (paths, truncated, _) = directory_plan(roots);
    (paths, truncated)
}

fn directory_plan(
    roots: &[(PathBuf, usize)],
) -> (
    BTreeSet<PathBuf>,
    bool,
    BTreeMap<PathBuf, hession_core::jsonl::Fingerprint>,
) {
    let mut stamps = BTreeMap::new();
    let mut found = BTreeSet::new();
    let mut visited = BTreeMap::<PathBuf, usize>::new();
    let mut stack: Vec<_> = roots
        .iter()
        .cloned()
        .map(|(path, limit)| (path, 0, limit))
        .collect();
    let mut truncated = false;
    let mut examined = 0usize;
    while let Some((path, depth, limit)) = stack.pop() {
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
        let followed = if meta.file_type().is_symlink() {
            std::fs::metadata(&path).ok()
        } else {
            None
        };
        stamps.entry(path.clone()).or_insert_with(|| {
            hession_core::jsonl::Fingerprint::of(followed.as_ref().unwrap_or(&meta))
        });
        let remaining = limit.saturating_sub(depth);
        let identity = routing_path(&path);
        if visited
            .get(&identity)
            .is_some_and(|previous| *previous >= remaining)
        {
            continue;
        }
        visited.insert(identity, remaining);
        found.insert(path.clone());
        if let Ok(entries) = std::fs::read_dir(&path) {
            if depth >= limit && limit < 24 {
                continue;
            }
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
                    if depth < limit && stack.len() < MAX_DIRECTORIES {
                        stack.push((entry.path(), depth + 1, limit));
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
    // Watch only the nearest existing ancestor of a missing root. Never walk
    // that ancestor: it may be the user's home directory. Creation events let
    // us attach to a newly installed agent without periodic whole-tree scans.
    for (root, _) in roots.iter().filter(|(root, _)| !root.is_dir()) {
        if let Some(parent) = root.ancestors().skip(1).find(|parent| parent.is_dir()) {
            if found.len() < MAX_DIRECTORIES {
                found.insert(parent.into());
            } else {
                truncated = true;
            }
        }
    }
    (found, truncated, stamps)
}

// FSEvents resolves aliases such as macOS /var -> /private/var; inotify
// commonly reports the requested spelling. Resolve existing ancestors as well
// so a deleted file or a not-yet-created SQLite WAL uses the same routing key.
fn routing_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.into()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    for ancestor in absolute.ancestors() {
        if let Ok(resolved) = std::fs::canonicalize(ancestor) {
            let suffix = absolute.strip_prefix(ancestor).unwrap_or(Path::new(""));
            // Joining an empty path adds a trailing slash. That is harmless
            // for directory routing, but makes opening a regular file fail.
            return if suffix.as_os_str().is_empty() {
                resolved
            } else {
                resolved.join(suffix)
            };
        }
    }
    absolute
}

fn directory_identity(root: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(root)
        .ok()
        .filter(|metadata| metadata.is_dir())
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

fn root_identities(roots: &[PathBuf]) -> Vec<Option<(u64, u64)>> {
    roots.iter().map(|root| directory_identity(root)).collect()
}

fn affected(family: &str, source: &Path, event: &Path) -> bool {
    let event = routing_path(event);
    importer::source_paths(family, source)
        .iter()
        .any(|path| routing_path(path).starts_with(&event))
}

fn native_watcher<W: Watcher>(
    roots: Arc<RwLock<Vec<PathBuf>>>,
    sender: mpsc::SyncSender<Notice>,
    overflow: Arc<AtomicBool>,
    errors: Arc<Mutex<Option<String>>>,
) -> notify::Result<W> {
    W::new(
        move |result: notify::Result<Event>| match result {
            Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                let rescan = event.need_rescan();
                let roots = roots.read().unwrap_or_else(|error| error.into_inner());
                let mut paths: Vec<_> = event
                    .paths
                    .into_iter()
                    .filter(|path| {
                        roots
                            .iter()
                            .any(|root| path.starts_with(root) || root.starts_with(path))
                    })
                    .take(33)
                    .collect();
                if rescan || paths.len() > 32 {
                    overflow.store(true, Ordering::SeqCst);
                }
                paths.truncate(32);
                if paths.is_empty() && !rescan {
                    return;
                }
                let structural = !matches!(
                    event.kind,
                    EventKind::Modify(
                        notify::event::ModifyKind::Data(_) | notify::event::ModifyKind::Metadata(_)
                    )
                );
                if sender
                    .try_send(Notice::Paths { paths, structural })
                    .is_err()
                {
                    overflow.store(true, Ordering::SeqCst);
                }
            }
            Err(error) => {
                *errors.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(error.to_string().chars().take(300).collect());
                overflow.store(true, Ordering::SeqCst);
                let _ = sender.try_send(Notice::Wake);
            }
            _ => {}
        },
        notify::Config::default(),
    )
}

// FSEvents may defer writes until an appender closes its file. Keep its scoped
// directory observation for topology, and add kqueue only for exact source files.
// Never give kqueue directories: notify's backend can recursively add children.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct FileWatches {
    watcher: Option<notify::KqueueWatcher>,
    identities: BTreeMap<PathBuf, (u64, u64)>,
}
#[cfg(target_os = "macos")]
impl FileWatches {
    fn reconcile(
        &mut self,
        files: &[SourceKey],
        capacity: usize,
        roots: Arc<RwLock<Vec<PathBuf>>>,
        sender: mpsc::SyncSender<Notice>,
        overflow: Arc<AtomicBool>,
        control: &Control,
    ) -> bool {
        use std::os::unix::fs::MetadataExt;
        let mut desired = BTreeMap::new();
        let mut complete = true;
        for (family, source) in files {
            for path in importer::source_paths(family, source) {
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                let path = routing_path(&path);
                if desired.contains_key(&path) {
                    continue;
                }
                if desired.len() >= capacity {
                    complete = false;
                    break;
                }
                desired.insert(path, (meta.dev(), meta.ino()));
            }
            if !complete {
                break;
            }
        }
        if self.watcher.is_none() && !desired.is_empty() {
            match native_watcher(roots, sender, overflow, control.native_error.clone()) {
                Ok(watcher) => self.watcher = Some(watcher),
                Err(error) => {
                    control.error(format!("Cannot observe open appenders; polling: {error}"));
                    return false;
                }
            }
        }
        let Some(watcher) = self.watcher.as_mut() else {
            return complete;
        };
        self.identities.retain(|path, identity| {
            if desired.get(path) == Some(identity) {
                true
            } else {
                let _ = watcher.unwatch(path);
                false
            }
        });
        for (path, identity) in desired {
            if self.identities.contains_key(&path) {
                continue;
            }
            match watcher.watch(&path, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.identities.insert(path, identity);
                }
                Err(error) => {
                    control.error(format!(
                        "Cannot observe {}; polling: {error}",
                        path.display()
                    ));
                    complete = false;
                }
            }
        }
        complete
    }
}

fn run(
    database: &Path,
    catalog: LocalCatalog,
    control: &Control,
    receiver: mpsc::Receiver<Notice>,
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
    let sender = control.wake.clone();
    let overflow = Arc::new(AtomicBool::new(false));
    let roots = catalog.roots();
    let event_roots = Arc::new(RwLock::new(
        roots
            .iter()
            .flat_map(|root| [root.clone(), routing_path(root)])
            .collect(),
    ));
    let mut root_ids = root_identities(&roots);
    let mut watcher = match native_watcher::<notify::RecommendedWatcher>(
        event_roots.clone(),
        sender.clone(),
        overflow.clone(),
        control.native_error.clone(),
    ) {
        Ok(watcher) => Some(watcher),
        Err(e) => {
            control.error(format!(
                "Native observation unavailable; polling every 15 seconds: {e}"
            ));
            None
        }
    };
    let mut last_native_attempt = Instant::now();
    #[cfg(target_os = "macos")]
    let mut file_watches = FileWatches::default();
    let mut watched = BTreeSet::<PathBuf>::new();
    let mut watched_ids = BTreeMap::<PathBuf, Option<(u64, u64)>>::new();
    let mut watched_paths = BTreeSet::<PathBuf>::new();
    let mut seen_roots = BTreeSet::new();
    let mut pending = Pending::default();
    let mut fingerprints = BTreeMap::<SourceKey, importer::SourceFingerprint>::new();
    let mut known_files = Vec::<SourceKey>::new();
    let mut dependencies = BTreeMap::<PathBuf, Vec<usize>>::new();
    let mut failed_sources = BTreeSet::<SourceKey>::new();
    let mut last_retry = Instant::now();
    let mut topology_pending = false;
    let mut observation_incomplete = true;
    let mut truncated = false;
    let mut last_poll = Instant::now();
    let mut last_audit = Instant::now();
    let mut scan = true;
    let mut coverage_scan = false;
    let mut was_suspended = false;
    let mut last_revision = library_revision::current(&conn).ok();
    let mut codex_titles = None;
    let mut changed = BTreeSet::new();
    let mut last_emit = Instant::now();
    let mut last_probe = Instant::now();
    let mut reclaim_at: Option<Instant> = None;
    let mut reclaim_ceiling: Option<Instant> = None;
    while !control.shutdown.load(Ordering::SeqCst) {
        let now = Instant::now();
        // Periodic revision checks also observe writes from another process.
        // Native observation does not need a 100ms timer or a 15s tree walk.
        let mut wait = (last_probe + POLL).saturating_duration_since(now);
        if control.paused.load(Ordering::SeqCst) || was_suspended {
            wait = POLL;
        } else {
            wait = wait.min((last_audit + AUDIT).saturating_duration_since(now));
            if scan {
                wait = Duration::ZERO;
            }
            if coverage_scan {
                wait = wait
                    .min((last_poll + Duration::from_millis(400)).saturating_duration_since(now));
            }
            if let Some(delay) = pending.next_ready_in(now) {
                wait = wait.min(delay);
            }
            if !failed_sources.is_empty() {
                wait = wait.min((last_retry + POLL).saturating_duration_since(now));
            }
            if observation_incomplete {
                wait = wait.min((last_poll + POLL).saturating_duration_since(now));
            }
            if watcher.is_none() {
                wait =
                    wait.min((last_native_attempt + NATIVE_RETRY).saturating_duration_since(now));
            }
        }
        for deadline in [reclaim_at, reclaim_ceiling].into_iter().flatten() {
            wait = wait.min(deadline.saturating_duration_since(now));
        }
        let mut receive = |notice| match notice {
            Notice::Paths { paths, structural } => {
                for path in paths {
                    let path = routing_path(&path);
                    topology_pending |= structural || watched_paths.contains(&path);
                    pending.push(&path, Instant::now());
                }
            }
            Notice::Wake => {}
        };
        match receiver.recv_timeout(wait) {
            Ok(notice) => receive(notice),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {}
        }
        for _ in 0..256 {
            match receiver.try_recv() {
                Ok(notice) => receive(notice),
                Err(_) => break,
            }
        }
        control.update(|s| s.wakeups = s.wakeups.saturating_add(1));
        if control.shutdown.load(Ordering::SeqCst) {
            break;
        }
        if overflow.swap(false, Ordering::SeqCst) {
            pending.overflow();
            control.update(|s| s.overflow_count = s.overflow_count.saturating_add(1));
            control.error("Observation queue overflow or native gap; reconciling known roots");
        }
        let native_error = control
            .native_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(error) = native_error {
            control.error(format!(
                "Native observation failed; polling until a successful retry: {error}"
            ));
            watcher = None;
            #[cfg(target_os = "macos")]
            {
                file_watches = FileWatches::default();
            }
            watched.clear();
            watched_ids.clear();
            watched_paths.clear();
            scan = true;
            last_native_attempt = Instant::now();
            control.update(|s| {
                s.phase = "degraded".into();
                s.watched_directories = 0;
            });
        }
        if [reclaim_at, reclaim_ceiling]
            .into_iter()
            .flatten()
            .any(|deadline| Instant::now() >= deadline)
        {
            reclaim_at = None;
            reclaim_ceiling = None;
            if let Err(error) = conn.execute_batch("PRAGMA shrink_memory") {
                control.error(error.to_string());
            }
            crate::memory::release();
        }
        // Clear the preceding pass's cancellation before reading the manual
        // sync gate. A manual starter holds that gate while interrupting us,
        // so its new cancellation cannot be cleared after admission is checked.
        control.cancel.store(false, Ordering::SeqCst);
        let suspended = control.paused.load(Ordering::SeqCst)
            || crate::sync::is_managing_data()
            || crate::sync::is_running();
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
            codex_titles = None;
        }
        // Native watches belong to inodes. Reattach after a root is replaced
        // at the same path or an existing directory symlink is retargeted.
        // Piggyback on structural events/revision checks, never a source walk.
        let mut roots_replaced = false;
        if topology_pending || last_probe.elapsed() >= POLL {
            let current = root_identities(&roots);
            if current != root_ids {
                root_ids = current;
                roots_replaced = true;
                watcher = None;
                #[cfg(target_os = "macos")]
                {
                    file_watches = FileWatches::default();
                }
                watched.clear();
                watched_ids.clear();
                watched_paths.clear();
                scan = true;
            }
        }
        if watcher.is_none() && (roots_replaced || last_native_attempt.elapsed() >= NATIVE_RETRY) {
            last_native_attempt = Instant::now();
            match native_watcher::<notify::RecommendedWatcher>(
                event_roots.clone(),
                sender.clone(),
                overflow.clone(),
                control.native_error.clone(),
            ) {
                Ok(next) => {
                    watcher = Some(next);
                    scan = true;
                }
                Err(error) => control.error(format!(
                    "Native observation retry failed; polling continues: {error}"
                )),
            }
        }
        let now = Instant::now();
        let (events, recovery) = pending.take_ready(now);
        let audit = now.duration_since(last_audit) >= AUDIT;
        let poll = observation_incomplete && now.duration_since(last_poll) >= POLL;
        let retry = !failed_sources.is_empty() && now.duration_since(last_retry) >= POLL;
        let coverage = coverage_scan && now.duration_since(last_poll) >= Duration::from_millis(400);
        let discover = scan
            || coverage
            || recovery
            || audit
            || poll
            || (!events.is_empty() && topology_pending);
        let do_scan = discover || retry || !events.is_empty();
        if do_scan {
            // Routine safety sweeps compare inode/size/mtime/ctime. Content
            // verification still handles native gaps and
            // affected sources without rehashing every quiet store every 10m.
            let full_audit = recovery;
            scan = false;
            coverage_scan = false;
            last_poll = now;
            if audit {
                last_audit = now;
            }
            if !events.is_empty() {
                topology_pending = false;
            }
            if discover {
                control.update(|s| s.discovery_passes = s.discovery_passes.saturating_add(1));
                known_files =
                    importer::with_cancellation(control.cancel.clone(), || catalog.files(&conn));
                let mut scopes: BTreeMap<_, _> =
                    importer::with_cancellation(control.cancel.clone(), || {
                        catalog.observation_roots()
                    })
                    .into_iter()
                    .collect();
                for (family, path) in &known_files {
                    for companion in importer::source_paths(family, path) {
                        // Explicit parent scopes also cover directory aliases
                        // used by profiles and companion stores outside roots.
                        if let Some(parent) = companion.parent() {
                            scopes.entry(parent.into()).or_insert(0);
                        }
                    }
                }
                let scopes: Vec<_> = scopes.into_iter().collect();
                let interest: BTreeSet<_> = roots
                    .iter()
                    .cloned()
                    .chain(scopes.iter().map(|(path, _)| path.clone()))
                    .flat_map(|path| [path.clone(), routing_path(&path)])
                    .collect();
                *event_roots
                    .write()
                    .unwrap_or_else(|error| error.into_inner()) = interest.into_iter().collect();
                let (desired, incomplete, directory_stamps) = directory_plan(&scopes);
                let desired: BTreeSet<_> = desired.iter().map(|path| routing_path(path)).collect();
                truncated = incomplete;
                #[cfg(target_os = "macos")]
                {
                    truncated |= !file_watches.reconcile(
                        &known_files,
                        MAX_DIRECTORIES.saturating_sub(desired.len()),
                        event_roots.clone(),
                        sender.clone(),
                        overflow.clone(),
                        control,
                    );
                }

                if truncated {
                    control.error("Directory observation is incomplete; some sources may be stale. Check source permissions or narrow the configured roots.");
                }
                if let Some(watcher) = watcher.as_mut() {
                    let desired_ids: BTreeMap<_, _> = desired
                        .iter()
                        .map(|path| (path.clone(), directory_identity(path)))
                        .collect();
                    let removed: Vec<_> = watched
                        .iter()
                        .filter(|path| {
                            !desired.contains(*path)
                                || watched_ids.get(*path) != desired_ids.get(*path)
                        })
                        .cloned()
                        .collect();
                    let mut actual = watched
                        .intersection(&desired)
                        .filter(|path| watched_ids.get(*path) == desired_ids.get(*path))
                        .cloned()
                        .collect::<BTreeSet<_>>();
                    let missing: Vec<_> = desired.difference(&actual).cloned().collect();
                    if !removed.is_empty() || !missing.is_empty() {
                        // FSEvents restarts its stream for each ordinary watch()
                        // call. A batch registers the entire topology once and
                        // avoids thousands of restarts and event-loss windows.
                        let mut edits = watcher.paths_mut();
                        for path in &removed {
                            let _ = edits.remove(path);
                        }
                        for path in &missing {
                            match edits.add(path, RecursiveMode::NonRecursive) {
                                Ok(()) => {
                                    actual.insert(path.clone());
                                }
                                Err(e) => control.error(format!(
                                    "Cannot watch {}; polling: {e}",
                                    path.display()
                                )),
                            }
                        }
                        if let Err(error) = edits.commit() {
                            actual.clear();
                            *control
                                .native_error
                                .lock()
                                .unwrap_or_else(|e| e.into_inner()) = Some(error.to_string());
                            let _ = sender.try_send(Notice::Wake);
                        }
                    }
                    watched = actual;
                    watched_ids = desired_ids
                        .into_iter()
                        .filter(|(path, _)| watched.contains(path))
                        .collect();
                    watched_paths = watched.iter().map(|path| routing_path(path)).collect();
                }
                // Close the list-before-subscribe gap. Directory cookies catch
                // new empty subtrees during registration; rediscovery catches
                // files created before their first watch was attached.
                let directories_changed = directory_stamps.iter().any(|(path, stamp)| {
                    std::fs::metadata(path).map_or(true, |metadata| {
                        hession_core::jsonl::Fingerprint::of(&metadata) != *stamp
                    })
                }) || scopes
                    .iter()
                    .any(|(path, _)| path.is_dir() && !desired.contains(&routing_path(path)));
                let refreshed =
                    importer::with_cancellation(control.cancel.clone(), || catalog.files(&conn));
                let previous: BTreeSet<_> = known_files.iter().collect();
                let current: BTreeSet<_> = refreshed.iter().collect();
                // A capped or failed watch plan uses bounded fallback polling;
                // it must not spin trying to attach unreachable directories.
                coverage_scan = !incomplete
                    && watched == desired
                    && (directories_changed || previous != current);
                known_files = refreshed;
                let unavailable = roots
                    .iter()
                    .filter(|root| !root.is_dir())
                    .cloned()
                    .collect::<Vec<_>>();
                let missing_previously_available =
                    unavailable.iter().any(|root| seen_roots.contains(root));
                seen_roots.extend(roots.iter().filter(|root| root.is_dir()).cloned());
                observation_incomplete = truncated
                    || watcher.is_none()
                    || watched != desired
                    || (!roots.is_empty() && unavailable.len() == roots.len())
                    || missing_previously_available;
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
                if known_files.iter().any(|(_, path)| {
                    std::fs::symlink_metadata(path)
                        .is_ok_and(|metadata| metadata.file_type().is_symlink())
                }) {
                    observation_incomplete = true;
                    control.update(|s| s.phase = "degraded".into());
                }
                dependencies.clear();
                for (index, key) in known_files.iter().enumerate() {
                    for path in importer::source_paths(&key.0, &key.1) {
                        dependencies
                            .entry(routing_path(&path))
                            .or_default()
                            .push(index);
                    }
                }
            }
            let epoch = match indexing::epoch(&conn) {
                Ok(epoch) => epoch,
                Err(e) => {
                    control.error(e.to_string());
                    observation_incomplete = true;
                    last_probe = now;
                    last_poll = now;
                    control.update(|s| s.phase = "degraded".into());
                    continue;
                }
            };
            let mut files = if discover {
                known_files.clone()
            } else {
                events
                    .iter()
                    .filter_map(|event| dependencies.get(event))
                    .flatten()
                    .copied()
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .map(|index| known_files[index].clone())
                    .collect()
            };
            if retry {
                last_retry = now;
                let selected: BTreeSet<_> = files
                    .into_iter()
                    .chain(failed_sources.iter().cloned())
                    .collect();
                files = selected.into_iter().collect();
            }
            let live = if discover {
                Some(known_files.iter().cloned().collect::<BTreeSet<_>>())
            } else {
                None
            };

            let mut imported = false;
            let mut remaining_sources = files.len();
            control.update(|s| s.pending_sources = remaining_sources);
            for (family, path) in files {
                if control.cancel.load(Ordering::SeqCst)
                    || control.paused.load(Ordering::SeqCst)
                    || control.shutdown.load(Ordering::SeqCst)
                {
                    scan = true;
                    break;
                }
                remaining_sources = remaining_sources.saturating_sub(1);
                control.update(|s| s.pending_sources = remaining_sources);
                let key = (family.clone(), path.clone());
                control.update(|s| s.fingerprint_checks = s.fingerprint_checks.saturating_add(1));
                let stamp = match catalog.fingerprint(&family, &path) {
                    Ok(stamp) => stamp,
                    Err(e) => {
                        control.error(e);
                        failed_sources.insert(key);
                        continue;
                    }
                };
                if !full_audit
                    && !failed_sources.contains(&key)
                    && fingerprints.get(&key) == Some(&stamp)
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
                });
                let before = conn
                    .query_row("SELECT COALESCE(MAX(seq),0) FROM index_change", [], |r| {
                        r.get::<_, i64>(0)
                    })
                    .unwrap_or(0);
                imported = true;
                control.update(|s| s.import_attempts = s.import_attempts.saturating_add(1));
                let import_started = Instant::now();
                let result = importer::with_cancellation(control.cancel.clone(), || {
                    if full_audit || events.iter().any(|event| affected(&family, &path, event)) {
                        catalog.import_with_policy(&mut conn, &family, &path, epoch, true)
                    } else {
                        catalog.import(&mut conn, &family, &path, epoch)
                    }
                });
                match result {
                    Ok(_) => {
                        failed_sources.remove(&key);
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
                        failed_sources.insert(key);
                    }
                    Err(_) => {
                        scan = true;
                        remaining_sources = remaining_sources.saturating_add(1);
                    }
                }
                // Initial indexing can span many sources: free old native
                // allocations on the bounded background schedule during it too.
                crate::memory::schedule();
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
                if let Some(live) = live {
                    fingerprints.retain(|key, _| live.contains(key));
                    failed_sources.retain(|key| live.contains(key));
                }
            }
            if imported {
                let now = Instant::now();
                reclaim_at = Some(now + IDLE_RECLAIM);
                reclaim_ceiling.get_or_insert(now + crate::memory::MAX_DELAY);
            }
            control.update(|s| {
                s.active_source = None;
                s.pending_sources = remaining_sources;
                s.may_be_stale = !failed_sources.is_empty()
                    || truncated
                    || observation_incomplete
                    || coverage_scan;
            });
        }
        if do_scan || last_probe.elapsed() >= POLL {
            if let Some(root) = catalog.codex_root() {
                let refreshed = importer::with_cancellation(control.cancel.clone(), || {
                    hession_core::indexing::epoch(&conn)
                        .map_err(|error| error.to_string())
                        .and_then(|epoch| {
                            importer::codex_titles::reconcile(
                                &mut conn,
                                root,
                                epoch,
                                &mut codex_titles,
                            )
                        })
                });
                match refreshed {
                    Ok(ids) => {
                        changed.extend(ids.into_iter().take(128usize.saturating_sub(changed.len())))
                    }
                    Err(error) if !control.cancel.load(Ordering::SeqCst) => {
                        control.error(format!("Codex session names: {error}"))
                    }
                    Err(_) => {}
                }
            }
            emit(&conn, &mut last_revision, &mut changed, on_change);
            last_probe = Instant::now();
        }
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
pub(crate) fn wake_after_manual() {
    if let Some(observer) = desktop().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        let _ = observer.control.wake.try_send(Notice::Wake);
    }
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

#[tauri::command]
pub async fn recent_local_sessions(
    state: tauri::State<'_, crate::AppState>,
    agent: Option<String>,
    project: Option<String>,
    since: Option<String>,
    limit: Option<u32>,
    offset: Option<u64>,
) -> Result<serde_json::Value, String> {
    let path = state.db_path.lock().map_err(|e| e.to_string())?.clone();
    let request = hession_core::live::RecentRequest {
        agent: agent.unwrap_or_default(),
        project: project.unwrap_or_default(),
        since: since.unwrap_or_default(),
        limit: limit.unwrap_or(50),
        offset: offset.unwrap_or(0),
        unhealthy: indexing_status().may_be_stale,
    };
    tauri::async_runtime::spawn_blocking(move || {
        hession_core::service::Library::new(path)
            .recent_local_sessions(&request)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn observed_library_revision(
    state: tauri::State<'_, crate::AppState>,
) -> Result<LibraryRevision, String> {
    let path = state.db_path.lock().map_err(|e| e.to_string())?.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let conn = db::open_read_only(&path).map_err(|e| e.to_string())?;
        library_revision::current(&conn).map_err(|e| e.to_string())
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
        let external: String = conn
            .query_row("SELECT external_id FROM session", [], |row| row.get(0))
            .unwrap();
        std::fs::write(
            source_root.join(importer::codex_titles::INDEX_FILE),
            format!(
                "{}\n",
                serde_json::json!({"id":external,"thread_name":"Renamed from Codex"})
            ),
        )
        .unwrap();
        wait_until(|| {
            conn.query_row("SELECT title FROM session", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap()
                == "Renamed from Codex"
        });
        assert_eq!(count(), 4);
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
    fn quiet_observer_sleeps_and_single_append_does_not_touch_neighboring_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("codex");
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        for n in 0..64 {
            std::fs::write(sessions.join(format!("{n}.jsonl")), format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"isolated-{n}\",\"cwd\":\"/synthetic\"}}}}\n{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"Initial history\"}}}}\n"
            )).unwrap();
        }
        let database = dir.path().join("library.sqlite");
        let observer = Observer::start(
            database.clone(),
            LocalCatalog::fixture(root),
            Arc::new(|_| {}),
        )
        .unwrap();
        let conn = db::open_db(&database).unwrap();
        let count = || {
            conn.query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        wait_until(|| {
            count() == 128
                && !observer.status().may_be_stale
                && observer.status().active_source.is_none()
        });
        let before = observer.status();
        // Cross the old unconditional 15s scan boundary. Idle status/revision
        // monitoring may wake, but no source tree or fingerprint should be read.
        thread::sleep(Duration::from_secs(16));
        let quiet = observer.status();
        assert_eq!(quiet.discovery_passes, before.discovery_passes);
        assert_eq!(quiet.fingerprint_checks, before.fingerprint_checks);
        assert_eq!(quiet.import_attempts, before.import_attempts);
        assert!(
            quiet.wakeups - before.wakeups <= 3,
            "quiet observer must stay asleep"
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(sessions.join("0.jsonl"))
            .unwrap();
        writeln!(file, "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"One small continuation\"}}}}").unwrap();
        wait_until(|| count() == 129 && observer.status().active_source.is_none());
        let after = observer.status();
        assert_eq!(after.discovery_passes, quiet.discovery_passes);
        assert_eq!(after.fingerprint_checks - quiet.fingerprint_checks, 1);
        assert_eq!(after.import_attempts - quiet.import_attempts, 1);
        // Different native backends can report the same committed write again
        // (for example kqueue on write and FSEvents when the writer closes).
        observer
            .control
            .wake
            .try_send(Notice::Paths {
                paths: vec![sessions.join("0.jsonl")],
                structural: false,
            })
            .unwrap();
        wait_until(|| observer.status().fingerprint_checks > after.fingerprint_checks);
        assert_eq!(observer.status().import_attempts, after.import_attempts);
        assert_eq!(count(), 129);
        // Control messages must interrupt the long native receive timeout.
        let start = Instant::now();
        drop(observer);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn native_discovery_tracks_nested_creation_atomic_replacement_and_failed_sources() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("codex");
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let good = sessions.join("good.jsonl");
        std::fs::write(
            &good,
            include_str!("../tests/fixtures/codex-concurrency.jsonl"),
        )
        .unwrap();
        let bad = sessions.join("bad.jsonl");
        std::fs::write(&bad, "invalid json\n").unwrap();
        let database = dir.path().join("library.sqlite");
        let observer = Observer::start(
            database.clone(),
            LocalCatalog::fixture(root),
            Arc::new(|_| {}),
        )
        .unwrap();
        let conn = db::open_db(&database).unwrap();
        wait_until(|| {
            !observer.status().errors.is_empty() && observer.status().active_source.is_none()
        });
        assert!(observer.status().may_be_stale);
        let nested = sessions.join("new/nested");
        std::fs::create_dir_all(&nested).unwrap();
        let new_file = nested.join("new.jsonl");
        let original = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"new-session\"}}\n{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"New nested history\"}}\n";
        std::fs::write(&new_file, original).unwrap();
        let body_exists = |body: &str| {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM content_part WHERE text=?)",
                [body],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
        };
        wait_until(|| {
            body_exists("New nested history") && observer.status().active_source.is_none()
        });
        assert!(
            observer.status().may_be_stale,
            "another successful source must not hide the failed source"
        );
        let replacement = nested.join("replacement.tmp");
        std::fs::write(
            &replacement,
            original.replace("New nested history", "Replaced history"),
        )
        .unwrap();
        std::fs::rename(replacement, &new_file).unwrap();
        wait_until(|| body_exists("Replaced history") && !body_exists("New nested history"));
        std::fs::write(
            &bad,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"repaired\"}}\n",
        )
        .unwrap();
        wait_until(|| !observer.status().may_be_stale && observer.status().active_source.is_none());
        let relocated = sessions.with_extension("old");
        std::fs::rename(&sessions, &relocated).unwrap();
        std::fs::create_dir(&sessions).unwrap();
        let replacement = sessions.join("replacement.jsonl");
        std::fs::write(
            &replacement,
            original
                .replace("new-session", "root-replacement")
                .replace("New nested history", "Replacement root"),
        )
        .unwrap();
        wait_until(|| body_exists("Replacement root") && !observer.status().may_be_stale);
        let mut output = std::fs::OpenOptions::new()
            .append(true)
            .open(&replacement)
            .unwrap();
        writeln!(output, "{{\"type\":\"event_msg\",\"payload\":{{\"type\":\"user_message\",\"message\":\"Append after root replacement\"}}}}").unwrap();
        wait_until(|| body_exists("Append after root replacement"));
        assert_eq!(
            conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
    }

    #[test]
    fn directory_permission_changes_mark_observation_stale_and_recover_without_polling() {
        use std::os::unix::fs::PermissionsExt;
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o700));
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("codex");
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(
            sessions.join("fixture.jsonl"),
            include_str!("../tests/fixtures/codex-concurrency.jsonl"),
        )
        .unwrap();
        let observer = Observer::start(
            dir.path().join("library.sqlite"),
            LocalCatalog::fixture(root),
            Arc::new(|_| {}),
        )
        .unwrap();
        wait_until(|| !observer.status().may_be_stale && observer.status().active_source.is_none());
        let _restore = Restore(sessions.clone());
        let before = observer.status().discovery_passes;
        std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o0)).unwrap();
        // Privileged test runners can read mode-000 directories. They should
        // still receive/reconcile metadata events, without claiming an IO error.
        let inaccessible = std::fs::read_dir(&sessions).is_err();
        let deadline = Instant::now() + Duration::from_secs(8);
        while if inaccessible {
            !observer.status().may_be_stale
        } else {
            observer.status().discovery_passes <= before
        } {
            assert!(
                Instant::now() < deadline,
                "directory permission event was ignored until a periodic poll"
            );
            thread::sleep(Duration::from_millis(20));
        }
        std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
        wait_until(|| !observer.status().may_be_stale && observer.status().phase == "active");
    }

    #[test]
    fn agent_package_trees_do_not_consume_history_watches_or_trigger_imports() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("hermes");
        let packages = root.join(".venv/lib/packages");
        std::fs::create_dir_all(&packages).unwrap();
        for number in 0..100 {
            std::fs::create_dir(packages.join(format!("package-{number}"))).unwrap();
        }
        let sessions = root.join("sessions");
        let profile = root.join("profiles/default");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::create_dir_all(profile.join(".venv/lib/cache")).unwrap();
        let file = sessions.join("fixture.jsonl");
        std::fs::write(
            &file,
            "{\"type\":\"message\",\"role\":\"user\",\"content\":\"Initial Hermes history\"}\n",
        )
        .unwrap();
        let catalog = LocalCatalog::fixture_hermes(root.clone());
        let (desired, truncated) = watch_directories(&catalog.observation_roots());
        assert!(!truncated);
        assert!(desired.contains(&sessions));
        assert!(desired.contains(&profile));
        assert!(!desired
            .iter()
            .any(|path| path.starts_with(root.join(".venv"))
                || path.starts_with(profile.join(".venv"))));
        let database = dir.path().join("library.sqlite");
        let observer = Observer::start(database.clone(), catalog, Arc::new(|_| {})).unwrap();
        let conn = db::open_db(&database).unwrap();
        let count = || {
            conn.query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        wait_until(|| {
            count() == 1
                && !observer.status().may_be_stale
                && observer.status().active_source.is_none()
        });
        let before = observer.status();
        std::fs::write(packages.join("package-0/cache.txt"), "Package cache noise").unwrap();
        let mut source = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        writeln!(
            source,
            "{{\"type\":\"message\",\"role\":\"user\",\"content\":\"A real history change\"}}"
        )
        .unwrap();
        wait_until(|| count() == 2 && observer.status().active_source.is_none());
        let after = observer.status();
        assert_eq!(after.discovery_passes, before.discovery_passes);
        assert_eq!(after.import_attempts - before.import_attempts, 1);
        assert!(!after.may_be_stale);
    }

    #[test]
    fn exhausted_watch_budget_uses_fallback_without_a_reconciliation_spin() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("codex");
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        for number in 0..MAX_DIRECTORIES {
            std::fs::create_dir(sessions.join(format!("empty-{number}"))).unwrap();
        }
        let observer = Observer::start(
            dir.path().join("library.sqlite"),
            LocalCatalog::fixture(root),
            Arc::new(|_| {}),
        )
        .unwrap();
        wait_until(|| {
            let status = observer.status();
            status.phase == "degraded" && status.discovery_passes > 0
        });
        let before = observer.status();
        thread::sleep(Duration::from_millis(1200));
        let after = observer.status();
        assert!(after.may_be_stale);
        assert_eq!(after.discovery_passes, before.discovery_passes);
        assert!(after.wakeups - before.wakeups <= 1);
    }

    #[test]
    #[ignore = "metadata-only diagnostic of explicitly selected installed source directories"]
    fn installed_observation_watch_plan() {
        let catalog = LocalCatalog::capture(None);
        let start = Instant::now();
        let (old, old_truncated) = directories(&catalog.roots());
        let old_ms = start.elapsed().as_secs_f64() * 1000.;
        let start = Instant::now();
        let (new, new_truncated) = watch_directories(&catalog.observation_roots());
        println!(
            "{}",
            serde_json::json!({"broadDirectoryCount":old.len(),"broadTruncated":old_truncated,
            "broadMs":old_ms,"historyDirectoryCount":new.len(),"historyTruncated":new_truncated,
            "historyMs":start.elapsed().as_secs_f64()*1000.})
        );
        assert!(!new_truncated, "installed history watch plan is incomplete");
    }

    #[test]
    fn directory_walk_is_bounded_and_does_not_follow_cycles() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join("nested/cycle")).unwrap();
        let alias = dir.path().join("alias");
        std::os::unix::fs::symlink(dir.path().join("nested"), &alias).unwrap();
        assert_eq!(
            routing_path(&alias.join("missing.db-wal")),
            routing_path(&dir.path().join("nested/missing.db-wal"))
        );
        let history = alias.join("history.jsonl");
        std::fs::write(&history, "Complete existing source").unwrap();
        assert_eq!(
            std::fs::read_to_string(routing_path(&history)).unwrap(),
            "Complete existing source"
        );
        assert!(affected(
            "codex",
            &alias.join("history.jsonl"),
            &dir.path().join("nested/history.jsonl")
        ));
        let (found, truncated) = directories(&[dir.path().into()]);
        assert_eq!(found.len(), 2);
        assert!(!truncated);
        let (anchors, truncated) = directories(&[dir.path().join("missing/child")]);
        assert_eq!(anchors, BTreeSet::from([dir.path().to_path_buf()]));
        assert!(
            !truncated,
            "missing roots must not walk their existing parent"
        );
        assert!(!affected(
            "codex",
            Path::new("/store/one.jsonl"),
            Path::new("/store/two.jsonl")
        ));
        assert!(!affected(
            "hermes",
            Path::new("/store/state.db"),
            Path::new("/store/other.db-wal")
        ));
        assert!(affected(
            "hermes",
            Path::new("/store/state.db"),
            Path::new("/store/state.db-wal")
        ));
        assert!(affected(
            "antigravity",
            Path::new("/store/brain/conversation/trace.jsonl"),
            Path::new("/store/conversation_summaries.db-wal")
        ));
        assert!(affected(
            "cline",
            Path::new("/store/tasks/task/api_conversation_history.json"),
            Path::new("/store/state/taskHistory.json")
        ));
    }

    #[test]
    fn native_callback_failure_stays_degraded_until_backend_retry_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("codex");
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        let observer = Observer::start(
            dir.path().join("library.sqlite"),
            LocalCatalog::fixture(root),
            Arc::new(|_| {}),
        )
        .unwrap();
        wait_until(|| observer.status().phase == "active");
        *observer.control.native_error.lock().unwrap() =
            Some("synthetic callback backend failure".into());
        let _ = observer.control.wake.try_send(Notice::Wake);
        wait_until(|| observer.status().phase == "degraded");
        let failed = observer.status();
        assert!(failed.may_be_stale);
        assert_eq!(failed.watched_directories, 0);
        assert!(failed
            .errors
            .iter()
            .any(|e| e.contains("synthetic callback")));
        wait_until(|| {
            observer.status().phase == "active" && observer.status().watched_directories > 0
        });
        assert!(!observer.status().may_be_stale);
        assert_eq!(observer.status().pending_sources, 0);
    }

    #[test]
    fn initially_missing_root_reports_degraded_and_polling_recovers_when_created() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("new-agent");
        let database = dir.path().join("library.sqlite");
        let observer = Observer::start(
            database.clone(),
            LocalCatalog::fixture(root.clone()),
            Arc::new(|_| {}),
        )
        .unwrap();
        wait_until(|| observer.status().phase == "degraded");
        assert!(observer.status().may_be_stale);
        assert_eq!(
            observer.status().unavailable_roots,
            vec![root.join("sessions")]
        );
        std::fs::create_dir_all(root.join("sessions")).unwrap();
        std::fs::write(
            root.join("sessions/new.jsonl"),
            include_str!("../tests/fixtures/codex-concurrency.jsonl"),
        )
        .unwrap();
        let conn = db::open_db(&database).unwrap();
        wait_until(|| {
            conn.query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))
                .unwrap()
                == 4
        });
        wait_until(|| observer.status().phase == "active");
        assert!(observer.status().unavailable_roots.is_empty());
        assert_eq!(observer.status().pending_sources, 0);
    }
}
