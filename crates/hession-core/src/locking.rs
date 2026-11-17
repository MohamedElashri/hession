//! Kernel-owned library locks. Never delete a lock file: unlinking it would
//! allow another process to lock a different inode for the same library.
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub enum LockKind {
    Write,
    WatchLeader,
}

impl LockKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::WatchLeader => "watch",
        }
    }
}

/// Closing the descriptor (including process termination) releases ownership.
/// Different database files in one directory have independent locks.
#[derive(Debug)]
pub struct LibraryLock {
    _file: File,
    path: PathBuf,
}

impl LibraryLock {
    /// Serialize parsing/checkpoint decisions for one source across processes
    /// without holding the library writer lock during filesystem reads.
    pub fn acquire_source(database: &Path, source: &str, timeout: Duration) -> io::Result<Self> {
        use sha2::{Digest, Sha256};
        let suffix = format!("source-{:x}", Sha256::digest(source.as_bytes()));
        Self::acquire_named(database, &suffix, timeout)
    }

    /// The library directory must already exist. Reject symlinks to avoid
    /// redirecting lock creation outside the selected library.
    pub fn acquire(database: &Path, kind: LockKind, timeout: Duration) -> io::Result<Self> {
        Self::acquire_named(database, kind.suffix(), timeout)
    }

    fn acquire_named(database: &Path, kind: &str, timeout: Duration) -> io::Result<Self> {
        let canonical;
        let database = if database.exists() {
            canonical = std::fs::canonicalize(database)?;
            canonical.as_path()
        } else {
            database
        };
        let parent = database
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = std::fs::canonicalize(parent)?;
        let name = database
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid library path"))?;
        let mut lock_name = name.to_os_string();
        lock_name.push(format!(".{kind}.lock"));
        let path = parent.join(lock_name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "The library lock must be a regular file",
            ));
        }
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file, path }),
                Err(TryLockError::WouldBlock) => {
                    let remaining = timeout.saturating_sub(started.elapsed());
                    if remaining.is_zero() {
                        return Err(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            format!(
                                "Library {} lock is busy at {}; finish the other operation and retry",
                                kind,
                                path.display()
                            ),
                        ));
                    }
                    std::thread::sleep(remaining.min(Duration::from_millis(20)));
                }
                Err(TryLockError::Error(error)) => return Err(error),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
