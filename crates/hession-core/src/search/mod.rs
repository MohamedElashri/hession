//! Read-only, scoped developer search shared by desktop, CLI and MCP.
//! Each call reads one SQLite snapshot. Cursors reject any changed library,
//! query, scope, filters or page size instead of quietly skipping/duplicating rows.
mod bounds;
mod compiler;
#[cfg(test)]
mod performance;
pub mod query;
use crate::{
    db,
    library_revision::{self, LibraryRevision},
    service::Library,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_RESULTS: u32 = 100;
pub const MAX_SCOPE_WORKSPACES: usize = 512;
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_OFFSET: u64 = 1_000_000;
pub const EXCERPT_CHARS: usize = 480;
/// Matches the existing continuation transfer policy. Kind matters as well as
/// role: a reasoning event can have role=assistant and an explicit text part.
pub const VISIBLE_CONVERSATION_KINDS: &[&str] = &[
    "message",
    "text",
    "user",
    "assistant",
    "user_message",
    "agent_message",
    "task",
    "user_feedback",
    "user_feedback_diff",
    "completion_result",
    "followup",
    "plan_mode_respond",
    "USER_INPUT",
];

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("{0}")]
    Syntax(#[from] query::QueryError),
    #[error("{0}")]
    Database(#[from] db::DbError),
    #[error("Search failed: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("Invalid search request: {0}")]
    Invalid(String),
    #[error("Search cursor expired because the library changed; restart from the first page")]
    StaleCursor,
    #[error("Search cursor does not match this request or scope")]
    InvalidCursor,
    #[error("Search input exceeds the bounded cell or scope byte budget; narrow the project scope or repair oversized indexed fields")]
    InputTooLarge,
    #[error("Search response exceeds 1 MiB; reduce the page size")]
    ResponseTooLarge,
    #[error(
        "Search exceeded its three-second execution budget; narrow the query or project scope"
    )]
    TimedOut,
}

/// Created by trusted adapters, never deserialized from untrusted requests.
/// An empty allowlist denies everything; only `all()` is unrestricted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SearchScope {
    workspaces: Option<Vec<i64>>,
}
impl SearchScope {
    pub fn all() -> Self {
        Self { workspaces: None }
    }
    pub fn workspaces(ids: impl IntoIterator<Item = i64>) -> Result<Self, SearchError> {
        let mut ids = ids
            .into_iter()
            .take(MAX_SCOPE_WORKSPACES + 1)
            .collect::<Vec<_>>();
        if ids.len() > MAX_SCOPE_WORKSPACES || ids.iter().any(|id| *id <= 0) {
            return Err(SearchError::Invalid(
                "Scope requires at most 512 positive workspace IDs".into(),
            ));
        }
        ids.sort_unstable();
        ids.dedup();
        Ok(Self {
            workspaces: Some(ids),
        })
    }
    pub fn workspace_ids(&self) -> Option<&[i64]> {
        self.workspaces.as_deref()
    }
    pub fn allows_workspace(&self, id: i64) -> bool {
        self.workspaces
            .as_ref()
            .is_none_or(|ids| ids.binary_search(&id).is_ok())
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Sessions,
    Messages,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchVisibility {
    /// Suitable for metadata-only MCP. Transcript text is neither matched nor returned.
    #[default]
    MetadataOnly,
    /// Only explicit text content parts on user/assistant entries are excerpted.
    Conversation,
    /// Desktop explicitly opts into excerpts of all indexed search text.
    AllIndexedText,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SearchFilters {
    /// Desktop's session list hides helpers by default; programmatic readers
    /// retain all history unless they explicitly request this filter.
    pub exclude_subagents: bool,
    pub agent: String,
    pub project: String,
    pub models: Vec<String>,
    pub tools: Vec<String>,
    pub branch: String,
    pub host: String,
    /// An absolute ISO timestamp or YYYY-MM-DD, compared with julianday.
    pub since: String,
    pub before: String,
    pub starred: bool,
    pub tags: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    pub mode: SearchMode,
    pub limit: u32,
    pub cursor: Option<String>,
    pub filters: SearchFilters,
    pub visibility: SearchVisibility,
}
impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: SearchMode::Sessions,
            limit: 20,
            cursor: None,
            filters: SearchFilters::default(),
            visibility: SearchVisibility::MetadataOnly,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HighlightSpan {
    /// Unicode scalar offsets into excerpt, never HTML or UTF-8 byte positions.
    pub start: usize,
    pub end: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub session_id: i64,
    pub workspace_id: Option<i64>,
    pub title: Option<String>,
    pub agent: String,
    pub project: Option<String>,
    pub host: Option<String>,
    pub activity_at: Option<String>,
    pub entry_id: Option<i64>,
    pub ordinal: Option<i64>,
    /// Zero-based row position; unlike ordinal this remains valid with gaps.
    pub position: Option<i64>,
    pub starred: bool,
    pub tags: Vec<String>,
    pub score: f64,
    pub excerpt: Option<String>,
    pub highlights: Vec<HighlightSpan>,
    pub matching_locations: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub revision: LibraryRevision,
    pub results: Vec<SearchResult>,
    pub next_cursor: Option<String>,
    /// Bounded lower bound: offset + page rows + one if another row exists.
    pub count_lower_bound: u64,
    pub visibility_notice: Option<String>,
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    revision: LibraryRevision,
    fingerprint: String,
    offset: u64,
}

impl Library {
    pub fn search(
        &self,
        request: &SearchRequest,
        scope: &SearchScope,
    ) -> Result<SearchPage, SearchError> {
        let mut conn = db::open_read_only(self.path())?;
        search(&mut conn, request, scope)
    }
}
/// Use an existing connection; the service owns a short read transaction.
pub fn search(
    conn: &mut Connection,
    request: &SearchRequest,
    scope: &SearchScope,
) -> Result<SearchPage, SearchError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    conn.progress_handler(10_000, Some(move || std::time::Instant::now() > deadline))?;
    let result = (|| {
        let tx = conn.transaction()?;
        let page = search_snapshot(&tx, request, scope)?;
        tx.commit()?;
        Ok(page)
    })();
    conn.progress_handler(0, None::<fn() -> bool>)?;
    match result {
        Err(SearchError::Sql(rusqlite::Error::SqliteFailure(e, _)))
            if e.code == rusqlite::ErrorCode::OperationInterrupted =>
        {
            Err(SearchError::TimedOut)
        }
        Err(SearchError::Sql(rusqlite::Error::SqliteFailure(_, Some(message))))
            if message.contains("HESSION_SEARCH_TIME_BUDGET") =>
        {
            Err(SearchError::TimedOut)
        }
        result => result,
    }
}
/// Reuse an existing snapshot for exports. Autocommit is rejected because it
/// cannot guarantee that the revision and results belong to the same snapshot.
pub fn search_snapshot(
    conn: &Connection,
    request: &SearchRequest,
    scope: &SearchScope,
) -> Result<SearchPage, SearchError> {
    if conn.is_autocommit() {
        return Err(SearchError::Invalid(
            "search_snapshot requires an active read transaction".into(),
        ));
    }
    validate_request(conn, request)?;
    compiler::register_visible_match(conn)?;
    let parsed = query::parse(&request.query)?;
    let revision = library_revision::current(conn)?;
    let fingerprint = fingerprint(request, scope)?;
    let offset = if let Some(cursor) = &request.cursor {
        if cursor.len() > 4096 {
            return Err(SearchError::InvalidCursor);
        }
        let c: Cursor = serde_json::from_str(cursor).map_err(|_| SearchError::InvalidCursor)?;
        if c.fingerprint != fingerprint || c.offset > MAX_OFFSET {
            return Err(SearchError::InvalidCursor);
        }
        if c.revision != revision {
            return Err(SearchError::StaleCursor);
        }
        c.offset
    } else {
        0
    };
    // Reject expired/foreign cursors before visiting any transcript or metadata
    // corpus. Valid requests still pass every bound in the same read snapshot.
    bounds::check(conn, request, scope, &parsed)?;
    let mut results = compiler::execute(conn, request, scope, &parsed, offset)
        .map_err(bounds::map_execution_error)?;
    let more = results.len() > request.limit as usize;
    if more {
        results.pop();
    }
    let next_offset = offset + results.len() as u64;
    let next_cursor = if more && next_offset <= MAX_OFFSET {
        Some(
            serde_json::to_string(&Cursor {
                revision: revision.clone(),
                fingerprint,
                offset: next_offset,
            })
            .map_err(|_| SearchError::InvalidCursor)?,
        )
    } else {
        None
    };
    let visibility_notice=match request.visibility {
        SearchVisibility::MetadataOnly=>Some("Only indexed metadata was searched; transcript content is excluded.".into()),
        SearchVisibility::Conversation=>Some("Only explicit user/assistant text parts were searched; tool, thinking, system and legacy combined-text records are excluded.".into()),
        SearchVisibility::AllIndexedText=>None,
    };
    let result = SearchPage {
        revision,
        results,
        next_cursor,
        count_lower_bound: next_offset + u64::from(more),
        visibility_notice,
    };
    if serde_json::to_vec(&result)
        .map_err(|_| SearchError::ResponseTooLarge)?
        .len()
        > MAX_RESPONSE_BYTES
    {
        return Err(SearchError::ResponseTooLarge);
    }
    Ok(result)
}
fn fingerprint(request: &SearchRequest, scope: &SearchScope) -> Result<String, SearchError> {
    let mut request = request.clone();
    request.cursor = None;
    let bytes = serde_json::to_vec(&(request, scope)).map_err(|_| SearchError::InvalidCursor)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
fn validate_request(conn: &Connection, r: &SearchRequest) -> Result<(), SearchError> {
    if r.limit == 0 || r.limit > MAX_RESULTS {
        return Err(SearchError::Invalid("Limit must be 1–100".into()));
    }
    if r.mode == SearchMode::Messages && r.visibility == SearchVisibility::MetadataOnly {
        return Err(SearchError::Invalid(
            "Message results require explicit conversation or indexed-text visibility".into(),
        ));
    }
    let f = &r.filters;
    if [
        &f.agent, &f.project, &f.branch, &f.host, &f.since, &f.before,
    ]
    .iter()
    .any(|v| v.len() > 4096)
        || [&f.models, &f.tools, &f.tags]
            .iter()
            .any(|v| v.len() > 50 || v.iter().any(|s| s.len() > 512))
    {
        return Err(SearchError::Invalid(
            "Filter values exceed size limits".into(),
        ));
    }
    for date in [&f.since, &f.before] {
        if !date.is_empty()
            && (date.len() < 10
                || !date.is_char_boundary(10)
                || !query::valid_date(&date[..10])
                || (date.len() > 10 && !matches!(date.as_bytes()[10], b'T' | b' '))
                || !conn.query_row("SELECT julianday(?) IS NOT NULL", [date], |r| {
                    r.get::<_, bool>(0)
                })?)
        {
            return Err(SearchError::Invalid(
                "Date filter must be an absolute ISO date or timestamp".into(),
            ));
        }
    }
    Ok(())
}
