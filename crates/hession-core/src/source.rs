//! Bounded, read-only fingerprints for adapters that cannot prove append semantics.
//! Metadata is a durable fast path; content hashes prevent harmless stat changes
//! from rebuilding quiet histories. A source is never opened for writing/locking.
use crate::jsonl::Fingerprint;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    path::PathBuf,
};

pub const MAX_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_SOURCE_FILES: usize = 16;
const BLOCK: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub metadata: Option<Fingerprint>,
    pub digest: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSnapshot {
    pub files: Vec<FileSnapshot>,
}

fn open(path: &std::path::Path) -> Result<(File, Fingerprint), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() {
        return Err("Source is not a regular file".into());
    }
    Ok((file, Fingerprint::of(&meta)))
}
fn validate(path: &std::path::Path, file: &File, before: &Fingerprint) -> Result<(), String> {
    let handle = Fingerprint::of(&file.metadata().map_err(|e| e.to_string())?);
    let current = Fingerprint::of(&fs::metadata(path).map_err(|e| e.to_string())?);
    if &handle != before || &current != before {
        return Err("Source changed while reading; retry indexing".into());
    }
    Ok(())
}
pub fn metadata(paths: &[PathBuf]) -> Result<Vec<Option<Fingerprint>>, String> {
    if paths.is_empty() || paths.len() > MAX_SOURCE_FILES {
        return Err("Source companion limit exceeded".into());
    }
    let mut total = 0u64;
    paths
        .iter()
        .enumerate()
        .map(|(index, path)| match fs::metadata(path) {
            Ok(meta) if meta.is_file() => {
                total = total.saturating_add(meta.len());
                if total > MAX_SOURCE_BYTES {
                    return Err(
                        "Source exceeds 512 MiB fingerprint budget; previous history was preserved"
                            .into(),
                    );
                }
                Ok(Some(Fingerprint::of(&meta)))
            }
            Ok(_) => Err("Source is not a regular file".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && index > 0 => Ok(None),
            Err(e) => Err(format!("stat {}: {e}", path.display())),
        })
        .collect()
}
impl SourceSnapshot {
    pub fn valid(&self) -> bool {
        !self.files.is_empty()
            && self.files.len() <= MAX_SOURCE_FILES
            && self
                .files
                .iter()
                .all(|file| match (&file.metadata, &file.digest) {
                    (None, None) => true,
                    (Some(_), Some(digest)) => {
                        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit())
                    }
                    _ => false,
                })
            && self
                .files
                .iter()
                .filter_map(|f| f.metadata.as_ref())
                .map(|m| m.size)
                .try_fold(0u64, |sum, size| sum.checked_add(size))
                .is_some_and(|sum| sum <= MAX_SOURCE_BYTES)
    }
    pub fn capture(paths: &[PathBuf], cancelled: &dyn Fn() -> bool) -> Result<Self, String> {
        let before = metadata(paths)?;
        let mut files = Vec::with_capacity(paths.len());
        let mut buffer = vec![0u8; BLOCK];
        for (path, expected) in paths.iter().zip(&before) {
            if cancelled() {
                return Err("Import stopped".into());
            }
            let Some(expected) = expected else {
                files.push(FileSnapshot {
                    metadata: None,
                    digest: None,
                });
                continue;
            };
            let (mut file, stat) = open(path)?;
            if &stat != expected {
                return Err("Source changed before reading; retry indexing".into());
            }
            let mut remaining = stat.size;
            let mut digest = Sha256::new();
            while remaining > 0 {
                if cancelled() {
                    return Err("Import stopped".into());
                }
                let count = (remaining as usize).min(buffer.len());
                let read = file.read(&mut buffer[..count]).map_err(|e| e.to_string())?;
                if read == 0 {
                    return Err("Source changed while reading; retry indexing".into());
                }
                digest.update(&buffer[..read]);
                remaining -= read as u64;
            }
            validate(path, &file, &stat)?;
            files.push(FileSnapshot {
                metadata: Some(stat),
                digest: Some(format!("{:x}", digest.finalize())),
            });
        }
        let snapshot = Self { files };
        snapshot.validate(paths)?;
        Ok(snapshot)
    }
    pub fn validate(&self, paths: &[PathBuf]) -> Result<(), String> {
        if self
            .files
            .iter()
            .map(|f| f.metadata.clone())
            .collect::<Vec<_>>()
            != metadata(paths)?
        {
            return Err("Source changed while reading; retry indexing".into());
        }
        Ok(())
    }
    pub fn same_contents(&self, other: &Self) -> bool {
        self.files.len() == other.files.len()
            && self
                .files
                .iter()
                .zip(&other.files)
                .all(|(a, b)| a.digest == b.digest)
    }
}

pub fn read_bounded(
    path: &std::path::Path,
    limit: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<u8>, String> {
    if cancelled() {
        return Err("Import stopped".into());
    }
    let (mut file, before) = open(path)?;
    if before.size > limit {
        return Err(format!(
            "Source exceeds {limit} byte read budget; previous history was preserved"
        ));
    }
    let mut bytes = Vec::new();
    let mut buffer = vec![0; BLOCK];
    let mut remaining = before.size;
    while remaining > 0 {
        if cancelled() {
            return Err("Import stopped".into());
        }
        let count = (remaining as usize).min(buffer.len());
        let read = file.read(&mut buffer[..count]).map_err(|e| e.to_string())?;
        if read == 0 {
            return Err("Source changed while reading; retry indexing".into());
        }
        bytes.extend_from_slice(&buffer[..read]);
        remaining -= read as u64;
    }
    validate(path, &file, &before)?;
    Ok(bytes)
}
