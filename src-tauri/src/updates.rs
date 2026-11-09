//! User-initiated, signed updates. Opening the dialog never uses the network.

use serde_json::{json, Value};
#[cfg(target_os = "linux")]
use tauri::Manager;

fn unsupported_reason(packaged: bool, configured: bool) -> Option<&'static str> {
    if !packaged {
        Some("Install a release package to use in-app updates. Rebuild source installations from source.")
    } else if !configured {
        Some("Update signing is not configured for this build. Download a release to update.")
    } else {
        None
    }
}

#[tauri::command]
pub fn app_version(app: tauri::AppHandle) -> Value {
    let configured = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|config| config.get("pubkey"))
        .and_then(Value::as_str)
        .is_some_and(|key| !key.trim().is_empty());
    #[cfg(target_os = "linux")]
    let appimage = app.env().appimage.is_some();
    #[cfg(not(target_os = "linux"))]
    let appimage = false;
    use tauri::utils::{config::BundleType, platform::bundle_type};
    let system_package = cfg!(target_os = "linux")
        && matches!(bundle_type(), Some(BundleType::Deb | BundleType::Rpm));
    let bundle = cfg!(target_os = "macos")
        && std::env::current_exe().ok().is_some_and(|path| {
            path.ancestors()
                .any(|p| p.extension().is_some_and(|ext| ext == "app"))
        });
    let reason = unsupported_reason(appimage || system_package || bundle, configured);
    json!({
        "name": crate::brand::NAME,
        "version": env!("CARGO_PKG_VERSION"),
        "supported": reason.is_none(),
        "reason": reason,
        "needsAuthorization": system_package,
    })
}

#[tauri::command]
pub fn restart_after_update(app: tauri::AppHandle) -> Result<(), String> {
    // The maintenance guard uses the sync launch lock. Keep it until exit so
    // another sync cannot begin between this check and the restart request.
    let guard = crate::sync::acquire_data_management().map_err(|_| {
        "Wait for session sync and saved-data operations to finish, then restart.".to_string()
    })?;
    app.request_restart();
    std::mem::forget(guard);
    Ok(())
}

#[tauri::command]
pub fn open_release_page() -> Result<(), String> {
    let url = format!("https://github.com/{}/releases", crate::brand::REPOSITORY);
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open the releases page: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_signed_release_packages_update_in_place() {
        assert!(unsupported_reason(true, true).is_none());
        assert!(unsupported_reason(false, true).unwrap().contains("source"));
        assert!(unsupported_reason(true, false).unwrap().contains("signing"));
    }
}
