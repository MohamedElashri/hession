#![allow(dead_code)]
use std::path::PathBuf;
pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub path: PathBuf,
}
impl Fixture {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.sqlite");
        let conn = hession_core::db::open_db(&path).unwrap();
        conn.execute_batch("BEGIN;INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
        INSERT INTO workspace(id,path,display_name) VALUES(1,'/fixture/allowed','Allowed'),(2,'/fixture/denied','Denied');
        INSERT INTO session(id,agent_id,workspace_id,source_uri,title,started_at) VALUES(1,1,1,'fixture:NATIVE_SOURCE_SECRET','Public session','2026-01-01'),(2,1,2,'fixture:DENIED_NATIVE_SOURCE','DENIED_PROJECT_SECRET','2026-02-01');
        INSERT INTO entry(id,session_id,ordinal,kind,role,search_text,payload_json) VALUES
        (1,1,0,'user_message','user','visible request',NULL),
        (2,1,900,'agent_message','assistant','visible reply THINKING_SECRET TOOL_SECRET','{\"private\":\"RAW_PROVIDER_SECRET\"}'),
        (3,1,901,'reasoning','assistant','REASONING_EVENT_SECRET',NULL),
        (4,1,902,'message','system','SYSTEM_SECRET',NULL),
        (5,1,903,'tool_result','tool','TOOL_ROLE_SECRET',NULL),
        (6,1,904,'message','assistant','LEGACY_COMBINED_SECRET',NULL),
        (7,2,0,'message','assistant','DENIED_TRANSCRIPT_SECRET',NULL);
        INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES
        (1,0,'text','visible request'),(2,0,'thinking','THINKING_SECRET'),(2,1,'text','visible reply'),(2,2,'tool_result','TOOL_SECRET'),
        (3,0,'text','REASONING_EVENT_SECRET'),(4,0,'text','SYSTEM_SECRET'),(5,0,'tool_result','TOOL_ROLE_SECRET'),(7,0,'text','DENIED_TRANSCRIPT_SECRET');
        INSERT INTO tool_invocation(id,session_id,entry_id,tool_name,status,exit_code,arguments_json,result_text,command_text) VALUES
        (1,1,2,'shell','failed',1,'{\"value\":\"ARGUMENT_SECRET\"}','RESULT_SECRET','COMMAND_SECRET'),
        (2,2,7,'DENIED_TOOL_NAME','success',0,NULL,NULL,NULL),
        (3,1,7,'mismatched','success',0,NULL,NULL,NULL);
        INSERT INTO file_activity(id,session_id,entry_id,tool_invocation_id,path,action,evidence) VALUES
        (1,1,2,1,'src/a.rs','edit','EVIDENCE_SECRET'),(2,2,7,2,'private.rs','read','DENIED_EVIDENCE_SECRET');
        INSERT INTO repository(id,common_dir,remote_url) VALUES(1,'/native/GIT_SOURCE_SECRET','https://user:REMOTE_SECRET@example.test/private.git');
        INSERT INTO session_context(session_id,entry_id,repository_id,branch,commit_sha,raw_json) VALUES(1,2,1,'main','abc','CONTEXT_SECRET');
        INSERT INTO session_annotation(session_id,starred) VALUES(1,1);INSERT INTO session_tag(session_id,label) VALUES(1,'review');COMMIT;").unwrap();
        Self { dir, path }
    }
    pub fn reader(&self, expose: bool) -> hession_core::reader::ScopedReader {
        hession_core::reader::ScopedReader::projects(
            hession_core::service::Library::new(&self.path),
            &[hession_core::projects::ProjectSelector::WorkspaceId(1)],
            expose,
        )
        .unwrap()
    }
    pub fn command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_hession-headless"));
        command.arg("--database").arg(&self.path);
        command
    }
}
pub fn no_secrets(value: &serde_json::Value) {
    let text = serde_json::to_string(value).unwrap();
    for secret in [
        "THINKING_SECRET",
        "TOOL_SECRET",
        "RAW_PROVIDER_SECRET",
        "REASONING_EVENT_SECRET",
        "SYSTEM_SECRET",
        "TOOL_ROLE_SECRET",
        "LEGACY_COMBINED_SECRET",
        "ARGUMENT_SECRET",
        "RESULT_SECRET",
        "COMMAND_SECRET",
        "NATIVE_SOURCE_SECRET",
        "CONTEXT_SECRET",
        "GIT_SOURCE_SECRET",
        "REMOTE_SECRET",
        "DENIED_PROJECT_SECRET",
        "DENIED_TRANSCRIPT_SECRET",
        "DENIED_TOOL_NAME",
        "DENIED_EVIDENCE_SECRET",
    ] {
        assert!(!text.contains(secret), "Unexpected {secret}: {text}");
    }
}
