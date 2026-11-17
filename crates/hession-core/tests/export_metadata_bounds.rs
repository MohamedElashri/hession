use hession_core::{
    db,
    project_export::{self, ExportCancellation, ExportError, ProjectExportRequest},
    search::SearchScope,
    service::Library,
};
use std::{io::Read, path::Path};

const HUGE: i64 = 64 * 1024 * 1024;

#[test]
fn metadata_limits_precede_allocation_and_private_or_denied_text_stays_unread() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.sqlite");
    let native = dir.path().join("native.jsonl");
    std::fs::write(&native, b"synthetic native sentinel\n").unwrap();
    let conn = db::open_db(&path).unwrap();
    conn.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex'),(2,'denied','Denied');
        INSERT INTO workspace(id,path) VALUES(1,'/synthetic/allowed'),(2,'remote:/synthetic/denied');
        INSERT INTO session(id,agent_id,workspace_id,source_uri,external_id,title) VALUES
        (1,1,1,'fixture://allowed','native-1','Allowed'),(2,2,2,'fixture://denied','native-2','DENIED_CANARY');
        INSERT INTO entry(id,session_id,ordinal,kind,role,occurred_at,payload_json) VALUES
        (1,1,0,'message','assistant','2026-11-17','{}'),(2,1,9000,'reasoning','assistant',NULL,'{}'),(3,2,0,'message','user',NULL,'{}');
        INSERT INTO content_part(id,entry_id,ordinal,kind,text) VALUES
        (1,1,0,'text','Visible reply'),(2,2,0,'text','PRIVATE_CANARY'),(3,3,0,'text','DENIED_CANARY');
        INSERT INTO tool_invocation(id,session_id,entry_id,call_id,tool_name,status) VALUES(1,1,1,'call-1','Read','done');
        INSERT INTO file_activity(id,session_id,entry_id,tool_invocation_id,path,action,evidence) VALUES(1,1,1,1,'src/lib.rs','read','reported target');
        INSERT INTO session_context(id,session_id,entry_id,branch,observed_at) VALUES(1,1,1,'main','2026-11-17');
        INSERT INTO session_tag(session_id,label) VALUES(1,'review');
        CREATE TABLE replication_identity(local_session_id INTEGER PRIMARY KEY,portable_id TEXT NOT NULL);
        INSERT INTO replication_identity VALUES(1,'portable-1');
        INSERT INTO session_analytics(session_id,agent_id,workspace_id,input_tokens,output_tokens,total_tokens) VALUES(1,1,1,10,2,12);").unwrap();
    // Fixture construction is deliberately in this writer process. Every reader
    // is a fresh process so writer allocations cannot pollute its HWM sample.
    conn.execute(
        "UPDATE content_part SET text=CAST('PRIVATE_CANARY'||zeroblob(?1) AS TEXT) WHERE id=2",
        [HUGE],
    )
    .unwrap();
    conn.execute(
        "UPDATE content_part SET text=CAST('DENIED_CANARY'||zeroblob(?1) AS TEXT) WHERE id=3",
        [HUGE],
    )
    .unwrap();
    conn.execute(
        "UPDATE entry SET payload_json=CAST(zeroblob(?1) AS TEXT) WHERE id=1",
        [HUGE],
    )
    .unwrap();
    conn.execute(
        "UPDATE agent SET product=CAST(zeroblob(?1) AS TEXT) WHERE id=2",
        [HUGE],
    )
    .unwrap();
    run_child(&path, "private_denied", false);
    for (case, update, restore, size) in [
        ("identity", "UPDATE library_revision SET identity=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE library_revision SET identity='fixture-library-identity' WHERE id=1", HUGE),
        ("portable", "UPDATE replication_identity SET portable_id=CAST(zeroblob(?1) AS TEXT) WHERE local_session_id=1", "UPDATE replication_identity SET portable_id='portable-1' WHERE local_session_id=1", HUGE),
        ("evidence", "UPDATE file_activity SET evidence=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE file_activity SET evidence='reported target' WHERE id=1", HUGE),
        ("observed", "UPDATE session_context SET observed_at=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE session_context SET observed_at='2026-11-17' WHERE id=1", HUGE),
        ("statistics", "UPDATE session_analytics SET duration_seconds=CAST(zeroblob(?1) AS TEXT) WHERE session_id=1", "UPDATE session_analytics SET duration_seconds=NULL WHERE session_id=1", HUGE),
        ("workspace", "UPDATE workspace SET path=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE workspace SET path='/synthetic/allowed' WHERE id=1", HUGE),
        ("family", "UPDATE agent SET family=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE agent SET family='codex' WHERE id=1", 2048),
        ("product", "UPDATE agent SET product=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE agent SET product='Codex' WHERE id=1", 2048),
        ("tag_small", "UPDATE session_tag SET label=CAST(zeroblob(?1) AS TEXT) WHERE session_id=1", "UPDATE session_tag SET label='review' WHERE session_id=1", 2048),
        ("tag", "UPDATE session_tag SET label=CAST(zeroblob(?1) AS TEXT) WHERE session_id=1", "UPDATE session_tag SET label='review' WHERE session_id=1", HUGE),
        ("entry_kind", "UPDATE entry SET kind=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE entry SET kind='message' WHERE id=1", 256),
        ("role", "UPDATE entry SET role=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE entry SET role='assistant' WHERE id=1", 256),
        ("occurred", "UPDATE entry SET occurred_at=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE entry SET occurred_at='2026-11-17' WHERE id=1", 256),
        ("part_kind", "UPDATE content_part SET kind=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE content_part SET kind='text' WHERE id=1", 256),
        ("tool_name", "UPDATE tool_invocation SET tool_name=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE tool_invocation SET tool_name='Read' WHERE id=1", 2048),
        ("status", "UPDATE tool_invocation SET status=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE tool_invocation SET status='done' WHERE id=1", 256),
        ("call_id", "UPDATE tool_invocation SET call_id=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE tool_invocation SET call_id='call-1' WHERE id=1", 8192),
        ("branch", "UPDATE session_context SET branch=CAST(zeroblob(?1) AS TEXT) WHERE id=1", "UPDATE session_context SET branch='main' WHERE id=1", 8192),
    ] {
        conn.execute(update,[size]).unwrap();
        run_child(&path,case,true);
        conn.execute(restore,[]).unwrap();
    }
    assert_eq!(
        std::fs::read(native).unwrap(),
        b"synthetic native sentinel\n"
    );
}

fn run_child(path: &Path, case: &str, fails: bool) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "export_metadata_reader_child",
            "--nocapture",
        ])
        .env("HESSION_EXPORT_BOUND_DB", path)
        .env("HESSION_EXPORT_BOUND_CASE", case)
        .env("HESSION_EXPORT_BOUND_FAIL", if fails { "1" } else { "0" })
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{case}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("{}", String::from_utf8_lossy(&output.stdout));
}

#[test]
#[ignore = "fresh reader helper invoked by metadata_limits_precede_allocation_and_private_or_denied_text_stays_unread"]
fn export_metadata_reader_child() {
    let path = std::path::PathBuf::from(std::env::var_os("HESSION_EXPORT_BOUND_DB").unwrap());
    let case = std::env::var("HESSION_EXPORT_BOUND_CASE").unwrap();
    let fails = std::env::var("HESSION_EXPORT_BOUND_FAIL").unwrap() == "1";
    let target = path.parent().unwrap().join(format!("{case}.zip"));
    let library = Library::new(&path);
    let mut request = ProjectExportRequest {
        workspace_id: 1,
        ..Default::default()
    };
    request.options.include_git_metadata = true;
    request.options.include_statistics = case == "statistics";
    let scope = SearchScope::workspaces([1]).unwrap();
    let cancel = ExportCancellation::default();
    let check = db::open_read_only(&path).unwrap();
    let before: i64 = check
        .query_row(
            "SELECT revision FROM library_revision WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let outcome = project_export::prepare(&library, &request, &scope, &target, &cancel)
        .and_then(|plan| project_export::write(&library, &plan, &scope, &cancel, |_| {}));
    if fails {
        let error = outcome.unwrap_err();
        if case == "workspace" {
            assert!(
                matches!(
                    error,
                    ExportError::Project(hession_core::projects::ProjectError::InvalidPath)
                ),
                "Unexpected error: {error:?}"
            );
        } else if case == "tag" {
            assert!(
                matches!(
                    error,
                    ExportError::Search(hession_core::search::SearchError::InputTooLarge)
                        | ExportError::Limit("metadata bytes")
                ),
                "Unexpected error: {error:?}"
            );
        } else {
            assert!(
                matches!(error, ExportError::Limit("metadata bytes")),
                "Unexpected error: {error:?}"
            );
        }
        assert!(!target.exists());
    } else {
        outcome.unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&target).unwrap()).unwrap();
        for n in 0..archive.len() {
            let mut member = archive.by_index(n).unwrap();
            assert!(member.size() < 64 * 1024);
            let mut body = String::new();
            member.read_to_string(&mut body).unwrap();
            assert!(!body.contains("PRIVATE_CANARY") && !body.contains("DENIED_CANARY"));
        }
    }
    let after: i64 = check
        .query_row(
            "SELECT revision FROM library_revision WHERE id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(before, after, "export must not mutate normalized history");
    assert!(std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .all(|item| !item
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".hession-project-export-")));
    assert_eq!(
        std::fs::read(path.parent().unwrap().join("native.jsonl")).unwrap(),
        b"synthetic native sentinel\n"
    );
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let high = status
            .lines()
            .find_map(|line| line.strip_prefix("VmHWM:"))
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        println!("{case}: reader VmHWM {high} KiB");
        assert!(
            high < 48 * 1024,
            "{case}: reader allocated oversized metadata: {high} KiB"
        );
    }
}
