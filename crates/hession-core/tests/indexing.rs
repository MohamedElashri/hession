use hession_core::{
    db,
    indexing::{self, DeltaCursor},
    service::Library,
};
use rusqlite::{params, Connection};
fn fixture() -> (tempfile::TempDir, Connection, Library) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let conn = db::open_db(&path).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex'); INSERT INTO session(id,agent_id,source_uri) VALUES(1,1,'codex:///fixture');").unwrap();
    let library = Library::new(path);
    (dir, conn, library)
}
fn insert(conn: &Connection, ordinal: i64) -> i64 {
    conn.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text,payload_json) VALUES(1,?,'message','assistant','hello','{}')",[ordinal]).unwrap();
    conn.last_insert_rowid()
}
fn cursor(value: &serde_json::Value) -> DeltaCursor {
    serde_json::from_value(value["cursor"].clone()).unwrap()
}

#[test]
fn durable_delta_follows_gapped_entries_and_updates_earlier_tool_results() {
    let (_dir, conn, library) = fixture();
    let first = insert(&conn, 100);
    let initial = library.session_delta(1, None, 10).unwrap();
    assert_eq!(initial["reset"], true);
    assert_eq!(initial["entries"].as_array().unwrap().len(), 1);
    conn.execute("INSERT INTO tool_invocation(session_id,entry_id,call_id,tool_name,status) VALUES(1,?,'late','read','invoked')",[first]).unwrap();
    let second = insert(&conn, 9000);
    let delta = library
        .session_delta(1, Some(cursor(&initial)), 10)
        .unwrap();
    assert_eq!(delta["reset"], false);
    assert_eq!(delta["entries"][0]["id"], first);
    assert_eq!(delta["entries"][0]["position"], 0);
    assert_eq!(delta["entries"][1]["id"], second);
    assert_eq!(delta["entries"][1]["position"], 1);
    conn.execute("UPDATE tool_invocation SET result_text='late result',status='completed' WHERE call_id='late'",[]).unwrap();
    let late = library.session_delta(1, Some(cursor(&delta)), 10).unwrap();
    assert_eq!(late["entries"].as_array().unwrap().len(), 1);
    assert_eq!(late["entries"][0]["tools"][0]["result_text"], "late result");
    assert!(
        library.session_delta(1, Some(cursor(&late)), 10).unwrap()["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    indexing::maintenance(&conn).unwrap();
    let reset = library.session_delta(1, Some(cursor(&late)), 1).unwrap();
    assert_eq!(reset["reset"], true);
    assert_eq!(reset["entries"][0]["id"], second);
    conn.execute("DELETE FROM session WHERE id=1", []).unwrap();
    let deleted = library.session_delta(1, Some(cursor(&reset)), 10).unwrap();
    assert_eq!(deleted["deleted"], true);
    assert_eq!(deleted["reset"], true);
}

#[test]
fn delta_retention_scope_and_payload_budgets_are_explicit() {
    let (_dir, mut conn, library) = fixture();
    insert(&conn, 0);
    let first = library.session_delta(1, None, 1).unwrap();
    let tx = conn.transaction().unwrap();
    for n in 1..10_005 {
        tx.execute(
            "INSERT INTO index_change(session_id,entry_id) VALUES(2,?)",
            [n],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM index_change", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        10_000
    );
    assert_eq!(
        library.session_delta(1, Some(cursor(&first)), 10).unwrap()["reset"],
        true
    );
    let mut wrong = cursor(&first);
    wrong.session_id = 2;
    assert_eq!(
        library.session_delta(1, Some(wrong), 1).unwrap()["reset"],
        true
    );
    conn.execute(
        "UPDATE entry SET payload_json=?",
        ["x".repeat(1024 * 1024 + 1)],
    )
    .unwrap();
    let oversized = library.session_delta(1, None, 1).unwrap();
    assert!(oversized["entries"].as_array().unwrap().is_empty());
    assert_eq!(
        oversized["oversizedEntries"][0]["reason"],
        "payload_too_large"
    );
    assert!(library
        .session_delta(1, Some(cursor(&oversized)), 1)
        .unwrap()["oversizedEntries"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(library.session_delta(1, None, 101).is_err());
}

#[test]
fn derived_state_is_not_revisioned_or_exported_and_maintenance_invalidates_jobs() {
    let (_dir, conn, _library) = fixture();
    let before = hession_core::library_revision::current(&conn).unwrap();
    let old = indexing::epoch(&conn).unwrap();
    indexing::exclude(&conn, "codex:///private/source").unwrap();
    assert!(indexing::excluded(&conn, "codex:///private/source").unwrap());
    let stored: String = conn
        .query_row("SELECT source_hash FROM index_exclusion", [], |r| r.get(0))
        .unwrap();
    assert!(!stored.contains("private"));
    assert_eq!(stored.len(), 64);
    indexing::save_checkpoint(
        &conn,
        "codex:///private/source",
        1,
        "{}",
        "{\"private\":true}",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO index_result VALUES(1,'call',?,'now')",
        params!["private transcript"],
    )
    .unwrap();
    assert_eq!(
        hession_core::library_revision::current(&conn).unwrap(),
        before
    );
    indexing::clear_local_state(&conn).unwrap();
    assert!(indexing::check_epoch(&conn, old).is_err());
    for table in indexing::LOCAL_TABLES
        .iter()
        .filter(|t| **t != "index_maintenance")
    {
        assert_eq!(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
    assert_eq!(
        hession_core::library_revision::current(&conn).unwrap(),
        before
    );
}
