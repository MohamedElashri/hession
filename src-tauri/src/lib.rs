//! Tauri v2 application shell. Product identity lives in `brand.json` (see `brand`).
//!
//! File modules (`db`, `analytics`, `pricing`, `commands`, `importer`,
//! `sync`) live in their own `*.rs` files.

use std::path::PathBuf;
use std::sync::Mutex;

pub mod analytics;
pub mod brand;
pub mod commands;
pub mod db;
pub mod data_management;
pub mod importer;
pub mod pricing;
pub mod remote;
pub mod resume;
pub mod sync;

/// Shared application state managed by Tauri.
#[derive(Debug, Default)]
pub struct AppState {
    /// Database path resolved at startup (`db::db_path()`).
    pub db_path: Mutex<PathBuf>,
}

pub fn run() {
    let db_path = db::db_path();
    let db_path_for_setup = db_path.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(commands::ExportState::default())
        .manage(AppState {
            db_path: Mutex::new(db_path),
        })
        .setup(move |app| {
            use tauri::Manager;
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
            }
            // Initialize on a blocking worker; the frontend starts sync after its first library paint.
            let path = db_path_for_setup.clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(error) = db::open_db(&path) {
                    eprintln!(
                        "{} could not initialize the database at {}: {error}",
                        brand::NAME,
                        path.display()
                    );
                } else {
                    analytics::request_refresh(&path, false);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::get_health,
            commands::search_sessions,
            commands::get_session,
            commands::update_session_annotations,
            commands::begin_export,
            commands::cancel_export,
            commands::finish_export,
            commands::export_sessions,
            commands::save_export,
            commands::get_agents,
            commands::get_projects,
            commands::get_facets,
            commands::get_stats,
            commands::get_analytics,
            commands::refresh_analytics,
            commands::get_pricing_status,
            commands::sync_pricing,
            commands::get_pricing_overrides,
            commands::save_pricing_override,
            commands::delete_pricing_override,
            commands::get_sync_status,
            commands::get_sync_checkpoints,
            commands::start_sync,
            commands::stop_sync,
            resume::get_resume_info,
            resume::resume_session,
            resume::list_terminals,
            resume::get_terminal_preference,
            resume::set_terminal_preference,
            resume::open_in_editor,
            remote::get_remote_hosts,
            remote::set_remote_hosts,
            remote::test_remote_host,
            data_management::preview_saved_data,
            data_management::export_saved_data,
            data_management::preview_data_import,
            data_management::import_saved_data,
            data_management::delete_saved_data,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| panic!("error while running {}: {error}", brand::NAME));
}
