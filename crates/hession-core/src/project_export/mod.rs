//! Portable, non-restorable project history archives. Preparation is a bounded read-only
//! snapshot; writing accepts only an immutable prepared selection and creates a new destination.
mod archive;
mod privacy;
mod transcript;

use crate::{
    db,
    library_revision::{self, LibraryRevision},
    projects::{self, Project},
    search::{self, SearchFilters, SearchMode, SearchRequest, SearchScope, SearchVisibility},
    service::Library,
};
pub use privacy::RedactionSummary;
use privacy::Sanitizer;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{self, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("{0}")]
    Database(#[from] db::DbError),
    #[error("{0}")]
    Sql(#[from] rusqlite::Error),
    #[error("{0}")]
    Search(#[from] search::SearchError),
    #[error("{0}")]
    Project(#[from] projects::ProjectError),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("Export cancelled; no destination was saved")]
    Cancelled,
    #[error("The library changed after preview; prepare the export again")]
    StalePreview,
    #[error("Export exceeds the {0} bound; narrow the query or reduce the exported content")]
    Limit(&'static str),
    #[error("{0}")]
    Invalid(String),
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptFormat {
    #[default]
    Markdown,
    Json,
    Both,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: TranscriptFormat,
    pub include_statistics: bool,
    pub include_tool_bodies: bool,
    pub include_absolute_paths: bool,
    pub redact: bool,
    pub include_git_metadata: bool,
}
impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            format: TranscriptFormat::Markdown,
            include_statistics: false,
            include_tool_bodies: false,
            include_absolute_paths: false,
            redact: true,
            include_git_metadata: false,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ExportLimits {
    pub max_sessions: u32,
    pub max_entries: u64,
    pub max_activity_rows: u64,
    pub max_archive_bytes: u64,
    pub max_member_bytes: u64,
    pub max_cell_bytes: usize,
    pub max_entry_bytes: usize,
    pub timeout_ms: u64,
}
impl Default for ExportLimits {
    fn default() -> Self {
        Self {
            max_sessions: 2000,
            max_entries: 1_000_000,
            max_activity_rows: 100_000,
            max_archive_bytes: 512 * 1024 * 1024,
            max_member_bytes: 64 * 1024 * 1024,
            max_cell_bytes: 256 * 1024,
            max_entry_bytes: 2 * 1024 * 1024,
            timeout_ms: 120_000,
        }
    }
}
impl ExportLimits {
    fn validate(self) -> Result<Self, ExportError> {
        if self.max_sessions == 0
            || self.max_sessions > 10_000
            || self.max_entries == 0
            || self.max_entries > 2_000_000
            || self.max_activity_rows > 1_000_000
            || self.max_archive_bytes == 0
            || self.max_archive_bytes > 1024 * 1024 * 1024
            || self.max_member_bytes == 0
            || self.max_member_bytes > 128 * 1024 * 1024
            || self.max_cell_bytes == 0
            || self.max_cell_bytes > 1024 * 1024
            || self.max_entry_bytes == 0
            || self.max_entry_bytes > 8 * 1024 * 1024
            || self.timeout_ms == 0
            || self.timeout_ms > 300_000
        {
            return Err(ExportError::Invalid(
                "Invalid export resource limits".into(),
            ));
        }
        Ok(self)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ProjectExportRequest {
    pub workspace_id: i64,
    pub query: String,
    pub filters: SearchFilters,
    pub options: ExportOptions,
    pub limits: ExportLimits,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreview {
    pub token: String,
    pub revision: LibraryRevision,
    pub prepared_at: String,
    pub project_key: String,
    pub project_display_name: String,
    pub selected_session_count: usize,
    pub session_count: usize,
    pub exported_entry_count: u64,
    pub estimated_uncompressed_bytes: u64,
    pub intended_output: PathBuf,
    pub options: ExportOptions,
    pub redactions: RedactionSummary,
    pub warnings: Vec<String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportStage {
    Preparing,
    Writing,
    Finalizing,
    Saved,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportProgress {
    pub stage: ExportStage,
    pub completed_sessions: usize,
    pub total_sessions: usize,
    pub entries: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub destination: PathBuf,
    pub session_count: usize,
    pub archive_bytes: u64,
    pub warnings: Vec<String>,
}
#[derive(Default)]
struct CancelState {
    cancelled: AtomicBool,
    used: AtomicBool,
    saved: Mutex<bool>,
}
/// Use one cancellation handle per write job. cancel() returns false once atomic publication won.
#[derive(Clone, Default)]
pub struct ExportCancellation(Arc<CancelState>);
impl ExportCancellation {
    pub fn cancel(&self) -> bool {
        let saved = self.0.saved.lock().unwrap_or_else(|e| e.into_inner());
        if *saved {
            return false;
        }
        self.0.cancelled.store(true, Ordering::Release);
        true
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }
    fn check(&self) -> Result<(), ExportError> {
        if self.is_cancelled() {
            Err(ExportError::Cancelled)
        } else {
            Ok(())
        }
    }
    fn begin(&self) -> Result<(), ExportError> {
        if self.0.used.swap(true, Ordering::AcqRel) {
            return Err(ExportError::Invalid(
                "Cancellation handles cannot be reused for another write".into(),
            ));
        }
        self.check()
    }
}
pub(super) struct Work<'a> {
    pub conn: &'a Connection,
    pub cancel: &'a ExportCancellation,
    pub limits: ExportLimits,
    pub deadline: Instant,
}
impl Work<'_> {
    pub fn check(&self) -> Result<(), ExportError> {
        self.cancel.check()?;
        if Instant::now() >= self.deadline {
            return Err(ExportError::Limit("execution time"));
        }
        Ok(())
    }
}
#[derive(Clone)]
pub(super) struct SessionPlan {
    pub local_id: i64,
    pub portable_id: String,
    pub snapshot_id: String,
    pub canonical_bytes: u64,
    pub identity_basis: &'static str,
    pub statistics: Option<serde_json::Value>,
    pub exported_entries: u64,
}
/// Not deserializable and fields are private: clients cannot alter a reviewed selection.
#[derive(Clone)]
pub struct PreparedProjectExport {
    database: PathBuf,
    destination: PathBuf,
    request: ProjectExportRequest,
    project: Project,
    preview: ExportPreview,
    sessions: Vec<SessionPlan>,
    exported_at: String,
}
impl PreparedProjectExport {
    pub fn preview(&self) -> &ExportPreview {
        &self.preview
    }
}

fn destination(database: &Path, path: &Path) -> Result<PathBuf, ExportError> {
    if path.as_os_str().is_empty() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(ExportError::Invalid(
            "Choose a new archive filename without parent traversal".into(),
        ));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent.canonicalize()?;
    let filename = path
        .file_name()
        .ok_or_else(|| ExportError::Invalid("Choose an archive filename".into()))?;
    let result = parent.join(filename);
    let database = database.canonicalize()?;
    if result == database
        || ["-wal", "-shm", ".write.lock", ".watch.lock"]
            .iter()
            .any(|suffix| result.as_os_str() == format!("{}{suffix}", database.display()).as_str())
    {
        return Err(ExportError::Invalid(
            "The archive must be separate from the active library and journals".into(),
        ));
    }
    if std::fs::symlink_metadata(&result).is_ok() {
        return Err(ExportError::Invalid(
            "The destination already exists; choose a new archive filename".into(),
        ));
    }
    Ok(result)
}
const METADATA_LIMIT_MARKER: &str = "HESSION_EXPORT_METADATA_LIMIT";

fn open(
    library: &Library,
    cancel: &ExportCancellation,
    limits: ExportLimits,
) -> Result<(Connection, Instant), ExportError> {
    limits.validate()?;
    cancel.check()?;
    let mut conn = db::open_read_only(library.path())?;
    // Only a numeric/zero-argument error branch reaches this function. CASE
    // must measure a direct column before SQLite can load its text payload.
    conn.create_scalar_function(
        "_hession_export_metadata_limit_v1",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8
            | rusqlite::functions::FunctionFlags::SQLITE_INNOCUOUS,
        |_| {
            Err::<i64, _>(rusqlite::Error::UserFunctionError(Box::new(
                io::Error::other(METADATA_LIMIT_MARKER),
            )))
        },
    )?;
    conn.query_row("SELECT CASE WHEN coalesce(octet_length(identity),0)<=4096 AND typeof(revision)='integer' THEN 1 ELSE _hession_export_metadata_limit_v1() END FROM library_revision WHERE id=1",[],|r|r.get::<_,i64>(0))?;
    let deadline = Instant::now() + Duration::from_millis(limits.timeout_ms);
    let interrupt = cancel.clone();
    conn.progress_handler(
        10_000,
        Some(move || interrupt.is_cancelled() || Instant::now() >= deadline),
    )?;
    Ok((conn, deadline))
}
fn normalize_error<T>(
    result: Result<T, ExportError>,
    cancel: &ExportCancellation,
) -> Result<T, ExportError> {
    match result {
        Err(_) if cancel.is_cancelled() => Err(ExportError::Cancelled),
        Err(ExportError::Sql(rusqlite::Error::SqliteFailure(_, Some(message))))
            if message.contains(METADATA_LIMIT_MARKER) =>
        {
            Err(ExportError::Limit("metadata bytes"))
        }
        Err(ExportError::Sql(rusqlite::Error::SqliteFailure(code, _)))
            if code.code == rusqlite::ErrorCode::OperationInterrupted =>
        {
            Err(ExportError::Limit("execution time"))
        }
        result => result,
    }
}
struct HashOutput<'a> {
    hash: Sha256,
    bytes: u64,
    work: &'a Work<'a>,
}
impl Write for HashOutput<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.work.check().map_err(io::Error::other)?;
        if self.bytes + data.len() as u64 > self.work.limits.max_member_bytes {
            return Err(io::Error::other("Transcript exceeds member byte bound"));
        }
        self.hash.update(data);
        self.bytes += data.len() as u64;
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Preview scans sanitized transcript content under a single read snapshot. No output file is made.
pub fn prepare(
    library: &Library,
    request: &ProjectExportRequest,
    scope: &SearchScope,
    target: &Path,
    cancel: &ExportCancellation,
) -> Result<PreparedProjectExport, ExportError> {
    normalize_error(
        (|| {
            let request = request.clone();
            request.limits.validate()?;
            let target = destination(library.path(), target)?;
            let database = library.path().canonicalize()?;
            let (mut conn, deadline) = open(library, cancel, request.limits)?;
            let tx = conn.transaction()?;
            let project = projects::project_on_connection(&tx, request.workspace_id, scope)?;
            let revision = library_revision::current(&tx)?;
            let scoped = SearchScope::workspaces([project.workspace_id])?;
            let work = Work {
                conn: &tx,
                cancel,
                limits: request.limits,
                deadline,
            };
            let mut search = SearchRequest {
                query: request.query.clone(),
                mode: SearchMode::Sessions,
                limit: 100,
                cursor: None,
                filters: request.filters.clone(),
                visibility: SearchVisibility::MetadataOnly,
            };
            let mut ids = Vec::new();
            loop {
                work.check()?;
                let page = search::search_snapshot(&tx, &search, &scoped)?;
                ids.extend(page.results.iter().map(|r| r.session_id));
                if ids.len() > request.limits.max_sessions as usize {
                    return Err(ExportError::Limit("selected session count"));
                }
                search.cursor = page.next_cursor;
                if search.cursor.is_none() {
                    break;
                }
            }
            let mut privacy = Sanitizer::new(&request.options);
            let mut sessions = Vec::new();
            let mut scanned = 0;
            let mut exported = 0;
            let mut bytes = 0;
            let mut identities = BTreeSet::new();
            let mut duplicate = 0;
            let mut fallback_identities = 0;
            for id in ids {
                work.check()?;
                let (portable, identity_basis) = transcript::logical_id(&work, id)?;
                if identity_basis == "source_digest" {
                    fallback_identities += 1;
                }
                let statistics = if request.options.include_statistics {
                    transcript::session_stats(&tx, id)?
                } else {
                    None
                };
                let mut hash = HashOutput {
                    hash: Sha256::new(),
                    bytes: 0,
                    work: &work,
                };
                let counts = {
                    let mut buffered = std::io::BufWriter::with_capacity(64 * 1024, &mut hash);
                    let counts = transcript::write_transcript(
                        &work,
                        id,
                        &portable,
                        transcript::TranscriptStyle {
                            format: "json",
                            exported_at: "",
                        },
                        &mut buffered,
                        &mut privacy,
                        statistics.as_ref(),
                    )?;
                    buffered.flush()?;
                    counts
                };
                scanned += counts.scanned;
                bytes += hash.bytes;
                if scanned > request.limits.max_entries {
                    return Err(ExportError::Limit("selected entry count"));
                }
                if bytes > request.limits.max_archive_bytes {
                    return Err(ExportError::Limit("sanitized transcript bytes"));
                }
                let snapshot_id = format!("{portable}-{:x}", hash.hash.finalize());
                if !identities.insert(snapshot_id.clone()) {
                    duplicate += 1;
                } else {
                    exported += counts.exported;
                }
                sessions.push(SessionPlan {
                    local_id: id,
                    portable_id: portable,
                    snapshot_id,
                    canonical_bytes: hash.bytes,
                    identity_basis,
                    statistics,
                    exported_entries: counts.exported,
                });
            }
            let mut activity_rows = 0;
            for session in &sessions {
                for table in if request.options.include_git_metadata {
                    &["file_activity", "session_context"][..]
                } else {
                    &["file_activity"][..]
                } {
                    let count = tx.query_row(
                        &format!("SELECT count(*) FROM {table} WHERE session_id=?"),
                        [session.local_id],
                        |r| r.get::<_, i64>(0),
                    );
                    activity_rows += count.map_err(ExportError::from)? as u64;
                    if activity_rows > request.limits.max_activity_rows {
                        return Err(ExportError::Limit("activity row count"));
                    }
                }
            }
            let mut warnings=vec!["This is a portable history archive, not a restorable backup or a repository copy.".into(),"Pattern redaction and freeform path masking cannot find every secret or encoded path. Review the archive before sharing.".into(),"Private reasoning, system/developer messages, raw event payloads, attachments and source URIs are always omitted.".into()];
            if fallback_identities > 0 {
                warnings.push(format!("{fallback_identities} histories lack native or replication IDs and use a source-identity digest; changed source paths can prevent cross-device matching."));
            }
            if request.options.include_tool_bodies {
                warnings.push("Tool inputs and outputs are included by explicit option; they can contain private code and credentials.".into());
            }
            if request.options.include_absolute_paths {
                warnings.push("Absolute recorded machine paths and host metadata are included by explicit option.".into());
            }
            if !request.options.redact {
                warnings.push("Secret pattern redaction is disabled.".into());
            }
            if request.options.include_statistics {
                warnings.push("Statistics use existing cached analytics; missing or stale analytics are not refreshed during export.".into());
            }
            if duplicate > 0 {
                warnings.push(format!("{duplicate} identical sanitized session snapshots share one transcript member; source evidence is retained."));
            }
            let display = privacy.text(
                project
                    .display_name
                    .as_deref()
                    .unwrap_or("Recorded project"),
            );
            let exported_at =
                tx.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
                    r.get::<_, String>(0)
                })?;
            let snapshots = serde_json::to_vec(
                &sessions
                    .iter()
                    .map(|session| &session.snapshot_id)
                    .collect::<Vec<_>>(),
            )?;
            let token = transcript::digest(&[
                b"hession-export-plan-v1",
                serde_json::to_vec(&request)?.as_slice(),
                serde_json::to_vec(&revision)?.as_slice(),
                target.as_os_str().as_encoded_bytes(),
                &snapshots,
            ]);
            let estimated = bytes
                .saturating_mul(if request.options.format == TranscriptFormat::Both {
                    3
                } else {
                    2
                })
                .saturating_add(activity_rows.saturating_mul(512))
                .saturating_add(16 * 1024);
            let preview = ExportPreview {
                token,
                revision,
                prepared_at: exported_at.clone(),
                project_key: project.project_key.clone(),
                project_display_name: display,
                selected_session_count: sessions.len(),
                session_count: identities.len(),
                exported_entry_count: exported,
                estimated_uncompressed_bytes: estimated,
                intended_output: target.clone(),
                options: request.options.clone(),
                redactions: privacy.summary,
                warnings,
            };
            work.check()?;
            Ok(PreparedProjectExport {
                database,
                destination: target,
                request,
                project,
                preview,
                sessions,
                exported_at,
            })
        })(),
        cancel,
    )
}
/// Write only the reviewed snapshot; any intervening library mutation requires a new preview.
pub fn write(
    library: &Library,
    plan: &PreparedProjectExport,
    scope: &SearchScope,
    cancel: &ExportCancellation,
    progress: impl Fn(ExportProgress),
) -> Result<ExportResult, ExportError> {
    cancel.begin()?;
    normalize_error(
        archive::write_archive(library, plan, scope, cancel, progress),
        cancel,
    )
}
