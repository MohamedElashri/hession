use hession_core::{db, library_revision};
use std::process::Command;

// These cases inspect the process-wide writer-priority state. Keep their
// parent fixtures isolated; each case still exercises real threads/processes.
static WRITER_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
#[ignore = "subprocess helper invoked by concurrent_initialization_has_one_schema_and_identity"]
fn child_initialize_library() {
    let path = std::path::PathBuf::from(std::env::var_os("HESSION_TEST_INIT_DATABASE").unwrap());
    let index = std::env::var("HESSION_TEST_INIT_INDEX").unwrap();
    std::fs::write(path.with_extension(format!("ready-{index}")), b"ready").unwrap();
    let conn = db::open_db(&path).unwrap();
    let revision = library_revision::current(&conn).unwrap();
    std::fs::write(
        path.with_extension(format!("result-{index}")),
        revision.identity,
    )
    .unwrap();
}

#[test]
fn concurrent_initialization_has_one_schema_and_identity() {
    let _test = WRITER_TESTS.lock().unwrap();
    use hession_core::locking::{LibraryLock, LockKind};
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let lock = LibraryLock::acquire(&path, LockKind::Write, Duration::ZERO).unwrap();
    let children = ["one", "two"].map(|index| {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "child_initialize_library",
                "--ignored",
                "--nocapture",
            ])
            .env("HESSION_TEST_INIT_DATABASE", &path)
            .env("HESSION_TEST_INIT_INDEX", index)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    });
    let start = Instant::now();
    while !["one", "two"]
        .iter()
        .all(|index| path.with_extension(format!("ready-{index}")).exists())
    {
        assert!(start.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(lock);
    let conn = db::open_db(&path).unwrap();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let identity = library_revision::current(&conn).unwrap().identity;
    for index in ["one", "two"] {
        assert_eq!(
            std::fs::read_to_string(path.with_extension(format!("result-{index}"))).unwrap(),
            identity
        );
    }
    assert_eq!(db::schema_version(&conn).unwrap(), db::SCHEMA_VERSION);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM application_schema", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn writers_reject_active_read_transactions_and_schema_changes_without_leaking_locks() {
    let _test = WRITER_TESTS.lock().unwrap();
    use hession_core::locking::{LibraryLock, LockKind};
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let mut conn = db::open_db(&path).unwrap();
    let snapshot = conn.transaction().unwrap();
    assert!(db::user_write(&snapshot)
        .unwrap_err()
        .to_string()
        .contains("before starting"));
    snapshot.commit().unwrap();
    conn.execute("UPDATE application_schema SET version=2", [])
        .unwrap();
    assert!(db::background_write(&conn)
        .unwrap_err()
        .to_string()
        .contains("schema changed"));
    assert!(db::open_db(&path)
        .unwrap_err()
        .to_string()
        .contains("schema changed"));
    LibraryLock::acquire(&path, LockKind::Write, Duration::ZERO).unwrap();
}

#[test]
fn established_desktop_connections_can_read_while_a_writer_owns_the_process_gate() {
    let _test = WRITER_TESTS.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let conn = db::open_db(&path).unwrap();
    let _writer = db::user_write(&conn).unwrap();
    let reader = db::open_db(&path).unwrap();
    assert_eq!(
        library_revision::current(&reader).unwrap(),
        library_revision::current(&conn).unwrap()
    );
}

#[test]
fn interactive_writers_take_priority_without_waiting_under_a_file_lock() {
    let _test = WRITER_TESTS.lock().unwrap();
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let conn = db::open_db(&path).unwrap();
    let initial = db::background_write(&conn).unwrap();
    let (events, receiver) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let user = {
        let path = path.clone();
        let events = events.clone();
        std::thread::spawn(move || {
            let conn = db::open_db(&path).unwrap();
            let _guard = db::user_write(&conn).unwrap();
            events.send("user").unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
        })
    };
    let started = Instant::now();
    while !db::user_write_pending() {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "user intent must be visible before its OS lock is granted"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let background = {
        let path = path.clone();
        std::thread::spawn(move || {
            let conn = db::open_db(&path).unwrap();
            let _guard = db::background_write(&conn).unwrap();
            events.send("background").unwrap();
        })
    };
    drop(initial);
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        "user"
    );
    assert!(receiver.try_recv().is_err());
    release.send(()).unwrap();
    assert_eq!(
        receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
        "background"
    );
    user.join().unwrap();
    background.join().unwrap();
    assert!(!db::user_write_pending());
}

#[test]
#[ignore = "subprocess helper invoked by wal_readers_remain_consistent_during_independent_process_commit"]
fn child_writer() {
    let path = std::env::var_os("HESSION_TEST_WRITER_DATABASE").expect("isolated library");
    let path = std::path::PathBuf::from(path);
    let mut conn = db::open_db(&path).unwrap();
    let _writer = db::background_write(&conn).unwrap();
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
#[ignore = "subprocess helper invoked by a_new_desktop_process_can_read_during_a_write_transaction"]
fn child_desktop_reader() {
    let path = std::path::PathBuf::from(std::env::var_os("HESSION_TEST_READER_DATABASE").unwrap());
    let conn = db::open_db(&path).unwrap();
    assert_eq!(
        conn.query_row("SELECT count(*) FROM agent", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn a_new_desktop_process_can_read_during_a_write_transaction() {
    let _test = WRITER_TESTS.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let mut conn = db::open_db(&path).unwrap();
    let _writer = db::user_write(&conn).unwrap();
    let transaction = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    transaction
        .execute(
            "INSERT INTO agent(id,family,product) VALUES(1,'codex','Pending')",
            [],
        )
        .unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "child_desktop_reader",
            "--ignored",
            "--nocapture",
        ])
        .env("HESSION_TEST_READER_DATABASE", &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    transaction.rollback().unwrap();
}

#[test]
fn wal_readers_remain_consistent_during_independent_process_commit() {
    let _test = WRITER_TESTS.lock().unwrap();
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
