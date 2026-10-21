//! Generates `src/dev/fixtures.json` for the browser preview (`make preview`):
//! synthetic Claude Code and Codex sessions, local and on two mirrored SSH
//! hosts, imported by the real importer and dumped through the real command
//! functions, so the preview always matches the backend's response shapes.
//!
//! Run with `make preview-fixtures` (an ignored test; it rewrites the file).
//! `make preview-fixtures FROM=<sessions.sqlite>` dumps a real library instead.

use super::*;
use chrono::{Duration, Utc};
use std::fs;
use std::path::Path;

struct Tool {
    name: &'static str,
    input: Value,
    output: &'static str,
    failed: bool,
}

struct Turn {
    user: &'static str,
    reply: &'static str,
    tool: Option<Tool>,
}

struct Spec {
    family: &'static str,
    host: Option<&'static str>,
    cwd: &'static str,
    branch: &'static str,
    model: &'static str,
    /// Hours before now that the session started.
    hours_ago: i64,
    turns: Vec<Turn>,
}

fn bash(command: &str) -> Value {
    json!({ "command": command })
}

fn specs() -> Vec<Spec> {
    // Generic, non-identifying sample work: no real people, hosts, or projects.
    vec![
        Spec { family: "claude", host: None, cwd: "/home/dev/projects/web-dashboard", branch: "main", model: "claude-sonnet-4-5", hours_ago: 1, turns: vec![
            Turn { user: "The sidebar title overflows on narrow windows. Can you make the header show only the app name?", reply: "I'll check how the sidebar header is laid out first.", tool: Some(Tool { name: "Grep", input: json!({"pattern": "sidebar-subtitle", "path": "src"}), output: "src/Sidebar.jsx:42: <span className=\"sidebar-subtitle\">{subtitle}</span>", failed: false }) },
            Turn { user: "Yes, drop the subtitle.", reply: "Removed the subtitle from the sidebar header. It now holds the logo and name only, so nothing wraps at 800 px.", tool: Some(Tool { name: "Edit", input: json!({"file_path": "src/Sidebar.jsx"}), output: "Applied 1 edit to src/Sidebar.jsx", failed: false }) },
        ]},
        Spec { family: "claude", host: Some("devbox"), cwd: "/srv/app/orders-api", branch: "fix/flaky-tests", model: "claude-opus-4-1", hours_ago: 3, turns: vec![
            Turn { user: "The integration tests fail randomly in CI. Look at the test logs and find out why.", reply: "Let me run the suite a few times with the test order randomized.", tool: Some(Tool { name: "Bash", input: bash("pytest tests/integration -p randomly -x"), output: "FAILED tests/integration/test_orders.py::test_cancel_order\nAssertionError: expected status 'cancelled', got 'pending'\n1 failed, 87 passed", failed: true }) },
            Turn { user: "What do you suggest?", reply: "`test_cancel_order` reads the order before the background worker finishes. I replaced the fixed sleep with a wait on the job queue and gave each test its own database schema. Twenty randomized runs now pass.", tool: Some(Tool { name: "Bash", input: bash("for i in $(seq 20); do pytest tests/integration -p randomly -q || break; done"), output: "88 passed in 41.2s\n(20 of 20 runs passed)", failed: false }) },
        ]},
        Spec { family: "codex", host: None, cwd: "/home/dev/projects/web-dashboard", branch: "main", model: "gpt-5-codex", hours_ago: 5, turns: vec![
            Turn { user: "Add a Makefile target that runs the tests in Docker.", reply: "I'll add `docker-test`, building a small image with the toolchain and mounting the dependency cache.", tool: Some(Tool { name: "shell", input: json!({"command": ["bash", "-lc", "make docker-test"]}), output: "Tests: 64 passed, 0 failed", failed: false }) },
        ]},
        Spec { family: "claude", host: Some("gpu-node"), cwd: "/srv/data/reports", branch: "weekly-summary", model: "claude-sonnet-4-5", hours_ago: 26, turns: vec![
            Turn { user: "Summarize request latency per endpoint from latency.csv and flag anything above 300 ms.", reply: "Reading the latency table.", tool: Some(Tool { name: "Read", input: json!({"file_path": "latency.csv"}), output: "endpoint,p95_ms\n/api/orders,212\n/api/search,418\n/api/users,96", failed: false }) },
            Turn { user: "Which ones should we optimize first?", reply: "Only `/api/search` is above 300 ms (418 ms at p95). An index on `products(name)` and caching the facet counts should bring it under 200 ms; the other endpoints are fine.", tool: None },
        ]},
        Spec { family: "codex", host: Some("devbox"), cwd: "/srv/ml/image-classifier", branch: "main", model: "gpt-5-codex", hours_ago: 30, turns: vec![
            Turn { user: "Training loss goes NaN after epoch 12. Find out why.", reply: "Checking the gradient norms logged by the trainer.", tool: Some(Tool { name: "shell", input: json!({"command": ["bash", "-lc", "grep -n grad_norm logs/train.log | tail -3"]}), output: "1203: epoch=12 grad_norm=inf\n1204: epoch=12 loss=nan", failed: false }) },
            Turn { user: "Fix it.", reply: "Enabled gradient clipping at 1.0 and switched the loss to the numerically stable log-softmax form. A 20-epoch rerun is stable.", tool: None },
        ]},
        Spec { family: "claude", host: None, cwd: "/home/dev/projects/docs-site", branch: "guide-rewrite", model: "claude-sonnet-4-5", hours_ago: 50, turns: vec![
            Turn { user: "Tighten the installation section of the guide; keep every command and version number.", reply: "I shortened three paragraphs, merged the two prerequisite tables, and left all commands and versions unchanged.", tool: Some(Tool { name: "Edit", input: json!({"file_path": "docs/install.md"}), output: "Applied 6 edits to docs/install.md", failed: false }) },
        ]},
        Spec { family: "codex", host: None, cwd: "/home/dev/projects/text-search-cli", branch: "fix/unicode", model: "gpt-5-codex", hours_ago: 75, turns: vec![
            Turn { user: "Searching for non-ASCII patterns panics. Reproduce and fix.", reply: "Reproducing with a multibyte pattern.", tool: Some(Tool { name: "shell", input: json!({"command": ["bash", "-lc", "cargo test unicode"]}), output: "thread 'unicode_pattern' panicked at 'byte index 3 is not a char boundary'", failed: true }) },
            Turn { user: "Go ahead.", reply: "The matcher sliced the pattern by bytes. It now iterates `char_indices`, and a regression test covers Greek and CJK patterns.", tool: Some(Tool { name: "shell", input: json!({"command": ["bash", "-lc", "cargo test"]}), output: "test result: ok. 42 passed; 0 failed", failed: false }) },
        ]},
        Spec { family: "claude", host: Some("devbox"), cwd: "/srv/app/orders-api", branch: "main", model: "claude-sonnet-4-5", hours_ago: 98, turns: vec![
            Turn { user: "Write a migration that adds a status index to the orders table.", reply: "Added `0042_orders_status_index` with a concurrent index build and a matching down migration. A dry run against the staging schema applies cleanly.", tool: Some(Tool { name: "Bash", input: bash("make migrate-dry-run"), output: "Applying 0042_orders_status_index... OK (dry run)", failed: false }) },
        ]},
        Spec { family: "claude", host: None, cwd: "/home/dev/projects/notes-app", branch: "main", model: "claude-haiku-4-5", hours_ago: 150, turns: vec![
            Turn { user: "Update the changelog for the 2.3 release from the merged pull requests.", reply: "Added 9 entries grouped into features and fixes, and linked each one to its pull request.", tool: None },
        ]},
        Spec { family: "codex", host: Some("gpu-node"), cwd: "/srv/data/etl-jobs", branch: "main", model: "gpt-5", hours_ago: 200, turns: vec![
            Turn { user: "Rerun the nightly import jobs that failed last night.", reply: "Listing the failed jobs.", tool: Some(Tool { name: "shell", input: json!({"command": ["bash", "-lc", "./jobs.sh failed --since yesterday"]}), output: "failed: import-users, import-events, import-billing", failed: false }) },
            Turn { user: "Rerun them.", reply: "Reran the three jobs with a longer timeout; all three completed and the row counts match the source.", tool: None },
        ]},
        Spec { family: "claude", host: None, cwd: "/home/dev/projects/web-dashboard", branch: "remote-sync", model: "claude-opus-4-1", hours_ago: 260, turns: vec![
            Turn { user: "Design how settings should sync between devices.", reply: "Proposal: store settings as a versioned document, merge per key with last-writer-wins, and sync on startup and on change. Offline edits queue locally and replay on reconnect.", tool: None },
        ]},
        Spec { family: "claude", host: Some("gpu-node"), cwd: "/srv/data/reports", branch: "main", model: "claude-sonnet-4-5", hours_ago: 330, turns: vec![
            Turn { user: "Plot daily active users for the last 90 days with a 7-day moving average.", reply: "Created `plot_dau.py`; it saves `dau_90d.png` with the raw series and the moving average.", tool: Some(Tool { name: "Bash", input: bash("python plot_dau.py"), output: "Saved dau_90d.png", failed: false }) },
        ]},
    ]
}

fn ts(hours_ago: i64, minute: i64) -> String {
    (Utc::now() - Duration::hours(hours_ago) + Duration::minutes(minute)).to_rfc3339()
}

fn claude_lines(spec: &Spec, id: &str) -> Vec<Value> {
    let base = json!({"sessionId": id, "cwd": spec.cwd, "gitBranch": spec.branch});
    let line = |kind: &str, minute: i64, uuid: String, message: Value| {
        let mut v = base.clone();
        v["type"] = json!(kind);
        v["timestamp"] = json!(ts(spec.hours_ago, minute));
        v["uuid"] = json!(uuid);
        v["message"] = message;
        v
    };
    let mut out = Vec::new();
    for (i, turn) in spec.turns.iter().enumerate() {
        let m = i as i64 * 6;
        out.push(line(
            "user",
            m,
            format!("{id}-u{i}"),
            json!({"role": "user", "content": turn.user}),
        ));
        let mut content = vec![json!({"type": "text", "text": turn.reply})];
        if let Some(tool) = &turn.tool {
            content.push(json!({"type": "tool_use", "id": format!("toolu_{i}"), "name": tool.name, "input": tool.input}));
        }
        let usage = json!({"input_tokens": 1800 + 900 * i, "output_tokens": 420 + 130 * i, "cache_read_input_tokens": 12000 * (i + 1)});
        out.push(line(
            "assistant",
            m + 1,
            format!("{id}-a{i}"),
            json!({"role": "assistant", "model": spec.model, "content": content, "usage": usage}),
        ));
        if let Some(tool) = &turn.tool {
            out.push(line("user", m + 2, format!("{id}-r{i}"), json!({"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": format!("toolu_{i}"), "content": tool.output, "is_error": tool.failed}
            ]})));
        }
    }
    out
}

fn codex_lines(spec: &Spec, id: &str) -> Vec<Value> {
    let line = |minute: i64, kind: &str, payload: Value| json!({"timestamp": ts(spec.hours_ago, minute), "type": kind, "payload": payload});
    let mut out = vec![line(
        0,
        "session_meta",
        json!({"id": id, "cwd": spec.cwd, "model": spec.model, "gitBranch": spec.branch, "timestamp": ts(spec.hours_ago, 0)}),
    )];
    let (mut input, mut output) = (0, 0);
    for (i, turn) in spec.turns.iter().enumerate() {
        let m = i as i64 * 6 + 1;
        out.push(line(
            m,
            "event_msg",
            json!({"type": "user_message", "message": turn.user}),
        ));
        out.push(line(m + 1, "response_item", json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": turn.reply}]})));
        if let Some(tool) = &turn.tool {
            let call = format!("call_{i}");
            out.push(line(m + 2, "response_item", json!({"type": "function_call", "name": tool.name, "arguments": tool.input.to_string(), "call_id": call})));
            out.push(line(
                m + 3,
                "response_item",
                json!({"type": "function_call_output", "call_id": call, "output": tool.output}),
            ));
        }
        input += 2400 + 700 * i;
        output += 380 + 90 * i;
        out.push(line(m + 4, "event_msg", json!({"type": "token_count", "info": {"total_token_usage": {"input_tokens": input, "cached_input_tokens": input / 2, "output_tokens": output}}})));
    }
    out
}

fn write_jsonl(path: &Path, lines: &[Value]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(path, text).unwrap();
}

#[test]
#[ignore = "regenerates src/dev/fixtures.json; run with `make preview-fixtures`"]
fn dump_preview_fixtures() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let (home, data, bin) = (
        tmp.path().join("home"),
        tmp.path().join("data"),
        tmp.path().join("bin"),
    );
    for dir in [&home, &data, &bin] {
        fs::create_dir_all(dir).unwrap();
    }
    // Hosts are "unreachable": the mirrors below are imported as-is.
    let ssh = bin.join("ssh");
    fs::write(&ssh, "#!/bin/sh\nexit 255\n").unwrap();
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var(
        "PATH",
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    std::env::set_var("HOME", &home);
    std::env::set_var(crate::brand::env_var("DATA_DIR"), &data);

    let mut hosts: Vec<&str> = Vec::new();
    for (n, spec) in specs().iter().enumerate() {
        let id = format!(
            "{:08x}-4b1e-4c2a-9f00-{:012x}",
            0x5e55_0000u32 + n as u32,
            n
        );
        let remote_home = format!("/home/{}", spec.host.map_or("dev", |_| "deploy"));
        let (store, lines) = match spec.family {
            "claude" => (
                format!(
                    "{remote_home}/.claude/projects/{}/{id}.jsonl",
                    spec.cwd.replace('/', "-")
                ),
                claude_lines(spec, &id),
            ),
            _ => (
                format!("{remote_home}/.codex/sessions/2026/10/rollout-{id}.jsonl"),
                codex_lines(spec, &id),
            ),
        };
        let path = match spec.host {
            Some(host) => {
                if !hosts.contains(&host) {
                    hosts.push(host);
                    let probe = json!({"host": host, "home": remote_home, "synced_at": Utc::now().to_rfc3339(), "roots": [
                        {"family": "claude", "kind": "jsonl", "root": format!("{remote_home}/.claude"), "sub": "projects"},
                        {"family": "codex", "kind": "jsonl", "root": format!("{remote_home}/.codex"), "sub": "sessions"},
                    ]});
                    let dir = data.join("remotes").join(host);
                    fs::create_dir_all(&dir).unwrap();
                    fs::write(dir.join("probe.json"), probe.to_string()).unwrap();
                }
                crate::remote::to_mirror(host, &store)
            }
            None => home.join(store.trim_start_matches(&format!("{remote_home}/"))),
        };
        write_jsonl(&path, &lines);
    }
    let config = json!({"hosts": hosts.iter().map(|h| json!({"name": h, "enabled": true})).collect::<Vec<_>>()});
    fs::write(data.join("remotes.json"), config.to_string()).unwrap();

    let summary = crate::importer::sync_all_with_progress(None, "incremental", &|_, _, _| {});
    assert!(
        summary
            .errors
            .iter()
            .all(|e| e.file.starts_with(crate::importer::HOST_ERROR_PREFIX)),
        "{:?}",
        summary.errors
    );

    let db = crate::db::db_path();
    // Stars and tags so the preview shows them.
    let conn = crate::db::open_db(&db).unwrap();
    conn.execute_batch(
        "INSERT INTO session_annotation(session_id, starred) SELECT id, 1 FROM session WHERE title LIKE '%integration tests%' OR title LIKE '%NaN%';
         INSERT INTO session_tag(session_id, label) SELECT id, 'backend' FROM session WHERE cwd LIKE '%orders-api%' OR cwd LIKE '%etl-jobs%';
         INSERT INTO session_tag(session_id, label) SELECT id, 'frontend' FROM session WHERE cwd LIKE '%web-dashboard%';",
    ).unwrap();
    drop(conn);

    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/dev/fixtures.json");
    write_fixtures(&db, &out, 100);
}

/// Preview your own library: `make preview-fixtures FROM=~/.hession/sessions.sqlite`.
/// Dumps the most recent sessions of a snapshot of that database to
/// src/dev/fixtures.local.json (git-ignored), which the preview prefers.
#[test]
#[ignore = "reads PREVIEW_FROM; run with `make preview-fixtures FROM=<database>`"]
fn dump_library_fixtures() {
    let Ok(source) = std::env::var("PREVIEW_FROM") else {
        panic!("set PREVIEW_FROM to a sessions.sqlite file");
    };
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var(crate::brand::env_var("DATA_DIR"), tmp.path());
    // The online backup API gives a consistent copy even while the app runs.
    let copy = tmp.path().join("sessions.sqlite");
    rusqlite::Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .backup(rusqlite::MAIN_DB, &copy, None)
        .unwrap();
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/dev/fixtures.local.json");
    write_fixtures(&copy, &out, 40);
}

/// Dump the newest `limit` sessions, their first transcript page, and the
/// library-wide responses the preview's mock IPC serves.
fn write_fixtures(db: &Path, out: &Path, limit: i64) {
    let db = db.to_path_buf();
    let all = search_sessions_inner(&db, &SessionFilters::default(), limit, 0).unwrap();
    let mut details = serde_json::Map::new();
    for session in all["sessions"].as_array().unwrap() {
        let id = session["id"].to_string();
        details.insert(id.clone(), get_session_inner(&db, &id, 500, 1).unwrap());
    }
    let fixtures = json!({
        "generatedAt": Utc::now().to_rfc3339(),
        "sessions": all["sessions"],
        "details": details,
        "agents": get_agents_inner(&db).unwrap(),
        "projects": get_projects_inner(&db).unwrap(),
        "stats": get_stats_inner(&db).unwrap(),
        "facets": get_facets_inner(&db).unwrap(),
        "checkpoints": get_sync_checkpoints_inner(&db).unwrap(),
        "analytics": get_analytics_inner(&db, "UTC", "", "", "", "", "").unwrap(),
        "pricingStatus": get_pricing_status_inner(&db).unwrap(),
        "pricingOverrides": get_pricing_overrides_inner(&db).unwrap(),
    });
    fs::create_dir_all(out.parent().unwrap()).unwrap();
    fs::write(out, serde_json::to_string(&fixtures).unwrap()).unwrap();
}
