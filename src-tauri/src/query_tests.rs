use super::*;

fn library() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    db::init_schema(&conn).unwrap();
    conn.execute(
        "INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex')",
        [],
    )
    .unwrap();
    for id in 1..=1005 {
        conn.execute(
            "INSERT INTO session(id,agent_id,source_uri,title) VALUES(?1,1,?2,?2)",
            rusqlite::params![id, format!("test://{id}")],
        )
        .unwrap();
    }
    conn
}

#[test]
fn paged_transcripts_keep_parts_tools_and_session_boundaries() {
    let mut conn = library();
    let tx = conn.transaction().unwrap();
    for ordinal in 0..1123 {
        tx.execute("INSERT INTO entry(id,session_id,ordinal,kind,role,payload_json) VALUES(?1,1,?2,'message','assistant',?3)", rusqlite::params![ordinal + 1, ordinal, "x".repeat(200)]).unwrap();
        for part in [1, 0] {
            tx.execute(
                "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?1,?2,'text',?3)",
                rusqlite::params![ordinal + 1, part, format!("{ordinal}:{part}")],
            )
            .unwrap();
        }
        tx.execute(
            "INSERT INTO tool_invocation(session_id,entry_id,tool_name) VALUES(1,?1,'Read')",
            [ordinal + 1],
        )
        .unwrap();
    }
    tx.execute(
        "INSERT INTO entry(id,session_id,ordinal,kind) VALUES(2000,2,0,'message')",
        [],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO tool_invocation(session_id,entry_id,tool_name) VALUES(2,2000,'Other')",
        [],
    )
    .unwrap();
    tx.execute("INSERT INTO tool_invocation(session_id,entry_id,tool_name) VALUES(2,1,'Mismatched session')", []).unwrap();
    tx.execute(
        "INSERT INTO tool_invocation(session_id,tool_name) VALUES(1,'Unlinked')",
        [],
    )
    .unwrap();
    let first = get_session_from_conn(&tx, 1, 500, &mut 0).unwrap();
    assert_eq!(first["total"], 1123);
    assert_eq!(first["entryCounts"]["tool"], 1123);
    let mut collected = first["entries"].as_array().unwrap().clone();
    for offset in [500, 1000] {
        let page = get_entries_from_conn(&tx, 1, 500, offset).unwrap();
        let full = get_session_from_conn(&tx, 1, 500, &mut { offset }).unwrap();
        assert_eq!(json!(page), full["entries"]);
        collected.extend(page);
    }
    assert_eq!(collected.len(), 1123);
    for (ordinal, entry) in collected.iter().enumerate() {
        assert_eq!(entry["ordinal"], json!(ordinal));
        assert_eq!(entry["parts"][0]["text"], format!("{ordinal}:0"));
        assert_eq!(entry["parts"][1]["text"], format!("{ordinal}:1"));
        assert_eq!(entry["parts"][0]["e_ord"], json!(ordinal));
        assert_eq!(entry["tools"].as_array().unwrap().len(), 1);
        assert_eq!(entry["tools"][0]["tool_name"], "Read");
        assert_eq!(entry["payload_json"], "x".repeat(200));
    }
    assert!(get_entries_from_conn(&tx, 1, 500, 2000).unwrap().is_empty());
    let mut offset = 9999;
    let clamped = get_session_from_conn(&tx, 1, 500, &mut offset).unwrap();
    assert_eq!(offset, 1000);
    assert_eq!(clamped["entries"].as_array().unwrap().len(), 123);
}

#[test]
fn batched_annotations_match_individual_reads_across_bind_chunks() {
    let conn = library();
    for id in [1, 499, 500, 501, 1000, 1005] {
        conn.execute("INSERT INTO session_annotation VALUES(?1,?2)", [id, id % 2])
            .unwrap();
        for label in ["zebra", "Alpha", "middle"] {
            conn.execute(
                "INSERT INTO session_tag VALUES(?1,?2)",
                rusqlite::params![id, label],
            )
            .unwrap();
        }
    }
    let mut sessions: Vec<Value> = (1..=1005).rev().map(|id| json!({"id": id})).collect();
    attach_annotations_batch(&conn, &mut sessions, None).unwrap();
    for session in sessions {
        let mut expected = json!({"id": session["id"]});
        attach_annotations(&conn, &mut expected).unwrap();
        assert_eq!(session, expected);
    }
    attach_annotations_batch(&conn, &mut [], None).unwrap();
}

#[test]
fn latest_context_uses_index_order_and_preserves_latest_nonnull_branch() {
    let conn = library();
    conn.execute(
        "INSERT INTO model(id,provider,model_key) VALUES(1,'','old'),(2,'','new')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO session_context(session_id,observed_at,branch,model_id) VALUES(1,'2026-10-24','main',1),(1,'2026-10-23','feature',2),(1,'2026-10-22',NULL,2)", []).unwrap();
    let session = get_session_from_conn(&conn, 1, 500, &mut 0).unwrap();
    assert_eq!(session["session"]["branch"], "feature");
    assert_eq!(session["session"]["model"], "new");
    let plan = query_all(&conn, "EXPLAIN QUERY PLAN SELECT model_id FROM session_context WHERE session_id=? ORDER BY id DESC LIMIT 1", &[SqlValue::Integer(1)]).unwrap();
    let plan = plan
        .iter()
        .map(|row| row["detail"].as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(plan.contains("context_session_latest_idx"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    db::init_schema(&conn).unwrap();
}

#[test]
fn pricing_cache_reuses_live_rates_but_reloads_overrides_and_new_snapshots() {
    let conn = library();
    let payload = |input| {
        json!({"openai":{"models":{"gpt-test":{"name":"Test","cost":{"input":input,"output":10.0}}}}}).to_string()
    };
    conn.execute("INSERT INTO model_pricing_cache(id,payload_json,model_count,updated_at) VALUES(1,?1,1,'2026-10-24T00:00:00Z')", [payload(2.0)]).unwrap();
    let mut state = pricing::PricingState::new();
    pricing::load_pricing_cache(&conn, &mut state).unwrap();
    assert_eq!(
        pricing::get_model_pricing(&mut state, "gpt-test").in_per_1m,
        2.0
    );
    conn.execute("INSERT INTO model_price_overrides(provider,model_id,in_per_1m,out_per_1m,cache_per_1m) VALUES('','gpt-test',3,11,1)", []).unwrap();
    pricing::load_pricing_cache(&conn, &mut state).unwrap();
    assert_eq!(
        pricing::get_model_pricing(&mut state, "gpt-test").in_per_1m,
        3.0
    );
    conn.execute("DELETE FROM model_price_overrides", [])
        .unwrap();
    conn.execute(
        "UPDATE model_pricing_cache SET payload_json=?1,updated_at='2026-10-24T01:00:00Z'",
        [payload(5.0)],
    )
    .unwrap();
    pricing::load_pricing_cache(&conn, &mut state).unwrap();
    assert_eq!(
        pricing::get_model_pricing(&mut state, "gpt-test").in_per_1m,
        5.0
    );
    conn.execute(
        "UPDATE model_pricing_cache SET payload_json='invalid',updated_at='2026-10-24T02:00:00Z'",
        [],
    )
    .unwrap();
    assert!(pricing::load_pricing_cache(&conn, &mut state).is_err());
}

fn request(format: &str) -> ExportRequest {
    ExportRequest {
        scope: "session".into(),
        session_id: Some(1),
        filters: SessionFilters::default(),
        include_stats: true,
        format: format.into(),
    }
}

#[test]
fn streamed_json_keeps_all_pages_annotations_and_unlinked_tools() {
    let mut conn = library();
    let tx = conn.transaction().unwrap();
    for ordinal in -2..1123 {
        tx.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text,payload_json) VALUES(1,?1,'message','user',?2,'{}')", rusqlite::params![ordinal,format!("entry {ordinal} 🦀")]).unwrap();
    }
    tx.execute("INSERT INTO session_annotation VALUES(1,1)", [])
        .unwrap();
    tx.execute("INSERT INTO session_tag VALUES(1,'tag')", [])
        .unwrap();
    tx.execute("INSERT INTO tool_invocation(session_id,tool_name,result_text) VALUES(1,'Unlinked','complete')", []).unwrap();
    let mut output = Vec::new();
    let mut pages = 0;
    let count = write_export(
        &tx,
        &request("json"),
        &mut output,
        &ExportJob::default(),
        |_, _, _| {},
    )
    .unwrap();
    let data: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(count, 1);
    assert_eq!(data["schemaVersion"], 1);
    assert_eq!(data["sessions"][0]["session"]["starred"], true);
    assert_eq!(data["sessions"][0]["session"]["tags"], json!(["tag"]));
    assert!(data["sessions"][0]["session"].get("source_uri").is_none());
    let entries = data["sessions"][0]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1125);
    assert_eq!(entries[0]["ordinal"], -2);
    assert_eq!(entries[1124]["ordinal"], 1122);
    assert_eq!(
        data["sessions"][0]["unlinkedTools"][0]["result_text"],
        "complete"
    );
    assert_eq!(data["sessions"][0]["stats"]["total_turns"], 1125);
    let mut after = None;
    loop {
        let page = get_entries_after(&tx, 1, after).unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 500);
        after = page.last().and_then(|entry| entry["ordinal"].as_i64());
        pages += 1;
    }
    assert_eq!(pages, 3);
}

#[test]
fn export_id_selection_matches_search_for_each_filter_and_sort() {
    let conn = library();
    conn.execute("INSERT INTO entry(session_id,ordinal,kind,search_text) VALUES(2,0,'message','find needle')",[]).unwrap();
    conn.execute("INSERT INTO session_tag VALUES(2,'important')", [])
        .unwrap();
    for sort in ["recent", "oldest", "longest", "messages"] {
        for filters in [
            SessionFilters {
                sort: sort.into(),
                ..Default::default()
            },
            SessionFilters {
                q: "needle".into(),
                sort: sort.into(),
                ..Default::default()
            },
            SessionFilters {
                tags: vec!["important".into()],
                sort: sort.into(),
                ..Default::default()
            },
        ] {
            let result = search_sessions_from_conn(&conn, &filters, -1, 0, None).unwrap();
            let expected: Vec<i64> = result["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|session| session["id"].as_i64().unwrap())
                .collect();
            let request = ExportRequest {
                scope: "filtered".into(),
                session_id: None,
                filters,
                include_stats: false,
                format: "json".into(),
            };
            assert_eq!(export_ids(&conn, &request).unwrap(), expected);
        }
    }
}

#[test]
fn streamed_export_is_atomic_and_cancellation_preserves_existing_file() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("sessions.sqlite");
    let conn = db::open_db(&database).unwrap();
    conn.execute(
        "INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO session(id,agent_id,source_uri) VALUES(1,1,'test://1')",
        [],
    )
    .unwrap();
    for ordinal in 0..1100 {
        conn.execute(
            "INSERT INTO entry(session_id,ordinal,kind,search_text) VALUES(1,?1,'message',?2)",
            rusqlite::params![ordinal, "large text ".repeat(100)],
        )
        .unwrap();
    }
    let target = directory.path().join("export.json");
    std::fs::write(&target, "original").unwrap();
    let job = ExportJob::default();
    let result = save_streamed_export(
        &database,
        &target,
        &request("json"),
        &job,
        |_, _, entries| {
            if entries >= 500 {
                job.cancelled.store(true, Ordering::SeqCst);
            }
        },
    );
    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
    assert!(!*job.saved.lock().unwrap());
    assert!(!std::fs::read_dir(directory.path())
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("-export-")));
    let job = ExportJob::default();
    assert_eq!(
        save_streamed_export(&database, &target, &request("json"), &job, |_, _, _| {}).unwrap(),
        1
    );
    assert!(*job.saved.lock().unwrap());
    let data: Value = serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
    assert_eq!(
        data["sessions"][0]["entries"].as_array().unwrap().len(),
        1100
    );
    assert!(save_streamed_export(
        &database,
        &database,
        &request("json"),
        &ExportJob::default(),
        |_, _, _| {}
    )
    .is_err());
}

#[test]
fn report_scopes_preserve_date_comparisons_buckets_and_persistent_data() {
    let conn = library();
    crate::library_revision::mark_managed(&conn).unwrap();
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    for (index, duration) in [0.0, 120.0, 600.0, 1800.0, 3600.0, 7200.0]
        .iter()
        .enumerate()
    {
        let id = index as i64 + 1;
        conn.execute(
            "UPDATE session SET started_at=?1 WHERE id=?2",
            rusqlite::params![today, id],
        )
        .unwrap();
        conn.execute("INSERT INTO session_analytics(session_id,agent_id,started_at,model_name,total_turns,total_tokens,duration_seconds) VALUES(?1,1,?2,?3,?1,?1,?4)",rusqlite::params![id,today,if id%2==0 {"even"}else{"odd"},duration]).unwrap();
        for count in 0..3 {
            conn.execute("INSERT INTO tool_invocation(session_id,tool_name,status,duration_ms) VALUES(?1,'Read','completed',?2)",[id,count*10]).unwrap();
        }
    }
    let before = crate::library_revision::managed_token(&conn, &[]).unwrap();
    conn.pragma_update(None, "query_only", true).unwrap();
    let filters = analytics::AnalyticsFilters {
        timezone: "UTC".into(),
        ..Default::default()
    };
    let report =
        analytics::get_full_analytics(&conn, &filters, &analytics::AnalyticsState::new()).unwrap();
    assert_eq!(report["kpis"]["total_sessions"], 6);
    for bucket in report["productivity_pulse"]["duration_distribution"]
        .as_array()
        .unwrap()
    {
        assert_eq!(bucket["count"], 1);
    }
    assert!(conn
        .pragma_query_value(None, "query_only", |row| row.get::<_, bool>(0))
        .unwrap());
    assert_eq!(
        crate::library_revision::managed_token(&conn, &[]).unwrap(),
        before
    );
    let filtered = analytics::get_full_analytics(
        &conn,
        &analytics::AnalyticsFilters {
            model: "even".into(),
            timezone: "UTC".into(),
            ..Default::default()
        },
        &analytics::AnalyticsState::new(),
    )
    .unwrap();
    assert_eq!(filtered["kpis"]["total_sessions"], 3);
    assert_eq!(
        filtered["tool_metrics"]["tool_error_rates"][0]["total_calls"],
        9
    );
    let empty = analytics::get_full_analytics(
        &conn,
        &analytics::AnalyticsFilters {
            model: "none".into(),
            timezone: "UTC".into(),
            ..Default::default()
        },
        &analytics::AnalyticsState::new(),
    )
    .unwrap();
    assert_eq!(empty["kpis"]["total_sessions"], 0);
    assert!(empty["tool_metrics"]["tool_error_rates"]
        .as_array()
        .unwrap()
        .is_empty());
    conn.pragma_update(None, "query_only", false).unwrap();
    let yesterday = (chrono::Utc::now() - chrono::TimeDelta::days(1))
        .format("%Y-%m-%d")
        .to_string();
    conn.execute("UPDATE session SET started_at=?1 WHERE id=7", [&yesterday])
        .unwrap();
    conn.execute("INSERT INTO session_analytics(session_id,agent_id,started_at,estimated_cost) VALUES(7,1,?1,7)",[yesterday]).unwrap();
    let since_today = analytics::get_full_analytics(
        &conn,
        &analytics::AnalyticsFilters {
            since: format!("{today}T00:00:00Z"),
            timezone: "UTC".into(),
            ..Default::default()
        },
        &analytics::AnalyticsState::new(),
    )
    .unwrap();
    assert_eq!(since_today["kpis"]["total_sessions"], 6);
    assert_eq!(
        since_today["tokens_and_costs"]["spend_outlook"]["weekly_deltas"]["estimated_cost"]
            ["current"],
        7.0
    );
}

#[test]
#[ignore = "synthetic benchmark; run with --ignored --nocapture"]
fn benchmark_latest_context_lookup() {
    let mut conn = library();
    let tx = conn.transaction().unwrap();
    {
        let mut insert = tx.prepare("INSERT INTO session_context(session_id,observed_at,branch) VALUES(?1,'2026-10-24','main')").unwrap();
        for id in 1..=1000 {
            for _ in 0..200 {
                insert.execute([id]).unwrap();
            }
        }
    }
    tx.commit().unwrap();
    let query = "SELECT s.id,(SELECT branch FROM session_context c WHERE c.session_id=s.id AND c.branch IS NOT NULL ORDER BY c.id DESC LIMIT 1) AS branch FROM session s";
    let measure = || {
        let mut times = Vec::new();
        let mut result = Vec::new();
        for _ in 0..7 {
            let start = std::time::Instant::now();
            result = query_all(&conn, query, &[]).unwrap();
            times.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(f64::total_cmp);
        (result, times[3])
    };
    let (indexed, after) = measure();
    conn.execute_batch("DROP INDEX context_session_latest_idx")
        .unwrap();
    let (original, before) = measure();
    assert_eq!(indexed, original);
    println!("1,000 sessions / 200,000 context rows: latest branch before {before:.3} ms, after {after:.3} ms ({:.1}x)", before / after);
}
