use hession_core::{
    db,
    projects::*,
    service::{Library, Page},
};
use rusqlite::Connection;
use std::{fs, io::Read, path::Path};
fn fixture() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    db::init_schema(&c).unwrap();
    c.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
    INSERT INTO workspace(id,path,display_name) VALUES(1,'/work/proj','proj'),(2,'box:/work/proj','proj'),(3,'/work/other','other');
    INSERT INTO session(id,agent_id,workspace_id,source_uri,cwd,host) VALUES(1,1,1,'test://one','/work/proj',NULL),(2,1,2,'test://remote','/work/proj','box'),(3,1,3,'test://other','/work/other',NULL);
    INSERT INTO entry(id,session_id,ordinal,kind,occurred_at) VALUES(1,1,4,'tool','2026-11-17'),(2,2,7,'tool','2026-11-18'),(3,3,0,'tool','2026-11-16');
    INSERT INTO tool_invocation(id,session_id,entry_id,tool_name,status) VALUES(1,1,1,'Write','success'),(2,2,2,'Read','success');
    INSERT INTO repository(id,common_dir,remote_url) VALUES(1,'/old/.git','ssh://recorded');
    INSERT INTO session_context(id,session_id,entry_id,cwd,branch,commit_sha,repository_id,observed_at) VALUES(1,1,1,'/work/proj','recorded','abc',1,'yesterday');
    INSERT INTO file_activity(id,session_id,entry_id,tool_invocation_id,path,action,evidence) VALUES
    (1,1,1,1,'src/a.rs','write','tool arguments'),(2,1,1,1,'../escape','read','tool arguments'),
    (3,2,2,2,'src/a.rs','read','tool arguments'),(4,3,3,NULL,'secret','read','tool arguments');").unwrap();
    c
}
#[test]
fn scoped_evidence_preserves_source_ids_and_never_upgrades_reported_success() {
    let c = fixture();
    let scope = ProjectScope::workspaces([1]).unwrap();
    let page = activity_on_connection(&c, 1, &scope, Page::default(), None).unwrap();
    assert_eq!(page.evidence.len(), 2);
    let item = page.evidence.iter().find(|f| f.id == 1).unwrap();
    assert_eq!(item.category, EvidenceCategory::ReportedWriteIntent);
    assert_eq!(item.source.entry_ordinal, Some(4));
    assert_eq!(item.source.tool_invocation_id, Some(1));
    assert_eq!(
        item.candidate_workspace_relative_path.as_deref(),
        Some("src/a.rs")
    );
    assert!(page
        .evidence
        .iter()
        .find(|f| f.id == 2)
        .unwrap()
        .candidate_workspace_relative_path
        .is_none());
    assert!(matches!(
        activity_on_connection(&c, 2, &scope, Page::default(), None),
        Err(ProjectError::OutsideScope)
    ));
    assert!(matches!(
        recorded_git_on_connection(&c, 3, &scope, Page::default()),
        Err(ProjectError::OutsideScope)
    ));
    assert!(project_on_connection(&c, 1, &ProjectScope::workspaces([]).unwrap()).is_err());
    let git = recorded_git_on_connection(&c, 1, &scope, Page::default()).unwrap();
    assert_eq!(git[0].branch.as_deref(), Some("recorded"));
    assert_eq!(git[0].source.entry_ordinal, Some(4));
    let remote = activity_on_connection(
        &c,
        2,
        &ProjectScope::workspaces([2]).unwrap(),
        Page::default(),
        None,
    )
    .unwrap();
    assert_eq!(remote.project.host.as_deref(), Some("box"));
    assert!(remote.evidence[0]
        .candidate_workspace_relative_path
        .is_none());
}
#[test]
fn lexical_paths_never_use_process_cwd_or_allow_traversal() {
    assert_eq!(
        relative_candidate("./src/a", Some("/work/proj/sub"), "/work/proj", false).as_deref(),
        Some("sub/src/a")
    );
    for (p, cwd) in [
        ("../outside", Some("/work/proj")),
        ("src/a", None),
        ("/work/projectevil/a", Some("/work/proj")),
        ("a\0b", Some("/work/proj")),
    ] {
        assert!(relative_candidate(p, cwd, "/work/proj", false).is_none());
    }
}
#[test]
fn opening_rejects_symlink_traversal_special_and_deleted_files() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(root.path().join("ok"), "safe").unwrap();
    fs::write(outside.path().join("secret"), "private").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret"), root.path().join("link")).unwrap();
    let mut contents = String::new();
    open_project_file(root.path(), Path::new("ok"))
        .unwrap()
        .read_to_string(&mut contents)
        .unwrap();
    assert_eq!(contents, "safe");
    for path in [
        "../secret",
        "escape/secret",
        "link",
        "missing",
        "/etc/passwd",
        ".",
    ] {
        assert!(
            open_project_file(root.path(), Path::new(path)).is_err(),
            "{path}"
        );
    }
}
#[test]
fn project_service_scope_is_applied_before_pagination_and_remote_git_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite");
    let c = fixture();
    c.backup(
        rusqlite::MAIN_DB,
        &path,
        None::<fn(rusqlite::backup::Progress)>,
    )
    .unwrap();
    let library = Library::new(path);
    let projects = library
        .get_projects(
            &ProjectScope::workspaces([2]).unwrap(),
            Page {
                limit: 1,
                offset: 0,
            },
        )
        .unwrap();
    assert_eq!(projects[0].workspace_id, 2);
    assert!(library
        .get_projects(&ProjectScope::workspaces([]).unwrap(), Page::default())
        .unwrap()
        .is_empty());
    assert!(library
        .inspect_project_git(
            2,
            &ProjectScope::workspaces([2]).unwrap(),
            Default::default()
        )
        .is_err());
    let filtered = library
        .get_file_history(
            1,
            "src/a.rs",
            &ProjectScope::workspaces([1]).unwrap(),
            Page::default(),
        )
        .unwrap();
    assert_eq!(filtered.evidence.len(), 1);
}
#[test]
fn pagination_and_caps_are_explicit() {
    let c = fixture();
    let scope = ProjectScope::workspaces([1]).unwrap();
    let page = activity_on_connection(
        &c,
        1,
        &scope,
        Page {
            limit: 1,
            offset: 0,
        },
        None,
    )
    .unwrap();
    assert_eq!(page.next_offset, Some(1));
    assert!(activity_on_connection(
        &c,
        1,
        &scope,
        Page {
            limit: 501,
            offset: 0
        },
        None
    )
    .is_err());
    assert!(
        activity_on_connection(&c, 1, &scope, Page::default(), Some(&"x".repeat(4097))).is_err()
    );
}

#[test]
fn portable_project_keys_do_not_depend_on_database_ids() {
    let c = fixture();
    let scope = ProjectScope::workspaces([1, 2]).unwrap();
    let local = project_on_connection(&c, 1, &scope).unwrap();
    let remote = project_on_connection(&c, 2, &scope).unwrap();
    assert_ne!(local.project_key, remote.project_key);
    assert_eq!(local.project_key, project_key("/work/proj"));
    assert_eq!(remote.project_key, project_key("box:/work/proj"));
    c.execute("UPDATE workspace SET id=20 WHERE id=1", [])
        .unwrap_err();
    // The same recorded identity has the same key in an unrelated database.
    let other = Connection::open_in_memory().unwrap();
    db::init_schema(&other).unwrap();
    other
        .execute(
            "INSERT INTO workspace(id,path) VALUES(500,'/work/proj')",
            [],
        )
        .unwrap();
    assert_eq!(
        project_on_connection(&other, 500, &ProjectScope::workspaces([500]).unwrap())
            .unwrap()
            .project_key,
        local.project_key
    );
}

#[test]
fn file_summary_deduplicates_references_but_drilldown_retains_every_row() {
    let c = fixture();
    let scope = ProjectScope::workspaces([1]).unwrap();
    c.execute("INSERT INTO file_activity(session_id,entry_id,tool_invocation_id,path,action) VALUES(1,1,1,'src/a.rs','write')",[]).unwrap();
    let files = files_on_connection(&c, 1, &scope, Page::default(), Some("a.rs")).unwrap();
    assert_eq!(files.files.len(), 1);
    assert_eq!(files.files[0].reported_write_intent_count, 1);
    assert_eq!(
        activity_on_connection(&c, 1, &scope, Page::default(), Some("src/a.rs"))
            .unwrap()
            .evidence
            .len(),
        2
    );
    assert!(files_on_connection(
        &c,
        1,
        &ProjectScope::workspaces([]).unwrap(),
        Page::default(),
        None
    )
    .is_err());
    c.execute("UPDATE entry SET occurred_at=NULL WHERE id=1", [])
        .unwrap();
    c.execute(
        "UPDATE session SET last_event_at='2099-01-01' WHERE id=1",
        [],
    )
    .unwrap();
    let files = files_on_connection(&c, 1, &scope, Page::default(), Some("a.rs")).unwrap();
    assert!(
        files.files[0].last_activity_at.is_none(),
        "A session timestamp must not become an inferred file-event timestamp"
    );
}
#[test]
fn scoped_selectors_are_exact_and_never_follow_matching_path_on_another_host() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite");
    fixture()
        .backup(
            rusqlite::MAIN_DB,
            &path,
            None::<fn(rusqlite::backup::Progress)>,
        )
        .unwrap();
    let library = Library::new(path);
    let scope = ProjectScope::workspaces([2]).unwrap();
    let selected = library
        .resolve_project(
            &ProjectSelector::RecordedPath("box:/work/proj".into()),
            &scope,
        )
        .unwrap();
    assert_eq!(selected.workspace_id, 2);
    assert!(library
        .resolve_project(&ProjectSelector::RecordedPath("/work/proj".into()), &scope)
        .is_err());
    assert!(library
        .resolve_project(&ProjectSelector::WorkspaceId(1), &scope)
        .is_err());
    assert!(library
        .resolve_project(&ProjectSelector::RecordedPath("' OR 1=1 --".into()), &scope)
        .is_err());
    assert!(library
        .get_project_files(2, &scope, Page::default(), Some("' OR 1=1 --"))
        .unwrap()
        .files
        .is_empty());

    assert!(library
        .open_recorded_project_file(2, Path::new("a"), &scope)
        .is_err());
}

#[test]
fn synced_only_history_does_not_claim_the_matching_local_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.sqlite");
    let c = fixture();
    c.execute(
        "UPDATE session SET source_uri='hession-sync://session/revision' WHERE id=1",
        [],
    )
    .unwrap();
    c.backup(
        rusqlite::MAIN_DB,
        &path,
        None::<fn(rusqlite::backup::Progress)>,
    )
    .unwrap();
    let scope = ProjectScope::workspaces([1]).unwrap();
    let library = Library::new(path);
    let project = library
        .resolve_project(&ProjectSelector::WorkspaceId(1), &scope)
        .unwrap();
    assert_eq!(project.local_session_count, 0);
    assert_eq!(project.synced_session_count, 1);
    assert!(library
        .inspect_project_git(1, &scope, Default::default())
        .is_err());
    assert!(library
        .open_recorded_project_file(1, Path::new("src/a.rs"), &scope)
        .is_err());
    assert!(library
        .get_project_activity(1, &scope, Page::default())
        .unwrap()
        .evidence
        .iter()
        .all(|row| row.candidate_workspace_relative_path.is_none()));
}
