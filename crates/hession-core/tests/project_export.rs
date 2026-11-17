use hession_core::{
    db,
    project_export::{
        self, ExportCancellation, ExportError, ExportOptions, ExportStage, ProjectExportRequest,
        TranscriptFormat,
    },
    search::SearchScope,
    service::Library,
};
use rusqlite::Connection;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
struct Fixture {
    dir: tempfile::TempDir,
    path: PathBuf,
    source: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        let source = dir.path().join("native.jsonl");
        fs::write(&source, b"native bytes must remain unchanged\n").unwrap();
        let c = db::open_db(&path).unwrap();
        c.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');INSERT INTO workspace(id,path,display_name) VALUES(1,'/private/machine/project','project'),(2,'/other/project','other');
        INSERT INTO session(id,agent_id,workspace_id,external_id,source_uri,cwd,title,started_at,last_event_at,raw_metadata_json) VALUES(10,1,1,'native-stable','fixture://placeholder','/private/machine/project','needle api_key=title-secret /private/machine/title','2026-11-17T10:00:00Z','2026-11-17T11:00:00Z','RAW_METADATA_SECRET'),(20,1,2,'another','fixture://another','/other/project','OTHER_PROJECT_SECRET needle','2026-11-16','2026-11-16','other');
        INSERT INTO session_annotation(session_id,starred) VALUES(10,1);INSERT INTO session_tag(session_id,label) VALUES(10,'/private/machine/tag');
        INSERT INTO entry(id,session_id,ordinal,kind,role,search_text,payload_json) VALUES(100,10,0,'message','user','needle','RAW_PAYLOAD_SECRET'),(101,10,900,'message','assistant','answer','RAW_PAYLOAD_SECRET'),(102,10,901,'reasoning','assistant','private','PRIVATE_THOUGHT_SECRET'),(103,10,902,'tool_result','tool','tool','RAW_TOOL_PAYLOAD_SECRET'),(200,20,0,'message','user','needle','other');
        INSERT INTO content_part(id,entry_id,ordinal,kind,text,data_json) VALUES(1,100,0,'text','needle OPENAI_API_KEY=plain-secret-value /private/machine/project/src/lib.rs',NULL),(2,101,0,'text','Safe answer 🦀',NULL),(3,101,1,'thinking','PRIVATE_THOUGHT_SECRET',NULL),(4,102,0,'text','PRIVATE_THOUGHT_SECRET',NULL),(5,103,0,'tool_result','RAW_TOOL_BODY_MARKER password=result-secret','{\"password\":\"structured-part-secret\"}'),(6,200,0,'text','OTHER_PROJECT_SECRET',NULL);
        INSERT INTO tool_invocation(id,session_id,entry_id,call_id,tool_name,command_text,arguments_json,result_text,status) VALUES(50,10,101,'native-call','Write','RAW_TOOL_BODY_MARKER password=command-secret','{\"file_path\":\"/private/machine/project/src/lib.rs\",\"password\":\"structured-secret\"}','RAW_TOOL_BODY_MARKER password=tool-secret','success'),(51,10,NULL,'unlinked-call','Read',NULL,'{\"token\":\"unlinked-secret\"}','RAW_UNLINKED_BODY_SECRET','done');
        INSERT INTO file_activity(id,session_id,entry_id,tool_invocation_id,path,action,evidence,confidence) VALUES(1,10,101,50,'/private/machine/project/src/lib.rs','write','patch target','verified');
        INSERT INTO repository(id,common_dir,remote_url) VALUES(1,'/private/machine/project/.git','https://user:REMOTE_CREDENTIAL_SECRET@example.invalid/project');
        INSERT INTO session_context(id,session_id,entry_id,repository_id,cwd,branch,commit_sha,raw_json) VALUES(1,10,101,1,'/private/machine/project','recorded-branch','1234567890abcdef','RAW_CONTEXT_SECRET');
        INSERT INTO session_analytics(session_id,agent_id,workspace_id,input_tokens,output_tokens,total_tokens,estimated_cost) VALUES(10,1,1,100,20,120,0.05);").unwrap();
        c.execute_batch("INSERT INTO entry(id,session_id,ordinal,kind,role) VALUES(104,10,903,'tool_result','SYSTEM'),(105,10,904,'message','developer');INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(104,0,'text','SYSTEM_MESSAGE_SECRET'),(105,0,'text','DEVELOPER_MESSAGE_SECRET');").unwrap();
        c.execute(
            "UPDATE session SET source_uri=? WHERE id=10",
            [format!("codex://{}", source.display())],
        )
        .unwrap();
        c.execute("INSERT OR REPLACE INTO session_analytics(session_id,agent_id,workspace_id,input_tokens,output_tokens,total_tokens,estimated_cost) VALUES(10,1,1,100,20,120,0.05)",[]).unwrap();
        Self { dir, path, source }
    }
    fn library(&self) -> Library {
        Library::new(&self.path)
    }
    fn scope(&self) -> SearchScope {
        SearchScope::workspaces([1]).unwrap()
    }
    fn request(&self) -> ProjectExportRequest {
        ProjectExportRequest {
            workspace_id: 1,
            options: ExportOptions {
                format: TranscriptFormat::Both,
                ..Default::default()
            },
            ..Default::default()
        }
    }
    fn target(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }
    fn unchanged(&self) {
        assert_eq!(
            fs::read(&self.source).unwrap(),
            b"native bytes must remain unchanged\n"
        );
    }
}
fn members(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut result = BTreeMap::new();
    for n in 0..zip.len() {
        let mut member = zip.by_index(n).unwrap();
        assert!(member.enclosed_name().is_some());
        let mut bytes = Vec::new();
        member.read_to_end(&mut bytes).unwrap();
        assert!(result
            .insert(member.name().unwrap().into_owned(), bytes)
            .is_none());
    }
    result
}
fn write_fixture(
    f: &Fixture,
    request: &ProjectExportRequest,
    name: &str,
) -> BTreeMap<String, Vec<u8>> {
    let target = f.target(name);
    let cancel = ExportCancellation::default();
    let plan =
        project_export::prepare(&f.library(), request, &f.scope(), &target, &cancel).unwrap();
    project_export::write(&f.library(), &plan, &f.scope(), &cancel, |_| {}).unwrap();
    members(&target)
}
fn no_temps(f: &Fixture) {
    assert!(fs::read_dir(f.dir.path()).unwrap().all(|p| !p
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".hession-project-export-")));
}
#[test]
fn valid_zip_has_manifest_checksums_shared_formats_portable_links_and_safe_defaults() {
    let f = Fixture::new();
    let target = f.target("history.zip");
    let cancel = ExportCancellation::default();
    let plan =
        project_export::prepare(&f.library(), &f.request(), &f.scope(), &target, &cancel).unwrap();
    assert_eq!(plan.preview().session_count, 1);
    assert_eq!(plan.preview().selected_session_count, 1);
    assert!(plan.preview().redactions.secret_patterns > 0);
    assert!(!target.exists());
    let result = project_export::write(&f.library(), &plan, &f.scope(), &cancel, |_| {}).unwrap();
    assert_eq!(result.session_count, 1);
    assert!(!cancel.cancel());
    let files = members(&target);
    let all = files
        .values()
        .flat_map(|b| b.iter().copied())
        .collect::<Vec<_>>();
    let all = String::from_utf8(all).unwrap();
    for secret in [
        "plain-secret-value",
        "title-secret",
        "RAW_TOOL_BODY_MARKER",
        "RAW_UNLINKED_BODY_SECRET",
        "structured-secret",
        "PRIVATE_THOUGHT_SECRET",
        "SYSTEM_MESSAGE_SECRET",
        "DEVELOPER_MESSAGE_SECRET",
        "RAW_PAYLOAD_SECRET",
        "RAW_METADATA_SECRET",
        "RAW_CONTEXT_SECRET",
        "OTHER_PROJECT_SECRET",
        "REMOTE_CREDENTIAL_SECRET",
        "/private/machine",
        "native.jsonl",
    ] {
        assert!(!all.contains(secret), "leaked {secret}");
    }
    assert!(!files.contains_key("activity/git.json"));
    assert!(!files.contains_key("analytics/summary.json"));
    let manifest: Value = serde_json::from_slice(&files["manifest.json"]).unwrap();
    assert_eq!(manifest["schemaVersion"], 1);
    assert_eq!(manifest["sessionCount"], 1);
    for item in manifest["checksums"].as_array().unwrap() {
        let name = item["path"].as_str().unwrap();
        assert_eq!(
            format!("{:x}", Sha256::digest(&files[name])),
            item["sha256"].as_str().unwrap()
        );
    }
    let json_name = files
        .keys()
        .find(|p| p.starts_with("sessions/") && p.ends_with(".json"))
        .unwrap();
    assert!(json_name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"-./".contains(&b)));
    let transcript: Value = serde_json::from_slice(&files[json_name]).unwrap();
    let session = &transcript["sessions"][0];
    assert!(session["session"]["id"]
        .as_str()
        .unwrap()
        .starts_with("session-"));
    assert_eq!(session["entries"].as_array().unwrap().len(), 2);
    let activity: Value = serde_json::from_slice(&files["activity/files.json"]).unwrap();
    let row = &activity["evidence"][0];
    assert_eq!(row["category"], "reported_write_intent");
    assert_eq!(row["path"], "src/lib.rs");
    assert_eq!(row["source"]["entryId"], session["entries"][1]["id"]);
    assert_eq!(
        row["source"]["toolInvocationId"],
        session["entries"][1]["tools"][0]["id"]
    );
    assert!(files.keys().any(|p| p.ends_with(".md")));
    f.unchanged();
    no_temps(&f);
}
#[test]
fn explicit_tool_git_statistics_and_path_options_are_independent() {
    let f = Fixture::new();
    let mut request = f.request();
    request.options.include_tool_bodies = true;
    request.options.include_git_metadata = true;
    request.options.include_statistics = true;
    let files = write_fixture(&f, &request, "opt-in.zip");
    let all = String::from_utf8(files.values().flat_map(|b| b.iter().copied()).collect()).unwrap();
    assert!(all.contains("RAW_TOOL_BODY_MARKER"));
    assert!(all.contains("recorded-branch"));
    for secret in [
        "structured-secret",
        "structured-part-secret",
        "command-secret",
        "tool-secret",
        "unlinked-secret",
        "REMOTE_CREDENTIAL_SECRET",
        "PRIVATE_THOUGHT_SECRET",
        "SYSTEM_MESSAGE_SECRET",
        "DEVELOPER_MESSAGE_SECRET",
        "/private/machine",
    ] {
        assert!(!all.contains(secret), "leaked {secret}");
    }
    let stats: Value = serde_json::from_slice(&files["analytics/summary.json"]).unwrap();
    assert_eq!(stats["totals"]["total_tokens"], 120.0);
    request.options.include_absolute_paths = true;
    request.options.redact = false;
    let files = write_fixture(&f, &request, "risky.zip");
    let all = String::from_utf8(files.values().flat_map(|b| b.iter().copied()).collect()).unwrap();
    assert!(all.contains("/private/machine/project"));
    assert!(all.contains("structured-secret"));
    assert!(!all.contains("REMOTE_CREDENTIAL_SECRET"));
    assert!(!all.contains("PRIVATE_THOUGHT_SECRET"));
    f.unchanged();
}
#[test]
fn scope_query_grammar_caps_and_existing_destinations_are_enforced() {
    let f = Fixture::new();
    let cancel = ExportCancellation::default();
    let mut request = f.request();
    request.query = "needle OR project:other".into();
    assert_eq!(
        project_export::prepare(
            &f.library(),
            &request,
            &f.scope(),
            &f.target("scope.zip"),
            &cancel
        )
        .unwrap()
        .preview()
        .session_count,
        1
    );
    assert!(project_export::prepare(
        &f.library(),
        &request,
        &SearchScope::workspaces([]).unwrap(),
        &f.target("denied.zip"),
        &cancel
    )
    .is_err());
    request.workspace_id = 2;
    assert!(project_export::prepare(
        &f.library(),
        &request,
        &f.scope(),
        &f.target("other.zip"),
        &cancel
    )
    .is_err());
    request.workspace_id = 1;
    assert!(
        project_export::prepare(&f.library(), &request, &f.scope(), &f.source, &cancel).is_err()
    );
    assert!(project_export::prepare(&f.library(), &request, &f.scope(), &f.path, &cancel).is_err());
    let c = db::open_db(&f.path).unwrap();
    c.execute(
        "UPDATE content_part SET text=? WHERE id=1",
        ["x".repeat(2 * 1024 * 1024)],
    )
    .unwrap();
    assert!(matches!(
        project_export::prepare(
            &f.library(),
            &request,
            &f.scope(),
            &f.target("large.zip"),
            &cancel
        ),
        Err(ExportError::Limit(_))
    ));
    f.unchanged();
    no_temps(&f);
}
#[test]
fn cancellation_during_writing_and_finalization_leaves_no_partial_destination() {
    for stage in [ExportStage::Writing, ExportStage::Finalizing] {
        let f = Fixture::new();
        let target = f.target("cancelled.zip");
        let cancel = ExportCancellation::default();
        let plan =
            project_export::prepare(&f.library(), &f.request(), &f.scope(), &target, &cancel)
                .unwrap();
        let failure = project_export::write(&f.library(), &plan, &f.scope(), &cancel, |progress| {
            if std::mem::discriminant(&progress.stage) == std::mem::discriminant(&stage) {
                assert!(cancel.cancel());
            }
        });
        assert!(matches!(failure, Err(ExportError::Cancelled)));
        assert!(!target.exists());
        f.unchanged();
        no_temps(&f);
    }
}
#[test]
fn stale_plans_destination_races_and_output_bounds_fail_without_source_damage() {
    let f = Fixture::new();
    let target = f.target("stale.zip");
    let cancel = ExportCancellation::default();
    let plan =
        project_export::prepare(&f.library(), &f.request(), &f.scope(), &target, &cancel).unwrap();
    db::open_db(&f.path)
        .unwrap()
        .execute("UPDATE session SET title='changed' WHERE id=10", [])
        .unwrap();
    assert!(matches!(
        project_export::write(&f.library(), &plan, &f.scope(), &cancel, |_| {}),
        Err(ExportError::StalePreview)
    ));
    assert!(!target.exists());
    let target = f.target("raced.zip");
    let cancel = ExportCancellation::default();
    let plan =
        project_export::prepare(&f.library(), &f.request(), &f.scope(), &target, &cancel).unwrap();
    let result = project_export::write(&f.library(), &plan, &f.scope(), &cancel, |p| {
        if matches!(p.stage, ExportStage::Finalizing) {
            std::os::unix::fs::symlink(&f.source, &target).unwrap();
        }
    });
    assert!(result.is_err());
    assert!(fs::symlink_metadata(&target)
        .unwrap()
        .file_type()
        .is_symlink());
    let mut request = f.request();
    request.query = "nothing-matches-this".into();
    request.limits.max_archive_bytes = 64;
    let cancel = ExportCancellation::default();
    let tiny = f.target("tiny.zip");
    let plan = project_export::prepare(&f.library(), &request, &f.scope(), &tiny, &cancel).unwrap();
    assert!(project_export::write(&f.library(), &plan, &f.scope(), &cancel, |_| {}).is_err());
    assert!(!tiny.exists());
    f.unchanged();
    no_temps(&f);
}
#[test]
fn archive_session_names_do_not_depend_on_local_database_row_ids() {
    let f = Fixture::new();
    let first = write_fixture(&f, &f.request(), "first.zip");
    let copy = f.target("copy.sqlite");
    Connection::open(&f.path)
        .unwrap()
        .backup(
            rusqlite::MAIN_DB,
            &copy,
            None::<fn(rusqlite::backup::Progress)>,
        )
        .unwrap();
    let c = Connection::open(&copy).unwrap();
    c.execute_batch("PRAGMA foreign_keys=OFF;BEGIN;UPDATE session SET id=id+1000;UPDATE entry SET id=id+10000,session_id=session_id+1000;UPDATE content_part SET entry_id=entry_id+10000;UPDATE tool_invocation SET id=id+1000,session_id=session_id+1000,entry_id=entry_id+10000;UPDATE file_activity SET session_id=session_id+1000,entry_id=entry_id+10000,tool_invocation_id=tool_invocation_id+1000;UPDATE session_context SET session_id=session_id+1000,entry_id=entry_id+10000;UPDATE session_tag SET session_id=session_id+1000;UPDATE session_annotation SET session_id=session_id+1000;UPDATE session_analytics SET session_id=session_id+1000;COMMIT;").unwrap();
    let target = f.target("copy.zip");
    let cancel = ExportCancellation::default();
    let plan = project_export::prepare(
        &Library::new(&copy),
        &f.request(),
        &f.scope(),
        &target,
        &cancel,
    )
    .unwrap();
    project_export::write(&Library::new(&copy), &plan, &f.scope(), &cancel, |_| {}).unwrap();
    let second = members(&target);
    let names = |files: &BTreeMap<String, Vec<u8>>| {
        files
            .keys()
            .filter(|p| p.starts_with("sessions/"))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&first), names(&second));
    f.unchanged();
}
#[test]
fn preparation_is_read_only_and_pre_cancelled_jobs_do_not_create_files() {
    let f = Fixture::new();
    let missing = f.target("missing.sqlite");
    assert!(project_export::prepare(
        &Library::new(&missing),
        &f.request(),
        &f.scope(),
        &f.target("none.zip"),
        &ExportCancellation::default()
    )
    .is_err());
    assert!(!missing.exists());
    let cancel = ExportCancellation::default();
    cancel.cancel();
    assert!(matches!(
        project_export::prepare(
            &f.library(),
            &f.request(),
            &f.scope(),
            &f.target("cancel.zip"),
            &cancel
        ),
        Err(ExportError::Cancelled)
    ));
    assert!(!f.target("cancel.zip").exists());
    f.unchanged();
    no_temps(&f);
}

#[test]
fn cached_statistics_never_drift_from_the_prepared_preview() {
    let f = Fixture::new();
    let mut request = f.request();
    request.options.include_statistics = true;
    let target = f.target("pinned-stats.zip");
    let cancel = ExportCancellation::default();
    let plan =
        project_export::prepare(&f.library(), &request, &f.scope(), &target, &cancel).unwrap();
    db::open_db(&f.path)
        .unwrap()
        .execute(
            "UPDATE session_analytics SET total_tokens=999 WHERE session_id=10",
            [],
        )
        .unwrap();
    match project_export::write(&f.library(), &plan, &f.scope(), &cancel, |_| {}) {
        Ok(_) => {
            let output = members(&target);
            let summary: Value = serde_json::from_slice(&output["analytics/summary.json"]).unwrap();
            assert_eq!(summary["totals"]["total_tokens"], 120.0);
        }
        Err(error) => assert!(matches!(error, ExportError::StalePreview)),
    }
}
#[test]
fn native_and_synced_history_share_portable_identity_without_database_ids() {
    let f = Fixture::new();
    let first = write_fixture(&f, &f.request(), "native.zip");
    let mut digest = Sha256::new();
    for part in [
        b"hession-session-v1".as_slice(),
        b"codex",
        b"native-stable",
        b"2026-11-17T10:00:00Z",
    ] {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    let portable = format!("{:x}", digest.finalize());
    db::open_db(&f.path)
        .unwrap()
        .execute(
            "UPDATE session SET source_uri=?,external_id=NULL WHERE id=10",
            [format!("hession-sync://{portable}/revision")],
        )
        .unwrap();
    let second = write_fixture(&f, &f.request(), "synced.zip");
    let session_members = |files: &BTreeMap<String, Vec<u8>>| {
        files
            .keys()
            .filter(|name| name.starts_with("sessions/"))
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(session_members(&first), session_members(&second));
    f.unchanged();
}

#[test]
fn hostile_recorded_identifiers_and_paths_never_become_zip_member_paths() {
    let f = Fixture::new();
    let c = db::open_db(&f.path).unwrap();
    c.execute(
        "UPDATE session SET external_id=? WHERE id=10",
        ["../../escape\\\0unusual\nname"],
    )
    .unwrap();
    c.execute(
        "UPDATE file_activity SET path='../outside/unusual\nname' WHERE id=1",
        [],
    )
    .unwrap();
    let output = write_fixture(&f, &f.request(), "hostile.zip");
    for name in output.keys() {
        assert!(!name.split('/').any(|part| part == ".."));
        assert!(!name.contains('\\'));
        assert!(!name.contains('\0'));
        assert!(!name.contains('\n'));
    }
    let activity: Value = serde_json::from_slice(&output["activity/files.json"]).unwrap();
    assert_eq!(
        activity["evidence"][0]["resolution"],
        "unresolved_recorded_path"
    );
    f.unchanged();
}

#[test]
fn redaction_expansion_is_included_in_the_entry_memory_budget() {
    let f = Fixture::new();
    db::open_db(&f.path)
        .unwrap()
        .execute(
            "UPDATE content_part SET text=? WHERE id=1",
            ["/a ".repeat(20_000)],
        )
        .unwrap();
    let mut request = f.request();
    request.limits.max_entry_bytes = 100 * 1024;
    let target = f.target("expanded.zip");
    assert!(matches!(
        project_export::prepare(
            &f.library(),
            &request,
            &f.scope(),
            &target,
            &ExportCancellation::default()
        ),
        Err(ExportError::Limit(_))
    ));
    assert!(!target.exists());
    f.unchanged();
}
