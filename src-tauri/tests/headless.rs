//! Exercise the installed app's real dispatch, without display/session services.
#![cfg(unix)]
use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, Command, Output, Stdio},
    time::{Duration, Instant},
};

struct ChildGuard(Child);
impl std::ops::Deref for ChildGuard {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}
impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hession"));
    command
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env("HESSION_DATA_DIR", root.join("library"))
        .env("CODEX_HOME", root.join("codex"));
    command
}
fn output(root: &Path, args: &[&str]) -> Output {
    command(root).args(args).output().unwrap()
}
fn fixture(root: &Path) -> std::path::PathBuf {
    fs::create_dir_all(root.join("codex/sessions")).unwrap();
    fs::create_dir_all(root.join("workspace")).unwrap();
    let file = root.join("codex/sessions/native.jsonl");
    let cwd = root.join("workspace");
    fs::write(&file, format!("{}\n{}\n", serde_json::json!({"type":"session_meta","payload":{"id":"native-headless-fixture","cwd":cwd}}), serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"native dispatch needle"}}))).unwrap();
    file
}
fn successful(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn wait(child: &mut Child) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if started.elapsed() > Duration::from_secs(12) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Headless child exceeded shutdown deadline");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn explicit_app_commands_never_initialize_a_webview_or_create_a_library_for_reads() {
    let root = tempfile::tempdir().unwrap();
    for args in [["--help"], ["--version"]] {
        let result = output(root.path(), &args);
        assert!(result.status.success());
        assert!(!result.stdout.is_empty());
    }
    let result = output(root.path(), &["status", "--json"]);
    assert_eq!(result.status.code(), Some(3));
    assert!(result.stdout.is_empty());
    assert!(serde_json::from_slice::<serde_json::Value>(&result.stderr).is_ok());
    assert!(!root.path().join("library/sessions.sqlite").exists());
}

#[test]
fn actual_app_one_shot_import_matches_shared_readers_without_altering_the_source() {
    let root = tempfile::tempdir().unwrap();
    let file = fixture(root.path());
    let before = fs::read(&file).unwrap();
    let summary = successful(output(
        root.path(),
        &["index", "--once", "--agent", "codex", "--json"],
    ));
    assert_eq!(summary["files_done"], 1);
    assert_eq!(summary["errors"], serde_json::json!([]));
    let db = root.path().join("library/sessions.sqlite");
    let conn = hession_core::db::open_read_only(&db).unwrap();
    let (id, count): (i64, i64) = conn
        .query_row(
            "SELECT id,(SELECT count(*) FROM entry WHERE session_id=session.id) FROM session",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 2); // Session metadata and the user message are both preserved.
    let listed = successful(output(root.path(), &["sessions", "list", "--json"]));
    assert_eq!(listed["results"][0]["sessionId"], id);
    let searched = successful(output(root.path(), &["search", "needle", "--json"]));
    assert!(searched.to_string().contains("native dispatch needle"));
    let shared = hession_core::service::Library::new(&db)
        .search(
            &hession_core::search::SearchRequest {
                query: "needle".into(),
                visibility: hession_core::search::SearchVisibility::Conversation,
                ..Default::default()
            },
            &hession_core::search::SearchScope::all(),
        )
        .unwrap();
    assert_eq!(shared.results[0].session_id, id);
    successful(output(
        root.path(),
        &["index", "--once", "--agent", "codex", "--json"],
    ));
    assert_eq!(
        conn.query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(fs::read(&file).unwrap(), before);
    assert_eq!(
        conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}

#[test]
fn actual_app_watch_leadership_concurrent_one_shot_and_signal_shutdown_preserve_history() {
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let root = tempfile::tempdir().unwrap();
        let file = fixture(root.path());
        successful(output(
            root.path(),
            &["index", "--once", "--agent", "codex", "--json"],
        ));
        let db = root.path().join("library/sessions.sqlite");
        let mut watcher = ChildGuard(
            command(root.path())
                .args(["index", "--watch", "--agent", "codex", "--json"])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let stdout = watcher.stdout.take().unwrap();
        let (send, receive) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut lines = BufReader::new(stdout);
            let mut first = String::new();
            let result = lines.read_line(&mut first).map(|_| first);
            let _ = send.send(result);
            std::io::copy(&mut lines, &mut std::io::sink()).ok();
        });
        let first = receive
            .recv_timeout(Duration::from_secs(15))
            .unwrap()
            .unwrap();
        assert!(serde_json::from_str::<serde_json::Value>(&first).is_ok());
        assert!(hession_core::locking::LibraryLock::acquire(
            &db,
            hession_core::locking::LockKind::WatchLeader,
            Duration::ZERO
        )
        .is_err());
        let duplicate = output(
            root.path(),
            &["index", "--watch", "--agent", "codex", "--json"],
        );
        assert_eq!(duplicate.status.code(), Some(5));
        assert!(String::from_utf8_lossy(&duplicate.stderr).contains("watcher"));
        let mut source = OpenOptions::new().append(true).open(&file).unwrap();
        writeln!(source,"{}", serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"concurrent native append"}})).unwrap();
        source.sync_all().unwrap();
        let expected = fs::read(&file).unwrap();
        successful(output(
            root.path(),
            &["index", "--once", "--agent", "codex", "--json"],
        ));
        assert_eq!(unsafe { libc::kill(watcher.id() as i32, signal) }, 0);
        assert_eq!(wait(&mut watcher).code(), Some(128 + signal));
        reader.join().unwrap();
        let conn = hession_core::db::open_read_only(&db).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            conn.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        assert_eq!(fs::read(&file).unwrap(), expected);
        assert!(hession_core::locking::LibraryLock::acquire(
            &db,
            hession_core::locking::LockKind::WatchLeader,
            Duration::ZERO
        )
        .is_ok());
    }
}

#[tokio::test]
async fn official_sdk_client_handshakes_with_the_actual_app_and_respects_metadata_default() {
    use rmcp::{model::CallToolRequestParams, ServiceExt};
    let root = tempfile::tempdir().unwrap();
    fixture(root.path());
    successful(output(
        root.path(),
        &["index", "--once", "--agent", "codex", "--json"],
    ));
    let mut child = tokio::process::Command::from(command(root.path()));
    child
        .args(["mcp", "serve", "--project"])
        .arg(root.path().join("workspace"));
    let mut child = child
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let transport = (child.stdout.take().unwrap(), child.stdin.take().unwrap());
    let mut client = tokio::time::timeout(Duration::from_secs(10), ().serve(transport))
        .await
        .unwrap()
        .unwrap();
    let tools = client.peer().list_tools(Default::default()).await.unwrap();
    assert_eq!(tools.tools.len(), 7);
    let recent = client
        .peer()
        .call_tool(CallToolRequestParams::new("get_recent_sessions"))
        .await
        .unwrap();
    assert_ne!(recent.is_error, Some(true));
    let session = client
        .peer()
        .call_tool(
            CallToolRequestParams::new("get_session").with_arguments(
                serde_json::json!({"sessionId":1})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    let encoded = serde_json::to_string(&session).unwrap();
    assert!(encoded.contains("Transcript access is disabled"));
    assert_eq!(
        session.structured_content.as_ref().unwrap()["data"]["entries"],
        serde_json::json!([])
    );
    tokio::time::timeout(Duration::from_secs(5), client.close())
        .await
        .unwrap()
        .unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .unwrap()
        .unwrap()
        .success());
}
