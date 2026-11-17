use hession_core::locking::{LibraryLock, LockKind};
use std::io;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
#[ignore = "subprocess helper invoked by kernel_lock_survives_contention_and_recovers_after_kill"]
fn child_lock_holder() {
    use std::io::Read;
    let path = std::env::var_os("HESSION_TEST_LOCK_DATABASE").expect("isolated child database");
    let database = std::path::PathBuf::from(path);
    let mut conn = hession_core::db::open_db(&database).unwrap();
    let _lock = hession_core::db::user_write(&conn).unwrap();
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    tx.execute(
        "INSERT INTO agent(id,family,product) VALUES(99,'codex','Uncommitted')",
        [],
    )
    .unwrap();
    std::fs::write(database.with_extension("ready"), b"ready").unwrap();
    // The parent kills this child; EOF also permits orderly exit.
    let _ = io::stdin().read(&mut [0_u8]);
}

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn kernel_lock_survives_contention_and_recovers_after_kill() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("sessions.sqlite");
    let conn = hession_core::db::open_db(&database).unwrap();
    let mut child = Child(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "child_lock_holder", "--ignored", "--nocapture"])
            .env("HESSION_TEST_LOCK_DATABASE", &database)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    while !database.with_extension("ready").exists() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "child did not acquire lock"
        );
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "child failed before readiness"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let error = LibraryLock::acquire(&database, LockKind::Write, Duration::ZERO).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
    assert!(error.to_string().contains("retry"));
    // Lock ownership never blocks independent WAL readers.
    let reader = hession_core::db::open_read_only(&database).unwrap();
    assert_eq!(
        hession_core::library_revision::current(&reader).unwrap(),
        hession_core::library_revision::current(&conn).unwrap()
    );
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert_eq!(
        reader
            .query_row("SELECT count(*) FROM agent", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0,
        "termination must roll back the pending write as well as release the lock"
    );
    let recovered = LibraryLock::acquire(&database, LockKind::Write, Duration::ZERO).unwrap();
    let lock_file = recovered.path().to_path_buf();
    drop(recovered);
    assert!(
        lock_file.exists(),
        "lock inode must not be removed on release"
    );
    LibraryLock::acquire(&database, LockKind::Write, Duration::ZERO).unwrap();
}

#[test]
fn watch_leadership_is_exclusive_and_libraries_are_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("first.sqlite");
    let first = LibraryLock::acquire(&database, LockKind::WatchLeader, Duration::ZERO).unwrap();
    assert_eq!(
        LibraryLock::acquire(&database, LockKind::WatchLeader, Duration::ZERO)
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    // A watcher may take short write locks; another independent library can watch.
    let _writer = LibraryLock::acquire(&database, LockKind::Write, Duration::ZERO).unwrap();
    let _other = LibraryLock::acquire(
        &dir.path().join("second.sqlite"),
        LockKind::WatchLeader,
        Duration::ZERO,
    )
    .unwrap();
    drop(first);
    LibraryLock::acquire(&database, LockKind::WatchLeader, Duration::ZERO).unwrap();
}

#[test]
fn source_locks_serialize_parsers_without_blocking_other_sources_or_wal_writes() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("sessions.sqlite");
    let source = "claude:///private/native/path/../history.jsonl";
    let guard = LibraryLock::acquire_source(&database, source, Duration::ZERO).unwrap();
    // macOS temporary directories themselves live below /private. Only the
    // generated filename must omit the native source's path components.
    let filename = guard.path().file_name().unwrap().to_string_lossy();
    assert!(!filename.contains("private"));
    assert!(!filename.contains("history"));
    assert_eq!(
        LibraryLock::acquire_source(&database, source, Duration::ZERO)
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    let _other =
        LibraryLock::acquire_source(&database, "codex:///another/source.jsonl", Duration::ZERO)
            .unwrap();
    let _write = LibraryLock::acquire(&database, LockKind::Write, Duration::ZERO).unwrap();
    drop(guard);
    LibraryLock::acquire_source(&database, source, Duration::ZERO).unwrap();
}

#[test]
fn lock_wait_is_bounded_and_does_not_steal_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("sessions.sqlite");
    let _owner = LibraryLock::acquire(&database, LockKind::Write, Duration::ZERO).unwrap();
    let start = Instant::now();
    assert_eq!(
        LibraryLock::acquire(&database, LockKind::Write, Duration::from_millis(50))
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    assert!(start.elapsed() >= Duration::from_millis(45));
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[cfg(unix)]
#[test]
fn symlink_database_uses_same_lock_and_symlink_lock_is_rejected() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("sessions.sqlite");
    std::fs::write(&database, b"").unwrap();
    let alias = dir.path().join("alias.sqlite");
    symlink(&database, &alias).unwrap();
    let _owner = LibraryLock::acquire(&database, LockKind::Write, Duration::ZERO).unwrap();
    assert_eq!(
        LibraryLock::acquire(&alias, LockKind::Write, Duration::ZERO)
            .unwrap_err()
            .kind(),
        io::ErrorKind::WouldBlock
    );
    let target = dir.path().join("unrelated");
    std::fs::write(&target, b"preserve").unwrap();
    symlink(&target, dir.path().join("other.sqlite.write.lock")).unwrap();
    assert!(LibraryLock::acquire(
        &dir.path().join("other.sqlite"),
        LockKind::Write,
        Duration::ZERO
    )
    .is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"preserve");
}

#[test]
fn invalid_library_filename_never_silently_drops_cross_process_coordination() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let invalid = dir
        .path()
        .join(std::ffi::OsString::from_vec(vec![b'x', 0xff]));
    assert!(hession_core::db::open_db(&invalid)
        .unwrap_err()
        .to_string()
        .contains("UTF-8"));
    assert!(hession_core::db::open_read_only(&invalid)
        .unwrap_err()
        .to_string()
        .contains("UTF-8"));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    // Linux permits such filenames; macOS APFS rejects them before SQLite.
    #[cfg(target_os = "linux")]
    {
        let conn = rusqlite::Connection::open(&invalid).unwrap();
        assert!(conn.path().is_none());
        assert!(hession_core::db::init_schema(&conn)
            .unwrap_err()
            .to_string()
            .contains("UTF-8"));
        assert_eq!(
            conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
