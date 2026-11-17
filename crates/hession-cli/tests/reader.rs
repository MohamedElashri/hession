mod common;
use common::{no_secrets, Fixture};
use hession_core::{
    projects::ProjectSelector,
    reader::{ReadCancellation, ReadError, ReadPage, ScopedReader},
    search::{SearchRequest, SearchVisibility},
    service::Library,
};
#[test]
fn scoped_direct_id_reads_never_fetch_or_return_disallowed_transcripts_or_tool_bodies() {
    let f = Fixture::new();
    let cancel = ReadCancellation::default();
    for expose in [false, true] {
        let reader = f.reader(expose);
        for id in [2, 999] {
            assert!(matches!(
                reader.session(id, &ReadPage::default(), &cancel),
                Err(ReadError::OutsideScope)
            ));
            assert!(matches!(
                reader.tool_calls(id, &ReadPage::default(), &cancel),
                Err(ReadError::OutsideScope)
            ));
        }
        let session = reader.session(1, &ReadPage::default(), &cancel).unwrap();
        no_secrets(&session);
        if expose {
            assert_eq!(session["entries"].as_array().unwrap().len(), 2);
            assert_eq!(session["entries"][1]["ordinal"], 900);
            assert_eq!(session["entries"][1]["text"], "visible reply");
        } else {
            assert_eq!(session["entries"], serde_json::json!([]));
        }
        let tools = reader.tool_calls(1, &ReadPage::default(), &cancel).unwrap();
        no_secrets(&tools);
        assert_eq!(tools["tools"].as_array().unwrap().len(), 2);
        assert!(tools["tools"][1]["entryId"].is_null());
    }
}
#[test]
fn owner_visibility_cannot_be_elevated_by_search_request_and_hidden_text_never_matches() {
    let f = Fixture::new();
    let cancel = ReadCancellation::default();
    for expose in [false, true] {
        let reader = f.reader(expose);
        for query in [
            "THINKING_SECRET",
            "TOOL_SECRET",
            "REASONING_EVENT_SECRET",
            "SYSTEM_SECRET",
            "LEGACY_COMBINED_SECRET",
            "DENIED_TRANSCRIPT_SECRET",
        ] {
            let page = reader
                .search(
                    &SearchRequest {
                        query: query.into(),
                        visibility: SearchVisibility::AllIndexedText,
                        ..Default::default()
                    },
                    &cancel,
                )
                .unwrap();
            assert!(page.results.is_empty(), "{expose}: {query}");
        }
        let visible = reader
            .search(
                &SearchRequest {
                    query: "visible".into(),
                    ..Default::default()
                },
                &cancel,
            )
            .unwrap();
        assert_eq!(visible.results.len(), usize::from(expose));
    }
}
#[test]
fn library_and_project_bindings_fail_closed_when_ids_are_reused() {
    let f = Fixture::new();
    let reader = f.reader(true);
    let cancel = ReadCancellation::default();
    let conn = hession_core::db::open_db(&f.path).unwrap();
    conn.execute("UPDATE workspace SET path='/new/private' WHERE id=1", [])
        .unwrap();
    assert!(matches!(
        reader.session(1, &ReadPage::default(), &cancel),
        Err(ReadError::ScopeChanged)
    ));
    conn.execute(
        "UPDATE workspace SET path='/fixture/allowed' WHERE id=1",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE library_revision SET identity='replacement' WHERE id=1",
        [],
    )
    .unwrap();
    assert!(matches!(
        reader.projects_page(&ReadPage::default(), &cancel),
        Err(ReadError::ScopeChanged)
    ));
}
#[test]
fn cursors_are_bound_to_operation_limit_permissions_and_snapshot_revision() {
    let f = Fixture::new();
    let reader = f.reader(true);
    let cancel = ReadCancellation::default();
    let first = reader
        .session(
            1,
            &ReadPage {
                limit: 1,
                cursor: None,
            },
            &cancel,
        )
        .unwrap();
    let cursor = first["nextCursor"].as_str().unwrap().to_owned();
    let second = reader
        .session(
            1,
            &ReadPage {
                limit: 1,
                cursor: Some(cursor.clone()),
            },
            &cancel,
        )
        .unwrap();
    assert_eq!(second["entries"][0]["id"], 2);
    assert!(matches!(
        reader.tool_calls(
            1,
            &ReadPage {
                limit: 1,
                cursor: Some(cursor.clone())
            },
            &cancel
        ),
        Err(ReadError::InvalidCursor)
    ));
    assert!(matches!(
        reader.session(
            1,
            &ReadPage {
                limit: 2,
                cursor: Some(cursor.clone())
            },
            &cancel
        ),
        Err(ReadError::InvalidCursor)
    ));
    hession_core::db::open_db(&f.path)
        .unwrap()
        .execute(
            "UPDATE content_part SET text='updated' WHERE entry_id=1",
            [],
        )
        .unwrap();
    assert!(matches!(
        reader.session(
            1,
            &ReadPage {
                limit: 1,
                cursor: Some(cursor)
            },
            &cancel
        ),
        Err(ReadError::StaleCursor)
    ));
}
#[test]
fn empty_scopes_traversal_cancellation_and_page_limits_are_explicit_errors() {
    let f = Fixture::new();
    assert!(ScopedReader::projects(Library::new(&f.path), &[], false).is_err());
    assert!(ScopedReader::projects(
        Library::new(&f.path),
        &[ProjectSelector::RecordedPath(
            "/fixture/allowed/../denied".into()
        )],
        false
    )
    .is_err());
    let reader = f.reader(true);
    let cancel = ReadCancellation::default();
    cancel.cancel();
    assert!(matches!(
        reader.session(1, &ReadPage::default(), &cancel),
        Err(ReadError::Interrupted)
    ));
    for limit in [0, 101] {
        assert!(reader
            .session(
                1,
                &ReadPage {
                    limit,
                    cursor: None
                },
                &ReadCancellation::default()
            )
            .is_err());
    }
    assert!(reader
        .session(
            1,
            &ReadPage {
                limit: 1,
                cursor: Some("x".repeat(4097))
            },
            &ReadCancellation::default()
        )
        .is_err());
    let missing = f.dir.path().join("missing/never.sqlite");
    assert!(ScopedReader::unrestricted(Library::new(&missing), true).is_err());
    assert!(!missing.parent().unwrap().exists());
}
#[test]
fn very_large_raw_payloads_are_not_loaded_by_safe_session_projection() {
    let f = Fixture::new();
    let conn = hession_core::db::open_db(&f.path).unwrap();
    conn.execute(
        "UPDATE entry SET payload_json=? WHERE id=2",
        ["SECRET_RAW".repeat(300_000)],
    )
    .unwrap();
    let value = f
        .reader(true)
        .session(1, &ReadPage::default(), &ReadCancellation::default())
        .unwrap();
    assert_eq!(value["entries"][1]["text"], "visible reply");
    no_secrets(&value);
    assert!(serde_json::to_vec(&value).unwrap().len() < 8192);
}

#[test]
fn huge_visible_cells_use_bounded_prefix_reads_and_search_rejects_them_before_materialization() {
    let f = Fixture::new();
    let conn = hession_core::db::open_db(&f.path).unwrap();
    conn.execute("UPDATE content_part SET text='visible prefix' || CAST(zeroblob(16*1024*1024) AS TEXT) WHERE entry_id=1",[]).unwrap();
    let cancel = ReadCancellation::default();
    let visible = f.reader(true);
    let page = visible.session(1, &ReadPage::default(), &cancel).unwrap();
    assert!(page["entries"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("visible prefix"));
    assert_eq!(
        page["entries"][0]["text"].as_str().unwrap().chars().count(),
        2048
    );
    assert!(matches!(
        visible.search(
            &SearchRequest {
                query: "visible".into(),
                ..Default::default()
            },
            &cancel
        ),
        Err(ReadError::DataTooLarge)
    ));
    assert!(f
        .reader(false)
        .search(
            &SearchRequest {
                query: "visible".into(),
                ..Default::default()
            },
            &cancel
        )
        .unwrap()
        .results
        .is_empty());
}
#[test]
fn huge_excluded_or_denied_text_and_raw_file_evidence_do_not_enter_safe_projections() {
    let f = Fixture::new();
    let conn = hession_core::db::open_db(&f.path).unwrap();
    conn.execute_batch("BEGIN;UPDATE content_part SET text=CAST(zeroblob(8*1024*1024) AS TEXT) WHERE entry_id IN(3,7);UPDATE file_activity SET evidence=CAST(zeroblob(8*1024*1024) AS TEXT) WHERE id=1;COMMIT;").unwrap();
    let cancel = ReadCancellation::default();
    let reader = f.reader(true);
    assert_eq!(
        reader
            .search(
                &SearchRequest {
                    query: "visible".into(),
                    ..Default::default()
                },
                &cancel
            )
            .unwrap()
            .results
            .len(),
        1
    );
    let activity = reader
        .project_activity(1, None, &ReadPage::default(), &cancel)
        .unwrap();
    assert!(activity["evidence"][0].get("evidence").is_none());
    assert!(serde_json::to_vec(&activity).unwrap().len() < 4096);
}
#[test]
fn oversized_metadata_is_omitted_or_rejected_before_shared_compiler_materialization() {
    let f = Fixture::new();
    let conn = hession_core::db::open_db(&f.path).unwrap();
    conn.execute(
        "UPDATE session SET title=CAST(zeroblob(4*1024*1024) AS TEXT) WHERE id=1",
        [],
    )
    .unwrap();
    let cancel = ReadCancellation::default();
    let reader = f.reader(false);
    assert_eq!(
        reader.session(1, &ReadPage::default(), &cancel).unwrap()["session"]["title"],
        "[metadata exceeds limit]"
    );
    assert!(matches!(
        reader.search(&SearchRequest::default(), &cancel),
        Err(ReadError::DataTooLarge)
    ));
}

#[test]
fn reported_external_paths_are_literal_metadata_and_owner_aliases_never_expand_scope() {
    let f = Fixture::new();
    let conn = hession_core::db::open_db(&f.path).unwrap();
    conn.execute("INSERT INTO file_activity(session_id,path,action,evidence) VALUES(1,'../outside.txt','read','EVIDENCE_SECRET')",[]).unwrap();
    let reader = f.reader(false);
    let cancel = ReadCancellation::default();
    let activity = reader
        .project_activity(1, Some("../outside.txt"), &ReadPage::default(), &cancel)
        .unwrap();
    assert_eq!(activity["evidence"].as_array().unwrap().len(), 1);
    assert_eq!(activity["evidence"][0]["reported_path"], "../outside.txt");
    #[cfg(unix)]
    {
        let target = f.dir.path().join("recorded-root");
        std::fs::create_dir(&target).unwrap();
        let alias = f.dir.path().join("alias-root");
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        conn.execute(
            "UPDATE workspace SET path=? WHERE id=1",
            [target.to_str().unwrap()],
        )
        .unwrap();
        assert!(ScopedReader::projects(
            Library::new(&f.path),
            &[ProjectSelector::RecordedPath(
                alias.to_str().unwrap().into()
            )],
            false
        )
        .is_err());
        assert!(ScopedReader::projects(
            Library::new(&f.path),
            &[ProjectSelector::RecordedPath(
                target.to_str().unwrap().into()
            )],
            false
        )
        .is_ok());
    }
}
