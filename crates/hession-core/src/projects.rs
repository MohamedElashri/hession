//! Recorded project and file evidence. These reads never probe a checkout or a remote host.
//! Workspace IDs remain the v0.4 IDs: neither remote URLs nor matching basenames merge projects.
use crate::{
    db, library_revision,
    service::{Library, Page, ServiceError},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path};

pub const MAX_PATH_BYTES: usize = 4096;

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("{0}")]
    Database(#[from] rusqlite::Error),
    #[error("{0}")]
    Service(#[from] ServiceError),
    #[error("Project is unavailable or outside the allowed workspace scope")]
    OutsideScope,
    #[error("Invalid or oversized path")]
    InvalidPath,
    #[error("Local checkout is unavailable for this recorded workspace")]
    Unavailable,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// Shared immutable workspace allowlist used by search, projects, exports and MCP.
pub type ProjectScope = crate::search::SearchScope;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceCategory {
    ReportedReference,
    ReportedRead,
    ReportedWriteIntent,
    ObservedWorktreeChange,
    VerifiedCommitChange,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub workspace_id: i64,
    /// Portable reference derived from recorded host/path; never a checkout-discovery result.
    pub project_key: String,
    pub recorded_path: String,
    pub display_name: Option<String>,
    /// A host is taken from session.host, never guessed from a colon in a path.
    pub host: Option<String>,
    pub ambiguous_host: bool,
    pub session_count: u64,
    pub local_session_count: u64,
    pub remote_session_count: u64,
    pub synced_session_count: u64,
    pub last_activity_at: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ProjectSelector {
    WorkspaceId(i64),
    RecordedPath(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileSummary {
    pub reported_path: String,
    /// Counts deduplicate repeated rows with the same session, entry, tool and evidence category.
    pub reported_read_count: u64,
    pub reported_write_intent_count: u64,
    pub reported_reference_count: u64,
    pub session_count: u64,
    pub last_activity_at: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileSummaryPage {
    pub project: Project,
    pub revision: library_revision::LibraryRevision,
    pub files: Vec<FileSummary>,
    pub next_offset: Option<u64>,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceReference {
    pub session_id: i64,
    pub entry_id: Option<i64>,
    pub entry_ordinal: Option<i64>,
    pub tool_invocation_id: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEvidence {
    pub id: i64,
    pub source: SourceReference,
    pub reported_path: String,
    pub candidate_workspace_relative_path: Option<String>,
    pub resolution: String,
    pub category: EvidenceCategory,
    pub action: Option<String>,
    pub evidence: Option<String>,
    pub occurred_at: Option<String>,
    pub agent_family: String,
    pub tool_name: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecordedGitContext {
    pub context_id: i64,
    pub source: SourceReference,
    pub observed_at: Option<String>,
    pub cwd: Option<String>,
    pub branch: Option<String>,
    pub commit_sha: Option<String>,
    pub repository_id: Option<i64>,
    pub common_dir: Option<String>,
    pub remote_url: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivityPage {
    pub project: Project,
    pub revision: library_revision::LibraryRevision,
    pub evidence: Vec<FileEvidence>,
    pub next_offset: Option<u64>,
    pub has_more: bool,
}

/// Schema-v1 portable identity over the exact v0.4 workspace key (already host-qualified
/// for remote histories). It stays stable when checkout availability or session evidence changes.
pub fn project_key(recorded_path: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"hession-project-v1\0");
    digest.update(recorded_path.as_bytes());
    format!("project-v1:{:x}", digest.finalize())
}

fn response_budget<T: Serialize>(value: T) -> Result<T, ProjectError> {
    if serde_json::to_vec(&value).map_err(query_err)?.len() > crate::service::MAX_RESPONSE_BYTES {
        return Err(ServiceError::ResponseTooLarge.into());
    }
    Ok(value)
}

fn read_connection(library: &Library) -> Result<Connection, ProjectError> {
    let mut conn = db::open_read_only(library.path()).map_err(ServiceError::from)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    conn.progress_handler(10_000, Some(move || std::time::Instant::now() > deadline))?;
    Ok(conn)
}

fn bounded(value: String) -> Result<String, ProjectError> {
    if value.len() > MAX_PATH_BYTES {
        Err(ProjectError::InvalidPath)
    } else {
        Ok(value)
    }
}
fn query_err(error: impl ToString) -> ProjectError {
    ProjectError::Service(ServiceError::Query(error.to_string()))
}

pub fn project_on_connection(
    conn: &Connection,
    workspace_id: i64,
    scope: &ProjectScope,
) -> Result<Project, ProjectError> {
    if !scope.allows_workspace(workspace_id) {
        return Err(ProjectError::OutsideScope);
    }
    let mut project=conn.query_row(
        "SELECT w.id,substr(w.path,1,4097),substr(w.display_name,1,4097),
        substr(min(s.host),1,4097),count(DISTINCT coalesce(s.host,'')),count(s.id),
        substr(max(coalesce(s.last_event_at,s.started_at)),1,128),
        sum(CASE WHEN s.id IS NOT NULL AND s.host IS NULL AND s.source_uri NOT LIKE 'hession-sync://%' THEN 1 ELSE 0 END),
        sum(CASE WHEN s.host IS NOT NULL AND s.source_uri NOT LIKE 'hession-sync://%' THEN 1 ELSE 0 END),
        sum(CASE WHEN s.source_uri LIKE 'hession-sync://%' THEN 1 ELSE 0 END)
        FROM workspace w LEFT JOIN session s ON s.workspace_id=w.id WHERE w.id=? GROUP BY w.id",
        [workspace_id],
        |row|Ok(Project {
            workspace_id:row.get(0)?,project_key:String::new(),recorded_path:row.get(1)?,
            display_name:row.get(2)?,host:row.get(3)?,ambiguous_host:row.get::<_,i64>(4)?>1,
            session_count:row.get::<_,i64>(5)? as u64,last_activity_at:row.get(6)?,
            local_session_count:row.get::<_,i64>(7)? as u64,
            remote_session_count:row.get::<_,i64>(8)? as u64,
            synced_session_count:row.get::<_,i64>(9)? as u64,
        })
    ).optional()?.ok_or(ProjectError::OutsideScope)?;
    bounded(project.recorded_path.clone())?;
    if let Some(host) = &project.host {
        bounded(host.clone())?;
    }
    project.project_key = project_key(&project.recorded_path);
    Ok(project)
}

/// A lexical candidate only; never accesses disk. Parent traversal, nonabsolute cwd and remote
/// contexts remain unresolved. A symlink can still make this candidate unsafe to open.
pub fn relative_candidate(
    reported: &str,
    cwd: Option<&str>,
    workspace: &str,
    remote: bool,
) -> Option<String> {
    if remote || reported.is_empty() || reported.len() > MAX_PATH_BYTES || reported.contains('\0') {
        return None;
    }
    let workspace = Path::new(workspace);
    let path = Path::new(reported);
    if !workspace.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return None;
    }
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        let cwd = Path::new(cwd?);
        if !cwd.is_absolute() || cwd.components().any(|c| matches!(c, Component::ParentDir)) {
            return None;
        }
        cwd.join(path)
    };
    let relative = joined.strip_prefix(workspace).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    Some(
        relative
            .components()
            .filter_map(|c| match c {
                Component::Normal(p) => Some(p.to_string_lossy()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/"),
    )
}

pub fn activity_on_connection(
    conn: &Connection,
    id: i64,
    scope: &ProjectScope,
    page: Page,
    path: Option<&str>,
) -> Result<ActivityPage, ProjectError> {
    let page = page.validate()?;
    if path.is_some_and(|s| s.len() > MAX_PATH_BYTES || s.contains('\0')) {
        return Err(ProjectError::InvalidPath);
    }
    let project = project_on_connection(conn, id, scope)?;
    let mut statement=conn.prepare("SELECT f.id,f.session_id,e.id,e.ordinal,t.id,
        substr(f.path,1,4097),substr(coalesce(c.cwd,s.cwd),1,4097),substr(f.action,1,128),substr(f.evidence,1,512),
        substr(e.occurred_at,1,128),substr(a.family,1,128),substr(t.tool_name,1,256),s.host,s.source_uri LIKE 'hession-sync://%'
        FROM file_activity f JOIN session s ON s.id=f.session_id JOIN agent a ON a.id=s.agent_id
        LEFT JOIN entry e ON e.id=f.entry_id AND e.session_id=f.session_id
        LEFT JOIN tool_invocation t ON t.id=f.tool_invocation_id AND t.session_id=f.session_id
        LEFT JOIN session_context c ON c.id=(SELECT max(c2.id) FROM session_context c2 WHERE c2.session_id=f.session_id
            AND (c2.entry_id IS NULL OR c2.entry_id IN (SELECT id FROM entry WHERE session_id=f.session_id AND ordinal<=e.ordinal)))
        WHERE s.workspace_id=?1 AND (?2 IS NULL OR f.path=?2)
        ORDER BY coalesce(e.occurred_at,s.last_event_at,s.started_at) DESC,f.id DESC LIMIT ?3 OFFSET ?4")?;
    let rows = statement.query_map(
        params![id, path, i64::from(page.limit) + 1, page.offset as i64],
        |r| {
            let action: Option<String> = r.get(7)?;
            let category = match action.as_deref() {
                Some("read") => EvidenceCategory::ReportedRead,
                Some("write" | "edit" | "patch" | "delete" | "create" | "rename") => {
                    EvidenceCategory::ReportedWriteIntent
                }
                _ => EvidenceCategory::ReportedReference,
            };
            Ok((
                FileEvidence {
                    id: r.get(0)?,
                    source: SourceReference {
                        session_id: r.get(1)?,
                        entry_id: r.get(2)?,
                        entry_ordinal: r.get(3)?,
                        tool_invocation_id: r.get(4)?,
                    },
                    reported_path: r.get(5)?,
                    candidate_workspace_relative_path: None,
                    resolution: "unresolved_recorded_path".into(),
                    category,
                    action,
                    evidence: r.get(8)?,
                    occurred_at: r.get(9)?,
                    agent_family: r.get(10)?,
                    tool_name: r.get(11)?,
                },
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(12)?,
                r.get::<_, bool>(13)?,
            ))
        },
    )?;
    let mut evidence = Vec::new();
    for row in rows {
        let (mut item, cwd, host, synced) = row?;
        bounded(item.reported_path.clone())?;
        item.candidate_workspace_relative_path = relative_candidate(
            &item.reported_path,
            cwd.as_deref(),
            &project.recorded_path,
            host.is_some() || project.ambiguous_host || synced,
        );
        if item.candidate_workspace_relative_path.is_some() {
            item.resolution = "lexical_candidate_unverified".into();
        }
        evidence.push(item);
    }
    let has_more = evidence.len() > page.limit as usize;
    let next_offset = if has_more {
        evidence.pop();
        (page.offset + u64::from(page.limit) <= crate::service::MAX_OFFSET)
            .then_some(page.offset + u64::from(page.limit))
    } else {
        None
    };
    response_budget(ActivityPage {
        project,
        revision: library_revision::current(conn).map_err(query_err)?,
        evidence,
        next_offset,
        has_more,
    })
}

pub fn recorded_git_on_connection(
    conn: &Connection,
    id: i64,
    scope: &ProjectScope,
    page: Page,
) -> Result<Vec<RecordedGitContext>, ProjectError> {
    let page = page.validate()?;
    project_on_connection(conn, id, scope)?;
    let mut stmt=conn.prepare("SELECT c.id,c.session_id,e.id,e.ordinal,substr(c.observed_at,1,128),substr(c.cwd,1,4097),substr(c.branch,1,4097),substr(c.commit_sha,1,128),c.repository_id,substr(r.common_dir,1,4097),substr(r.remote_url,1,4097)
        FROM session_context c JOIN session s ON s.id=c.session_id LEFT JOIN entry e ON e.id=c.entry_id AND e.session_id=c.session_id
        LEFT JOIN repository r ON r.id=c.repository_id WHERE s.workspace_id=?1 AND (c.branch IS NOT NULL OR c.commit_sha IS NOT NULL OR c.repository_id IS NOT NULL)
        ORDER BY c.id DESC LIMIT ?2 OFFSET ?3")?;
    let rows = stmt.query_map(params![id, page.limit, page.offset as i64], |r| {
        Ok(RecordedGitContext {
            context_id: r.get(0)?,
            source: SourceReference {
                session_id: r.get(1)?,
                entry_id: r.get(2)?,
                entry_ordinal: r.get(3)?,
                tool_invocation_id: None,
            },
            observed_at: r.get(4)?,
            cwd: r.get(5)?,
            branch: r.get(6)?,
            commit_sha: r.get(7)?,
            repository_id: r.get(8)?,
            common_dir: r.get(9)?,
            remote_url: r.get(10)?,
        })
    })?;
    response_budget(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn files_on_connection(
    conn: &Connection,
    id: i64,
    scope: &ProjectScope,
    page: Page,
    filename_filter: Option<&str>,
) -> Result<FileSummaryPage, ProjectError> {
    let page = page.validate()?;
    let project = project_on_connection(conn, id, scope)?;
    if filename_filter.is_some_and(|s| s.len() > MAX_PATH_BYTES || s.contains('\0')) {
        return Err(ProjectError::InvalidPath);
    }
    let mut stmt=conn.prepare("WITH references_deduplicated AS (
        SELECT DISTINCT f.session_id,f.entry_id,f.tool_invocation_id,f.path,
        CASE WHEN f.action='read' THEN 'read' WHEN f.action IN ('write','edit','patch','delete','create','rename') THEN 'write' ELSE 'reference' END category,
        e.occurred_at occurred_at
        FROM file_activity f JOIN session s ON s.id=f.session_id
        LEFT JOIN entry e ON e.id=f.entry_id AND e.session_id=f.session_id
        WHERE s.workspace_id=?1 AND (?2 IS NULL OR instr(lower(f.path),lower(?2))>0))
        SELECT substr(path,1,4097),sum(category='read'),sum(category='write'),sum(category='reference'),count(DISTINCT session_id),substr(max(occurred_at),1,128)
        FROM references_deduplicated GROUP BY path ORDER BY max(occurred_at) DESC,path COLLATE BINARY LIMIT ?3 OFFSET ?4")?;
    let rows = stmt.query_map(
        params![
            id,
            filename_filter,
            i64::from(page.limit) + 1,
            page.offset as i64
        ],
        |r| {
            Ok(FileSummary {
                reported_path: r.get(0)?,
                reported_read_count: r.get::<_, i64>(1)? as u64,
                reported_write_intent_count: r.get::<_, i64>(2)? as u64,
                reported_reference_count: r.get::<_, i64>(3)? as u64,
                session_count: r.get::<_, i64>(4)? as u64,
                last_activity_at: r.get(5)?,
            })
        },
    )?;
    let mut files = rows.collect::<Result<Vec<_>, _>>()?;
    for file in &files {
        bounded(file.reported_path.clone())?;
    }
    let has_more = files.len() > page.limit as usize;
    let next_offset = if has_more {
        files.pop();
        (page.offset + u64::from(page.limit) <= crate::service::MAX_OFFSET)
            .then_some(page.offset + u64::from(page.limit))
    } else {
        None
    };
    response_budget(FileSummaryPage {
        project,
        revision: library_revision::current(conn)?,
        files,
        next_offset,
        has_more,
    })
}

impl Library {
    /// Exact stable selectors only. A missing checkout never changes selector meaning.
    pub fn resolve_project(
        &self,
        selector: &ProjectSelector,
        scope: &ProjectScope,
    ) -> Result<Project, ProjectError> {
        let mut conn = read_connection(self)?;
        let tx = conn.transaction()?;
        let id = match selector {
            ProjectSelector::WorkspaceId(id) => *id,
            ProjectSelector::RecordedPath(path) => {
                bounded(path.clone())?;
                if path.contains('\0') {
                    return Err(ProjectError::InvalidPath);
                }
                tx.query_row("SELECT id FROM workspace WHERE path=?", [path], |r| {
                    r.get(0)
                })
                .optional()?
                .ok_or(ProjectError::OutsideScope)?
            }
        };
        project_on_connection(&tx, id, scope)
    }
    pub fn get_project_files(
        &self,
        id: i64,
        scope: &ProjectScope,
        page: Page,
        filename_filter: Option<&str>,
    ) -> Result<FileSummaryPage, ProjectError> {
        let mut conn = read_connection(self)?;
        let tx = conn.transaction()?;
        files_on_connection(&tx, id, scope, page, filename_filter)
    }
    /// Only opens files beneath the recorded local workspace; remote histories never contact SSH.
    /// Existing source evidence is not modified, and the returned descriptor must not be reopened by path.
    pub fn open_recorded_project_file(
        &self,
        id: i64,
        relative: &Path,
        scope: &ProjectScope,
    ) -> Result<std::fs::File, ProjectError> {
        let conn = read_connection(self)?;
        let project = project_on_connection(&conn, id, scope)?;
        if project.host.is_some() || project.ambiguous_host || project.local_session_count == 0 {
            return Err(ProjectError::Unavailable);
        }
        open_project_file(Path::new(&project.recorded_path), relative)
    }
    pub fn get_projects(
        &self,
        scope: &ProjectScope,
        page: Page,
    ) -> Result<Vec<Project>, ProjectError> {
        let page = page.validate()?;
        let mut conn = read_connection(self)?;
        let conn = conn.transaction()?;
        // Scope is applied in SQL before pagination, including deny-all empty scopes.
        let ids = serde_json::to_string(&scope.workspace_ids()).map_err(query_err)?;
        let mut stmt=conn.prepare("SELECT id FROM workspace WHERE (?1='null' OR id IN (SELECT value FROM json_each(?1))) ORDER BY id LIMIT ?2 OFFSET ?3")?;
        let rows = stmt.query_map(params![ids, page.limit, page.offset as i64], |r| {
            r.get::<_, i64>(0)
        })?;
        response_budget(
            rows.map(|id| project_on_connection(&conn, id?, scope))
                .collect::<Result<Vec<_>, _>>()?,
        )
    }
    pub fn get_project_activity(
        &self,
        id: i64,
        scope: &ProjectScope,
        page: Page,
    ) -> Result<ActivityPage, ProjectError> {
        let mut conn = read_connection(self)?;
        let tx = conn.transaction()?;
        activity_on_connection(&tx, id, scope, page, None)
    }
    pub fn get_file_history(
        &self,
        id: i64,
        path: &str,
        scope: &ProjectScope,
        page: Page,
    ) -> Result<ActivityPage, ProjectError> {
        let mut conn = read_connection(self)?;
        let tx = conn.transaction()?;
        activity_on_connection(&tx, id, scope, page, Some(path))
    }
    pub fn get_git_context(
        &self,
        id: i64,
        scope: &ProjectScope,
        page: Page,
    ) -> Result<Vec<RecordedGitContext>, ProjectError> {
        let mut conn = read_connection(self)?;
        let tx = conn.transaction()?;
        recorded_git_on_connection(&tx, id, scope, page)
    }
    /// Explicit local inspection only. Remote/ambiguous projects are refused before filesystem access.
    pub fn inspect_project_git(
        &self,
        id: i64,
        scope: &ProjectScope,
        limits: crate::git::GitLimits,
    ) -> Result<crate::git::GitInspection, crate::git::GitError> {
        let conn = read_connection(self)?;
        let project = project_on_connection(&conn, id, scope)?;
        if project.host.is_some() || project.ambiguous_host || project.local_session_count == 0 {
            return Err(ProjectError::Unavailable.into());
        }
        drop(conn);
        crate::git::inspect(Path::new(&project.recorded_path), limits)
    }
}

/// Open a relative path with openat + O_NOFOLLOW at every component. This returns an already-open
/// regular file, so a caller cannot reintroduce a symlink race by reopening a returned path.
#[cfg(unix)]
pub fn open_project_file(root: &Path, relative: &Path) -> Result<std::fs::File, ProjectError> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::ffi::OsStrExt,
        },
    };
    if !root.is_absolute()
        || root.as_os_str().len() > MAX_PATH_BYTES
        || root.components().any(|c| matches!(c, Component::ParentDir))
        || relative.is_absolute()
        || relative.as_os_str().len() > MAX_PATH_BYTES
    {
        return Err(ProjectError::InvalidPath);
    }
    let components = relative
        .components()
        .map(|c| match c {
            Component::Normal(s) => {
                CString::new(s.as_bytes()).map_err(|_| ProjectError::InvalidPath)
            }
            _ => Err(ProjectError::InvalidPath),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components.is_empty() {
        return Err(ProjectError::InvalidPath);
    }
    let root = root.canonicalize()?;
    let mut directory = std::fs::File::open("/")?;
    // Resolve the canonical root again through descriptors. If an ancestor is replaced by a
    // symlink after canonicalize, O_NOFOLLOW refuses it rather than opening another tree.
    for component in root.components() {
        if let Component::Normal(component) = component {
            let component =
                CString::new(component.as_bytes()).map_err(|_| ProjectError::InvalidPath)?;
            // SAFETY: the live directory descriptor and C string are valid for this call.
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    component.as_ptr(),
                    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            // SAFETY: openat returned a newly owned descriptor.
            directory = unsafe { std::fs::File::from_raw_fd(fd) };
        }
    }
    for (index, name) in components.iter().enumerate() {
        let final_component = index + 1 == components.len();
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if final_component {
                0
            } else {
                libc::O_DIRECTORY
            };
        // SAFETY: directory and nul-terminated component live through openat; owned fd is returned.
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: fd is newly owned following successful openat.
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        if final_component {
            if !file.metadata()?.is_file() {
                return Err(ProjectError::InvalidPath);
            }
            return Ok(file);
        }
        directory = file;
    }
    Err(ProjectError::InvalidPath)
}
