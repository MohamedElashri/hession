//! Thin desktop adapter. The shared core owns parsing, bounds and snapshots.
use crate::AppState;
use hession_core::{
    search::{SearchError, SearchPage, SearchRequest, SearchScope},
    service::Library,
};
use serde::Serialize;
use std::sync::atomic::{AtomicUsize, Ordering};
use tauri::State;

static SEARCH_WORKERS: AtomicUsize = AtomicUsize::new(0);
#[derive(Debug)]
struct SearchPermit;
impl Drop for SearchPermit {
    fn drop(&mut self) {
        SEARCH_WORKERS.fetch_sub(1, Ordering::Release);
    }
}
fn search_worker() -> Result<SearchPermit, SearchIpcError> {
    SEARCH_WORKERS
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
            if active < 2 {
                Some(active + 1)
            } else {
                None
            }
        })
        .map(|_| SearchPermit)
        .map_err(|_| SearchIpcError {
            kind: "busy",
            message: "Search workers are busy. Wait briefly and retry.".into(),
            position: None,
        })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchIpcError {
    kind: &'static str,
    message: String,
    position: Option<usize>,
}
impl From<SearchError> for SearchIpcError {
    fn from(error: SearchError) -> Self {
        let (kind, position) = match &error {
            SearchError::Syntax(e) => ("syntax", Some(e.position)),
            SearchError::StaleCursor => ("stale_cursor", None),
            SearchError::InvalidCursor => ("invalid_cursor", None),
            SearchError::TimedOut => ("timeout", None),
            SearchError::Invalid(_) => ("invalid_request", None),
            _ => ("search", None),
        };
        Self {
            kind,
            message: error.to_string(),
            position,
        }
    }
}
#[tauri::command]
pub async fn developer_search(
    request: SearchRequest,
    state: State<'_, AppState>,
) -> Result<SearchPage, SearchIpcError> {
    let permit = search_worker()?;
    let path = state
        .db_path
        .lock()
        .map_err(|_| SearchIpcError {
            kind: "state",
            message: "App state lock was poisoned".into(),
            position: None,
        })?
        .clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        Library::new(path).search(&request, &SearchScope::all())
    })
    .await
    .map_err(|error| SearchIpcError {
        kind: "worker",
        message: format!("Search worker failed: {error}"),
        position: None,
    })?
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn search_admission_is_bounded_and_released_without_queueing() {
        let first = search_worker().unwrap();
        let second = search_worker().unwrap();
        assert_eq!(search_worker().unwrap_err().kind, "busy");
        drop(first);
        let replacement = search_worker().unwrap();
        drop(second);
        drop(replacement);
        assert_eq!(SEARCH_WORKERS.load(Ordering::Acquire), 0);
    }
}
