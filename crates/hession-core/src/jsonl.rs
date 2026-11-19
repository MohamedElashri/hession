//! Bounded complete-line snapshots for append-compatible native JSONL stores.
//! No source writes or locks; a second prefix pass detects rewrites before the
//! caller commits normalized data and its cursor in one library transaction.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const HASH_BUFFER_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("Source IO failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Source is not a regular file")]
    NotRegular,
    #[error("Source line {line} exceeds {MAX_LINE_BYTES} bytes; previous history was preserved")]
    LineTooLarge { line: u64 },
    #[error("Invalid JSONL at line {line}: {message}")]
    InvalidLine { line: u64, message: String },
    #[error("Source changed while reading; retry indexing")]
    Changed,
    #[error("Indexing cancelled")]
    Cancelled,
    #[error("Parser version must be positive")]
    InvalidParserVersion,
    #[error("Source snapshot already failed; retry indexing")]
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fingerprint {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified_secs: i64,
    pub modified_nanos: i64,
    pub changed_secs: i64,
    pub changed_nanos: i64,
}

impl Fingerprint {
    pub fn of(metadata: &Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified_secs: metadata.mtime(),
            modified_nanos: metadata.mtime_nsec(),
            changed_secs: metadata.ctime(),
            changed_nanos: metadata.ctime_nsec(),
        }
    }

    fn same_file(&self, other: &Self) -> bool {
        self.device == other.device && self.inode == other.inode
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub fingerprint: Fingerprint,
    pub offset: u64,
    pub physical_lines: u64,
    pub records: u64,
    pub prefix_hash: String,
    pub parser_version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validation {
    Metadata,
    Content,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetReason {
    Initial,
    ParserChanged,
    Replaced,
    Truncated,
    Rewritten,
    InvalidCursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Append,
    Full(ResetReason),
}

pub struct Reader<'a> {
    path: PathBuf,
    reader: BufReader<std::io::Take<File>>,
    fingerprint: Fingerprint,
    offset: u64,
    physical_lines: u64,
    records: u64,
    parser_version: u32,
    prefix: Sha256,
    mode: Mode,
    ended: bool,
    failed: bool,
    cancelled: &'a dyn Fn() -> bool,
    allow_final_record: bool,
    completing_final_record: bool,
}

fn hash_prefix(
    file: &mut File,
    bytes: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<Sha256, ReadError> {
    file.seek(SeekFrom::Start(0))?;
    let mut remaining = bytes;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; HASH_BUFFER_BYTES];
    while remaining > 0 {
        if cancelled() {
            return Err(ReadError::Cancelled);
        }
        let count = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..count])?;
        if read == 0 {
            return Err(ReadError::Changed);
        }
        digest.update(&buffer[..read]);
        remaining -= read as u64;
    }
    Ok(digest)
}

impl<'a> Reader<'a> {
    /// `None` means unchanged metadata. Native modify/overflow notifications
    /// use Content validation so even identical metadata is rechecked.
    pub fn open(
        path: &Path,
        previous: Option<&Cursor>,
        parser_version: u32,
        validation: Validation,
        cancelled: &'a dyn Fn() -> bool,
    ) -> Result<Option<Self>, ReadError> {
        Self::open_policy(path, previous, parser_version, validation, false, cancelled)
    }

    /// Legacy JSONL stores permit a valid final object without a newline.
    /// Invalid final JSON still fails the snapshot instead of committing a tail.
    pub fn open_complete_snapshot(
        path: &Path,
        previous: Option<&Cursor>,
        parser_version: u32,
        validation: Validation,
        cancelled: &'a dyn Fn() -> bool,
    ) -> Result<Option<Self>, ReadError> {
        Self::open_policy(path, previous, parser_version, validation, true, cancelled)
    }

    fn open_policy(
        path: &Path,
        previous: Option<&Cursor>,
        parser_version: u32,
        validation: Validation,
        allow_final_record: bool,
        cancelled: &'a dyn Fn() -> bool,
    ) -> Result<Option<Self>, ReadError> {
        if parser_version == 0 {
            return Err(ReadError::InvalidParserVersion);
        }
        if cancelled() {
            return Err(ReadError::Cancelled);
        }
        use std::os::unix::fs::OpenOptionsExt;
        // A hostile FIFO replacement must not block the indexing worker.
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(ReadError::NotRegular);
        }
        let fingerprint = Fingerprint::of(&metadata);
        let mut mode = Mode::Full(ResetReason::Initial);
        let mut offset = 0;
        let mut physical_lines = 0;
        let mut records = 0;
        let mut prefix = Sha256::new();
        let mut completing_final_record = false;
        if let Some(previous) = previous {
            let valid = previous.offset <= previous.fingerprint.size
                && previous.physical_lines <= previous.offset
                && previous.records <= previous.physical_lines
                && previous.prefix_hash.len() == 64
                && previous.prefix_hash.bytes().all(|b| b.is_ascii_hexdigit());
            mode = Mode::Full(if !valid {
                ResetReason::InvalidCursor
            } else if previous.parser_version != parser_version {
                ResetReason::ParserChanged
            } else if !previous.fingerprint.same_file(&fingerprint) {
                ResetReason::Replaced
            } else if previous.offset > fingerprint.size {
                ResetReason::Truncated
            } else {
                ResetReason::Rewritten
            });
            if valid
                && previous.parser_version == parser_version
                && previous.fingerprint.same_file(&fingerprint)
                && previous.offset <= fingerprint.size
            {
                let mut boundary_was_eof = false;
                let boundary_valid = if previous.offset == 0 {
                    true
                } else {
                    file.seek(SeekFrom::Start(previous.offset - 1))?;
                    let mut byte = [0];
                    file.read_exact(&mut byte)?;
                    boundary_was_eof = byte[0] != b'\n'
                        && allow_final_record
                        && previous.offset == previous.fingerprint.size;
                    byte[0] == b'\n' || boundary_was_eof
                };
                if boundary_valid {
                    if validation == Validation::Metadata && previous.fingerprint == fingerprint {
                        return Ok(None);
                    }
                    let candidate = hash_prefix(&mut file, previous.offset, cancelled)?;
                    if format!("{:x}", candidate.clone().finalize()) == previous.prefix_hash {
                        mode = Mode::Append;
                        offset = previous.offset;
                        physical_lines = previous.physical_lines;
                        records = previous.records;
                        prefix = candidate;
                        completing_final_record = boundary_was_eof;
                    }
                } else {
                    mode = Mode::Full(ResetReason::InvalidCursor);
                }
            }
        }
        file.seek(SeekFrom::Start(offset))?;
        let bytes = fingerprint.size - offset;
        Ok(Some(Self {
            path: path.to_owned(),
            reader: BufReader::new(file.take(bytes)),
            fingerprint,
            offset,
            physical_lines,
            records,
            parser_version,
            prefix,
            mode,
            ended: false,
            failed: false,
            cancelled,
            allow_final_record,
            completing_final_record,
        }))
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// A partial final line (including partial UTF-8) stays pending at its
    /// preceding complete newline; complete malformed records fail explicitly.
    pub fn next_record(&mut self) -> Result<Option<Value>, ReadError> {
        if self.failed {
            return Err(ReadError::Failed);
        }
        let result = self.read_next();
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn read_next(&mut self) -> Result<Option<Value>, ReadError> {
        if self.ended {
            return Ok(None);
        }
        loop {
            let mut line = Vec::new();
            let mut complete = false;
            loop {
                if (self.cancelled)() {
                    return Err(ReadError::Cancelled);
                }
                let available = self.reader.fill_buf()?;
                if available.is_empty() {
                    break;
                }
                let count = available
                    .iter()
                    .position(|b| *b == b'\n')
                    .map_or(available.len(), |n| n + 1);
                if line.len().saturating_add(count) > MAX_LINE_BYTES {
                    return Err(ReadError::LineTooLarge {
                        line: self.physical_lines + 1,
                    });
                }
                complete = available[count - 1] == b'\n';
                line.extend_from_slice(&available[..count]);
                self.reader.consume(count);
                if complete {
                    break;
                }
            }
            if !complete {
                self.ended = true;
                if !self.allow_final_record || line.is_empty() {
                    return Ok(None);
                }
            }
            if self.completing_final_record {
                if !line.iter().all(u8::is_ascii_whitespace) {
                    return Err(ReadError::InvalidLine {
                        line: self.physical_lines,
                        message: "Appending after a final object requires a newline".into(),
                    });
                }
                self.offset += line.len() as u64;
                self.prefix.update(&line);
                if !complete {
                    return Ok(None);
                }
                self.completing_final_record = false;
                continue;
            }
            self.offset += line.len() as u64;
            self.physical_lines += 1;
            self.prefix.update(&line);
            let text = std::str::from_utf8(&line).map_err(|e| ReadError::InvalidLine {
                line: self.physical_lines,
                message: e.to_string(),
            })?;
            if text.trim().is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(text).map_err(|e| ReadError::InvalidLine {
                line: self.physical_lines,
                message: e.to_string(),
            })?;
            if !value.is_object() {
                return Err(ReadError::InvalidLine {
                    line: self.physical_lines,
                    message: "expected an object".into(),
                });
            }
            self.records += 1;
            return Ok(Some(value));
        }
    }

    /// Consume only after reaching the snapshot end. A fresh prefix check and
    /// path/file identity check precede the caller's atomic cursor commit.
    pub fn finish(mut self) -> Result<Cursor, ReadError> {
        if self.failed {
            return Err(ReadError::Failed);
        }
        if !self.ended {
            return Err(ReadError::Changed);
        }
        let file = self.reader.get_mut().get_mut();
        let actual = hash_prefix(file, self.offset, self.cancelled)?;
        let expected = format!("{:x}", self.prefix.finalize());
        if format!("{:x}", actual.finalize()) != expected {
            return Err(ReadError::Changed);
        }
        let descriptor = Fingerprint::of(&file.metadata()?);
        let path_metadata = std::fs::metadata(&self.path)?;
        let path = Fingerprint::of(&path_metadata);
        if !path_metadata.is_file()
            || !self.fingerprint.same_file(&descriptor)
            || !self.fingerprint.same_file(&path)
            || path.size < self.offset
        {
            return Err(ReadError::Changed);
        }
        Ok(Cursor {
            fingerprint: self.fingerprint,
            offset: self.offset,
            physical_lines: self.physical_lines,
            records: self.records,
            prefix_hash: expected,
            parser_version: self.parser_version,
        })
    }
}
