use super::*;
use rusqlite::params;

#[test]
fn facets_preserve_distinct_values_and_exclude_orphaned_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("facets.sqlite");
    let conn = open_db(&path).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
        INSERT INTO session(id,agent_id,source_uri) VALUES(1,1,'fixture://1');
        INSERT INTO model(id,provider,model_key) VALUES(1,'fixture','used'),(2,'fixture','orphan');
        INSERT INTO session_context(session_id,model_id,branch) VALUES(1,1,'main'),(1,1,'main'),(1,1,'feature');
        INSERT INTO tool_invocation(session_id,tool_name) VALUES(1,'Read'),(1,'Read');
        PRAGMA foreign_keys=OFF;
        INSERT INTO session_context(session_id,model_id,branch) VALUES(999,2,'orphan');
        INSERT INTO tool_invocation(session_id,tool_name) VALUES(999,'orphan');").unwrap();
    let facets = get_facets_inner(&path).unwrap();
    assert_eq!(
        facets["models"],
        json!([{"value":"used","provider":"fixture"}])
    );
    assert_eq!(facets["branches"], json!(["feature", "main"]));
    assert_eq!(facets["tools"], json!(["Read"]));
}

#[test]
fn statistics_fail_explicitly_instead_of_reporting_zero_for_broken_storage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("statistics.sqlite");
    let conn = open_db(&path).unwrap();
    assert_eq!(get_stats_inner(&path).unwrap()["files"], 0);
    conn.execute_batch("DROP TABLE file_activity").unwrap();
    assert!(get_stats_inner(&path)
        .unwrap_err()
        .contains("file_activity"));
}

#[test]
fn main_session_browsing_uses_partial_activity_indexes() {
    let conn = library();
    conn.execute_batch("ANALYZE; UPDATE sqlite_stat1 SET stat='100000 2001 1' WHERE idx='session_agent_idx'; ANALYZE sqlite_schema;").unwrap();
    for (filters, wanted) in [
        (SessionFilters::default(), "session_main_activity_idx"),
        (
            SessionFilters {
                project: "/fixture".into(),
                ..Default::default()
            },
            "session_main_workspace_activity_idx",
        ),
    ] {
        let SessionQuery {
            base,
            params,
            order_by,
            ..
        } = session_query(&filters);
        let plan = query_all(
            &conn,
            &format!("EXPLAIN QUERY PLAN SELECT s.id {base} ORDER BY {order_by} LIMIT 20"),
            &params,
        )
        .unwrap();
        let plan = serde_json::to_string(&plan).unwrap();
        assert!(plan.contains(wanted), "{plan}");
        assert!(!plan.contains("TEMP B-TREE FOR ORDER BY"), "{plan}");
    }
}

#[test]
fn background_filter_preserves_distinct_sessions_and_full_library_exports() {
    let conn = library();
    conn.execute("UPDATE session SET is_subagent=1 WHERE id=2", [])
        .unwrap();
    conn.execute(
        "UPDATE session SET title='Same prompt' WHERE id IN (1,2,3)",
        [],
    )
    .unwrap();
    let filters = SessionFilters {
        q: "Same prompt".into(),
        ..Default::default()
    };
    let result = search_sessions_from_conn(&conn, &filters, 20, 0, None).unwrap();
    assert_eq!(result["total"], 2);
    let filters = SessionFilters {
        include_subagents: true,
        ..filters
    };
    assert_eq!(
        search_sessions_from_conn(&conn, &filters, 20, 0, None).unwrap()["total"],
        3
    );
    let request = ExportRequest {
        scope: "library".into(),
        ..request("json")
    };
    assert_eq!(export_ids(&conn, &request).unwrap().len(), 1005);
}

#[test]
fn global_search_includes_metadata_and_preserves_filters() {
    let conn = library();
    conn.execute(
        "UPDATE session SET title='Unique title',cwd='/work/metadata' WHERE id=1",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO entry(session_id,ordinal,kind,search_text) VALUES(2,0,'message','unique transcript')", []).unwrap();
    let filters = SessionFilters {
        q: "unique".into(),
        ..Default::default()
    };
    let result = search_sessions_from_conn(&conn, &filters, 20, 0, None).unwrap();
    assert_eq!(result["total"], 2);
    let filters = SessionFilters {
        q: "metadata".into(),
        ..Default::default()
    };
    assert_eq!(
        search_sessions_from_conn(&conn, &filters, 20, 0, None).unwrap()["sessions"][0]["id"],
        1
    );
    let filters = SessionFilters {
        q: "unique".into(),
        agent: "claude".into(),
        ..Default::default()
    };
    assert_eq!(
        search_sessions_from_conn(&conn, &filters, 20, 0, None).unwrap()["total"],
        0
    );
}

#[test]
fn session_search_spans_pages_and_keeps_session_boundaries() {
    let conn = library();
    for ordinal in 0..1100 {
        conn.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text) VALUES(1,?1,'message','tool',?2)",
            rusqlite::params![ordinal * 2, if ordinal >= 500 { "résumé needle tool output" } else { "other text" }]).unwrap();
    }
    conn.execute(
        "INSERT INTO entry(session_id,ordinal,kind,search_text) VALUES(2,0,'message','needle')",
        [],
    )
    .unwrap();
    let result = search_session_entries_from_conn(&conn, 1, "NEED", 20, 0).unwrap();
    assert_eq!(result["total"], 600);
    assert_eq!(result["entries"][0]["position"], 500);
    assert_eq!(result["entries"][0]["ordinal"], 1000);
    assert!(result["entries"][0]["excerpt"]
        .as_str()
        .unwrap()
        .contains("\u{1}needle\u{2}"));
    let last = search_session_entries_from_conn(&conn, 1, "resume", 20, 580).unwrap();
    assert_eq!(last["entries"].as_array().unwrap().len(), 20);
    assert_eq!(last["entries"][19]["position"], 1099);
    assert_eq!(
        search_session_entries_from_conn(&conn, 1, "missing", 20, 0).unwrap()["total"],
        0
    );
    assert_eq!(
        search_session_entries_from_conn(&conn, 1, "  ", 20, 0).unwrap()["total"],
        0
    );
    assert!(search_session_entries_from_conn(&conn, 9999, "needle", 20, 0).is_err());
}

#[test]
fn session_search_handles_punctuation_and_sql_wildcards_literally() {
    let conn = library();
    for (ordinal, text) in ["100%", "1000", "___", "other"].iter().enumerate() {
        conn.execute(
            "INSERT INTO entry(session_id,ordinal,kind,search_text) VALUES(1,?1,'message',?2)",
            rusqlite::params![ordinal as i64, text],
        )
        .unwrap();
    }
    assert_eq!(
        search_session_entries_from_conn(&conn, 1, "%", 20, 0).unwrap()["total"],
        1
    );
    assert_eq!(
        search_session_entries_from_conn(&conn, 1, "\" OR *", 20, 0).unwrap()["total"],
        0
    );
}

#[test]
fn session_search_counts_sparse_positions_without_repeated_prefix_scans() {
    let mut conn = library();
    conn.execute_batch(
        "WITH RECURSIVE numbers(n) AS (
           VALUES(1) UNION ALL SELECT n+1 FROM numbers WHERE n<25000
         )
         INSERT INTO entry(session_id,ordinal,kind,search_text)
            SELECT 1,n*10,'message',CASE WHEN n>=24600 AND n%2=0
              THEN 'late needle' ELSE 'other entry' END FROM numbers;",
    )
    .unwrap();

    let mut budget = 350;
    conn.progress_handler(
        1000,
        Some(move || {
            budget -= 1;
            budget == 0
        }),
    )
    .unwrap();
    let results = search_session_entries_from_conn(&conn, 1, "needle", 50, 100).unwrap();
    assert_eq!(results["total"], 201);
    let entries = results["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 50);
    for (index, entry) in entries.iter().enumerate() {
        let source_index = 24800 + 2 * index;
        assert_eq!(entry["ordinal"], source_index * 10);
        assert_eq!(entry["position"], source_index - 1);
    }
}

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
fn session_search_scopes_fts_with_dense_and_interleaved_entry_ids() {
    let conn = library();
    conn.execute_batch(
        "INSERT INTO entry(id,session_id,ordinal,kind,search_text) VALUES
          (10,1,100,'message','earlier text'),
          (11,2,0,'message','needle from another session'),
          (12,1,200,'message','first needle'),
          (14,1,500,'message','last needle');",
    )
    .unwrap();
    let dense = search_session_entries_from_conn(&conn, 1, "needle", 20, 0).unwrap();
    assert_eq!(dense["total"], 2);
    assert_eq!(dense["entries"][0]["position"], 1);
    assert_eq!(dense["entries"][1]["position"], 2);
    assert_eq!(dense["entries"][0]["id"], 12);
    assert_eq!(dense["entries"][1]["id"], 14);
    conn.execute(
        "INSERT INTO entry(id,session_id,ordinal,kind,search_text)
         VALUES(10000,1,600,'message','later text')",
        [],
    )
    .unwrap();
    let interleaved = search_session_entries_from_conn(&conn, 1, "needle", 20, 0).unwrap();
    assert_eq!(interleaved, dense);
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
    // The test exercises export durability; batch fixture writes before exporting.
    let fixture = conn.unchecked_transaction().unwrap();
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
    fixture.commit().unwrap();
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

#[test]
fn conditional_transcript_reads_track_parts_tools_annotations_pages_and_deletion() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conditional.sqlite");
    let conn = open_db(&path).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
        INSERT INTO session(id,agent_id,source_uri) VALUES(1,1,'fixture://1'),(2,1,'fixture://2');
        INSERT INTO entry(id,session_id,ordinal,kind,role) VALUES(1,1,0,'message','user'),(2,1,1,'message','assistant');
        INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(1,0,'text','hello'),(2,0,'text','reply');").unwrap();
    let full = get_session_if_changed(&path, "1", 1, 1, None).unwrap();
    let mut token = full["readToken"].as_str().unwrap().to_owned();
    let unchanged = || get_session_if_changed(&path, "1", 1, 1, Some(&token)).unwrap();
    assert_eq!(unchanged()["unchanged"], true);
    conn.execute("UPDATE session SET title='unrelated' WHERE id=2", [])
        .unwrap();
    assert_eq!(unchanged()["unchanged"], true);
    for query in [
        "UPDATE content_part SET text='edited' WHERE entry_id=1",
        "INSERT INTO tool_invocation(session_id,entry_id,tool_name) VALUES(1,1,'Read')",
        "INSERT INTO session_annotation(session_id,starred) VALUES(1,1)",
        "INSERT INTO session_tag(session_id,label) VALUES(1,'important')",
    ] {
        conn.execute_batch(query).unwrap();
        let changed = get_session_if_changed(&path, "1", 1, 1, Some(&token)).unwrap();
        assert!(changed.get("entries").is_some(), "{query}");
        let next = changed["readToken"].as_str().unwrap();
        assert_ne!(next, token);
        token = next.into();
    }
    let page = get_session_if_changed(&path, "1", 1, 2, Some(&token)).unwrap();
    assert_eq!(page["entries"][0]["id"], 2);
    let resized = get_session_if_changed(&path, "1", 2, 1, Some(&token)).unwrap();
    assert_eq!(resized["entries"].as_array().unwrap().len(), 2);
    conn.execute_batch("DROP TRIGGER index_read_content_part_update; UPDATE content_part SET text='untracked' WHERE entry_id=1;").unwrap();
    let fallback = get_session_if_changed(&path, "1", 1, 1, Some(&token)).unwrap();
    assert_eq!(fallback["entries"][0]["parts"][0]["text"], "untracked");
    assert!(fallback["readToken"].is_null());
    conn.execute("DELETE FROM session WHERE id=1", []).unwrap();
    assert_eq!(
        get_session_if_changed(&path, "1", 1, 1, Some(&token)).unwrap()["deleted"],
        true
    );
}

#[test]
fn last_transcript_page_tracks_the_final_entries_within_the_read_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("last-page.sqlite");
    let conn = open_db(&path).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
        INSERT INTO session(id,agent_id,source_uri) VALUES(1,1,'fixture://1'),(2,1,'fixture://empty');
        WITH RECURSIVE numbers(n) AS (
          VALUES(1) UNION ALL SELECT n+1 FROM numbers WHERE n<12501
        ) INSERT INTO entry(id,session_id,ordinal,kind,role)
          SELECT n,1,n*10,'message','assistant' FROM numbers;").unwrap();
    let first = get_session_inner(&path, "1", 100, 1).unwrap();
    assert_eq!(first["entries"][0]["id"], 1);
    let last = get_session_inner(&path, "1", 100, i64::MAX).unwrap();
    assert_eq!(last["offset"], 12500);
    assert_eq!(last["total"], 12501);
    assert_eq!(last["entries"].as_array().unwrap().len(), 1);
    assert_eq!(last["entries"][0]["id"], 12501);
    let empty = get_session_inner(&path, "2", 100, i64::MAX).unwrap();
    assert_eq!(empty["offset"], 0);
    assert_eq!(empty["entries"], json!([]));

    let token = last["readToken"].as_str();
    assert_eq!(
        get_session_if_changed(&path, "1", 100, i64::MAX, token).unwrap()["unchanged"],
        true
    );
    conn.execute_batch("INSERT INTO entry(id,session_id,ordinal,kind,role) VALUES(12502,1,125020,'message','assistant');").unwrap();
    let updated = get_session_if_changed(&path, "1", 100, i64::MAX, token).unwrap();
    assert_eq!(updated["entries"][1]["id"], 12502);
}

#[test]
#[ignore = "synthetic full-page versus unchanged refresh probe; run release and serial"]
fn conditional_transcript_performance() {
    use std::time::Instant;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refresh.sqlite");
    let mut conn = open_db(&path).unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex'); INSERT INTO session(id,agent_id,source_uri) VALUES(1,1,'fixture://refresh');").unwrap();
    for id in 1..=10000 {
        tx.execute("INSERT INTO entry(id,session_id,ordinal,kind,role) VALUES(?,1,?,'message','assistant')",params![id,id]).unwrap();
        tx.execute(
            "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'text',?)",
            params![id, "synthetic conversation ".repeat(250)],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let full = get_session_if_changed(&path, "1", 500, 1, None).unwrap();
    let token = full["readToken"].as_str().unwrap();
    let mut cases = Vec::new();
    for known in [None, Some(token)] {
        let mut samples = Vec::new();
        let mut bytes = 0;
        for run in 0..10 {
            let start = Instant::now();
            let result = get_session_if_changed(&path, "1", 500, 1, known).unwrap();
            bytes = serde_json::to_vec(&result).unwrap().len();
            if run >= 3 {
                samples.push(start.elapsed().as_secs_f64() * 1000.);
            }
        }
        samples.sort_by(f64::total_cmp);
        cases.push(json!({"conditional":known.is_some(),"medianMs":samples[3],"samplesMs":samples,"responseBytes":bytes}));
    }
    let report = json!({"historyEntries":10000,"pageSize":500,"cases":cases});
    if let Ok(path) = std::env::var("HESSION_REFRESH_BENCH_OUTPUT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    println!("{report}");
}
