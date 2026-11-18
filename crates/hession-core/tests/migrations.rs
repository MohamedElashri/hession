use hession_core::{
    db, library_revision,
    service::{Library, Page},
};
use rusqlite::{params, Connection};

fn legacy(conn: &Connection) {
    conn.execute_batch("PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;")
        .unwrap();
    conn.execute_batch(include_str!("fixtures/v0_4_schema.sql"))
        .unwrap();
    conn.execute_batch(r#"PRAGMA foreign_keys=ON;
        INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
        INSERT INTO workspace(id,path,display_name) VALUES(1,'/work/old','old');
        INSERT INTO session(id,agent_id,workspace_id,source_uri,title,host)
            VALUES(42,1,1,'test://preserved','Existing history','remote');
        INSERT INTO entry(id,session_id,ordinal,kind,role,search_text,payload_json)
            VALUES(88,42,0,'message','assistant','résumé needle','{"preserved":true}');
        INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(88,0,'text','Keep me');
        INSERT INTO tool_invocation(session_id,entry_id,tool_name,result_text) VALUES(42,88,'Read','Existing result');
        INSERT INTO session_annotation(session_id,starred) VALUES(42,1);
        INSERT INTO session_tag(session_id,label) VALUES(42,'release');
        PRAGMA user_version=1; COMMIT;"#).unwrap();
}

#[test]
fn classification_only_libraries_receive_catalog_indexes_without_reclassifying_history() {
    let conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    conn.execute_batch("DROP INDEX session_main_activity_idx; DROP INDEX session_main_workspace_activity_idx; DROP INDEX session_main_agent_activity_idx; DROP INDEX session_workspace_activity_idx; DROP INDEX session_agent_activity_idx; DROP INDEX context_model_session_idx; DROP INDEX context_branch_session_idx; UPDATE application_schema SET version=4;").unwrap();
    db::init_schema(&conn).unwrap();
    assert_eq!(db::schema_version(&conn).unwrap(), db::SCHEMA_VERSION);
    for name in [
        "session_main_activity_idx",
        "session_main_workspace_activity_idx",
        "session_main_agent_activity_idx",
        "session_workspace_activity_idx",
        "session_agent_activity_idx",
        "context_model_session_idx",
        "context_branch_session_idx",
    ] {
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE type='index' AND name=?",
                [name],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
}

#[test]
fn background_classification_migrates_existing_library_without_deleting_sessions() {
    let conn = Connection::open_in_memory().unwrap();
    legacy(&conn);
    conn.execute_batch(r#"
        INSERT INTO agent(id,family,product) VALUES(2,'claude','Claude Code'),(3,'opencode','OpenCode');
        INSERT INTO session(id,agent_id,source_uri,raw_metadata_json) VALUES
        (43,1,'codex:///guardian','{"source":{"subagent":{"other":"guardian"}}}'),
        (44,2,'claude:///parent/subagents/agent-child.jsonl','{}'),
        (45,3,'opencode:///db#child','{"parent_id":"parent"}'),
        (46,1,'codex:///main','malformed legacy metadata');
    "#).unwrap();
    db::init_schema(&conn).unwrap();
    let classified: Vec<(i64, bool)> = conn
        .prepare("SELECT id,is_subagent FROM session ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        classified,
        vec![(42, false), (43, true), (44, true), (45, true), (46, false)]
    );
    assert_eq!(
        conn.query_row(
            "SELECT starred FROM session_annotation WHERE session_id=42",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let revision = library_revision::current(&conn).unwrap();
    db::init_schema(&conn).unwrap();
    assert_eq!(library_revision::current(&conn).unwrap(), revision);
}

#[test]
fn legacy_migration_preserves_history_ids_annotations_fts_and_backup_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let conn = Connection::open(&path).unwrap();
    legacy(&conn);
    db::init_schema(&conn).unwrap();
    let revision = library_revision::current(&conn).unwrap();
    db::init_schema(&conn).unwrap();
    assert_eq!(library_revision::current(&conn).unwrap(), revision);
    assert_eq!(db::schema_version(&conn).unwrap(), db::SCHEMA_VERSION);
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT rowid FROM entry_fts WHERE entry_fts MATCH 'resume'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        88
    );
    let result = Library::new(&path).session(42, Page::default()).unwrap();
    assert_eq!(result["session"]["host"], "remote");
    assert_eq!(result["session"]["starred"], true);
    assert_eq!(result["session"]["tags"][0], "release");
    assert_eq!(result["entries"][0]["id"], 88);
    assert_eq!(result["entries"][0]["parts"][0]["text"], "Keep me");
    assert_eq!(
        result["entries"][0]["tools"][0]["result_text"],
        "Existing result"
    );
    assert_eq!(
        conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    // SQLite backup is still a coherent copy after migration.
    let copy = dir.path().join("backup.sqlite");
    conn.backup(
        rusqlite::MAIN_DB,
        &copy,
        None::<fn(rusqlite::backup::Progress)>,
    )
    .unwrap();
    let copy = db::open_db(&copy).unwrap();
    assert_eq!(library_revision::current(&copy).unwrap(), revision);
    assert_eq!(
        copy.query_row("SELECT title FROM session WHERE id=42", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Existing history"
    );
}

#[test]
fn migration_failure_rolls_back_schema_and_leaves_legacy_rows_readable() {
    let conn = Connection::open_in_memory().unwrap();
    legacy(&conn);
    // Force DDL failure partway through initialization.
    conn.execute_batch("DROP TABLE sync_checkpoint; DROP INDEX session_activity_idx; CREATE TABLE session_activity_idx(x);")
        .unwrap();
    assert!(db::init_schema(&conn).is_err());
    assert_eq!(db::schema_version(&conn).unwrap(), 0);
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name='sync_checkpoint'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "DDL applied before the failure must roll back with the migration"
    );
    assert_eq!(
        conn.query_row("SELECT title FROM session WHERE id=42", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "Existing history"
    );
    conn.execute_batch("DROP TABLE session_activity_idx")
        .unwrap();
    db::init_schema(&conn).unwrap();
    assert_eq!(db::schema_version(&conn).unwrap(), db::SCHEMA_VERSION);
}

#[test]
fn future_and_corrupt_versions_fail_without_changing_data() {
    for version in [db::SCHEMA_VERSION + 1, -1] {
        let conn = Connection::open_in_memory().unwrap();
        legacy(&conn);
        conn.execute_batch("CREATE TABLE application_schema(id,version)")
            .unwrap();
        conn.execute("INSERT INTO application_schema VALUES(1,?)", [version])
            .unwrap();
        assert!(db::init_schema(&conn).is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM session", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT version FROM application_schema", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            version
        );
    }
}

#[test]
fn readonly_queries_do_not_create_or_migrate_libraries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    assert!(db::open_read_only(&path).is_err());
    assert!(!path.exists());
    let conn = Connection::open(&path).unwrap();
    legacy(&conn);
    let before = std::fs::read(&path).unwrap();
    assert!(db::open_read_only(&path)
        .unwrap_err()
        .to_string()
        .contains("upgrade"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    db::init_schema(&conn).unwrap();
    let reader = db::open_read_only(&path).unwrap();
    assert!(reader.execute("DELETE FROM session", []).is_err());
    assert_eq!(
        library_revision::current(&reader).unwrap(),
        library_revision::current(&conn).unwrap()
    );
}

#[test]
fn page_and_response_bounds_are_enforced_at_the_shared_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions.sqlite");
    let conn = Connection::open(&path).unwrap();
    legacy(&conn);
    db::init_schema(&conn).unwrap();
    let library = Library::new(&path);
    for page in [
        Page {
            limit: 0,
            offset: 0,
        },
        Page {
            limit: 501,
            offset: 0,
        },
        Page {
            limit: 1,
            offset: u64::MAX,
        },
    ] {
        assert!(matches!(
            library.session(42, page),
            Err(hession_core::service::ServiceError::InvalidPage)
        ));
    }
    conn.execute(
        "UPDATE entry SET payload_json=? WHERE id=88",
        params!["x".repeat(1024 * 1024)],
    )
    .unwrap();
    assert!(matches!(
        library.session(42, Page::default()),
        Err(hession_core::service::ServiceError::ResponseTooLarge)
    ));
}
