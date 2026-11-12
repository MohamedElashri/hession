//! Device-local consent for optional capabilities. Reading this never starts work.
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

static SETTINGS_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct OptionalFeatures {
    pub continuation_enabled: bool,
}

fn read_from(path: &Path) -> Result<OptionalFeatures, String> {
    match std::fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(64 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("Could not read optional feature settings: {e}"))?;
            if bytes.len() > 64 * 1024 {
                return Err("Optional feature settings exceed the size limit. Repair optional-features.json before enabling continuation.".into());
            }
            // Serde also accepts a struct's positional array representation;
            // [true] must not become an unexpected opt-in configuration.
            let shape:serde_json::Value=serde_json::from_slice(&bytes)
                .map_err(|_|"Optional feature settings are invalid. Repair optional-features.json before enabling continuation.")?;
            if !shape.is_object() {
                return Err("Optional feature settings must be a JSON object. Repair optional-features.json before enabling continuation.".into());
            }
            // Parse the original bytes again so duplicate fields are rejected.
            serde_json::from_slice(&bytes)
                .map_err(|_| "Optional feature settings are invalid. Repair optional-features.json before enabling continuation.".into())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(OptionalFeatures::default())
        }
        Err(error) => Err(format!("Could not read optional feature settings: {error}")),
    }
}

fn write_to(path: &Path, settings: &OptionalFeatures) -> Result<(), String> {
    let parent = path.parent().ok_or("Settings directory is unavailable.")?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Could not create settings directory: {e}"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .map_err(|e| format!("Could not prepare settings: {e}"))?;
    serde_json::to_writer_pretty(file.as_file_mut(), settings)
        .map_err(|e| format!("Could not encode settings: {e}"))?;
    file.as_file()
        .sync_all()
        .map_err(|e| format!("Could not save settings: {e}"))?;
    file.persist(path)
        .map_err(|e| format!("Could not replace settings: {e}"))?;
    // Persist the rename itself, including an explicit revocation, before
    // reporting success. A crash must not resurrect the previous consent file.
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|e| format!("Could not persist optional feature settings directory: {e}"))?;
    Ok(())
}

pub(crate) fn require_continuation() -> Result<(), String> {
    with_continuation_enabled(|| Ok(()))
}

/// Serialize the final process spawn with a consent change. Once disabling
/// returns, a previously prepared launch cannot subsequently open a terminal.
pub(crate) fn with_continuation_enabled<T>(
    action: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let _guard = SETTINGS_LOCK
        .lock()
        .map_err(|_| "Feature settings are busy.")?;
    if read_from(&crate::db::data_dir().join("optional-features.json"))?.continuation_enabled {
        action()
    } else {
        Err(
            "Cross-harness continuation is disabled. Enable it in Settings → Continuation first."
                .into(),
        )
    }
}

#[tauri::command]
pub fn get_optional_features() -> Result<OptionalFeatures, String> {
    let _guard = SETTINGS_LOCK
        .lock()
        .map_err(|_| "Feature settings are busy.")?;
    read_from(&crate::db::data_dir().join("optional-features.json"))
}

#[tauri::command]
pub fn set_continuation_enabled(enabled: bool) -> Result<OptionalFeatures, String> {
    let _guard = SETTINGS_LOCK
        .lock()
        .map_err(|_| "Feature settings are busy.")?;
    let path = crate::db::data_dir().join("optional-features.json");
    // An explicit toggle can repair invalid settings; no other capability is enabled.
    let settings = OptionalFeatures {
        continuation_enabled: enabled,
    };
    write_to(&path, &settings)?;
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_and_legacy_settings_never_enable_continuation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("optional-features.json");
        assert!(!read_from(&path).unwrap().continuation_enabled);
        assert!(!path.exists());
        std::fs::write(&path, b"{}").unwrap();
        assert!(!read_from(&path).unwrap().continuation_enabled);
        std::fs::write(&path, b"invalid").unwrap();
        assert!(read_from(&path).is_err());
        std::fs::write(
            &path,
            format!("{{\"continuationEnabled\":true}}{}", " ".repeat(64 * 1024)),
        )
        .unwrap();
        assert!(read_from(&path).unwrap_err().contains("size limit"));
        for value in [
            "null",
            "true",
            "[]",
            "[true]",
            "{\"continuationEnabled\":\"true\"}",
            "{\"continuationEnabled\":false,\"continuationEnabled\":true}",
        ] {
            std::fs::write(&path, value).unwrap();
            assert!(
                read_from(&path).is_err(),
                "malformed settings must fail closed: {value}"
            );
        }
    }

    #[test]
    fn explicit_consent_is_local_and_can_be_revoked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("optional-features.json");
        write_to(
            &path,
            &OptionalFeatures {
                continuation_enabled: true,
            },
        )
        .unwrap();
        assert!(read_from(&path).unwrap().continuation_enabled);
        write_to(&path, &OptionalFeatures::default()).unwrap();
        assert!(!read_from(&path).unwrap().continuation_enabled);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
