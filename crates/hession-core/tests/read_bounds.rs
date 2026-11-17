use hession_core::{
    db,
    live::RecentRequest,
    search::{self, SearchError, SearchRequest, SearchScope, SearchVisibility},
    service::{Library, Page, ServiceError},
    text_read::{read_prefix, TextColumn, TextReadError},
};
use rusqlite::Connection;

fn seed(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
        INSERT INTO workspace(id,path) VALUES(1,'/synthetic/allowed'),(2,'/synthetic/denied');
        INSERT INTO session(id,agent_id,workspace_id,source_uri,title,last_event_at) VALUES
        (1,1,1,'codex:///synthetic/missing','Allowed','2026-01-01'),
        (2,1,2,'codex:///synthetic/denied','Denied','2026-01-01');
        INSERT INTO entry(id,session_id,ordinal,kind,role,search_text,payload_json) VALUES
        (1,1,9000,'message','assistant','needle','{}'),(2,2,0,'message','assistant','secret','{}');
        INSERT INTO content_part(id,entry_id,ordinal,kind,text) VALUES
        (1,1,0,'text','needle café 🦀'),(2,2,0,'text','secret');",
    )
    .unwrap();
}

#[test]
fn prefixes_preserve_utf8_and_bound_both_bytes_and_characters() {
    let conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    seed(&conn);
    let full = read_prefix(&conn, TextColumn::ContentPartText, 1, 100, 100).unwrap();
    assert_eq!(full.text, "needle café 🦀");
    assert!(!full.truncated);
    let prefix = read_prefix(&conn, TextColumn::ContentPartText, 1, 11, 100).unwrap();
    assert_eq!(prefix.text, "needle caf"); // byte cap splits é
    assert!(prefix.truncated);
    assert_eq!(
        read_prefix(&conn, TextColumn::ContentPartText, 1, 100, 3)
            .unwrap()
            .text,
        "nee"
    );
    assert_eq!(
        read_prefix(&conn, TextColumn::EntrySearchText, 1, 0, 0)
            .unwrap()
            .text,
        ""
    );
    assert!(matches!(
        read_prefix(&conn, TextColumn::ContentPartText, 1, 65537, 1),
        Err(TextReadError::InvalidLimit)
    ));
    conn.execute(
        "UPDATE content_part SET text=CAST(x'ff' AS TEXT) WHERE id=1",
        [],
    )
    .unwrap();
    assert!(matches!(
        read_prefix(&conn, TextColumn::ContentPartText, 1, 100, 100),
        Err(TextReadError::InvalidUtf8)
    ));
}

#[test]
fn search_size_guards_respect_scope_and_conversation_policy() {
    let mut conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    seed(&conn);
    conn.execute(
        "UPDATE content_part SET text=CAST(zeroblob(1048577) AS TEXT) WHERE id=2",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE session SET title=CAST(zeroblob(65537) AS TEXT) WHERE id=2",
        [],
    )
    .unwrap();
    let scope = SearchScope::workspaces([1]).unwrap();
    let mut request = SearchRequest {
        query: "needle".into(),
        visibility: SearchVisibility::Conversation,
        ..Default::default()
    };
    assert_eq!(
        search::search(&mut conn, &request, &scope)
            .unwrap()
            .results
            .len(),
        1
    );
    assert!(
        search::search(&mut conn, &request, &SearchScope::workspaces([]).unwrap())
            .unwrap()
            .results
            .is_empty()
    );
    assert!(matches!(
        search::search(&mut conn, &request, &SearchScope::all()),
        Err(SearchError::InputTooLarge)
    ));
    conn.execute("UPDATE entry SET kind='reasoning' WHERE id=1", [])
        .unwrap();
    conn.execute(
        "UPDATE content_part SET text=CAST(zeroblob(1048577) AS TEXT) WHERE id=1",
        [],
    )
    .unwrap();
    assert!(search::search(&mut conn, &request, &scope)
        .unwrap()
        .results
        .is_empty());
    conn.execute("UPDATE entry SET kind='message' WHERE id=1", [])
        .unwrap();
    assert!(matches!(
        search::search(&mut conn, &request, &scope),
        Err(SearchError::InputTooLarge)
    ));
    request.visibility = SearchVisibility::MetadataOnly;
    assert!(search::search(&mut conn, &request, &scope).is_ok());
    conn.execute("UPDATE file_activity SET path=''", [])
        .unwrap();
    conn.execute(
        "INSERT INTO session_tag(session_id,label) VALUES(1,CAST(zeroblob(65537) AS TEXT))",
        [],
    )
    .unwrap();
    assert!(matches!(
        search::search(&mut conn, &request, &scope),
        Err(SearchError::InputTooLarge)
    ));
    conn.execute("DELETE FROM session_tag WHERE session_id=1", [])
        .unwrap();
    conn.execute(
        "UPDATE entry SET search_text=CAST(zeroblob(1048577) AS TEXT) WHERE id=1",
        [],
    )
    .unwrap();
    request.visibility = SearchVisibility::AllIndexedText;
    assert!(matches!(
        search::search(&mut conn, &request, &scope),
        Err(SearchError::InputTooLarge)
    ));
}

#[test]
fn live_rejects_oversized_local_metadata_before_string_functions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.sqlite");
    let conn = db::open_db(&path).unwrap();
    seed(&conn);
    conn.execute(
        "UPDATE session SET host='remote',title=CAST(zeroblob(1048577) AS TEXT) WHERE id=2",
        [],
    )
    .unwrap();
    let library = Library::new(path);
    assert!(library
        .recent_local_sessions(&RecentRequest::default())
        .is_ok());
    conn.execute(
        "UPDATE session SET title=CAST(zeroblob(1048577) AS TEXT) WHERE id=1",
        [],
    )
    .unwrap();
    assert!(matches!(
        library.recent_local_sessions(&RecentRequest::default()),
        Err(ServiceError::ResponseTooLarge)
    ));
}

// A fresh child measures reader memory independently of the process that
// constructed the large fixture. Linux HWM assertions catch whole-cell loads;
// the same behavioral tests run on macOS without /proc.
#[test]
fn large_text_reads_stay_bounded_in_a_fresh_process() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.sqlite");
    let conn = db::open_db(&path).unwrap();
    seed(&conn);
    conn.execute(
        "UPDATE content_part SET text=CAST(zeroblob(67108864) AS TEXT) WHERE id=1",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE entry SET payload_json=CAST(zeroblob(67108864) AS TEXT) WHERE id=1",
        [],
    )
    .unwrap();
    drop(conn);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "large_text_reader_child",
            "--nocapture",
        ])
        .env("HESSION_READ_BOUND_FIXTURE", &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "fresh reader helper invoked by large_text_reads_stay_bounded_in_a_fresh_process"]
fn large_text_reader_child() {
    let path = std::env::var_os("HESSION_READ_BOUND_FIXTURE").expect("synthetic fixture path");
    let library = Library::new(path);
    assert!(matches!(
        library.session(1, Page::default()),
        Err(ServiceError::ResponseTooLarge)
    ));
    let delta = library.session_delta(1, None, 100).unwrap();
    assert!(delta["entries"].as_array().unwrap().is_empty());
    assert_eq!(delta["oversizedEntries"][0]["id"], 1);
    assert_eq!(delta["oversizedEntries"][0]["position"], 0);
    assert_eq!(delta["oversizedEntries"][0]["reason"], "payload_too_large");
    let cursor = serde_json::from_value(delta["cursor"].clone()).unwrap();
    assert!(
        library.session_delta(1, Some(cursor), 100).unwrap()["oversizedEntries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let live = library
        .recent_local_sessions(&RecentRequest::default())
        .unwrap();
    let session = live["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == 1)
        .unwrap();
    assert_eq!(
        session["latestEvent"]["summary"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        240
    );
    assert_eq!(session["latestEvent"]["summaryTruncated"], true);
    assert!(matches!(
        library.search(
            &SearchRequest {
                query: "needle".into(),
                visibility: SearchVisibility::Conversation,
                ..Default::default()
            },
            &SearchScope::workspaces([1]).unwrap()
        ),
        Err(SearchError::InputTooLarge)
    ));
    assert!(library
        .search(
            &SearchRequest::default(),
            &SearchScope::workspaces([1]).unwrap()
        )
        .is_ok());
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let high = status
            .lines()
            .find_map(|l| l.strip_prefix("VmHWM:"))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        println!("reader high-water KiB: {high}");
        assert!(high < 48 * 1024, "reader loaded oversized cells: {high}KiB");
    }
}

#[test]
fn aggregate_delta_budget_produces_explicit_markers_and_advances() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.sqlite");
    let conn = db::open_db(&path).unwrap();
    seed(&conn);
    for id in 3..=25 {
        conn.execute("INSERT INTO entry(id,session_id,ordinal,kind,role,payload_json) VALUES(?1,1,?1+10000,'message','assistant',CAST(zeroblob(16000) AS TEXT))",[id]).unwrap();
    }
    let library = Library::new(path);
    let delta = library.session_delta(1, None, 100).unwrap();
    let entries = delta["entries"].as_array().unwrap();
    let omitted = delta["oversizedEntries"].as_array().unwrap();
    assert_eq!(entries.len() + omitted.len(), 24);
    assert!(omitted.iter().any(|m| m["reason"] == "page_budget"));
    assert!(delta.to_string().len() <= hession_core::service::MAX_RESPONSE_BYTES);
    let cursor = serde_json::from_value(delta["cursor"].clone()).unwrap();
    let next = library.session_delta(1, Some(cursor), 100).unwrap();
    assert!(next["entries"].as_array().unwrap().is_empty());
    assert!(next["oversizedEntries"].as_array().unwrap().is_empty());
}
