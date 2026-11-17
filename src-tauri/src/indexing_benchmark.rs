//! Opt-in performance measurement; all stores and observations are disposable.
//! Run alone after compilation: cargo test --release -p app --lib
//! native_indexing_latency_benchmark -- --ignored --nocapture --test-threads=1
use super::*;
use crate::indexer::Observer;
use hession_core::library_revision;
use serde_json::json;
use std::{
    io::Write,
    sync::mpsc,
    time::{Duration, Instant},
};

pub(super) fn percentiles(samples: &[f64]) -> Value {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) / 2.
    } else {
        sorted[middle]
    };
    json!({"samples":sorted.len(),"medianMs":median,"p95Ms":sorted[(sorted.len()*95).div_ceil(100)-1],"maxMs":sorted[sorted.len()-1]})
}
fn wait_until(timeout: Duration, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + timeout;
    while !ready() {
        assert!(Instant::now() < until, "benchmark timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
pub(super) fn host() -> Value {
    let cpu = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let memory = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    json!({
        "os": std::env::consts::OS, "arch": std::env::consts::ARCH,
        "logicalCpus":std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
        "cpu":cpu.lines().find(|l| l.starts_with("model name")).unwrap_or("unavailable"),
        "memory":memory.lines().find(|l| l.starts_with("MemTotal:")).unwrap_or("unavailable"),
        "load":fs::read_to_string("/proc/loadavg").unwrap_or_else(|_| "unavailable".into()).trim(),
        "debugAssertions":cfg!(debug_assertions), "profile":if cfg!(debug_assertions) {"debug"} else {"release"}
    })
}
pub(super) fn codex_message(id: &str, text: &str) -> Value {
    json!({"timestamp":"2026-11-17T10:00:01Z","type":"response_item","payload":{"type":"message","role":"user","id":id,"content":[{"type":"input_text","text":text}]}})
}

#[test]
#[ignore = "synthetic native watcher benchmark; run alone after builds and other tests finish"]
fn native_indexing_latency_benchmark() {
    const CODEX_FILES: usize = 24;
    const LEGACY_FILES: usize = 12;
    const ENTRIES: usize = 200;
    const SAMPLES: usize = 30;
    let host_before = host();
    let dir = tempfile::tempdir().unwrap();
    let codex_root = dir.path().join("codex");
    let legacy_root = dir.path().join("pi");
    fs::create_dir_all(codex_root.join("sessions")).unwrap();
    fs::create_dir_all(legacy_root.join("sessions")).unwrap();
    let mut native_bytes = 0;
    for number in 0..CODEX_FILES {
        let path = codex_root.join(format!("sessions/{number}.jsonl"));
        let mut file = fs::File::create(&path).unwrap();
        writeln!(file, "{}", json!({"timestamp":"2026-11-17T10:00:00Z","type":"session_meta","payload":{"id":format!("benchmark-{number}"),"cwd":"/synthetic/project"}})).unwrap();
        for entry in 0..ENTRIES {
            writeln!(
                file,
                "{}",
                codex_message(
                    &format!("old-{number}-{entry}"),
                    &format!(
                        "Synthetic existing Codex history {entry}: {}",
                        "x".repeat(1024)
                    )
                )
            )
            .unwrap();
        }
        native_bytes += file.metadata().unwrap().len();
    }
    let mut legacy_files = Vec::new();
    for number in 0..LEGACY_FILES {
        let path = legacy_root.join(format!("sessions/{number}.jsonl"));
        let mut file = fs::File::create(&path).unwrap();
        for entry in 0..ENTRIES {
            // Include the valid no-newline final record accepted by legacy readers.
            let record = json!({"type":"message","id":format!("legacy-{number}-{entry}"),"message":{"role":"user","content":[{"type":"text","text":format!("Synthetic legacy history: {}", "y".repeat(512))}]}});
            write!(
                file,
                "{record}{}",
                if entry + 1 == ENTRIES { "" } else { "\n" }
            )
            .unwrap();
        }
        native_bytes += file.metadata().unwrap().len();
        legacy_files.push(path);
    }
    let catalog = || {
        LocalCatalog(vec![
            Source {
                family: "codex",
                product: "Codex",
                root: codex_root.clone(),
                sub: "sessions",
                kind: SourceKind::Jsonl,
            },
            Source {
                family: "pi",
                product: "Pi",
                root: legacy_root.clone(),
                sub: "sessions",
                kind: SourceKind::Jsonl,
            },
        ])
    };
    let database = dir.path().join("library/history.sqlite");
    let mut conn = crate::db::open_db(&database).unwrap();
    crate::db::configure_bulk_writer(&conn).unwrap();
    let (sender, receiver) = mpsc::channel();
    let start = Instant::now();
    let observer = Observer::start(
        database,
        catalog(),
        Arc::new(move |change| {
            let _ = sender.send((Instant::now(), change));
        }),
    )
    .unwrap();
    let expected = CODEX_FILES * (ENTRIES + 1) + LEGACY_FILES * ENTRIES;
    wait_until(Duration::from_secs(180), || {
        let count = conn
            .query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))
            .unwrap();
        let status = observer.status();
        count == expected as i64
            && status.phase == "active"
            && !status.may_be_stale
            && status.active_source.is_none()
    });
    let startup_ms = start.elapsed().as_secs_f64() * 1000.;
    let hot_file = codex_root.join("sessions/0.jsonl");
    let source = native_source_uri("codex", &hot_file).unwrap();
    let session = conn
        .query_row("SELECT id FROM session WHERE source_uri=?", [source], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap();
    let mut native = fs::OpenOptions::new().append(true).open(&hot_file).unwrap();
    let mut committed = Vec::new();
    let mut events = Vec::new();
    for sample in 0..SAMPLES {
        while receiver.try_recv().is_ok() {}
        let marker = format!("sample-{sample}");
        let start = Instant::now();
        writeln!(
            native,
            "{}",
            codex_message(&marker, "Controlled measured append — café")
        )
        .unwrap();
        native.flush().unwrap();
        native.sync_data().unwrap();
        wait_until(Duration::from_secs(15), || {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM entry WHERE session_id=? AND external_entry_id=?)",
                params![session, marker],
                |r| r.get::<_, bool>(0),
            )
            .unwrap()
        });
        committed.push(start.elapsed().as_secs_f64() * 1000.);
        let revision = library_revision::current(&conn).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let (at, change) = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("committed revision must become visible in the watcher event");
            if change.revision.revision >= revision.revision
                && change.session_ids.contains(&session)
            {
                events.push(at.saturating_duration_since(start).as_secs_f64() * 1000.);
                break;
            }
        }
        assert_eq!(observer.status().phase, "active");
        assert!(!observer.status().may_be_stale);
    }
    drop(observer);
    let catalog = catalog();
    let epoch = hession_core::indexing::epoch(&conn).unwrap();
    let revision = library_revision::current(&conn).unwrap();
    let mut quiet_metadata = Vec::new();
    let mut quiet_hash = Vec::new();
    for _ in 0..SAMPLES {
        let start = Instant::now();
        for path in &legacy_files {
            assert_eq!(catalog.import(&mut conn, "pi", path, epoch).unwrap(), 0);
        }
        quiet_metadata.push(start.elapsed().as_secs_f64() * 1000.);
        let start = Instant::now();
        for path in &legacy_files {
            assert_eq!(
                catalog
                    .import_with_policy(&mut conn, "pi", path, epoch, true)
                    .unwrap(),
                0
            );
        }
        quiet_hash.push(start.elapsed().as_secs_f64() * 1000.);
    }
    assert_eq!(
        library_revision::current(&conn).unwrap(),
        revision,
        "quiet audits must not rebuild normalized history"
    );
    let commit_stats = percentiles(&committed);
    let event_stats = percentiles(&events);
    let report = json!({"hostBefore":host_before,"hostAfter":host(),"corpus":{"codexFiles":CODEX_FILES,"legacyFiles":LEGACY_FILES,"existingEntries":expected,"nativeBytes":native_bytes,"startupMs":startup_ms},"writeToCommittedRevision":commit_stats,"writeToWatcherEvent":event_stats,"quietLegacyMetadataPass":percentiles(&quiet_metadata),"quietLegacyContentAuditPass":percentiles(&quiet_hash),"rawCommittedMs":committed,"rawEventMs":events});
    println!(
        "INDEXING_BENCHMARK {}",
        serde_json::to_string_pretty(&report).unwrap()
    );
    if let Ok(output) = std::env::var("HESSION_INDEX_BENCH_OUTPUT") {
        fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert!(
        commit_stats["p95Ms"].as_f64().unwrap() < 2000.,
        "write-to-commit p95 exceeded 2s"
    );
    assert!(
        event_stats["p95Ms"].as_f64().unwrap() < 2000.,
        "write-to-event p95 exceeded 2s"
    );
}
