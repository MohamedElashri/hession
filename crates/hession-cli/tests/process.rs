mod common;
use common::{no_secrets, Fixture};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
fn parsed(output: &std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn help_version_and_bad_args_are_headless_and_never_create_a_library() {
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("missing-data");
    for flag in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_hession-headless"))
            .arg(flag)
            .env("HESSION_DATA_DIR", &missing)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(!output.stdout.is_empty());
        assert!(!missing.exists());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_hession-headless"))
        .args(["sessions", "list", "--limit", "101", "--json"])
        .env("HESSION_DATA_DIR", &missing)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["kind"], "usage");
    assert!(output.stdout.is_empty());
    assert!(!missing.exists());
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let output = Command::new(env!("CARGO_BIN_EXE_hession-headless"))
            .args(["sessions", "list", "--project"])
            .arg(std::ffi::OsString::from_vec(vec![0xff]))
            .arg("--json")
            .env("HESSION_DATA_DIR", &missing)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(serde_json::from_slice::<Value>(&output.stderr).is_ok());
        assert!(!missing.exists());
    }
    let output = Command::new(env!("CARGO_BIN_EXE_hession-headless"))
        .args(["status", "--json"])
        .env("HESSION_DATA_DIR", &missing)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    assert!(!missing.exists());
}
#[test]
fn cli_reads_scoped_json_unicode_and_conversation_without_raw_provider_fields() {
    let f = Fixture::new();
    let status = parsed(&f.command().args(["status", "--json"]).output().unwrap());
    assert_eq!(status["sessions"], 2);
    let sessions = parsed(
        &f.command()
            .args(["sessions", "list", "--project", "workspace:1", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(sessions["results"].as_array().unwrap().len(), 1);
    no_secrets(&sessions);
    let shown = parsed(
        &f.command()
            .args([
                "sessions",
                "show",
                "1",
                "--project",
                "/fixture/allowed",
                "--format",
                "json",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(shown["entries"].as_array().unwrap().len(), 2);
    no_secrets(&shown);
    let denied = f
        .command()
        .args([
            "sessions",
            "show",
            "2",
            "--project",
            "workspace:1",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(denied.status.code(), Some(4));
    assert!(denied.stdout.is_empty());
    let metadata = parsed(
        &f.command()
            .args([
                "search",
                "visible",
                "--project",
                "workspace:1",
                "--metadata-only",
                "--json",
            ])
            .output()
            .unwrap(),
    );
    assert!(metadata["results"].as_array().unwrap().is_empty());
    let full = parsed(
        &f.command()
            .args([
                "search",
                "THINKING_SECRET",
                "--project",
                "workspace:1",
                "--json",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(full["results"].as_array().unwrap().len(), 1);
    let invalid = f
        .command()
        .args(["search", "α unknown:value", "--json"])
        .output()
        .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&invalid.stderr).unwrap();
    assert_eq!(error["kind"], "syntax");
    assert_eq!(error["position"], 3);
}
#[test]
fn project_export_preview_is_read_only_and_write_is_explicit_and_no_clobber() {
    let f = Fixture::new();
    let destination = f.dir.path().join("project.zip");
    let mut command = f.command();
    command
        .args(["export", "project", "workspace:1", "--output"])
        .arg(&destination)
        .args(["--preview", "--json"]);
    let preview = parsed(&command.output().unwrap());
    assert_eq!(preview["sessionCount"], 1);
    assert!(!destination.exists());
    let mut command = f.command();
    command
        .args(["export", "project", "workspace:1", "--output"])
        .arg(&destination)
        .arg("--json");
    let result = parsed(&command.output().unwrap());
    assert_eq!(result["sessionCount"], 1);
    assert!(destination.exists());
    let original = std::fs::read(&destination).unwrap();
    assert!(original.starts_with(b"PK"));
    let again = command.output().unwrap();
    assert!(!again.status.success());
    assert_eq!(std::fs::read(&destination).unwrap(), original);
}

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    messages: Receiver<Value>,
}
impl Client {
    fn start(f: &Fixture, expose: bool, multiple: bool) -> Self {
        let mut command = f.command();
        command.args(["mcp", "serve", "--project", "workspace:1"]);
        if expose {
            command.arg("--expose-transcripts");
        }
        if multiple {
            command.args(["--project", "workspace:2"]);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (sender, messages) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                assert!(line.len() < 1024 * 1024, "Unbounded MCP response");
                let value = serde_json::from_str(&line)
                    .unwrap_or_else(|_| panic!("Non-JSON stdout: {line}"));
                if sender.send(value).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            messages,
        };
        client.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"synthetic-client","version":"1"}}}));
        let initialized = client.response(1);
        assert!(initialized.get("result").is_some(), "{initialized}");
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        client
    }
    fn send(&mut self, value: Value) {
        self.raw(&serde_json::to_vec(&value).unwrap());
    }
    fn raw(&mut self, bytes: &[u8]) {
        let input = self.input.as_mut().unwrap();
        input.write_all(bytes).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
    }
    fn response(&mut self, id: i64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let value = self
                .messages
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("MCP response deadline");
            if value["id"] == id {
                return value;
            }
        }
    }
    fn tool(&mut self, id: i64, name: &str, args: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}}));
        self.response(id)
    }
    fn eof(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "{status}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "Server did not exit after disconnect"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn data(response: &Value) -> &Value {
    assert_ne!(response["result"]["isError"], true, "{response}");
    &response["result"]["structuredContent"]["data"]
}
#[test]
fn mcp_real_stdio_handshake_advertises_exactly_seven_read_only_tools_and_empty_scope_is_denied() {
    let f = Fixture::new();
    let mut client = Client::start(&f, false, false);
    client.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    let listed = client.response(2);
    let tools = listed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 7);
    let mut names = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        vec![
            "get_file_history",
            "get_project_activity",
            "get_recent_sessions",
            "get_session",
            "get_tool_calls",
            "list_projects",
            "search_sessions"
        ]
    );
    for tool in tools {
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
        assert_eq!(tool["annotations"]["destructiveHint"], false);
        assert_eq!(tool["annotations"]["openWorldHint"], false);
    }
    client.eof();
    let output = f
        .command()
        .args(["mcp", "serve"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let output = f
        .command()
        .args(["mcp", "serve", "--project", ""])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}
#[test]
fn mcp_all_tools_enforce_scope_and_metadata_policy_without_raw_evidence_or_source_fields() {
    let f = Fixture::new();
    let mut client = Client::start(&f, false, false);
    let session = client.tool(10, "get_session", json!({"sessionId":1}));
    no_secrets(&session);
    assert_eq!(data(&session)["entries"], json!([]));
    let projects = client.tool(11, "list_projects", json!({}));
    no_secrets(&projects);
    assert_eq!(data(&projects)["projects"].as_array().unwrap().len(), 1);
    let activity = client.tool(12, "get_project_activity", json!({"workspaceId":1}));
    no_secrets(&activity);
    assert_eq!(data(&activity)["activity"][0]["reportedPath"], "src/a.rs");
    assert!(data(&activity)["activity"][0].get("evidence").is_none());
    let history = client.tool(
        13,
        "get_file_history",
        json!({"workspaceId":1,"path":"src/a.rs"}),
    );
    no_secrets(&history);
    assert_eq!(data(&history)["activity"].as_array().unwrap().len(), 1);
    let tools = client.tool(14, "get_tool_calls", json!({"sessionId":1}));
    no_secrets(&tools);
    assert!(data(&tools)["tools"][0].get("arguments").is_none());
    let recent = client.tool(15, "get_recent_sessions", json!({}));
    no_secrets(&recent);
    assert_eq!(data(&recent)["results"].as_array().unwrap().len(), 1);
    let search = client.tool(16, "search_sessions", json!({"query":"visible"}));
    no_secrets(&search);
    assert!(data(&search)["results"].as_array().unwrap().is_empty());
    for (index, (name, args)) in [
        ("get_session", json!({"sessionId":2})),
        ("get_tool_calls", json!({"sessionId":2})),
        ("get_project_activity", json!({"workspaceId":2})),
        (
            "get_file_history",
            json!({"workspaceId":2,"path":"private.rs"}),
        ),
        (
            "get_session",
            json!({"sessionId":1,"exposeTranscripts":true}),
        ),
        (
            "get_tool_calls",
            json!({"sessionId":1,"includeResults":true}),
        ),
        ("index", json!({})),
        ("export_project", json!({})),
    ]
    .into_iter()
    .enumerate()
    {
        let response = client.tool(20 + index as i64, name, args);
        assert_eq!(response["result"]["isError"], true, "{response}");
        no_secrets(&response);
    }
    client.eof();
}
#[test]
fn mcp_transcript_consent_still_excludes_reasoning_system_tools_and_denied_projects() {
    let f = Fixture::new();
    let mut client = Client::start(&f, true, false);
    let session = client.tool(10, "get_session", json!({"sessionId":1}));
    no_secrets(&session);
    assert_eq!(data(&session)["entries"].as_array().unwrap().len(), 2);
    for (index, query) in [
        "THINKING_SECRET",
        "TOOL_SECRET",
        "REASONING_EVENT_SECRET",
        "SYSTEM_SECRET",
        "DENIED_TRANSCRIPT_SECRET",
    ]
    .into_iter()
    .enumerate()
    {
        let result = client.tool(20 + index as i64, "search_sessions", json!({"query":query}));
        no_secrets(&result);
        assert!(data(&result)["results"].as_array().unwrap().is_empty());
    }
    let visible = client.tool(30, "search_sessions", json!({"query":"visible"}));
    assert_eq!(data(&visible)["results"].as_array().unwrap().len(), 1);
    no_secrets(&visible);
    let tools = client.tool(31, "get_tool_calls", json!({"sessionId":1}));
    no_secrets(&tools);
    client.eof();
}
#[test]
fn mcp_rejects_stale_cursors_reused_workspace_ids_and_library_replacement() {
    let f = Fixture::new();
    let mut client = Client::start(&f, true, false);
    let first = client.tool(10, "get_session", json!({"sessionId":1,"limit":1}));
    let cursor = data(&first)["nextCursor"].as_str().unwrap().to_owned();
    let conn = hession_core::db::open_db(&f.path).unwrap();
    conn.execute(
        "UPDATE content_part SET text='updated visible' WHERE entry_id=1",
        [],
    )
    .unwrap();
    let stale = client.tool(
        11,
        "get_session",
        json!({"sessionId":1,"limit":1,"cursor":cursor}),
    );
    assert_eq!(stale["result"]["isError"], true);
    conn.execute(
        "UPDATE workspace SET path='/changed/private' WHERE id=1",
        [],
    )
    .unwrap();
    let changed = client.tool(12, "get_session", json!({"sessionId":1}));
    assert_eq!(changed["result"]["isError"], true);
    no_secrets(&changed);
    conn.execute(
        "UPDATE workspace SET path='/fixture/allowed' WHERE id=1",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE library_revision SET identity='new-library' WHERE id=1",
        [],
    )
    .unwrap();
    let replaced = client.tool(13, "list_projects", json!({}));
    assert_eq!(replaced["result"]["isError"], true);
    client.eof();
}
#[test]
fn mcp_frame_and_response_budgets_remain_bounded_and_external_paths_never_read_files() {
    let f = Fixture::new();
    let external = f.dir.path().join("external.txt");
    std::fs::write(&external, "FILESYSTEM_CONTENT_SECRET").unwrap();
    let conn = hession_core::db::open_db(&f.path).unwrap();
    conn.execute("INSERT INTO file_activity(session_id,path,action,evidence) VALUES(1,?,'read','EVIDENCE_SECRET')",[external.to_str().unwrap()]).unwrap();
    let mut client = Client::start(&f, true, false);
    let external_result = client.tool(
        10,
        "get_file_history",
        json!({"workspaceId":1,"path":external}),
    );
    assert_eq!(
        data(&external_result)["activity"].as_array().unwrap().len(),
        1
    );
    assert!(!external_result
        .to_string()
        .contains("FILESYSTEM_CONTENT_SECRET"));
    assert!(!external_result.to_string().contains("EVIDENCE_SECRET"));
    // The SDK drops oversized lines without parsing their IDs or replying.
    client.raw(&vec![b'x'; 129 * 1024]);
    let after = client.tool(11, "get_session", json!({"sessionId":1}));
    no_secrets(&after);
    conn.execute_batch("BEGIN").unwrap();
    for ordinal in 1000..1100 {
        conn.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text) VALUES(1,?,'message','assistant','body')",[ordinal]).unwrap();
        let entry = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'text',?)",
            (entry, "🦀".repeat(2048)),
        )
        .unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();
    let large = client.tool(12, "get_session", json!({"sessionId":1,"limit":100}));
    assert_eq!(large["result"]["isError"], true);
    assert!(large.to_string().len() < 4096);
    client.eof();
}
#[test]
fn mcp_multiple_projects_require_explicit_owner_flags() {
    let f = Fixture::new();
    let mut client = Client::start(&f, false, true);
    let result = client.tool(10, "list_projects", json!({}));
    assert_eq!(data(&result)["projects"].as_array().unwrap().len(), 2);
    client.eof();
}

#[test]
fn mcp_pipelined_requests_and_cancellation_leave_protocol_usable() {
    let f = Fixture::new();
    let mut client = Client::start(&f, true, false);
    for id in 100..164 {
        client.send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":"get_session","arguments":{"sessionId":1}}}));
    }
    let mut seen = std::collections::BTreeSet::new();
    while seen.len() < 64 {
        let reply = client
            .messages
            .recv_timeout(Duration::from_secs(15))
            .unwrap();
        if let Some(id) = reply["id"].as_i64() {
            assert!((100..164).contains(&id));
            assert!(seen.insert(id));
            no_secrets(&reply);
            assert!(reply.get("result").is_some());
        }
    }
    client.send(json!({"jsonrpc":"2.0","id":200,"method":"tools/call","params":{"name":"get_session","arguments":{"sessionId":1}}}));
    client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":200,"reason":"synthetic cancellation"}}));
    let usable = client.tool(201, "list_projects", json!({}));
    assert_eq!(data(&usable)["projects"].as_array().unwrap().len(), 1);
    client.eof();
}
#[cfg(unix)]
#[test]
fn follow_emits_durable_append_and_tool_updates_without_raw_payload_and_exits_on_sigterm() {
    let f = Fixture::new();
    let mut child = f
        .command()
        .args(["sessions", "follow", "1", "--jsonl"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, messages) = mpsc::sync_channel(16);
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if sender
                .send(serde_json::from_str::<Value>(&line.unwrap()).unwrap())
                .is_err()
            {
                break;
            }
        }
    });
    let initial = messages.recv_timeout(Duration::from_secs(15)).unwrap();
    assert_eq!(initial["reset"], true);
    no_secrets(&initial);
    let conn = hession_core::db::open_db(&f.path).unwrap();
    conn.execute_batch("BEGIN;INSERT INTO entry(id,session_id,ordinal,kind,role,search_text,payload_json) VALUES(100,1,5000,'agent_message','assistant','appended visible','RAW_PROVIDER_SECRET');INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(100,0,'text','appended visible');UPDATE tool_invocation SET status='success',exit_code=0,result_text='RESULT_SECRET' WHERE id=1;COMMIT;").unwrap();
    let changed = messages.recv_timeout(Duration::from_secs(15)).unwrap();
    assert_eq!(changed["reset"], false);
    no_secrets(&changed);
    assert!(changed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == 100 && row["text"] == "appended visible"));
    assert!(changed["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == 2 && row["tools"][0]["status"] == "success"));
    // SAFETY: signal only the child this test owns, never a process group.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(143));
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            let mut error = String::new();
            std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut error).unwrap();
            panic!("follow did not stop on SIGTERM: {error}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn cli_and_mcp_results_equal_the_shared_search_service_under_the_same_policy() {
    use hession_core::search::{SearchRequest, SearchScope, SearchVisibility};
    let f = Fixture::new();
    let library = hession_core::service::Library::new(&f.path);
    let expected = library
        .search(
            &SearchRequest {
                query: "visible".into(),
                visibility: SearchVisibility::AllIndexedText,
                ..Default::default()
            },
            &SearchScope::workspaces([1]).unwrap(),
        )
        .unwrap();
    let cli = parsed(
        &f.command()
            .args(["search", "visible", "--project", "workspace:1", "--json"])
            .output()
            .unwrap(),
    );
    assert_eq!(cli, serde_json::to_value(expected).unwrap());
    let expected = f
        .reader(true)
        .search(
            &SearchRequest {
                query: "visible".into(),
                ..Default::default()
            },
            &hession_core::reader::ReadCancellation::default(),
        )
        .unwrap();
    let mut client = Client::start(&f, true, false);
    let actual = client.tool(50, "search_sessions", json!({"query":"visible"}));
    assert_eq!(data(&actual), &serde_json::to_value(expected).unwrap());
    client.eof();
}

#[cfg(unix)]
#[test]
fn follow_sigterm_remains_responsive_when_stdout_is_unread_and_backpressured() {
    use std::os::fd::AsRawFd;
    let f = Fixture::new();
    let conn = hession_core::db::open_db(&f.path).unwrap();
    conn.execute_batch("BEGIN").unwrap();
    let text = "🦀".repeat(2048);
    for ordinal in 1000..1100 {
        conn.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text) VALUES(1,?,'agent_message','assistant','visible')",[ordinal]).unwrap();
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'text',?)",
            (id, &text),
        )
        .unwrap();
    }
    conn.execute_batch("COMMIT").unwrap();
    let mut child = f
        .command()
        .args(["sessions", "follow", "1", "--jsonl", "--limit", "100"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let fd = child.stdout.as_ref().unwrap().as_raw_fd();
    #[cfg(target_os = "linux")]
    // SAFETY: shrink only this test child's output pipe; no borrowed buffers.
    assert!(unsafe { libc::fcntl(fd, libc::F_SETPIPE_SZ, 4096) } > 0);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut available = 0_i32; // SAFETY: valid owned pipe fd and writable integer pointer.
        assert_eq!(
            unsafe { libc::ioctl(fd, libc::FIONREAD, &mut available) },
            0
        );
        if available >= 4096 {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("follow did not reach the deliberately unread pipe");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(child.try_wait().unwrap().is_none());
    // SAFETY: signal only the child owned by this test.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert_eq!(status.code(), Some(143));
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("backpressured follow did not exit on SIGTERM");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
