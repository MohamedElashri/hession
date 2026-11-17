use hession_core::{
    db,
    search::{self, SearchError, SearchMode, SearchRequest, SearchScope, SearchVisibility},
    service::Library,
};
use rusqlite::{params, Connection};
fn seed(conn: &Connection) {
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');INSERT INTO workspace(id,path) VALUES(1,'/synthetic/allowed'),(2,'/synthetic/denied');INSERT INTO session(id,agent_id,workspace_id,source_uri,title) VALUES(1,1,1,'fixture:1','Normal history'),(2,1,2,'fixture:2','Denied history');").unwrap();
}
#[test]
fn ordinary_transcript_corpus_over_sixteen_mib_remains_searchable_and_pageable() {
    // In-memory storage isolates query correctness from the other test's large
    // on-disk fixture writes; the production execution deadline stays intact.
    let mut conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');INSERT INTO workspace(id,path) VALUES(1,'/synthetic/project');").unwrap();
    let text = format!(
        "needle::Type normal conversation {}",
        "ordinary recorded sentence ".repeat(800)
    );
    for id in 1..=1000_i64 {
        tx.execute("INSERT INTO session(id,agent_id,workspace_id,source_uri,title) VALUES(?,1,1,?,'Normal history')",params![id,format!("fixture:{id}")]).unwrap();
        tx.execute("INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(?,?,0,'message','assistant',?)",params![id,id,text]).unwrap();
        tx.execute(
            "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'text',?)",
            params![id, text],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let bytes: i64 = conn
        .query_row(
            "SELECT sum(octet_length(text)) FROM content_part",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(bytes > 16 * 1024 * 1024);
    let scope = SearchScope::workspaces([1]).unwrap();
    for visibility in [
        SearchVisibility::Conversation,
        SearchVisibility::AllIndexedText,
    ] {
        let mut request = SearchRequest {
            query: "needle::Type".into(),
            visibility,
            ..Default::default()
        };
        let first = search::search(&mut conn, &request, &scope).unwrap();
        assert_eq!(first.results.len(), 20);
        assert!(first.results.iter().all(|r| r
            .excerpt
            .as_deref()
            .unwrap()
            .contains("needle::Type")));
        request.cursor = first.next_cursor;
        let second = search::search(&mut conn, &request, &scope).unwrap();
        assert_eq!(second.results.len(), 20);
        assert!(first
            .results
            .iter()
            .all(|a| second.results.iter().all(|b| a.session_id != b.session_id)));
    }
    let metadata = search::search(
        &mut conn,
        &SearchRequest {
            query: "Normal".into(),
            ..Default::default()
        },
        &scope,
    )
    .unwrap();
    assert_eq!(metadata.results.len(), 20);
    assert!(metadata.results.iter().all(|r| r.excerpt.is_none()));
}
#[test]
fn actual_materialized_hits_are_bounded_and_failed_budget_does_not_poison_next_query() {
    let mut conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    seed(&conn);
    conn.execute_batch("BEGIN;WITH RECURSIVE n(id) AS(VALUES(1) UNION ALL SELECT id+1 FROM n WHERE id<6000) INSERT INTO entry(session_id,ordinal,kind,role,search_text) SELECT 1,id,'message','assistant','counteralpha' FROM n;INSERT INTO entry(session_id,ordinal,kind,role,search_text) VALUES(2,0,'message','assistant','counteralpha');COMMIT;").unwrap();
    let mut request = SearchRequest {
        query: vec!["counteralpha"; 16].join(" "),
        visibility: SearchVisibility::AllIndexedText,
        ..Default::default()
    };
    assert!(matches!(
        search::search(&mut conn, &request, &SearchScope::workspaces([1]).unwrap()),
        Err(SearchError::InputTooLarge)
    ));
    assert_eq!(
        search::search(&mut conn, &request, &SearchScope::workspaces([2]).unwrap())
            .unwrap()
            .results[0]
            .session_id,
        2
    );
    assert!(
        search::search(&mut conn, &request, &SearchScope::workspaces([]).unwrap())
            .unwrap()
            .results
            .is_empty()
    );
    request.query = "counteralpha".into();
    assert_eq!(
        search::search(&mut conn, &request, &SearchScope::workspaces([1]).unwrap())
            .unwrap()
            .results[0]
            .session_id,
        1
    );
}
#[test]
fn unused_metadata_is_not_scanned_but_queried_metadata_is_size_checked() {
    let mut conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    seed(&conn);
    conn.execute_batch("INSERT INTO tool_invocation(session_id,tool_name,status) VALUES(1,CAST(zeroblob(65537) AS TEXT),CAST(zeroblob(65537) AS TEXT));INSERT INTO session_context(session_id,branch) VALUES(1,CAST(zeroblob(65537) AS TEXT));").unwrap();
    let scope = SearchScope::workspaces([1]).unwrap();
    assert!(search::search(
        &mut conn,
        &SearchRequest {
            query: "Normal".into(),
            ..Default::default()
        },
        &scope
    )
    .is_ok());
    for query in ["tool:shell", "has:errors", "branch:main"] {
        assert!(matches!(
            search::search(
                &mut conn,
                &SearchRequest {
                    query: query.into(),
                    ..Default::default()
                },
                &scope
            ),
            Err(SearchError::InputTooLarge)
        ));
    }
}
#[test]
fn large_scoped_cells_are_rejected_without_loading_denied_or_hidden_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.sqlite");
    let conn = db::open_db(&path).unwrap();
    seed(&conn);
    conn.execute_batch("BEGIN;INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(1,1,0,'message','assistant','needle'),(2,2,0,'message',CAST(zeroblob(67108864) AS TEXT),CAST(zeroblob(67108864) AS TEXT)),(3,1,1,'reasoning','assistant','excluded reasoning');INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(1,0,'text','needle'),(2,0,'text','denied text'),(3,0,'text',CAST(zeroblob(67108864) AS TEXT));COMMIT;").unwrap();
    conn.execute_batch("INSERT INTO file_activity(session_id,path,action) VALUES(1,'src/allowed.rs','read'),(2,CAST(zeroblob(67108864) AS TEXT),'read');").unwrap();
    conn.execute("UPDATE session SET title=? WHERE id=1", ["t".repeat(60000)])
        .unwrap();
    conn.execute_batch("BEGIN;WITH RECURSIVE n(id) AS(VALUES(0) UNION ALL SELECT id+1 FROM n WHERE id<9999) INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) SELECT id+10,1,id+2,'message','assistant','regular' FROM n;COMMIT;").unwrap();
    drop(conn);
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "search_bounds_reader_child",
            "--nocapture",
        ])
        .env("HESSION_SEARCH_BOUNDS_FIXTURE", &path)
        .output()
        .unwrap();
    println!("{}", String::from_utf8_lossy(&output.stdout));
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
#[ignore = "fresh-process reader invoked by large_scoped_cells_are_rejected_without_loading_denied_or_hidden_bodies"]
fn search_bounds_reader_child() {
    let library = Library::new(std::env::var_os("HESSION_SEARCH_BOUNDS_FIXTURE").unwrap());
    for visibility in [
        SearchVisibility::Conversation,
        SearchVisibility::AllIndexedText,
    ] {
        let request = SearchRequest {
            query: "needle".into(),
            visibility,
            ..Default::default()
        };
        let result = library.search(&request, &SearchScope::workspaces([1]).unwrap());
        if result.is_err() {
            eprintln!(
                "{visibility:?} allowed search error: {:?}\n{}",
                result,
                std::fs::read_to_string("/proc/self/status").unwrap_or_default()
            );
        }
        assert_eq!(result.unwrap().results.len(), 1);
        assert!(matches!(
            library.search(&request, &SearchScope::workspaces([2]).unwrap()),
            Err(SearchError::InputTooLarge)
        ));
    }
    // Same immutable startup policy used by the MCP dispatcher. Its search
    // preflight must not reintroduce temporary copies before the shared fence.
    let cancel = hession_core::reader::ReadCancellation::default();
    let reader = hession_core::reader::ScopedReader::projects(
        library.clone(),
        &[hession_core::projects::ProjectSelector::WorkspaceId(1)],
        true,
    )
    .unwrap();
    let page = reader
        .search(
            &SearchRequest {
                query: "needle".into(),
                visibility: SearchVisibility::AllIndexedText,
                ..Default::default()
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(page.results.len(), 1);
    assert!(reader
        .search(
            &SearchRequest {
                query: "secret".into(),
                ..Default::default()
            },
            &cancel
        )
        .unwrap()
        .results
        .is_empty());
    assert!(reader
        .session(1, &hession_core::reader::ReadPage::default(), &cancel)
        .is_ok());
    assert!(reader
        .tool_calls(1, &hession_core::reader::ReadPage::default(), &cancel)
        .is_ok());
    assert!(reader
        .session(2, &hession_core::reader::ReadPage::default(), &cancel)
        .is_err());
    assert!(reader
        .project_activity(1, None, &hession_core::reader::ReadPage::default(), &cancel)
        .is_ok());
    assert!(reader
        .project_files(1, None, &hession_core::reader::ReadPage::default(), &cancel)
        .is_ok());
    let mut conn = db::open_read_only(library.path()).unwrap();
    assert!(matches!(
        hession_core::projects::files_on_connection(
            &conn,
            2,
            &SearchScope::workspaces([2]).unwrap(),
            hession_core::service::Page::default(),
            None
        ),
        Err(hession_core::projects::ProjectError::InvalidPath)
    ));
    conn.execute_batch("PRAGMA temp_store=MEMORY").unwrap();
    let mut request = SearchRequest {
        mode: SearchMode::Messages,
        visibility: SearchVisibility::AllIndexedText,
        ..Default::default()
    };
    let first =
        search::search(&mut conn, &request, &SearchScope::workspaces([1]).unwrap()).unwrap();
    let mut cursor: serde_json::Value =
        serde_json::from_str(first.next_cursor.as_ref().unwrap()).unwrap();
    cursor["offset"] = serde_json::json!(9800);
    request.cursor = Some(cursor.to_string());
    let page = search::search(&mut conn, &request, &SearchScope::workspaces([1]).unwrap()).unwrap();
    assert_eq!(page.results.len(), 20);
    assert_eq!(page.results[0].ordinal, Some(9800));
    cursor["offset"] = serde_json::json!(200000);
    request.cursor = Some(cursor.to_string());
    assert!(matches!(
        search::search(&mut conn, &request, &SearchScope::workspaces([1]).unwrap()),
        Err(SearchError::InputTooLarge)
    ));
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let high = status
            .lines()
            .find_map(|s| s.strip_prefix("VmHWM:"))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        println!("search reader peak KiB: {high}");
        assert!(
            high < 48 * 1024,
            "materialized an oversized cell: {high}KiB"
        );
    }
}

#[test]
fn project_descriptor_guards_metadata_before_path_hashing_and_native_source_aggregation() {
    use hession_core::projects::{project_on_connection, ProjectError};
    use hession_core::service::ServiceError;
    let conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    seed(&conn);
    let scope = SearchScope::workspaces([1]).unwrap();
    conn.execute_batch("UPDATE workspace SET display_name=CAST(zeroblob(1048576) AS TEXT) WHERE id=2;UPDATE session SET host=CAST(zeroblob(1048576) AS TEXT) WHERE id=2;").unwrap();
    assert!(project_on_connection(&conn, 1, &scope).is_ok());
    assert!(matches!(
        project_on_connection(&conn, 2, &scope),
        Err(ProjectError::OutsideScope)
    ));
    conn.execute(
        "UPDATE session SET source_uri=CAST(zeroblob(1048576) AS TEXT) WHERE id=1",
        [],
    )
    .unwrap();
    assert!(matches!(
        project_on_connection(&conn, 1, &scope),
        Err(ProjectError::Service(ServiceError::ResponseTooLarge))
    ));
    conn.execute_batch("UPDATE session SET source_uri='fixture:1',started_at=CAST(zeroblob(1048576) AS TEXT) WHERE id=1;").unwrap();
    assert!(matches!(
        project_on_connection(&conn, 1, &scope),
        Err(ProjectError::Service(ServiceError::ResponseTooLarge))
    ));
    conn.execute_batch(
        "UPDATE session SET started_at=NULL,title=CAST(zeroblob(1048576) AS TEXT) WHERE id=1;",
    )
    .unwrap();
    assert!(project_on_connection(&conn, 1, &scope).is_ok()); // titles are not descriptor input
    conn.execute(
        "UPDATE workspace SET path=CAST(zeroblob(1048576) AS TEXT) WHERE id=1",
        [],
    )
    .unwrap();
    assert!(matches!(
        project_on_connection(&conn, 1, &scope),
        Err(ProjectError::InvalidPath)
    ));
}
