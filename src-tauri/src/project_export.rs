//! Desktop archive jobs keep immutable reviewed plans in trusted memory, never in IPC input.
use crate::AppState;
use hession_core::{
    db, library_revision,
    project_export::{
        self, ExportCancellation, ExportPreview, ExportProgress, ExportResult,
        PreparedProjectExport, ProjectExportRequest,
    },
    projects,
    search::SearchScope,
    service::Library,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{Emitter, State};

const MAX_JOBS: usize = 4;
#[derive(Default)]
pub struct ProjectArchiveState(Arc<Mutex<HashMap<String, Arc<Job>>>>);
#[derive(Default)]
struct Job {
    cancel: ExportCancellation,
    plan: Mutex<Option<PreparedProjectExport>>,
    active: AtomicBool,
    closed: AtomicBool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePreview {
    job_id: String,
    preview: ExportPreview,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveProgress {
    job_id: String,
    #[serde(flatten)]
    progress: ExportProgress,
}
impl ProjectArchiveState {
    fn reserve(&self, id: &str) -> Result<Arc<Job>, String> {
        if id.len() != 36 || uuid::Uuid::parse_str(id).is_err() {
            return Err("Invalid project export job ID".into());
        }
        let mut jobs = self
            .0
            .lock()
            .map_err(|_| "Project export state lock was poisoned")?;
        if jobs.contains_key(id) {
            return Err("That project export job already exists".into());
        }
        if jobs.len() >= MAX_JOBS {
            return Err(
                "Too many project exports are open; close an export preview and retry".into(),
            );
        }
        let job = Arc::new(Job::default());
        job.active.store(true, Ordering::Release);
        jobs.insert(id.to_owned(), job.clone());
        Ok(job)
    }
    fn get(&self, id: &str) -> Result<Arc<Job>, String> {
        self.0
            .lock()
            .map_err(|_| "Project export state lock was poisoned")?
            .get(id)
            .cloned()
            .ok_or_else(|| "Project export job is no longer available".into())
    }
    fn remove(&self, id: &str) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|_| "Project export state lock was poisoned")?
            .remove(id);
        Ok(())
    }
    fn release(&self, id: &str) -> Result<(), String> {
        let job = self.get(id)?;
        job.closed.store(true, Ordering::Release);
        job.cancel.cancel();
        if !job.active.load(Ordering::Acquire) {
            self.remove(id)?;
        }
        Ok(())
    }
}
fn path_of(state: &State<'_, AppState>) -> Result<PathBuf, String> {
    state
        .db_path
        .lock()
        .map(|p| p.clone())
        .map_err(|_| "App state lock was poisoned".into())
}
fn prepare_at(
    path: &Path,
    mut request: ProjectExportRequest,
    expected_key: &str,
    target: &Path,
    cancel: &ExportCancellation,
) -> Result<PreparedProjectExport, String> {
    let scope = SearchScope::workspaces([request.workspace_id]).map_err(|e| e.to_string())?;
    let mut conn = db::open_read_only(path).map_err(|e| e.to_string())?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let project = projects::project_on_connection(&tx, request.workspace_id, &scope)
        .map_err(|e| e.to_string())?;
    let identity = library_revision::current(&tx)
        .map_err(|e| e.to_string())?
        .identity;
    if project.project_key != expected_key {
        return Err("The selected workspace changed; reopen its export preview".into());
    }
    request.filters.project = project.recorded_path;
    tx.commit().map_err(|e| e.to_string())?;
    drop(conn);
    let plan = project_export::prepare(&Library::new(path), &request, &scope, target, cancel)
        .map_err(|e| e.to_string())?;
    if plan.preview().project_key != expected_key || plan.preview().revision.identity != identity {
        return Err("The selected library or workspace changed; reopen its export preview".into());
    }
    Ok(plan)
}

#[tauri::command]
pub async fn prepare_project_archive(
    state: State<'_, AppState>,
    exports: State<'_, ProjectArchiveState>,
    job_id: String,
    request: ProjectExportRequest,
    expected_project_key: String,
    target: String,
) -> Result<ArchivePreview, String> {
    let path = path_of(&state)?;
    let job = exports.reserve(&job_id)?;
    let worker = job.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        prepare_at(
            &path,
            request,
            &expected_project_key,
            Path::new(&target),
            &worker.cancel,
        )
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    job.active.store(false, Ordering::Release);
    match result {
        Ok(plan) if !job.closed.load(Ordering::Acquire) && !job.cancel.is_cancelled() => {
            let preview = plan.preview().clone();
            *job.plan
                .lock()
                .map_err(|_| "Project export preview lock was poisoned")? = Some(plan);
            Ok(ArchivePreview { job_id, preview })
        }
        result => {
            exports.remove(&job_id)?;
            Err(result
                .err()
                .unwrap_or_else(|| "Project export preview cancelled".into()))
        }
    }
}
#[tauri::command]
pub async fn write_project_archive(
    state: State<'_, AppState>,
    exports: State<'_, ProjectArchiveState>,
    app: tauri::AppHandle,
    job_id: String,
) -> Result<ExportResult, String> {
    let path = path_of(&state)?;
    let job = exports.get(&job_id)?;
    if job
        .active
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("This project export is already running".into());
    }
    let plan = job
        .plan
        .lock()
        .map_err(|_| "Project export preview lock was poisoned")?
        .take();
    let Some(plan) = plan else {
        job.active.store(false, Ordering::Release);
        return Err("Prepare a new project export preview before saving".into());
    };
    let worker = job.clone();
    let progress_id = job_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        project_export::write(
            &Library::new(path),
            &plan,
            &SearchScope::all(),
            &worker.cancel,
            |progress| {
                let _ = app.emit(
                    "project-export-progress",
                    ArchiveProgress {
                        job_id: progress_id.clone(),
                        progress,
                    },
                );
            },
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    job.active.store(false, Ordering::Release);
    if job.closed.load(Ordering::Acquire) {
        exports.remove(&job_id)?;
    }
    result
}
#[tauri::command]
pub fn cancel_project_archive(
    exports: State<'_, ProjectArchiveState>,
    job_id: String,
) -> Result<bool, String> {
    Ok(exports.get(&job_id)?.cancel.cancel())
}
#[tauri::command]
pub fn release_project_archive(
    exports: State<'_, ProjectArchiveState>,
    job_id: String,
) -> Result<(), String> {
    exports.release(&job_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn job_registry_bounds_duplicate_and_active_release() {
        let state = ProjectArchiveState::default();
        let mut ids = Vec::new();
        for _ in 0..MAX_JOBS {
            let id = uuid::Uuid::new_v4().to_string();
            let job = state.reserve(&id).unwrap();
            assert!(state.reserve(&id).is_err());
            ids.push((id, job));
        }
        assert!(state.reserve(&uuid::Uuid::new_v4().to_string()).is_err());
        let (id, job) = &ids[0];
        state.release(id).unwrap();
        assert!(job.cancel.is_cancelled());
        assert!(state.get(id).is_ok());
        job.active.store(false, Ordering::Release);
        state.release(id).unwrap();
        assert!(state.get(id).is_err());
        assert!(state.reserve(&uuid::Uuid::new_v4().to_string()).is_ok());
    }
    #[test]
    fn prepared_jobs_bind_exact_portable_project_key_without_source_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        let c = db::open_db(&path).unwrap();
        c.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');INSERT INTO workspace(id,path,display_name) VALUES(1,'/recorded/project','project');INSERT INTO session(id,agent_id,workspace_id,source_uri,external_id) VALUES(1,1,1,'fixture://one','stable');").unwrap();
        let target = dir.path().join("project.zip");
        let request = ProjectExportRequest {
            workspace_id: 1,
            ..Default::default()
        };
        assert!(prepare_at(
            &path,
            request.clone(),
            "wrong-key",
            &target,
            &ExportCancellation::default()
        )
        .is_err());
        let key = projects::project_key("/recorded/project");
        let plan = prepare_at(
            &path,
            request,
            &key,
            &target,
            &ExportCancellation::default(),
        )
        .unwrap();
        assert_eq!(plan.preview().session_count, 1);
        assert!(!target.exists());
    }
}
