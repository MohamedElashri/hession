use super::*;
use serde_json::json;
use std::fs::{File, FileTimes};

fn legacy(text: &str) -> String {
    json!({"type":"message","message":{"role":"user","content":[{"type":"text","text":text}]},"timestamp":"2026-01-01T12:00:00Z"}).to_string()
}
fn open(dir: &Path) -> Connection {
    let conn = crate::db::open_db(&dir.join("library.sqlite")).unwrap();
    crate::db::configure_bulk_writer(&conn).unwrap();
    conn
}
fn import(conn: &mut Connection, uri: &str) -> Result<usize, String> {
    with_cancellation(Arc::new(AtomicBool::new(false)), || {
        import_file_with_conn(conn, uri, false, None, &mut ImportPricing::new())
    })
}
fn body(conn: &Connection) -> String {
    conn.query_row(
        "SELECT search_text FROM entry ORDER BY id LIMIT 1",
        [],
        |r| r.get(0),
    )
    .unwrap()
}
fn catalog(root: PathBuf) -> LocalCatalog {
    LocalCatalog(vec![Source {
        family: "pi",
        product: "Pi",
        root,
        sub: "sessions",
        kind: SourceKind::Jsonl,
    }])
}

#[test]
fn legacy_same_stat_rewrite_is_imported_but_quiet_restart_and_audits_do_not_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("source.jsonl");
    fs::write(&file, legacy("alpha")).unwrap();
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    let uri = format!("pi://{}", file.display());
    let mut conn = open(dir.path());
    assert_eq!(import(&mut conn, &uri).unwrap(), 1);
    assert_eq!(body(&conn), "alpha");
    let session_id = conn
        .query_row("SELECT id FROM session", [], |r| r.get::<_, i64>(0))
        .unwrap();
    conn.execute(
        "INSERT INTO session_annotation(session_id,starred) VALUES(?,1)",
        [session_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session_tag(session_id,label) VALUES(?,'retained')",
        [session_id],
    )
    .unwrap();
    let id = conn
        .query_row("SELECT id FROM entry", [], |r| r.get::<_, i64>(0))
        .unwrap();
    let revision = hession_core::library_revision::current(&conn).unwrap();
    assert_eq!(import(&mut conn, &uri).unwrap(), 0);
    assert_eq!(
        hession_core::library_revision::current(&conn).unwrap(),
        revision
    );
    drop(conn);
    let mut conn = open(dir.path());
    assert_eq!(import(&mut conn, &uri).unwrap(), 0);
    assert_eq!(
        conn.query_row("SELECT id FROM entry", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        id
    );
    File::open(&file)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified + std::time::Duration::from_secs(2)))
        .unwrap();
    assert_eq!(import(&mut conn, &uri).unwrap(), 0);
    assert_eq!(
        hession_core::library_revision::current(&conn).unwrap(),
        revision
    );
    fs::write(&file, legacy("bravo")).unwrap();
    File::open(&file)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(import(&mut conn, &uri).unwrap(), 1);
    assert_eq!(body(&conn), "bravo");
    assert_eq!(
        conn.query_row("SELECT id FROM session", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        session_id
    );
    assert_eq!(
        conn.query_row(
            "SELECT starred FROM session_annotation WHERE session_id=?",
            [session_id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT label FROM session_tag WHERE session_id=?",
            [session_id],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "retained"
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(parse_jsonl_file(&file).unwrap().len(), 1); // Valid legacy last line has no newline.
}

#[test]
fn legacy_checkpoint_and_normalized_history_rollback_together_on_error_cancel_and_epoch_change() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("source.jsonl");
    fs::write(&file, legacy("alpha")).unwrap();
    let uri = format!("pi://{}", file.display());
    let mut conn = open(dir.path());
    import(&mut conn, &uri).unwrap();
    let revision = hession_core::library_revision::current(&conn).unwrap();
    let stamp: String = conn
        .query_row("SELECT fingerprint_json FROM index_full_source", [], |r| {
            r.get(0)
        })
        .unwrap();
    fs::write(&file, legacy("bravo")).unwrap();
    conn.execute_batch("CREATE TRIGGER fixture_checkpoint_failure BEFORE UPDATE ON index_full_source WHEN NEW.invalidated=0 BEGIN SELECT RAISE(ABORT,'fixture checkpoint failure'); END;").unwrap();
    assert!(import(&mut conn, &uri)
        .unwrap_err()
        .contains("checkpoint failure"));
    assert_eq!(body(&conn), "alpha");
    assert_eq!(
        hession_core::library_revision::current(&conn).unwrap(),
        revision
    );
    assert_eq!(
        conn.query_row("SELECT fingerprint_json FROM index_full_source", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        stamp
    );
    conn.execute_batch("DROP TRIGGER fixture_checkpoint_failure;")
        .unwrap();
    let token = Arc::new(AtomicBool::new(false));
    let callback = token.clone();
    conn.create_scalar_function(
        "fixture_cancel",
        0,
        rusqlite::functions::FunctionFlags::SQLITE_UTF8,
        move |_| {
            callback.store(true, Ordering::SeqCst);
            Ok(0i64)
        },
    )
    .unwrap();
    conn.execute_batch("CREATE TRIGGER fixture_cancel_import AFTER INSERT ON entry BEGIN SELECT fixture_cancel(); END;").unwrap();
    assert!(with_cancellation(token, || import_file_with_conn(
        &mut conn,
        &uri,
        false,
        None,
        &mut ImportPricing::new()
    ))
    .is_err());
    assert_eq!(body(&conn), "alpha");
    assert_eq!(
        hession_core::library_revision::current(&conn).unwrap(),
        revision
    );
    conn.execute_batch("DROP TRIGGER fixture_cancel_import;")
        .unwrap();
    let mut pricing = ImportPricing::new();
    pricing.epoch = Some(hession_core::indexing::epoch(&conn).unwrap());
    hession_core::indexing::maintenance(&conn).unwrap();
    assert!(
        with_cancellation(Arc::new(AtomicBool::new(false)), || import_file_with_conn(
            &mut conn,
            &uri,
            false,
            None,
            &mut pricing
        ))
        .unwrap_err()
        .contains("changed during indexing")
    );
    assert_eq!(body(&conn), "alpha");
    assert!(
        hession_core::indexing::full_source(&conn, &uri, "pi:full-source-v1")
            .unwrap()
            .is_none()
    );
    assert_eq!(import(&mut conn, &uri).unwrap(), 1);
    assert_eq!(body(&conn), "bravo");
}

#[test]
fn manual_local_pass_restores_an_explicitly_excluded_source_but_background_import_does_not() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("sessions")).unwrap();
    let file = dir.path().join("sessions/source.jsonl");
    fs::write(&file, legacy("alpha")).unwrap();
    let uri = format!("pi://{}", file.display());
    let mut conn = open(dir.path());
    import(&mut conn, &uri).unwrap();
    hession_core::indexing::exclude(&conn, &uri).unwrap();
    conn.execute("DELETE FROM session", []).unwrap();
    let catalog = catalog(dir.path().into());
    let epoch = hession_core::indexing::epoch(&conn).unwrap();
    assert_eq!(
        with_cancellation(Arc::new(AtomicBool::new(false)), || catalog
            .import(&mut conn, "pi", &file, epoch))
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM session", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let summary = with_cancellation(Arc::new(AtomicBool::new(false)), || {
        catalog.manual_pass(&mut conn)
    });
    assert!(summary.errors.is_empty(), "{:?}", summary.errors);
    assert_eq!(summary.records, 1);
    assert_eq!(summary.files_done, 1);
    assert!(!hession_core::indexing::excluded(&conn, &uri).unwrap());
    assert_eq!(body(&conn), "alpha");
}

#[test]
fn wal_updates_with_unchanged_session_counts_and_timestamps_rebuild_and_then_quietly_skip() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("state.db");
    let source = Connection::open(&file).unwrap();
    source.execute_batch("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0;CREATE TABLE sessions(id TEXT PRIMARY KEY,started_at REAL,last_activity_at REAL);CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id TEXT,role TEXT,content TEXT,timestamp REAL,tool_calls TEXT);INSERT INTO sessions VALUES('one',1,1);INSERT INTO messages VALUES(1,'one','user','alpha',1,'[]');").unwrap();
    let primary = hession_core::jsonl::Fingerprint::of(&fs::metadata(&file).unwrap());
    let uri = format!("hermes://{}", file.display());
    let mut conn = open(dir.path());
    let original =
        hession_core::source::SourceSnapshot::capture(&source_paths("hermes", &file), &|| false)
            .unwrap();
    assert_eq!(import(&mut conn, &uri).unwrap(), 1);
    assert_eq!(
        hession_core::source::SourceSnapshot::capture(&source_paths("hermes", &file), &|| false)
            .unwrap(),
        original
    );
    source
        .execute("UPDATE messages SET content='bravo' WHERE id=1", [])
        .unwrap();
    assert_eq!(
        hession_core::jsonl::Fingerprint::of(&fs::metadata(&file).unwrap()),
        primary
    );
    let rewritten =
        hession_core::source::SourceSnapshot::capture(&source_paths("hermes", &file), &|| false)
            .unwrap();
    assert_eq!(import(&mut conn, &uri).unwrap(), 1);
    assert_eq!(
        hession_core::source::SourceSnapshot::capture(&source_paths("hermes", &file), &|| false)
            .unwrap(),
        rewritten
    );
    assert_eq!(body(&conn), "bravo");
    let revision = hession_core::library_revision::current(&conn).unwrap();
    assert_eq!(import(&mut conn, &uri).unwrap(), 0);
    assert_eq!(
        hession_core::library_revision::current(&conn).unwrap(),
        revision
    );
    drop(conn);
    let mut conn = open(dir.path());
    assert_eq!(import(&mut conn, &uri).unwrap(), 0);
    assert_eq!(
        source
            .query_row("SELECT content FROM messages", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "bravo"
    );
}

#[test]
fn bounded_legacy_reads_reject_oversized_lines_nonregular_sources_and_large_decoded_values() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("source.jsonl");
    fs::write(&file, vec![b'x'; hession_core::jsonl::MAX_LINE_BYTES + 1]).unwrap();
    assert!(parse_jsonl_file(&file).unwrap_err().contains("8 MiB"));
    File::create(&file)
        .unwrap()
        .set_len(MAX_LEGACY_BYTES + 1)
        .unwrap();
    assert!(parse_jsonl_file(&file).unwrap_err().contains("128 MiB"));
    assert!(parse_jsonl_file(dir.path())
        .unwrap_err()
        .contains("regular"));
    let mut budget = 256;
    assert!(charge_value(&json!({"nested":[1,2,3]}), &mut budget).is_err());
    fs::write(&file, b"{\"valid\":true}").unwrap();
    assert_eq!(parse_jsonl_file(&file).unwrap().len(), 1);
}

#[test]
fn a_late_prepared_source_cannot_clear_invalidation_or_overwrite_another_writers_history() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("source.jsonl");
    fs::write(&file, legacy("alpha")).unwrap();
    let uri = native_source_uri("pi", &file).unwrap();
    let mut conn = open(dir.path());
    import(&mut conn, &uri).unwrap();
    let mut pricing = ImportPricing::new();
    pricing.epoch = Some(hession_core::indexing::epoch(&conn).unwrap());
    pricing.source_uri = Some(uri.clone());
    pricing.source_generation =
        Some(hession_core::indexing::source_generation(&conn, &uri).unwrap());
    fs::write(&file, legacy("bravo")).unwrap();
    let paths = vec![file.clone()];
    let prepared = PreparedSource {
        uri: uri.clone(),
        capability: "pi:full-source-v1".into(),
        snapshot: hession_core::source::SourceSnapshot::capture(&paths, &|| false).unwrap(),
        paths,
    };
    let values = parse_jsonl_file(&file).unwrap();
    let stat = fs::metadata(&file).unwrap();
    let data = formats::session_data(
        "pi",
        &uri,
        &file,
        values,
        stat.len() as i64,
        source_mtime(&stat),
    )
    .unwrap();
    let other = Connection::open(dir.path().join("library.sqlite")).unwrap();
    other
        .execute("UPDATE entry SET search_text='other writer'", [])
        .unwrap();
    assert!(persist_full_source(&mut conn, &pricing, &prepared)
        .unwrap_err()
        .contains("changed during preparation"));
    pricing.prepared_full = Some(prepared);
    assert!(
        with_cancellation(Arc::new(AtomicBool::new(false)), || commit_batch(
            &mut conn,
            "pi",
            "Pi",
            "/fixture",
            &mut pricing,
            vec![data]
        ))
        .unwrap_err()
        .contains("changed during preparation")
    );
    assert_eq!(body(&conn), "other writer");
    assert!(
        hession_core::indexing::full_source(&conn, &uri, "pi:full-source-v1")
            .unwrap()
            .is_none()
    );
    assert_eq!(import(&mut conn, &uri).unwrap(), 1);
    assert_eq!(body(&conn), "bravo");
}

#[test]
fn content_audits_recheck_even_identical_metadata_without_rebuilding_quiet_sources() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("sessions")).unwrap();
    let file = dir.path().join("sessions/source.jsonl");
    fs::write(&file, legacy("alpha")).unwrap();
    let uri = native_source_uri("pi", &file).unwrap();
    let mut conn = open(dir.path());
    import(&mut conn, &uri).unwrap();
    let revision = hession_core::library_revision::current(&conn).unwrap();
    let catalog = catalog(dir.path().into());
    let epoch = hession_core::indexing::epoch(&conn).unwrap();
    assert_eq!(
        with_cancellation(Arc::new(AtomicBool::new(false)), || catalog
            .import_with_policy(&mut conn, "pi", &file, epoch, true))
        .unwrap(),
        0
    );
    assert_eq!(
        hession_core::library_revision::current(&conn).unwrap(),
        revision
    );
    fs::write(&file, legacy("bravo")).unwrap();
    // Simulate a coarse filesystem reporting exactly the saved metadata; the
    // event/audit policy must still compare bytes rather than trust that stamp.
    let mut cached = hession_core::indexing::full_source(&conn, &uri, "pi:full-source-v1")
        .unwrap()
        .unwrap();
    cached.files[0].metadata = Some(hession_core::jsonl::Fingerprint::of(
        &fs::metadata(&file).unwrap(),
    ));
    hession_core::indexing::save_full_source(&conn, &uri, "pi:full-source-v1", &cached).unwrap();
    assert_eq!(
        with_cancellation(Arc::new(AtomicBool::new(false)), || catalog
            .import(&mut conn, "pi", &file, epoch))
        .unwrap(),
        0
    );
    assert_eq!(
        with_cancellation(Arc::new(AtomicBool::new(false)), || catalog
            .import_with_policy(&mut conn, "pi", &file, epoch, true))
        .unwrap(),
        1
    );
    assert_eq!(body(&conn), "bravo");
}

#[test]
fn native_non_utf8_paths_are_rejected_without_aliasing_but_unicode_paths_import() {
    use std::os::unix::ffi::OsStringExt;
    let dir = tempfile::tempdir().unwrap();
    let mut conn = open(dir.path());
    let catalog = catalog(dir.path().into());
    let epoch = hession_core::indexing::epoch(&conn).unwrap();
    for byte in [0xfe, 0xff] {
        let file = dir.path().join(std::ffi::OsString::from_vec(vec![
            b'x', byte, b'.', b'j', b's', b'o', b'n', b'l',
        ]));
        fs::write(&file, legacy("alpha")).unwrap();
        assert!(catalog
            .import_with_policy(&mut conn, "pi", &file, epoch, false)
            .unwrap_err()
            .contains("UTF-8"));
    }
    assert_eq!(
        conn.query_row("SELECT count(*) FROM session", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    let unicode = dir.path().join("résumé-🦀.jsonl");
    fs::write(&unicode, legacy("alpha")).unwrap();
    assert_eq!(
        with_cancellation(Arc::new(AtomicBool::new(false)), || catalog
            .import_with_policy(&mut conn, "pi", &unicode, epoch, false))
        .unwrap(),
        1
    );
    assert_eq!(body(&conn), "alpha");
}

#[test]
fn a_failed_sqlite_source_keeps_completed_batches_but_never_publishes_a_fresh_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("state.db");
    let source = Connection::open(&file).unwrap();
    source.execute_batch("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0;CREATE TABLE sessions(id TEXT PRIMARY KEY,started_at REAL,last_activity_at REAL);CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id TEXT,role TEXT,content TEXT,timestamp REAL,tool_calls TEXT);").unwrap();
    for id in 1..=17 {
        source
            .execute(
                "INSERT INTO sessions VALUES(?,?,?)",
                params![id.to_string(), id, id],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO messages VALUES(?,?,'user','recorded',1,?)",
                params![
                    id,
                    id.to_string(),
                    if id == 17 { "invalid JSON" } else { "[]" }
                ],
            )
            .unwrap();
    }
    let uri = native_source_uri("hermes", &file).unwrap();
    let mut conn = open(dir.path());
    assert!(import(&mut conn, &uri)
        .unwrap_err()
        .contains("Invalid JSON"));
    let completed = conn
        .query_row("SELECT count(*) FROM session", [], |r| r.get::<_, i64>(0))
        .unwrap();
    assert!(completed > 0 && completed < 17);
    assert!(
        hession_core::indexing::full_source(&conn, &uri, "hermes:full-source-v1")
            .unwrap()
            .is_none()
    );
    source
        .execute("UPDATE messages SET tool_calls='[]' WHERE id=17", [])
        .unwrap();
    assert_eq!(import(&mut conn, &uri).unwrap(), 17);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM session", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        17
    );
    assert!(
        hession_core::indexing::full_source(&conn, &uri, "hermes:full-source-v1")
            .unwrap()
            .is_some()
    );
    let revision = hession_core::library_revision::current(&conn).unwrap();
    assert_eq!(import(&mut conn, &uri).unwrap(), 0);
    assert_eq!(
        hession_core::library_revision::current(&conn).unwrap(),
        revision
    );
}

#[test]
fn antigravity_summary_companions_are_rechecked_and_oversized_metadata_preserves_history() {
    let dir = tempfile::tempdir().unwrap();
    let conversation = dir.path().join("brain/conversation");
    fs::create_dir_all(&conversation).unwrap();
    let file = conversation.join("trace.jsonl");
    fs::write(
        &file,
        json!({"type":"USER_INPUT","content":"alpha","step_index":0}).to_string(),
    )
    .unwrap();
    let database = dir.path().join("conversation_summaries.db");
    let source = Connection::open(&database).unwrap();
    source.execute_batch("PRAGMA journal_mode=WAL;PRAGMA wal_autocheckpoint=0;CREATE TABLE conversation_summaries(conversation_id TEXT PRIMARY KEY,title TEXT,workspace_uris TEXT);INSERT INTO conversation_summaries VALUES('conversation','first','[]');").unwrap();
    let uri = native_source_uri("antigravity", &file).unwrap();
    let mut conn = open(dir.path());
    assert_eq!(import(&mut conn, &uri).unwrap(), 1);
    assert_eq!(body(&conn), "alpha");
    let title = |conn: &Connection| {
        conn.query_row("SELECT title FROM session", [], |r| r.get::<_, String>(0))
            .unwrap()
    };
    assert_eq!(title(&conn), "first");
    source
        .execute("UPDATE conversation_summaries SET title='other'", [])
        .unwrap();
    assert_eq!(import(&mut conn, &uri).unwrap(), 1);
    assert_eq!(title(&conn), "other");
    let revision = hession_core::library_revision::current(&conn).unwrap();
    assert_eq!(import(&mut conn, &uri).unwrap(), 0);
    assert_eq!(
        hession_core::library_revision::current(&conn).unwrap(),
        revision
    );
    source
        .execute_batch(
            "UPDATE conversation_summaries SET workspace_uris=CAST(zeroblob(8388609) AS TEXT)",
        )
        .unwrap();
    assert!(import(&mut conn, &uri).unwrap_err().contains("8 MiB"));
    assert_eq!(title(&conn), "other");
    assert_eq!(body(&conn), "alpha");
}

#[test]
fn remaining_legacy_adapters_import_and_reopen_without_rebuilding_quiet_history() {
    for family in ["commandcode", "muse", "cline", "opencode"] {
        let dir = tempfile::tempdir().unwrap();
        let file = if family == "cline" {
            let task = dir.path().join("tasks/task");
            fs::create_dir_all(&task).unwrap();
            let file = task.join("api_conversation_history.json");
            fs::write(
                &file,
                json!([{"role":"user","content":[{"type":"text","text":"alpha"}]}]).to_string(),
            )
            .unwrap();
            file
        } else if family == "opencode" {
            let file = dir.path().join("opencode.db");
            let source = Connection::open(&file).unwrap();
            source.execute_batch("CREATE TABLE session(id TEXT PRIMARY KEY,time_created INTEGER,time_updated INTEGER);CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT);CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,time_created INTEGER,data TEXT);INSERT INTO session VALUES('one',1000,1000);INSERT INTO message VALUES('message','one',1000,'{\"role\":\"user\",\"time\":{\"created\":1000}}');INSERT INTO part VALUES('part','message',1000,'{\"type\":\"text\",\"text\":\"alpha\"}');").unwrap();
            file
        } else {
            let file = dir.path().join("session.jsonl");
            let record = if family == "muse" {
                json!({"type":"message","payload":{"role":"user","content":"alpha"}}).to_string()
            } else {
                legacy("alpha")
            };
            fs::write(&file, record).unwrap();
            file
        };
        let original =
            hession_core::source::SourceSnapshot::capture(&source_paths(family, &file), &|| false)
                .unwrap();
        let uri = native_source_uri(family, &file).unwrap();
        let mut conn = open(dir.path());
        assert_eq!(import(&mut conn, &uri).unwrap(), 1, "{family}");
        assert_eq!(body(&conn), "alpha", "{family}");
        let revision = hession_core::library_revision::current(&conn).unwrap();
        drop(conn);
        let mut conn = open(dir.path());
        assert_eq!(import(&mut conn, &uri).unwrap(), 0, "{family}");
        assert_eq!(
            hession_core::library_revision::current(&conn).unwrap(),
            revision,
            "{family}"
        );
        assert_eq!(
            hession_core::source::SourceSnapshot::capture(&source_paths(family, &file), &|| false)
                .unwrap(),
            original,
            "{family}"
        );
    }
}
