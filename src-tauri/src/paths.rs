use std::path::PathBuf;

/// Read an optional path override, treating whitespace-only values as unset.
pub(crate) fn env_path(key: &str, default: PathBuf) -> PathBuf {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => PathBuf::from(value.trim()),
        _ => default,
    }
}

pub(crate) fn home_dir() -> PathBuf {
    env_path("HOME", PathBuf::from("."))
}
