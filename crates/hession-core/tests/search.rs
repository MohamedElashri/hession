use hession_core::{
    db,
    search::{
        self, query, SearchError, SearchFilters, SearchMode, SearchRequest, SearchScope,
        SearchVisibility,
    },
    service::Library,
};
use rusqlite::{params, Connection};

fn fixture() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::init_schema(&conn).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex'),(2,'claude','Claude Code');
      INSERT INTO workspace(id,path,display_name) VALUES(1,'/work/one','Project One'),(2,'box:/work/two','Project Two');
      INSERT INTO session(id,agent_id,workspace_id,source_uri,title,started_at,last_event_at,host) VALUES
      (1,1,1,'fixture:1','Compiler fixes','2026-01-01T12:00:00Z','2026-01-02T12:00:00Z',NULL),
      (2,2,2,'fixture:2','Metadata match dragonfruit','2026-02-01T12:00:00Z','2026-02-02T12:00:00Z','box'),
      (3,1,1,'fixture:3','Empty history','2026-03-01T12:00:00Z',NULL,NULL);
      INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES
      (11,1,0,'message','user','alpha compiler C++ foo::bar src/a-b.ts 100% _private \\server élan'),
      (12,1,1,'message','assistant','beta gamma quoted phrase phrase and compiler fixes'),
      (21,2,0,'message','assistant','alpha beta cloud otherprojectsecret'),
      (22,2,1,'message','assistant','alpha beta cloud');
      INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES
      (11,0,'text','alpha compiler C++ foo::bar src/a-b.ts 100% _private \\server élan'),
      (12,0,'text','beta gamma quoted phrase phrase and compiler fixes'),
      (21,0,'text','alpha beta cloud otherprojectsecret'),(22,0,'text','alpha beta cloud');
      INSERT INTO file_activity(session_id,entry_id,path,action) VALUES(1,11,'src/a-b.ts','read'),(2,21,'secret.rs','write');
      INSERT INTO session_context(session_id,entry_id,branch) VALUES(1,11,'feature/a-b'),(2,21,'main');
      INSERT INTO tool_invocation(session_id,entry_id,tool_name,status,exit_code) VALUES(1,11,'shell','failed',1),(2,21,'read','success',0);
      INSERT INTO session_annotation(session_id,starred) VALUES(1,1);
      INSERT INTO session_tag(session_id,label) VALUES(1,'review');").unwrap();
    conn
}
fn request(q: &str) -> SearchRequest {
    SearchRequest {
        query: q.into(),
        visibility: SearchVisibility::AllIndexedText,
        ..Default::default()
    }
}
fn ids(conn: &mut Connection, q: &str) -> Vec<i64> {
    search::search(conn, &request(q), &SearchScope::all())
        .unwrap()
        .results
        .iter()
        .map(|r| r.session_id)
        .collect()
}
#[test]
fn typed_grammar_positions_and_limits() {
    let parsed = query::parse("alpha beta OR -agent:\"Claude Code\" file:/work/a.rs").unwrap();
    assert_eq!(parsed.alternatives.len(), 2);
    assert_eq!(parsed.alternatives[0].len(), 2);
    assert!(parsed.alternatives[1][0].excluded);
    assert_eq!(parsed.alternatives[1][1].field, Some(query::Field::File));
    for (q, pos) in [
        ("OR alpha", 0),
        ("alpha OR", 6),
        ("x OR OR y", 5),
        ("agent:", 0),
        ("\"open", 0),
        ("foo:bar", 0),
        ("after:2025-02-29", 0),
        ("before:2024-13-01", 0),
        ("has:success", 0),
        ("-", 0),
        ("(a OR b)", 0),
        ("α \"bad", 3),
    ] {
        let e = query::parse(q).unwrap_err();
        assert_eq!(e.position, pos, "{q}: {e}");
    }
    assert!(query::parse("after:2024-02-29 before:2025-03-01").is_ok());
    assert!(query::parse(&"x".repeat(4097)).is_err());
    assert!(query::parse(&vec!["x"; 65].join(" ")).is_err());
    assert_eq!(query::fts_literal("a\" OR b"), "\"a\"\" OR b\"");
    for q in [
        "C++",
        "foo::bar",
        "CUDA::memory",
        "src/a-b.ts",
        "http://example.test",
        "C:\\foo\\bar",
        "100%",
        "_private",
        "a\\b",
        "\"literal (x)\"",
    ] {
        assert!(query::parse(q).is_ok(), "{q}");
    }
}
#[test]
fn sessions_and_messages_have_explicit_and_exclusion_semantics() {
    let mut c = fixture();
    let mut and = ids(&mut c, "alpha beta");
    and.sort();
    assert_eq!(and, vec![1, 2]);
    assert_eq!(ids(&mut c, "alpha -cloud"), vec![1]);
    let mut union = ids(&mut c, "gamma OR cloud");
    union.sort();
    assert_eq!(union, vec![1, 2]);
    let mut r = request("alpha beta");
    r.mode = SearchMode::Messages;
    let p = search::search(&mut c, &r, &SearchScope::all()).unwrap();
    assert_eq!(p.results.len(), 2);
    assert!(p.results.iter().all(|r| r.session_id == 2));
    assert_eq!(ids(&mut c, "\"quoted phrase\""), vec![1]);
    assert!(ids(&mut c, "\"phrase quoted\"").is_empty());
    assert_eq!(ids(&mut c, "-cloud"), vec![3, 1]);
}
#[test]
fn metadata_filters_empty_sessions_literals_and_injection_are_safe() {
    let mut c = fixture();
    for q in [
        "C++",
        "foo::bar",
        "src/a-b.ts",
        "100%",
        "_private",
        "\\server",
        "elan",
        "branch:feature/a-b",
        "file:src/a-b.ts",
        "agent:codex has:errors",
        "host:@local before:2026-02-01 after:2026-01-02",
    ] {
        assert_eq!(ids(&mut c, q), vec![1], "{q}");
    }
    assert_eq!(ids(&mut c, "project:\"Project One\""), vec![3, 1]);
    assert_eq!(ids(&mut c, "dragonfruit"), vec![2]);
    assert_eq!(ids(&mut c, "\"Empty history\""), vec![3]);
    assert_eq!(ids(&mut c, "host:box tool:read"), vec![2]);
    for q in [
        "\"' OR 1=1 --\"",
        "\"%\"",
        "\"_\"",
        "\"NEAR(alpha beta)\"",
        "\"alpha\"\"beta\"",
    ] {
        let result = search::search(&mut c, &request(q), &SearchScope::all());
        if q == "\"%\"" || q == "\"_\"" {
            assert_eq!(result.unwrap().results.len(), 1);
        } else if q == "\"alpha\"\"beta\"" {
            assert!(result.is_err());
        } else {
            assert!(result.unwrap().results.is_empty(), "{q}");
        }
    }
    let mut r = request("");
    r.filters = SearchFilters {
        starred: true,
        tags: vec!["REVIEW".into()],
        tools: vec!["shell".into()],
        branch: "a-b".into(),
        project: "/work/one".into(),
        ..Default::default()
    };
    assert_eq!(
        search::search(&mut c, &r, &SearchScope::all())
            .unwrap()
            .results[0]
            .session_id,
        1
    );
    r.filters.since = "now".into();
    assert!(search::search(&mut c, &r, &SearchScope::all()).is_err());
}
#[test]
fn scopes_are_outer_immutable_allowlists_across_all_or_and_direct_modes() {
    let mut c = fixture();
    let scope = SearchScope::workspaces([1]).unwrap();
    for q in [
        "otherprojectsecret OR alpha",
        "host:box OR project:two",
        "-missing OR alpha",
        "file:secret.rs OR dragonfruit",
        "",
        "alpha OR -alpha",
    ] {
        let p = search::search(&mut c, &request(q), &scope).unwrap();
        assert!(p.results.iter().all(|r| r.workspace_id == Some(1)), "{q}");
    }
    assert!(
        search::search(&mut c, &request("otherprojectsecret"), &scope)
            .unwrap()
            .results
            .is_empty()
    );
    let empty = SearchScope::workspaces([]).unwrap();
    assert!(search::search(&mut c, &request(""), &empty)
        .unwrap()
        .results
        .is_empty());
    assert_eq!(
        SearchScope::workspaces([2, 1, 1]).unwrap().workspace_ids(),
        Some(&[1, 2][..])
    );
    assert!(SearchScope::workspaces([0]).is_err());
    assert!(SearchScope::workspaces(1..=513).is_err());
    let mut r = request("otherprojectsecret OR alpha");
    r.mode = SearchMode::Messages;
    assert!(search::search(&mut c, &r, &scope)
        .unwrap()
        .results
        .iter()
        .all(|r| r.session_id == 1));
}
#[test]
fn visibility_restricts_matching_and_never_probes_hidden_text() {
    let mut c = fixture();
    c.execute_batch("INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(13,1,2,'message','assistant','visibletext thinkingsecret toolsecret'),(14,1,3,'message','system','systemsecret'),(15,1,4,'message','tool','toolrolesecret');
      INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(13,0,'text','visibletext élan quoted phrase'),(13,1,'thinking','thinkingsecret'),(13,2,'tool_result','toolsecret'),(14,0,'text','systemsecret'),(15,0,'text','toolrolesecret');").unwrap();
    for visibility in [
        SearchVisibility::MetadataOnly,
        SearchVisibility::Conversation,
    ] {
        for q in [
            "thinkingsecret",
            "toolsecret",
            "systemsecret",
            "toolrolesecret",
        ] {
            let mut r = request(q);
            r.visibility = visibility;
            assert!(
                search::search(&mut c, &r, &SearchScope::all())
                    .unwrap()
                    .results
                    .is_empty(),
                "{visibility:?}: {q}"
            );
        }
    }
    let mut r = request("visibletext");
    r.visibility = SearchVisibility::MetadataOnly;
    assert!(search::search(&mut c, &r, &SearchScope::all())
        .unwrap()
        .results
        .is_empty());
    r.query = "dragonfruit".into();
    let p = search::search(&mut c, &r, &SearchScope::all()).unwrap();
    assert!(p
        .results
        .iter()
        .all(|r| r.excerpt.is_none() && r.entry_id.is_none()));
    r.query = "visibletext".into();
    r.visibility = SearchVisibility::Conversation;
    let p = search::search(&mut c, &r, &SearchScope::all()).unwrap();
    assert_eq!(p.results.len(), 1);
    let excerpt = p.results[0].excerpt.as_ref().unwrap();
    assert!(excerpt.contains("visibletext"));
    assert!(!excerpt.contains("secret"));
    let mut metadata_messages = r.clone();
    metadata_messages.query = "agent:codex".into();
    metadata_messages.mode = SearchMode::Messages;
    let safe = search::search(&mut c, &metadata_messages, &SearchScope::all()).unwrap();
    assert!(safe
        .results
        .iter()
        .all(|r| !matches!(r.entry_id, Some(14 | 15))));
    for q in ["elan", "\"quoted phrase\"", "C++"] {
        r.query = q.into();
        assert!(
            !search::search(&mut c, &r, &SearchScope::all())
                .unwrap()
                .results
                .is_empty(),
            "{q}"
        );
    }
    r.query = "visible".into();
    assert!(search::search(&mut c, &r, &SearchScope::all())
        .unwrap()
        .results
        .is_empty());
}
#[test]
fn deterministic_cursor_rejects_stale_library_changed_query_or_scope() {
    let mut c = fixture();
    let mut r = request("");
    r.limit = 1;
    let one = search::search(&mut c, &r, &SearchScope::all()).unwrap();
    assert_eq!(one.results[0].session_id, 3);
    assert_eq!(one.count_lower_bound, 2);
    r.cursor = one.next_cursor;
    let two = search::search(&mut c, &r, &SearchScope::all()).unwrap();
    assert_eq!(two.results[0].session_id, 2);
    assert!(matches!(
        search::search(&mut c, &r, &SearchScope::workspaces([1]).unwrap()),
        Err(SearchError::InvalidCursor)
    ));
    let mut changed = r.clone();
    changed.query = "alpha".into();
    assert!(matches!(
        search::search(&mut c, &changed, &SearchScope::all()),
        Err(SearchError::InvalidCursor)
    ));
    changed = r.clone();
    changed.limit = 2;
    assert!(matches!(
        search::search(&mut c, &changed, &SearchScope::all()),
        Err(SearchError::InvalidCursor)
    ));
    c.execute("UPDATE session SET title='changed' WHERE id=1", [])
        .unwrap();
    assert!(matches!(
        search::search(&mut c, &r, &SearchScope::all()),
        Err(SearchError::StaleCursor)
    ));
    assert!(search::search_snapshot(&c, &request(""), &SearchScope::all()).is_err());
}
#[test]
fn bounded_excerpts_untrusted_html_unicode_and_metadata() {
    let mut c = fixture();
    let giant = format!(
        "{} tokenneedle <script>alert('x')</script> {}",
        "x".repeat(900_000),
        "🦀".repeat(1000)
    );
    c.execute("UPDATE entry SET search_text=? WHERE id=11", [giant])
        .unwrap();
    let page = search::search(&mut c, &request("tokenneedle"), &SearchScope::all()).unwrap();
    let result = &page.results[0];
    let excerpt = result.excerpt.as_ref().unwrap();
    assert!(excerpt.chars().count() <= search::EXCERPT_CHARS);
    assert!(excerpt.contains("tokenneedle"));
    assert!(excerpt.contains("<script>"));
    assert!(result
        .highlights
        .iter()
        .all(|s| s.start < s.end && s.end <= excerpt.chars().count()));
    assert_eq!(result.entry_id, Some(11));
    c.execute(
        "UPDATE entry SET search_text=CAST(zeroblob(1048577) AS TEXT) WHERE id=11",
        [],
    )
    .unwrap();
    assert!(matches!(
        search::search(&mut c, &request("tokenneedle"), &SearchScope::all()),
        Err(SearchError::InputTooLarge)
    ));
    let mut r = request("");
    r.limit = 101;
    assert!(search::search(&mut c, &r, &SearchScope::all()).is_err());
}
#[test]
fn library_service_is_read_only_and_wal_pages_expire_after_concurrent_commit() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("library.sqlite");
    let c = db::open_db(&path).unwrap();
    c.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex'); INSERT INTO workspace(id,path) VALUES(1,'/fixture');INSERT INTO session(id,agent_id,workspace_id,source_uri,title) VALUES(1,1,1,'fixture:a','alpha'),(2,1,1,'fixture:b','alpha');").unwrap();
    let mut r = request("alpha");
    r.limit = 1;
    let l = Library::new(&path);
    let p = l.search(&r, &SearchScope::all()).unwrap();
    r.cursor = p.next_cursor;
    let other = Connection::open(&path).unwrap();
    other
        .execute("UPDATE session SET title='beta' WHERE id=2", [])
        .unwrap();
    assert!(matches!(
        l.search(&r, &SearchScope::all()),
        Err(SearchError::StaleCursor)
    ));
    let missing = d.path().join("missing.sqlite");
    assert!(Library::new(&missing)
        .search(&request(""), &SearchScope::all())
        .is_err());
    assert!(!missing.exists());
    let ro = db::open_read_only(&path).unwrap();
    assert!(ro.execute("DELETE FROM session", []).is_err());
}

/// Reproducible CPU/storage benchmark, intentionally opt-in. No user histories.
/// Run: cargo test --locked -p hession-core --test search benchmark_10k -- --ignored --nocapture
#[test]
#[ignore = "synthetic performance benchmark; run explicitly with --nocapture"]
fn benchmark_10k() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("benchmark.sqlite");
    let mut c = db::open_db(&path).unwrap();
    let tx = c.transaction().unwrap();
    tx.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');INSERT INTO workspace(id,path) VALUES(1,'/fixture/project');").unwrap();
    for id in 1..=10_000_i64 {
        tx.execute("INSERT INTO session(id,agent_id,workspace_id,source_uri,title,started_at) VALUES(?,1,1,?,?,?)",params![id,format!("synthetic:{id}"),format!("Compiler task {id}"),format!("2026-01-{:02}T12:00:00Z",id%28+1)]).unwrap();
        for ordinal in 0..5_i64 {
            let text=format!("compiler implementation session {id} message {ordinal} Rust C++ src/module-{}.rs {}",id%100,if id%25==0 {"regression needle"}else{"routine work"});
            tx.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text) VALUES(?,?,'message','assistant',?)",params![id,ordinal,text]).unwrap();
            tx.execute("INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(last_insert_rowid(),0,'text',?)",[text]).unwrap();
        }
    }
    tx.commit().unwrap();
    c.execute_batch("PRAGMA optimize").unwrap();
    drop(c);
    let lib = Library::new(&path);
    for visibility in [
        SearchVisibility::AllIndexedText,
        SearchVisibility::Conversation,
        SearchVisibility::MetadataOnly,
    ] {
        let requests = [
            "compiler regression",
            "\"regression needle\"",
            "src/module-42.rs",
            "agent:codex compiler",
        ]
        .map(|q| SearchRequest {
            visibility,
            ..request(q)
        });
        let mut samples = Vec::new();
        let start = std::time::Instant::now();
        let cold = lib.search(&requests[0], &SearchScope::all()).unwrap();
        let first = start.elapsed();
        for _ in 0..5 {
            for r in &requests {
                let _ = lib.search(r, &SearchScope::all()).unwrap();
            }
        }
        for _ in 0..25 {
            for r in &requests {
                let t = std::time::Instant::now();
                let _ = lib.search(r, &SearchScope::all()).unwrap();
                samples.push(t.elapsed().as_secs_f64() * 1000.0);
            }
        }
        samples.sort_by(f64::total_cmp);
        eprintln!("{visibility:?}: 100 warm searches / 10k sessions / 50k entries: median={:.2}ms p95={:.2}ms first-connection={:.2}ms rows={} db_bytes={}",samples[50],samples[94],first.as_secs_f64()*1000.0,cold.results.len(),std::fs::metadata(&path).unwrap().len());
    }
}

#[test]
fn one_read_snapshot_keeps_cursor_stable_during_independent_wal_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.sqlite");
    let mut conn = db::open_db(&path).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');INSERT INTO session(id,agent_id,source_uri,title,started_at) VALUES(1,1,'fixture:a','alpha','2026-01-01'),(2,1,'fixture:b','alpha','2026-02-01');").unwrap();
    let tx = conn.transaction().unwrap();
    let mut r = request("alpha");
    r.limit = 1;
    let first = search::search_snapshot(&tx, &r, &SearchScope::all()).unwrap();
    assert_eq!(first.results[0].session_id, 2);
    let writer = Connection::open(&path).unwrap();
    writer
        .execute("UPDATE session SET title='changed' WHERE id=1", [])
        .unwrap();
    r.cursor = first.next_cursor;
    let second = search::search_snapshot(&tx, &r, &SearchScope::all()).unwrap();
    assert_eq!(second.results[0].session_id, 1);
    assert_eq!(first.revision, second.revision);
    tx.commit().unwrap();
    assert!(matches!(
        search::search(&mut conn, &r, &SearchScope::all()),
        Err(SearchError::StaleCursor)
    ));
}
#[test]
fn message_pages_are_deterministic_and_bound_to_visibility_and_filters() {
    let mut c = fixture();
    let mut r = request("alpha");
    r.mode = SearchMode::Messages;
    r.limit = 1;
    let mut seen = std::collections::HashSet::new();
    loop {
        let p = search::search(&mut c, &r, &SearchScope::all()).unwrap();
        assert_eq!(p.results.len(), 1);
        assert!(seen.insert(p.results[0].entry_id.unwrap()));
        if p.next_cursor.is_none() {
            break;
        }
        r.cursor = p.next_cursor;
    }
    assert_eq!(seen.len(), 3);
    let first = search::search(
        &mut c,
        &SearchRequest {
            limit: 1,
            ..request("")
        },
        &SearchScope::all(),
    )
    .unwrap();
    let mut changed = SearchRequest {
        limit: 1,
        cursor: first.next_cursor,
        visibility: SearchVisibility::Conversation,
        ..request("")
    };
    assert!(matches!(
        search::search(&mut c, &changed, &SearchScope::all()),
        Err(SearchError::InvalidCursor)
    ));
    changed.visibility = SearchVisibility::AllIndexedText;
    changed.filters.tags = vec!["review".into()];
    assert!(matches!(
        search::search(&mut c, &changed, &SearchScope::all()),
        Err(SearchError::InvalidCursor)
    ));
    changed.cursor = Some("x".repeat(4097));
    assert!(matches!(
        search::search(&mut c, &changed, &SearchScope::all()),
        Err(SearchError::InvalidCursor)
    ));
}
#[test]
fn adversarial_queries_and_filter_values_never_become_sql_or_fts_syntax() {
    let mut c = fixture();
    for input in [
        "'",
        "\"\"",
        "-\"\"",
        "agent:\"x' OR 1=1 --\"",
        "file:../../../etc/passwd",
        "α\u{301}",
        "\0",
        "\"x\0y\"",
        "***",
        "NOT",
        "NEAR",
        "\"alpha OR beta\"",
        "\"\\\"\"",
        "branch:%",
        "tool:_",
        "branch:main OR -agent:codex",
    ] {
        let outcome = search::search(
            &mut c,
            &request(input),
            &SearchScope::workspaces([1]).unwrap(),
        );
        if let Ok(p) = outcome {
            assert!(p.results.iter().all(|r| r.workspace_id == Some(1)));
        }
    }
    for value in ["' OR 1=1 --", "%", "_", "\\", "\"; DROP TABLE entry;--"] {
        let mut r = request("");
        r.filters = SearchFilters {
            project: value.into(),
            models: vec![value.into()],
            tools: vec![value.into()],
            tags: vec![value.into()],
            ..Default::default()
        };
        assert!(search::search(&mut c, &r, &SearchScope::all())
            .unwrap()
            .results
            .is_empty());
    }
    assert_eq!(
        c.query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[test]
fn ranking_uses_best_bm25_then_title_and_exact_path_boosts_without_duplicates() {
    let mut c = Connection::open_in_memory().unwrap();
    db::init_schema(&c).unwrap();
    c.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
      INSERT INTO session(id,agent_id,source_uri,title,started_at) VALUES(1,1,'fixture:a','ordinary','2026-01-01'),(2,1,'fixture:b','alpha','2026-01-01'),(3,1,'fixture:c','ordinary','2026-01-01');
      INSERT INTO entry(session_id,ordinal,kind,role,search_text) VALUES(1,0,'message','user','alpha src/a-b.ts'),(2,0,'message','user','alpha src/a-b.ts'),(3,0,'message','user','alpha src/a-b.ts'),(2,1,'message','assistant','alpha src/a-b.ts');
      INSERT INTO file_activity(session_id,path) VALUES(3,'src/a-b.ts');").unwrap();
    let word = search::search(&mut c, &request("alpha"), &SearchScope::all()).unwrap();
    assert_eq!(word.results.len(), 3);
    assert_eq!(word.results[0].session_id, 2);
    assert!(word.results[0].score < word.results[1].score);
    let path = search::search(&mut c, &request("src/a-b.ts"), &SearchScope::all()).unwrap();
    assert_eq!(path.results[0].session_id, 3);
    assert!(path.results[0]
        .matching_locations
        .iter()
        .any(|l| l == "file"));
}
#[test]
fn deep_accented_visible_matches_produce_safe_centered_excerpts() {
    let mut c = fixture();
    let content = format!(
        "{} élan onlyvisible {}",
        "word ".repeat(500),
        " tail".repeat(300)
    );
    c.execute("INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(13,1,2,'message','assistant','hiddenmixed')",[]).unwrap();
    c.execute(
        "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(13,0,'text',?)",
        [content],
    )
    .unwrap();
    let r = SearchRequest {
        visibility: SearchVisibility::Conversation,
        ..request("elan onlyvisible")
    };
    let page = search::search(&mut c, &r, &SearchScope::all()).unwrap();
    let result = &page.results[0];
    assert_eq!(result.entry_id, Some(13));
    assert!(result
        .excerpt
        .as_ref()
        .unwrap()
        .contains("élan onlyvisible"));
    assert!(!result.highlights.is_empty());
    assert!(result.excerpt.as_ref().unwrap().chars().count() <= search::EXCERPT_CHARS);
    assert!(!result.excerpt.as_ref().unwrap().contains("hiddenmixed"));
}

#[test]
fn supported_normalized_conversation_kinds_are_visible_but_reasoning_text_is_not() {
    let mut c = Connection::open_in_memory().unwrap();
    db::init_schema(&c).unwrap();
    // These are actual normalized kinds from normalize_record and the
    // OpenCode/Hermes SQLite adapters, matching continuation's transfer policy.
    let cases = [
        ("claude", "user", "user"),
        ("claude", "assistant", "assistant"),
        ("codex", "user_message", "user"),
        ("codex", "agent_message", "assistant"),
        ("codex", "message", "assistant"),
        ("cline", "task", "user"),
        ("cline", "completion_result", "assistant"),
        ("cline", "user_feedback", "user"),
        ("pi", "message", "assistant"),
        ("commandcode", "message", "assistant"),
        ("muse", "message", "assistant"),
        ("opencode", "message", "assistant"),
        ("hermes", "message", "assistant"),
        ("antigravity", "USER_INPUT", "user"),
    ];
    for (index, (family, kind, role)) in cases.iter().enumerate() {
        let id = index as i64 + 1;
        c.execute(
            "INSERT INTO agent(id,family,product,install_root) VALUES(?,?,?,?)",
            params![id, family, family, format!("fixture:{id}")],
        )
        .unwrap();
        c.execute(
            "INSERT INTO session(id,agent_id,source_uri) VALUES(?,?,?)",
            params![id, id, format!("fixture:{id}")],
        )
        .unwrap();
        c.execute("INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(?,?,0,?,?,'visibleconversation')",params![id,id,kind,role]).unwrap();
        c.execute("INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'text','visibleconversation')",[id]).unwrap();
    }
    for (index, kind) in [
        "reasoning",
        "agent_reasoning",
        "thinking",
        "tool",
        "command",
        "permission",
        "unknown_MODEL_EVENT",
    ]
    .iter()
    .enumerate()
    {
        let id = 100 + index as i64;
        c.execute("INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(?,1,?,?,'assistant','privateeventsecret')",params![id,id,kind]).unwrap();
        c.execute("INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'text','privateeventsecret')",[id]).unwrap();
    }
    let mut r = SearchRequest {
        visibility: SearchVisibility::Conversation,
        ..request("visibleconversation")
    };
    assert_eq!(
        search::search(&mut c, &r, &SearchScope::all())
            .unwrap()
            .results
            .len(),
        cases.len()
    );
    r.query = "privateeventsecret".into();
    assert!(search::search(&mut c, &r, &SearchScope::all())
        .unwrap()
        .results
        .is_empty());
    r.query = String::new();
    r.mode = SearchMode::Messages;
    let page = search::search(&mut c, &r, &SearchScope::all()).unwrap();
    assert_eq!(page.results.len(), cases.len());
    assert!(page.results.iter().all(|r| r.entry_id.unwrap() < 100));
}

#[test]
fn results_preserve_annotations_and_report_row_positions_when_ordinals_have_gaps() {
    let mut c = fixture();
    c.execute("UPDATE entry SET ordinal=900 WHERE id=12", [])
        .unwrap();
    let page = search::search(
        &mut c,
        &request("quoted phrase"),
        &SearchScope::workspaces([1]).unwrap(),
    )
    .unwrap();
    let result = &page.results[0];
    assert_eq!(result.entry_id, Some(12));
    assert_eq!(result.ordinal, Some(900));
    assert_eq!(result.position, Some(1));
    assert!(result.starred);
    assert_eq!(result.tags, vec!["review"]);
}

#[test]
fn message_text_terms_cannot_be_satisfied_or_excluded_by_session_metadata() {
    let mut c = Connection::open_in_memory().unwrap();
    db::init_schema(&c).unwrap();
    c.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');INSERT INTO workspace(id,path) VALUES(1,'/alpha');INSERT INTO session(id,agent_id,workspace_id,source_uri,title) VALUES(1,1,1,'fixture:metadata','alpha');INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(1,1,0,'message','assistant','beta');INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(1,0,'text','beta');").unwrap();
    for visibility in [
        SearchVisibility::AllIndexedText,
        SearchVisibility::Conversation,
    ] {
        let mut r = SearchRequest {
            mode: SearchMode::Messages,
            visibility,
            ..request("alpha beta")
        };
        for query in ["alpha beta", "alpha", "alpha OR missing"] {
            r.query = query.into();
            assert!(
                search::search(&mut c, &r, &SearchScope::all())
                    .unwrap()
                    .results
                    .is_empty(),
                "{visibility:?}: {query}"
            );
        }
        for query in ["beta -alpha", "agent:codex beta", "project:alpha beta"] {
            r.query = query.into();
            assert_eq!(
                search::search(&mut c, &r, &SearchScope::all())
                    .unwrap()
                    .results
                    .len(),
                1,
                "{visibility:?}: {query}"
            );
        }
        r.mode = SearchMode::Sessions;
        r.query = "alpha beta".into();
        assert_eq!(
            search::search(&mut c, &r, &SearchScope::all())
                .unwrap()
                .results
                .len(),
            1
        );
    }
}

#[test]
fn unicode_literals_have_consistent_case_matching_in_both_text_policies() {
    let mut c = Connection::open_in_memory().unwrap();
    db::init_schema(&c).unwrap();
    c.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');INSERT INTO session(id,agent_id,source_uri) VALUES(1,1,'fixture:unicode');INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(1,1,0,'message','assistant','beta É::Type C++ src/a-b.ts');INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(1,0,'text','beta É::Type C++ src/a-b.ts');").unwrap();
    for visibility in [
        SearchVisibility::AllIndexedText,
        SearchVisibility::Conversation,
    ] {
        for mode in [SearchMode::Sessions, SearchMode::Messages] {
            for query in ["\"é::Type\"", "C++", "src/A-B.ts"] {
                let r = SearchRequest {
                    mode,
                    visibility,
                    ..request(query)
                };
                let page = search::search(&mut c, &r, &SearchScope::all()).unwrap();
                assert_eq!(page.results.len(), 1, "{visibility:?}/{mode:?}: {query}");
                assert!(page.results[0].excerpt.is_some());
            }
        }
    }
}

#[test]
fn field_named_namespaces_are_literal_without_breaking_absolute_field_paths() {
    for name in [
        "agent", "project", "file", "tool", "branch", "after", "before", "has", "host",
    ] {
        let token = format!("{name}::value");
        let parsed = query::parse(&token).unwrap();
        assert_eq!(parsed.alternatives[0][0].field, None, "{token}");
        assert_eq!(parsed.alternatives[0][0].value, token);
    }
    let absolute = query::parse("file:/work/a.rs project:/work/project").unwrap();
    assert_eq!(absolute.alternatives[0][0].field, Some(query::Field::File));
    assert_eq!(
        absolute.alternatives[0][1].field,
        Some(query::Field::Project)
    );
    let mut c = fixture();
    c.execute(
        "UPDATE entry SET search_text=search_text||' agent::foo file::Type' WHERE id=11",
        [],
    )
    .unwrap();
    c.execute("UPDATE content_part SET text=text||' agent::foo file::Type' WHERE entry_id=11 AND kind='text'",[]).unwrap();
    for visibility in [
        SearchVisibility::AllIndexedText,
        SearchVisibility::Conversation,
    ] {
        for token in ["agent::foo", "file::type"] {
            let r = SearchRequest {
                mode: SearchMode::Messages,
                visibility,
                ..request(token)
            };
            let page = search::search(&mut c, &r, &SearchScope::workspaces([1]).unwrap()).unwrap();
            assert_eq!(page.results.len(), 1);
            assert_eq!(page.results[0].entry_id, Some(11));
        }
    }
}
