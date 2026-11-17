use hession_core::{db, live::RecentRequest, service::Library};
use rusqlite::params;

#[test]
fn local_activity_is_passive_scoped_bounded_and_never_includes_remote_or_synced_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let native = dir.path().join("native.jsonl");
    std::fs::write(&native, b"{}\n").unwrap();
    let conn = db::open_db(&path).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex'); INSERT INTO workspace(id,path) VALUES(1,'/fixture/project');").unwrap();
    for (id, uri, host, time) in [
        (1, format!("codex://{}", native.display()), None, "now"),
        (2, "codex:///remote/source".into(), Some("server"), "now"),
        (3, "hession-sync://copied/revision".into(), None, "now"),
        (
            4,
            "codex:///missing/native.jsonl".into(),
            None,
            "2000-01-01",
        ),
    ] {
        conn.execute("INSERT INTO session(id,agent_id,workspace_id,source_uri,host,title,last_event_at) VALUES(?,1,1,?,?,?,datetime(?))",params![id,uri,host,"Recorded task",time]).unwrap();
    }
    conn.execute("INSERT INTO entry(id,session_id,ordinal,kind,role,occurred_at,search_text,payload_json) VALUES(1,1,8,'message','assistant',datetime('now'),'rawhidden','{\"secret\":\"hidden-payload\"}')",[]).unwrap();
    conn.execute(
        "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(1,0,'text',?)",
        ["Visible reply — café ".repeat(100)],
    )
    .unwrap();
    conn.execute("INSERT INTO entry(id,session_id,ordinal,kind,role,search_text,payload_json) VALUES(2,1,9000,'reasoning','assistant','hiddenreasoning','{}')",[]).unwrap();
    conn.execute("INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(2,0,'thinking','private thought')",[]).unwrap();
    let library = Library::new(&path);
    let first = library
        .recent_local_sessions(&RecentRequest::default())
        .unwrap();
    let ids = first["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_i64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec![1, 4]);
    assert_eq!(first["sessions"][0]["status"], "recent");
    assert_eq!(first["sessions"][0]["latestEvent"]["position"], 1);
    assert_eq!(first["sessions"][0]["latestEvent"]["summary"], "");
    assert_eq!(
        first["sessions"][0]["latestEvent"]["occurredAt"],
        serde_json::Value::Null
    );
    assert_eq!(first["sessions"][1]["status"], "unavailable");
    let serialized = first.to_string();
    for secret in [
        "private thought",
        "hidden-payload",
        "hiddenreasoning",
        "rawhidden",
        "native.jsonl",
    ] {
        assert!(!serialized.contains(secret));
    }
    let stale = library
        .recent_local_sessions(&RecentRequest {
            unhealthy: true,
            ..RecentRequest::default()
        })
        .unwrap();
    assert_eq!(stale["sessions"][0]["status"], "stale");
    conn.execute("DELETE FROM entry WHERE id=2", []).unwrap();
    let safe = library
        .recent_local_sessions(&RecentRequest::default())
        .unwrap();
    assert_eq!(
        safe["sessions"][0]["latestEvent"]["summary"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        240
    );
    let filtered = library
        .recent_local_sessions(&RecentRequest {
            since: "2020-01-01".into(),
            limit: 1,
            ..RecentRequest::default()
        })
        .unwrap();
    assert_eq!(filtered["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["hasMore"], false);
    assert!(library
        .recent_local_sessions(&RecentRequest {
            project: "' OR 1=1--".into(),
            ..RecentRequest::default()
        })
        .unwrap()["sessions"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(library
        .recent_local_sessions(&RecentRequest {
            since: "now".into(),
            ..RecentRequest::default()
        })
        .is_err());
    assert!(library
        .recent_local_sessions(&RecentRequest {
            limit: 101,
            ..RecentRequest::default()
        })
        .is_err());
    std::fs::remove_file(native).unwrap();
    assert_eq!(
        library
            .recent_local_sessions(&RecentRequest::default())
            .unwrap()["sessions"][0]["status"],
        "unavailable"
    );
}

#[test]
fn quiet_records_and_recent_paging_do_not_infer_running_or_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let native = dir.path().join("old.jsonl");
    std::fs::write(&native, b"{}\n").unwrap();
    let conn = db::open_db(&path).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'claude','Claude');")
        .unwrap();
    for id in 1..=3 {
        conn.execute("INSERT INTO session(id,agent_id,source_uri,last_event_at) VALUES(?,1,?,'2000-01-01T00:00:00Z')",params![id,format!("claude://{}#{id}",native.display())]).unwrap();
    }
    // Distinct source filenames are real regular files, not process signals.
    for id in 1..=3 {
        let file = dir.path().join(format!("old-{id}.jsonl"));
        std::fs::write(&file, b"{}\n").unwrap();
        conn.execute(
            "UPDATE session SET source_uri=? WHERE id=?",
            params![format!("claude://{}", file.display()), id],
        )
        .unwrap();
    }
    let library = Library::new(path);
    let first = library
        .recent_local_sessions(&RecentRequest {
            limit: 2,
            ..RecentRequest::default()
        })
        .unwrap();
    assert_eq!(first["hasMore"], true);
    assert_eq!(first["nextOffset"], 2);
    assert!(first["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["status"] == "quiet" && s["latestEvent"].is_null()));
    let second = library
        .recent_local_sessions(&RecentRequest {
            limit: 2,
            offset: 2,
            ..RecentRequest::default()
        })
        .unwrap();
    assert_eq!(second["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(second["hasMore"], false);
}
