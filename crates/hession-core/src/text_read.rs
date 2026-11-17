//! Bounded, read-only prefixes of authorized text cells. Callers must select
//! the row ID inside their permission-fenced read transaction before using this
//! helper; this module deliberately does not decide access permissions.
use rusqlite::Connection;
use std::io::Read;

pub const MAX_PREFIX_BYTES: usize = 64 * 1024;

/// Fixed schema identifiers prevent a request from choosing arbitrary tables
/// or injecting SQL identifiers. Neither variant accesses raw provider JSON.
#[derive(Clone, Copy, Debug)]
pub enum TextColumn {
    ContentPartText,
    EntrySearchText,
}

#[derive(Debug, thiserror::Error)]
pub enum TextReadError {
    #[error("Text prefix limits must be at most 65536 bytes and characters")]
    InvalidLimit,
    #[error("{0}")]
    Database(#[from] rusqlite::Error),
    #[error("{0}")]
    Read(#[from] std::io::Error),
    #[error("Text cell contains invalid UTF-8")]
    InvalidUtf8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextPrefix {
    pub text: String,
    pub total_bytes: usize,
    pub truncated: bool,
}

/// SQLite `substr(TEXT, ...)` still loads the entire TEXT cell. Incremental BLOB
/// handles read only the requested prefix, including when the cell is TEXT.
pub fn read_prefix(
    conn: &Connection,
    column: TextColumn,
    row_id: i64,
    max_bytes: usize,
    max_chars: usize,
) -> Result<TextPrefix, TextReadError> {
    if max_bytes > MAX_PREFIX_BYTES || max_chars > MAX_PREFIX_BYTES {
        return Err(TextReadError::InvalidLimit);
    }
    let (table, column) = match column {
        TextColumn::ContentPartText => ("content_part", "text"),
        TextColumn::EntrySearchText => ("entry", "search_text"),
    };
    let mut blob = conn.blob_open("main", table, column, row_id, true)?;
    let total_bytes = blob.len();
    let mut bytes = vec![0; total_bytes.min(max_bytes)];
    blob.read_exact(&mut bytes)?;
    let valid = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() && bytes.len() < total_bytes => {
            std::str::from_utf8(&bytes[..error.valid_up_to()])
                .map_err(|_| TextReadError::InvalidUtf8)?
        }
        Err(_) => return Err(TextReadError::InvalidUtf8),
    };
    let end = valid
        .char_indices()
        .nth(max_chars)
        .map_or(valid.len(), |(index, _)| index);
    Ok(TextPrefix {
        text: valid[..end].to_owned(),
        total_bytes,
        truncated: end < total_bytes,
    })
}
