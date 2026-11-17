use hession_core::{
    db, library_revision,
    locking::{LibraryLock, LockKind},
};
use std::{process::Command, time::Duration};

#[test]
#[ignore = "subprocess helper invoked by wal_readers_remain_consistent_during_independent_process_commit"]
fn child_writer() {
    let path = std::env::var_os("HESSION_TEST_WRITER_DATABASE").expect("isolated library");
    let path = std::path::PathBuf::from(path);
    let mut conn = db::open_db(&path).unwrap();
    let _writer = LibraryLock::acquire(&path, LockKind::Write, Duration::ZERO).unwrap();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    tx.execute(
        "UPDATE session SET title='Committed by another process' WHERE id=42",
        [],
    )
    .unwrap();
    tx.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text) VALUES(42,0,'message','assistant','process snapshot needle')", []).unwrap();
    tx.commit().unwrap();
}

#[test]
fn wal_readers_remain_consistent_during_independent_process_commit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let conn = db::open_db(&path).unwrap();
    conn.execute_batch(
        "BEGIN IMMEDIATE;
        INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
        INSERT INTO session(id,agent_id,source_uri,title) VALUES(42,1,'test://snapshot','Original');
        COMMIT;",
    )
    .unwrap();
    let mut reader = db::open_read_only(&path).unwrap();
    let snapshot = reader.transaction().unwrap();
    let before = library_revision::current(&snapshot).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_writer", "--ignored", "--nocapture"])
        .env("HESSION_TEST_WRITER_DATABASE", &path)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    assert_eq!(library_revision::current(&snapshot).unwrap(), before);
    assert_eq!(
        snapshot
            .query_row("SELECT title FROM session WHERE id=42", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "Original"
    );
    assert_eq!(
        snapshot
            .query_row(
                "SELECT count(*) FROM entry_fts WHERE entry_fts MATCH 'needle'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    snapshot.commit().unwrap();
    assert_ne!(library_revision::current(&reader).unwrap(), before);
    assert_eq!(
        reader
            .query_row("SELECT title FROM session WHERE id=42", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "Committed by another process"
    );
    assert_eq!(
        reader
            .query_row(
                "SELECT count(*) FROM entry_fts WHERE entry_fts MATCH 'needle'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}
