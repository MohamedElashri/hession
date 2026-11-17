//! Foreground local indexing; dispatched before desktop initialization.
use crate::{
    db,
    importer::{self, LocalCatalog},
    indexer::{LibraryChanged, Observer},
};
use hession_cli::{CliError, IndexRequest};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub fn run(request: IndexRequest) -> i32 {
    let json = request.json;
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            return hession_cli::report_error(&CliError::new(3, "runtime", error.to_string()), json)
        }
    };
    let result = runtime.block_on(execute(request));
    // A disconnected pipe may leave one bounded stdout worker in an OS write.
    // main exits after this result; shutdown never waits indefinitely for it.
    runtime.shutdown_timeout(Duration::from_millis(250));
    match result {
        Ok(code) => code,
        Err(error) => hession_cli::report_error(&error, json),
    }
}

async fn execute(request: IndexRequest) -> Result<i32, CliError> {
    let shutdown = hession_cli::wait_for_shutdown();
    tokio::pin!(shutdown);
    if !request.watch {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let mut worker = tokio::task::spawn_blocking(move || {
            importer::with_cancellation(worker_cancel, || {
                let mut conn = db::open_db(&request.db_path)
                    .map_err(|e| CliError::new(3, "library", e.to_string()))?;
                Ok::<_, CliError>(
                    LocalCatalog::capture(request.agent.as_deref()).manual_pass(&mut conn),
                )
            })
        });
        let summary = tokio::select! {
            result = &mut worker => result.map_err(|e| CliError::new(3, "index", e.to_string()))??,
            code = &mut shutdown => {
                cancel.store(true, Ordering::SeqCst);
                let _ = tokio::time::timeout(Duration::from_secs(6), worker).await;
                return Ok(code);
            }
        };
        let failed = summary.errors.len();
        let value = serde_json::to_value(summary)?;
        tokio::select! {
            result = hession_cli::write_output_async(value, request.json) => result?,
            code = &mut shutdown => return Ok(code),
        }
        return if failed == 0 {
            Ok(0)
        } else {
            Err(CliError::new(
                5,
                "index",
                format!("Indexing completed with {failed} source errors"),
            ))
        };
    }
    let pending = Arc::new(Mutex::new(None::<LibraryChanged>));
    let changed = Arc::new(tokio::sync::Notify::new());
    let callback_pending = pending.clone();
    let callback_changed = changed.clone();
    let json = request.json;
    let mut starter = tokio::task::spawn_blocking(move || {
        Observer::start(
            request.db_path,
            LocalCatalog::capture(request.agent.as_deref()),
            Arc::new(move |event| {
                *callback_pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(event);
                callback_changed.notify_one();
            }),
        )
    });
    let observer = tokio::select! {
        result = &mut starter => result.map_err(|e| CliError::new(3, "index", e.to_string()))?
            .map_err(|e| CliError::new(5, "watcher", e))?,
        code = &mut shutdown => {
            if let Ok(Ok(Ok(observer))) = tokio::time::timeout(Duration::from_secs(6), starter).await { drop(observer); }
            return Ok(code);
        }
    };
    // One latest change and one outstanding output; a slow consumer cannot
    // enqueue events or block the observer/importer's database work.
    let mut interval = tokio::time::interval(Duration::from_secs(10));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let output: Value = tokio::select! {
            code = &mut shutdown => { drop(observer); return Ok(code); },
            _ = changed.notified() => {
                let event = pending.lock().unwrap_or_else(|e| e.into_inner()).take();
                let Some(event) = event else { continue; };
                json!({"type":"library_changed","change":event})
            },
            _ = interval.tick() => json!({"type":"indexing_status","status":observer.status()}),
        };
        tokio::select! {
            result = hession_cli::write_output_async(output, json) => result?,
            code = &mut shutdown => { drop(observer); return Ok(code); },
        }
    }
}
