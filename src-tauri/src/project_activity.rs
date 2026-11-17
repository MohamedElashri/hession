//! Desktop-only IPC adapters. Trusted desktop reads the local library; external adapters must
//! construct their own explicit SearchScope. These commands never accept a path to execute Git in.
use crate::AppState;
use hession_core::{
    db,
    git::{GitInspection, GitLimits},
    projects::{self, ActivityPage, FileSummaryPage, Project, ProjectScope, RecordedGitContext},
    service::{Library, Page},
};
use rusqlite::Connection;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use tauri::State;

static GIT_INSPECTION_ACTIVE: AtomicBool = AtomicBool::new(false);
struct GitInspectionGuard;
impl GitInspectionGuard {
    fn acquire() -> Result<Self, String> {
        GIT_INSPECTION_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "A local Git inspection is already running; retry when it finishes".into())
    }
}
impl Drop for GitInspectionGuard {
    fn drop(&mut self) {
        GIT_INSPECTION_ACTIVE.store(false, Ordering::Release);
    }
}

const DESKTOP_PAGE_MAX: u32 = 100;
#[derive(Debug, Serialize)]
pub struct ProjectCatalog {
    pub projects: Vec<Project>,
    pub has_more: bool,
    pub next_offset: Option<u64>,
}
#[derive(Debug, Serialize)]
pub struct DesktopActivity {
    #[serde(flatten)]
    pub activity: ActivityPage,
    pub entry_positions: BTreeMap<i64, i64>,
}

#[derive(Debug, Serialize)]
pub struct DesktopRecordedGit {
    pub contexts: Vec<RecordedGitContext>,
    pub entry_positions: BTreeMap<i64, i64>,
    pub has_more: bool,
    pub next_offset: Option<u64>,
}
fn bounded_response<T: Serialize>(value: T) -> Result<T, String> {
    if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len()
        > hession_core::service::MAX_RESPONSE_BYTES
    {
        return Err("Project response exceeds 1 MiB; reduce the page size".into());
    }
    Ok(value)
}
fn positions_of(
    conn: &Connection,
    sources: impl Iterator<Item = projects::SourceReference>,
) -> Result<BTreeMap<i64, i64>, String> {
    let mut positions = BTreeMap::new();
    let mut stmt=conn.prepare("SELECT count(*) FROM entry WHERE session_id=?1 AND ordinal < (SELECT ordinal FROM entry WHERE id=?2 AND session_id=?1)").map_err(|e|e.to_string())?;
    for source in sources {
        if let Some(id) = source.entry_id {
            let position = stmt
                .query_row((source.session_id, id), |r| r.get::<_, i64>(0))
                .map_err(|e| e.to_string())?;
            positions.insert(id, position);
        }
    }
    Ok(positions)
}
fn recorded_at(path: &Path, workspace_id: i64, page: Page) -> Result<DesktopRecordedGit, String> {
    let page = validate_page(page)?;
    let mut conn = open_read(path)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut contexts = projects::recorded_git_on_connection(
        &tx,
        workspace_id,
        &ProjectScope::all(),
        Page {
            limit: page.limit + 1,
            offset: page.offset,
        },
    )
    .map_err(|e| e.to_string())?;
    let has_more = contexts.len() > page.limit as usize;
    if has_more {
        contexts.pop();
    }
    let entry_positions = positions_of(&tx, contexts.iter().map(|c| c.source.clone()))?;
    let next_offset = (has_more
        && page.offset + u64::from(page.limit) <= hession_core::service::MAX_OFFSET)
        .then_some(page.offset + u64::from(page.limit));
    bounded_response(DesktopRecordedGit {
        contexts,
        entry_positions,
        has_more,
        next_offset,
    })
}
fn path_of(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .db_path
        .lock()
        .map(|p| p.clone())
        .map_err(|_| "App state lock was poisoned".into())
}
fn validate_page(page: Page) -> Result<Page, String> {
    let page = page.validate().map_err(|e| e.to_string())?;
    if page.limit > DESKTOP_PAGE_MAX {
        return Err("Use a project page size of 1–100".into());
    }
    Ok(page)
}
fn open_read(path: &Path) -> Result<Connection, String> {
    let mut conn = db::open_read_only(path).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(3);
    conn.progress_handler(10_000, Some(move || Instant::now() > deadline))
        .map_err(|e| e.to_string())?;
    Ok(conn)
}
fn catalog_at(path: &Path, page: Page) -> Result<ProjectCatalog, String> {
    let page = validate_page(page)?;
    let mut projects = Library::new(path)
        .get_projects(
            &ProjectScope::all(),
            Page {
                limit: page.limit + 1,
                offset: page.offset,
            },
        )
        .map_err(|e| e.to_string())?;
    let has_more = projects.len() > page.limit as usize;
    if has_more {
        projects.pop();
    }
    let next_offset = (has_more
        && page.offset + u64::from(page.limit) <= hession_core::service::MAX_OFFSET)
        .then_some(page.offset + u64::from(page.limit));
    bounded_response(ProjectCatalog {
        projects,
        has_more,
        next_offset,
    })
}
fn filtered_catalog_at(
    path: &Path,
    page: Page,
    filter: Option<&str>,
) -> Result<ProjectCatalog, String> {
    let Some(filter) = filter.filter(|value| !value.is_empty()) else {
        return catalog_at(path, page);
    };
    let page = validate_page(page)?;
    if filter.len() > projects::MAX_PATH_BYTES || filter.contains('\0') {
        return Err("Workspace filter exceeds bounds".into());
    }
    let mut conn = open_read(path)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut stmt=tx.prepare("SELECT id FROM workspace WHERE instr(lower(path),lower(?1))>0 OR instr(lower(coalesce(display_name,'')),lower(?1))>0 ORDER BY id LIMIT ?2 OFFSET ?3").map_err(|e|e.to_string())?;
    let ids = stmt
        .query_map(
            (filter, i64::from(page.limit) + 1, page.offset as i64),
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for id in ids {
        rows.push(
            projects::project_on_connection(
                &tx,
                id.map_err(|e| e.to_string())?,
                &ProjectScope::all(),
            )
            .map_err(|e| e.to_string())?,
        );
    }
    let has_more = rows.len() > page.limit as usize;
    if has_more {
        rows.pop();
    }
    let next_offset = (has_more
        && page.offset + u64::from(page.limit) <= hession_core::service::MAX_OFFSET)
        .then_some(page.offset + u64::from(page.limit));
    let result = ProjectCatalog {
        projects: rows,
        has_more,
        next_offset,
    };
    if serde_json::to_vec(&result)
        .map_err(|e| e.to_string())?
        .len()
        > hession_core::service::MAX_RESPONSE_BYTES
    {
        return Err("Workspace response exceeds 1 MiB; reduce the page size".into());
    }
    Ok(result)
}
fn activity_at(
    path: &Path,
    workspace_id: i64,
    page: Page,
    reported_path: Option<&str>,
) -> Result<DesktopActivity, String> {
    let page = validate_page(page)?;
    let mut conn = open_read(path)?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let activity = projects::activity_on_connection(
        &tx,
        workspace_id,
        &ProjectScope::all(),
        page,
        reported_path,
    )
    .map_err(|e| e.to_string())?;
    let positions = positions_of(
        &tx,
        activity.evidence.iter().map(|item| item.source.clone()),
    )?;
    bounded_response(DesktopActivity {
        activity,
        entry_positions: positions,
    })
}

#[tauri::command]
pub async fn project_catalog(
    state: State<'_, AppState>,
    page: Page,
    project_filter: Option<String>,
) -> Result<ProjectCatalog, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        filtered_catalog_at(&path, page, project_filter.as_deref())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn project_files(
    state: State<'_, AppState>,
    workspace_id: i64,
    page: Page,
    filename_filter: Option<String>,
) -> Result<FileSummaryPage, String> {
    let path = path_of(&state)?;
    let page = validate_page(page)?;
    tauri::async_runtime::spawn_blocking(move || {
        Library::new(path)
            .get_project_files(
                workspace_id,
                &ProjectScope::all(),
                page,
                filename_filter.as_deref(),
            )
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn project_activity(
    state: State<'_, AppState>,
    workspace_id: i64,
    page: Page,
) -> Result<DesktopActivity, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || activity_at(&path, workspace_id, page, None))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn file_history(
    state: State<'_, AppState>,
    workspace_id: i64,
    reported_path: String,
    page: Page,
) -> Result<DesktopActivity, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        activity_at(&path, workspace_id, page, Some(&reported_path))
    })
    .await
    .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn recorded_git(
    state: State<'_, AppState>,
    workspace_id: i64,
    page: Page,
) -> Result<DesktopRecordedGit, String> {
    let path = path_of(&state)?;
    tauri::async_runtime::spawn_blocking(move || recorded_at(&path, workspace_id, page))
        .await
        .map_err(|e| e.to_string())?
}
#[tauri::command]
pub async fn inspect_git(
    state: State<'_, AppState>,
    workspace_id: i64,
) -> Result<GitInspection, String> {
    let path = path_of(&state)?;
    let inspection = GitInspectionGuard::acquire()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _inspection = inspection;
        Library::new(path)
            .inspect_project_git(workspace_id, &ProjectScope::all(), GitLimits::default())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        let c = db::open_db(&path).unwrap();
        c.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');INSERT INTO workspace(id,path) VALUES(1,'/missing/project'),(2,'box:/missing/project');INSERT INTO session(id,agent_id,workspace_id,source_uri,cwd,host) VALUES(1,1,1,'fixture://one','/missing/project',NULL),(2,1,2,'fixture://remote','/missing/project','box');INSERT INTO entry(id,session_id,ordinal,kind,payload_json) VALUES(10,1,2,'tool','secret payload'),(11,1,999,'tool','secret payload');INSERT INTO file_activity(session_id,entry_id,path,action) VALUES(1,11,'src/a.rs','write');").unwrap();
        (dir, path)
    }
    #[test]
    fn adapter_preserves_core_categories_and_uses_position_not_gapped_ordinal() {
        let (_dir, path) = fixture();
        let result = activity_at(&path, 1, Page::default(), Some("src/a.rs")).unwrap();
        assert_eq!(result.activity.evidence[0].source.entry_ordinal, Some(999));
        assert_eq!(result.entry_positions.get(&11), Some(&1));
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("reported_write_intent"));
        assert!(!json.contains("secret payload"));
        let conn = db::open_db(&path).unwrap();
        conn.execute(
            "INSERT INTO session_context(session_id,entry_id,branch) VALUES(1,11,'recorded')",
            [],
        )
        .unwrap();
        let git = recorded_at(&path, 1, Page::default()).unwrap();
        assert_eq!(git.entry_positions.get(&11), Some(&1));
    }
    #[test]
    fn adapter_rejects_caps_missing_projects_and_never_creates_database() {
        let (dir, path) = fixture();
        assert!(catalog_at(
            &path,
            Page {
                limit: 101,
                offset: 0
            }
        )
        .is_err());
        assert!(activity_at(&path, 99, Page::default(), None).is_err());
        let missing = dir.path().join("absent.sqlite");
        assert!(catalog_at(&missing, Page::default()).is_err());
        assert!(!missing.exists());
    }
    #[test]
    fn catalog_pages_exact_workspace_ids_and_remote_inspection_is_refused() {
        let (_dir, path) = fixture();
        let first = catalog_at(
            &path,
            Page {
                limit: 1,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(first.projects[0].workspace_id, 1);
        assert_eq!(first.next_offset, Some(1));
        let second = catalog_at(
            &path,
            Page {
                limit: 1,
                offset: 1,
            },
        )
        .unwrap();
        assert_eq!(second.projects[0].workspace_id, 2);
        assert!(!second.has_more);
        let filtered = filtered_catalog_at(
            &path,
            Page {
                limit: 1,
                offset: 0,
            },
            Some("box:"),
        )
        .unwrap();
        assert_eq!(filtered.projects[0].workspace_id, 2);
        assert!(
            filtered_catalog_at(&path, Page::default(), Some("' OR 1=1 --"))
                .unwrap()
                .projects
                .is_empty()
        );

        assert!(Library::new(path)
            .inspect_project_git(2, &ProjectScope::all(), GitLimits::default())
            .is_err());
    }
    #[test]
    fn concurrent_git_requests_do_not_queue_unbounded_inspections() {
        let first = GitInspectionGuard::acquire().unwrap();
        assert!(
            std::thread::spawn(|| GitInspectionGuard::acquire().is_err())
                .join()
                .unwrap()
        );
        drop(first);
        assert!(GitInspectionGuard::acquire().is_ok());
    }
}
