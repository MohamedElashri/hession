use hession_core::{db, indexing, session_changes};
use rusqlite::Connection;

fn fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
        INSERT INTO workspace(id,path) VALUES(1,'/synthetic');
        INSERT INTO session(id,agent_id,workspace_id,source_uri) VALUES(1,1,1,'fixture://1'),(2,1,1,'fixture://2');
        INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(1,1,0,'message','user','one'),(2,2,0,'message','user','two');
        INSERT INTO content_part(id,entry_id,ordinal,kind,text) VALUES(1,1,0,'text','one'),(2,2,0,'text','two');").unwrap();
    conn
}
fn token(conn: &Connection, id: i64) -> String {
    session_changes::token(conn, id).expect("trusted read token")
}

#[test]
fn tokens_follow_every_detail_mutation_without_invalidating_unrelated_sessions() {
    let mut conn = fixture();
    let other = token(&conn, 2);
    let mut previous = token(&conn, 1);
    for sql in [
        "UPDATE session SET title='changed' WHERE id=1",
        "UPDATE content_part SET text='content-part-only edit 🌍' WHERE id=1",
        "UPDATE entry SET search_text='changed' WHERE id=1",
        "INSERT INTO session_annotation(session_id,starred) VALUES(1,1)",
        "INSERT INTO session_tag(session_id,label) VALUES(1,'tag')",
        "INSERT INTO session_context(session_id,entry_id,branch) VALUES(1,1,'main')",
        "INSERT INTO tool_invocation(session_id,entry_id,tool_name,status) VALUES(1,1,'Read','invoked')",
        "UPDATE tool_invocation SET status='error',exit_code=1 WHERE session_id=1",
        "INSERT INTO file_activity(session_id,entry_id,path,action) VALUES(1,1,'a.rs','read')",
        "DELETE FROM content_part WHERE id=1",
    ] {
        conn.execute_batch(sql).unwrap();let next=token(&conn,1);assert_ne!(previous,next,"{sql}");previous=next;assert_eq!(token(&conn,2),other);
    }
    let tx = conn.transaction().unwrap();
    tx.execute("UPDATE entry SET search_text='rolled back' WHERE id=1", [])
        .unwrap();
    assert_ne!(token(&tx, 1), previous);
    tx.rollback().unwrap();
    assert_eq!(token(&conn, 1), previous);
    let before = token(&conn, 2);
    conn.execute("UPDATE agent SET product='Renamed' WHERE id=1", [])
        .unwrap();
    assert_ne!(token(&conn, 2), before);
}

#[test]
fn deletion_reuse_maintenance_and_missing_tracking_cannot_reuse_old_content() {
    let conn = fixture();
    let first = token(&conn, 1);
    conn.execute("DELETE FROM session WHERE id=1", []).unwrap();
    assert!(session_changes::token(&conn, 1).is_none());
    conn.execute(
        "INSERT INTO session(id,agent_id,source_uri) VALUES(1,1,'fixture://replacement')",
        [],
    )
    .unwrap();
    assert_ne!(token(&conn, 1), first);
    let before = token(&conn, 1);
    indexing::clear_local_state(&conn).unwrap();
    let after = token(&conn, 1);
    assert_ne!(before, after);
    conn.execute(
        "UPDATE session SET title='after maintenance' WHERE id=1",
        [],
    )
    .unwrap();
    assert_ne!(token(&conn, 1), after);
    conn.execute_batch("DROP TRIGGER index_read_content_part_update")
        .unwrap();
    assert!(session_changes::token(&conn, 1).is_none());
}

#[test]
fn overflow_and_corrupt_derived_nonce_recover_without_rewriting_history() {
    let conn = fixture();
    conn.execute(
        "UPDATE index_read_revision SET generation=9223372036854775807 WHERE session_id=1",
        [],
    )
    .unwrap();
    let before = token(&conn, 1);
    conn.execute("UPDATE entry SET search_text='new' WHERE id=1", [])
        .unwrap();
    assert_ne!(token(&conn, 1), before);
    conn.execute(
        "UPDATE index_read_revision SET nonce=CAST(zeroblob(1048576) AS TEXT) WHERE session_id=1",
        [],
    )
    .unwrap();
    assert!(session_changes::token(&conn, 1).is_none());
    conn.execute(
        "UPDATE session SET title='repair nonce on next edit' WHERE id=1",
        [],
    )
    .unwrap();
    assert!(session_changes::token(&conn, 1).is_some());
}
