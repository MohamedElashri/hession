use hession_core::jsonl::{
    Cursor, Mode, ReadError, Reader, ResetReason, Validation, MAX_LINE_BYTES,
};
use serde_json::json;
use std::{
    fs,
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

fn snapshot(path: &Path, previous: Option<&Cursor>) -> (Mode, Vec<serde_json::Value>, Cursor) {
    let mut reader = Reader::open(path, previous, 1, Validation::Content, &|| false)
        .unwrap()
        .unwrap();
    let mode = reader.mode();
    let mut values = Vec::new();
    while let Some(value) = reader.next_record().unwrap() {
        values.push(value);
    }
    (mode, values, reader.finish().unwrap())
}

#[test]
fn complete_offsets_lines_and_normalized_counts_survive_partial_utf8_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("history.jsonl");
    let complete = "\n{\"text\":\"café\"}\r\n \n";
    let mut bytes = complete.as_bytes().to_vec();
    bytes.extend_from_slice(b"{\"text\":\"\xe2\x82");
    fs::write(&file, &bytes).unwrap();
    let (mode, values, cursor) = snapshot(&file, None);
    assert_eq!(mode, Mode::Full(ResetReason::Initial));
    assert_eq!(values, [json!({"text":"café"})]);
    assert_eq!(cursor.offset, complete.len() as u64);
    assert_eq!(cursor.physical_lines, 3);
    assert_eq!(cursor.records, 1);
    assert_eq!(cursor.fingerprint.size, bytes.len() as u64);
    assert!(
        Reader::open(&file, Some(&cursor), 1, Validation::Metadata, &|| false)
            .unwrap()
            .is_none()
    );
    let restored: Cursor = serde_json::from_slice(&serde_json::to_vec(&cursor).unwrap()).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(b"\xac\"}\n")
        .unwrap();
    let (mode, values, cursor) = snapshot(&file, Some(&restored));
    assert_eq!(mode, Mode::Append);
    assert_eq!(values, [json!({"text":"€"})]);
    assert_eq!((cursor.physical_lines, cursor.records), (4, 2));
    assert_eq!(cursor.offset, fs::metadata(&file).unwrap().len());
    let (_, repeated, repeated_cursor) = snapshot(&file, Some(&cursor));
    assert!(repeated.is_empty());
    assert_eq!(repeated_cursor, cursor);
}

#[test]
fn complete_malformed_nonobject_and_oversized_lines_poison_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("history.jsonl");
    for bad in [b"{broken}\n".as_slice(), b"[1,2]\n", b"{\"x\":\"\xff\"}\n"] {
        fs::write(
            &file,
            [b"{\"ok\":true}\n".as_slice(), bad, b"{\"later\":true}\n"].concat(),
        )
        .unwrap();
        let mut reader = Reader::open(&file, None, 1, Validation::Content, &|| false)
            .unwrap()
            .unwrap();
        assert!(reader.next_record().unwrap().is_some());
        assert!(matches!(
            reader.next_record(),
            Err(ReadError::InvalidLine { line: 2, .. })
        ));
        assert!(matches!(reader.next_record(), Err(ReadError::Failed)));
        assert!(matches!(reader.finish(), Err(ReadError::Failed)));
    }
    fs::write(&file, vec![b'x'; MAX_LINE_BYTES + 1]).unwrap();
    let mut reader = Reader::open(&file, None, 1, Validation::Content, &|| false)
        .unwrap()
        .unwrap();
    assert!(matches!(
        reader.next_record(),
        Err(ReadError::LineTooLarge { line: 1 })
    ));
    assert!(matches!(reader.finish(), Err(ReadError::Failed)));
}

#[test]
fn same_size_prefix_rewrite_with_restored_mtime_never_uses_append() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("history.jsonl");
    fs::write(&file, "{\"text\":\"old\"}\n").unwrap();
    let (_, _, cursor) = snapshot(&file, None);
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    fs::write(&file, "{\"text\":\"new\"}\n").unwrap();
    fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let (mode, values, _) = snapshot(&file, Some(&cursor));
    assert_eq!(mode, Mode::Full(ResetReason::Rewritten));
    assert_eq!(values, [json!({"text":"new"})]);
}

#[test]
fn replacement_truncation_and_parser_change_reset_safely() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("history.jsonl");
    fs::write(&file, "{\"one\":1}\n{\"two\":2}\n").unwrap();
    let (_, _, cursor) = snapshot(&file, None);
    fs::write(&file, "{\"one\":1}\n").unwrap();
    assert_eq!(
        snapshot(&file, Some(&cursor)).0,
        Mode::Full(ResetReason::Truncated)
    );
    let (_, _, cursor) = snapshot(&file, None);
    let replacement = dir.path().join("replacement");
    fs::write(&replacement, "{\"one\":1}\n{\"two\":2}\n").unwrap();
    fs::rename(replacement, &file).unwrap();
    assert_eq!(
        snapshot(&file, Some(&cursor)).0,
        Mode::Full(ResetReason::Replaced)
    );
    let (_, _, cursor) = snapshot(&file, None);
    let reader = Reader::open(&file, Some(&cursor), 2, Validation::Content, &|| false)
        .unwrap()
        .unwrap();
    assert_eq!(reader.mode(), Mode::Full(ResetReason::ParserChanged));
    let mut corrupt = cursor.clone();
    corrupt.offset -= 1;
    assert_eq!(
        snapshot(&file, Some(&corrupt)).0,
        Mode::Full(ResetReason::InvalidCursor)
    );
}

#[test]
fn edits_and_atomic_replacements_during_a_read_cannot_commit_a_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("history.jsonl");
    for replace in [false, true] {
        fs::write(&file, "{\"one\":1}\n").unwrap();
        let mut reader = Reader::open(&file, None, 1, Validation::Content, &|| false)
            .unwrap()
            .unwrap();
        assert!(reader.next_record().unwrap().is_some());
        assert!(reader.next_record().unwrap().is_none());
        if replace {
            let other = dir.path().join("replacement");
            fs::write(&other, "{\"one\":1}\n").unwrap();
            fs::rename(other, &file).unwrap();
        } else {
            fs::write(&file, "{\"one\":2}\n").unwrap();
        }
        assert!(matches!(reader.finish(), Err(ReadError::Changed)));
    }
}

#[test]
fn appends_during_read_are_deferred_and_never_skipped_by_metadata_shortcuts() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("history.jsonl");
    fs::write(&file, "{\"one\":1}\n").unwrap();
    let mut reader = Reader::open(&file, None, 1, Validation::Content, &|| false)
        .unwrap()
        .unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(b"{\"two\":2}\n")
        .unwrap();
    assert_eq!(reader.next_record().unwrap(), Some(json!({"one":1})));
    assert_eq!(reader.next_record().unwrap(), None);
    let cursor = reader.finish().unwrap();
    let (mode, values, _) = snapshot(&file, Some(&cursor));
    assert_eq!(mode, Mode::Append);
    assert_eq!(values, [json!({"two":2})]);
}

#[test]
fn cancellation_unread_snapshots_and_nonregular_files_cannot_commit() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("history.jsonl");
    fs::write(&file, "{\"one\":1}\n").unwrap();
    let stop = AtomicBool::new(false);
    let cancel = || stop.load(Ordering::SeqCst);
    let mut reader = Reader::open(&file, None, 1, Validation::Content, &cancel)
        .unwrap()
        .unwrap();
    stop.store(true, Ordering::SeqCst);
    assert!(matches!(reader.next_record(), Err(ReadError::Cancelled)));
    assert!(matches!(reader.finish(), Err(ReadError::Failed)));
    stop.store(false, Ordering::SeqCst);
    let reader = Reader::open(&file, None, 1, Validation::Content, &cancel)
        .unwrap()
        .unwrap();
    assert!(matches!(reader.finish(), Err(ReadError::Changed)));
    assert!(matches!(
        Reader::open(dir.path(), None, 1, Validation::Content, &cancel),
        Err(ReadError::NotRegular)
    ));
    assert!(matches!(
        Reader::open(&file, None, 0, Validation::Content, &cancel),
        Err(ReadError::InvalidParserVersion)
    ));
}

#[test]
fn fifo_sources_are_rejected_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("history.jsonl");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    assert!(matches!(
        Reader::open(&fifo, None, 1, Validation::Content, &|| false),
        Err(ReadError::NotRegular)
    ));
}
