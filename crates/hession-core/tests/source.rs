use hession_core::{
    db, indexing, library_revision,
    source::{self, SourceSnapshot},
};
use rusqlite::Connection;
use std::{
    fs::{self, File, FileTimes},
    sync::atomic::{AtomicUsize, Ordering},
};

#[test]
fn fingerprints_detect_same_stat_rewrites_and_skip_metadata_only_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.jsonl");
    fs::write(&path, b"original").unwrap();
    let paths = vec![path.clone(), dir.path().join("optional-wal")];
    let first = SourceSnapshot::capture(&paths, &|| false).unwrap();
    assert!(first.validate(&paths).is_ok());
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    fs::write(&path, b"rewritte").unwrap();
    File::open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    let changed = SourceSnapshot::capture(&paths, &|| false).unwrap();
    assert_ne!(first, changed);
    assert!(!first.same_contents(&changed));
    // Some filesystems have coarse ctime. Hash verification must detect the
    // rewrite even when restoring mtime leaves every metadata field equal.
    let metadata_equal = first
        .files
        .iter()
        .map(|file| &file.metadata)
        .eq(changed.files.iter().map(|file| &file.metadata));
    assert_eq!(first.validate(&paths).is_ok(), metadata_equal);
    File::open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified + std::time::Duration::from_secs(2)))
        .unwrap();
    let touched = SourceSnapshot::capture(&paths, &|| false).unwrap();
    assert_ne!(touched, changed);
    assert!(touched.same_contents(&changed));
    assert!(changed.validate(&paths).is_err());
    fs::write(&paths[1], b"wal update").unwrap();
    assert!(touched.validate(&paths).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"rewritte");
}

#[test]
fn regular_file_bounds_cancellation_and_replacement_are_checked_without_source_locks() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source");
    fs::write(&path, b"first contents").unwrap();
    assert!(source::read_bounded(&path, 2, &|| false)
        .unwrap_err()
        .contains("budget"));
    assert!(
        SourceSnapshot::capture(std::slice::from_ref(&path), &|| true)
            .unwrap_err()
            .contains("stopped")
    );
    let calls = AtomicUsize::new(0);
    let replacement = dir.path().join("replacement");
    fs::write(&replacement, b"other contents").unwrap();
    let result = SourceSnapshot::capture(std::slice::from_ref(&path), &|| {
        if calls.fetch_add(1, Ordering::SeqCst) == 1 {
            fs::rename(&replacement, &path).unwrap();
        }
        false
    });
    assert!(result.unwrap_err().contains("changed"));
    assert!(source::read_bounded(dir.path(), 100, &|| false).is_err());
    let fifo = dir.path().join("fifo");
    let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let start = std::time::Instant::now();
    assert!(source::read_bounded(&fifo, 100, &|| false).is_err());
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
}

fn fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'pi','Pi');INSERT INTO session(id,agent_id,source_uri) VALUES(1,1,'pi:///source');INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(1,1,0,'message','user','hello');INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(1,0,'text','hello');").unwrap();
    conn
}

#[test]
fn full_source_state_is_atomic_invalidated_by_history_and_excluded_from_portable_data() {
    let mut conn = fixture();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source");
    fs::write(&path, b"hello").unwrap();
    let snapshot = SourceSnapshot::capture(&[path], &|| false).unwrap();
    indexing::map_source_session(&conn, "pi:///source", 1).unwrap();
    let generation = indexing::source_generation(&conn, "pi:///source").unwrap();
    indexing::save_full_source(&conn, "pi:///source", "pi:v1", &snapshot).unwrap();
    let revision = library_revision::current(&conn).unwrap();
    assert!(indexing::full_source(&conn, "pi:///source", "pi:v2")
        .unwrap()
        .is_none());
    conn.execute("INSERT INTO session_annotation VALUES(1,1)", [])
        .unwrap();
    assert!(indexing::full_source(&conn, "pi:///source", "pi:v1")
        .unwrap()
        .is_some());
    let before = library_revision::current(&conn).unwrap();
    {
        let tx = conn.transaction().unwrap();
        tx.execute("UPDATE content_part SET text='temporary'", [])
            .unwrap();
        assert!(indexing::full_source(&tx, "pi:///source", "pi:v1")
            .unwrap()
            .is_none());
    }
    assert_eq!(library_revision::current(&conn).unwrap(), before);
    assert!(indexing::full_source(&conn, "pi:///source", "pi:v1")
        .unwrap()
        .is_some());
    conn.execute("UPDATE content_part SET text='changed'", [])
        .unwrap();
    let first_change = indexing::source_generation(&conn, "pi:///source").unwrap();
    assert!(first_change > generation);
    conn.execute("UPDATE content_part SET text='changed again'", [])
        .unwrap();
    assert!(indexing::source_generation(&conn, "pi:///source").unwrap() > first_change);
    assert!(indexing::check_source_generation(&conn, "pi:///source", generation).is_err());
    assert!(indexing::full_source(&conn, "pi:///source", "pi:v1")
        .unwrap()
        .is_none());
    indexing::save_full_source(&conn, "pi:///source", "pi:v1", &snapshot).unwrap();
    conn.execute("DELETE FROM session WHERE id=1", []).unwrap();
    assert!(indexing::full_source(&conn, "pi:///source", "pi:v1")
        .unwrap()
        .is_none());
    assert_eq!(
        conn.query_row("SELECT count(*) FROM index_full_source", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    indexing::clear_local_state(&conn).unwrap();
    for table in [
        "index_full_source",
        "index_source_session",
        "index_source_generation",
    ] {
        assert!(!library_revision::DATA_TABLES.contains(&table));
        assert!(!library_revision::SETTINGS_TABLES.contains(&table));
        assert_eq!(
            conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert!(library_revision::current(&conn).unwrap().revision > revision.revision);
}

#[test]
fn migration_three_is_ordered_transactional_and_indexes_checkpoint_invalidation() {
    let conn = fixture();
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_schema WHERE type='trigger' AND name LIKE 'full_source_%'",
        )
        .unwrap();
    let names = statement
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    drop(statement);
    for name in names {
        conn.execute_batch(&format!("DROP TRIGGER \"{name}\";"))
            .unwrap();
    }
    conn.execute_batch("DROP TABLE index_source_session;DROP TABLE index_full_source;DROP TABLE index_source_generation;DROP INDEX index_checkpoint_session;UPDATE application_schema SET version=2;").unwrap();
    conn.execute_batch("CREATE TABLE index_full_source(conflict);")
        .unwrap();
    assert!(db::init_schema(&conn).is_err());
    assert_eq!(db::schema_version(&conn).unwrap(), 2);
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='index_checkpoint_session'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    conn.execute_batch("DROP TABLE index_full_source;").unwrap();
    db::init_schema(&conn).unwrap();
    db::init_schema(&conn).unwrap();
    assert_eq!(db::schema_version(&conn).unwrap(), 3);
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN SELECT source_uri FROM index_checkpoint WHERE session_id=1",
            [],
            |r| r.get(3),
        )
        .unwrap();
    assert!(plan.contains("index_checkpoint_session"), "{plan}");
    assert_eq!(
        conn.query_row("SELECT search_text FROM entry WHERE id=1", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "hello"
    );
}
