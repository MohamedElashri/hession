//! Run ignored tests serially after all builds/tests on the host finish.
use super::{
    indexing_benchmark::{codex_message, host, percentiles},
    *,
};
use crate::indexer::Observer;
use hession_core::library_revision;
use serde_json::json;
use std::{
    io::Write,
    time::{Duration, Instant},
};

fn memory() -> Value {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    json!({"rss":status.lines().find(|l| l.starts_with("VmRSS:")).unwrap_or("unavailable"),"highWater":status.lines().find(|l| l.starts_with("VmHWM:")).unwrap_or("unavailable")})
}
fn save_report(environment: &str, prefix: &str, report: &Value) {
    println!("{prefix} {}", serde_json::to_string_pretty(report).unwrap());
    if let Ok(path) = std::env::var(environment) {
        fs::write(path, serde_json::to_vec_pretty(report).unwrap()).unwrap();
    }
}
fn import(conn: &mut Connection, uri: &str, force: bool) -> usize {
    with_cancellation(Arc::new(AtomicBool::new(false)), || {
        import_file_with_conn(conn, uri, force, None, &mut ImportPricing::new()).unwrap()
    })
}

#[test]
#[ignore = "1k/10k/100k fixed-ten append benchmark; run optimized, alone after compilation"]
fn fixed_ten_append_scaling_benchmark() {
    let before = host();
    let mut rows = Vec::new();
    for size in [1000, 10_000, 100_000] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("history.jsonl");
        let mut writer = std::io::BufWriter::new(fs::File::create(&file).unwrap());
        writeln!(writer, "{}", json!({"type":"session_meta","payload":{"id":format!("scaling-{size}"),"cwd":"/synthetic/project"}})).unwrap();
        for entry in 1..size {
            writeln!(
                writer,
                "{}",
                codex_message(
                    &format!("old-{entry}"),
                    &format!("Synthetic archived event {entry}: {}", "x".repeat(128))
                )
            )
            .unwrap();
        }
        writer.flush().unwrap();
        drop(writer);
        let uri = native_source_uri("codex", &file).unwrap();
        let mut conn = crate::db::open_db(&dir.path().join("library/history.sqlite")).unwrap();
        crate::db::configure_bulk_writer(&conn).unwrap();
        let start = Instant::now();
        assert_eq!(import(&mut conn, &uri, false), size as usize);
        let initial_ms = start.elapsed().as_secs_f64() * 1000.;
        let mut appended = Vec::new();
        let mut rebuilt = Vec::new();
        let mut bytes = Vec::new();
        let mut native = fs::OpenOptions::new().append(true).open(&file).unwrap();
        for sample in 0..5 {
            let first_id: i64 = conn
                .query_row("SELECT id FROM entry ORDER BY ordinal LIMIT 1", [], |r| {
                    r.get(0)
                })
                .unwrap();
            for entry in 0..10 {
                writeln!(
                    native,
                    "{}",
                    codex_message(
                        &format!("sample-{sample}-{entry}"),
                        "Fixed-ten-event append"
                    )
                )
                .unwrap();
            }
            native.flush().unwrap();
            native.sync_data().unwrap();
            bytes.push(native.metadata().unwrap().len());
            let start = Instant::now();
            assert_eq!(import(&mut conn, &uri, false), 10);
            appended.push(start.elapsed().as_secs_f64() * 1000.);
            let count: i64 = conn
                .query_row("SELECT count(*) FROM entry", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, size + (sample + 1) * 10);
            assert_eq!(
                conn.query_row("SELECT id FROM entry ORDER BY ordinal LIMIT 1", [], |r| r
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
                first_id,
                "append must retain existing entry identity"
            );
            let start = Instant::now();
            assert_eq!(import(&mut conn, &uri, true), count as usize);
            rebuilt.push(start.elapsed().as_secs_f64() * 1000.);
            assert_eq!(
                conn.query_row("SELECT count(*) FROM entry", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                count
            );
        }
        rows.push(json!({"initialEntries":size,"appendedEventsPerSample":10,"initialImportMs":initial_ms,"nativeBytesPerSample":bytes,"append":percentiles(&appended),"fullReimport":percentiles(&rebuilt),"rawAppendMs":appended,"rawFullReimportMs":rebuilt,"processMemory":memory()}));
    }
    let report = json!({"hostBefore":before,"hostAfter":host(),"cases":rows,"note":"Fixed ten-event normalization is incremental; verified old-prefix SHA-256 still reads O(total source bytes). Full reimport timing forces parser/checkpoint reset over the same source immediately after append."});
    save_report(
        "HESSION_INDEX_SCALING_OUTPUT",
        "INDEXING_SCALING_BENCHMARK",
        &report,
    );
}

#[test]
#[ignore = "tool-heavy fixed-ten append benchmark; run release, serially, after builds finish"]
fn tool_result_append_scaling_benchmark() {
    let mut cases = Vec::new();
    for calls in [1000, 10_000] {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tools.jsonl");
        let mut native = std::io::BufWriter::new(fs::File::create(&file).unwrap());
        writeln!(native,"{}",json!({"type":"session_meta","payload":{"id":"tool-benchmark","cwd":"/synthetic/project"}})).unwrap();
        for call in 0..calls {
            writeln!(native,"{}",json!({"type":"response_item","payload":{"type":"function_call","name":"Read","call_id":format!("call-{call}"),"arguments":"{\"path\":\"src/main.rs\"}"}})).unwrap();
            writeln!(native,"{}",json!({"type":"response_item","payload":{"type":"function_call_output","call_id":format!("call-{call}"),"output":"x".repeat(1024)}})).unwrap();
        }
        native.flush().unwrap();
        drop(native);
        let uri = native_source_uri("codex", &file).unwrap();
        let mut conn = crate::db::open_db(&dir.path().join("library/history.sqlite")).unwrap();
        crate::db::configure_bulk_writer(&conn).unwrap();
        assert_eq!(import(&mut conn, &uri, false), 2 * calls + 1);
        let initial_revision = library_revision::current(&conn).unwrap();
        let mut samples = Vec::new();
        for sample in 0..8 {
            let mut native = fs::OpenOptions::new().append(true).open(&file).unwrap();
            for entry in 0..10 {
                writeln!(
                    native,
                    "{}",
                    codex_message(
                        &format!("append-{sample}-{entry}"),
                        "A small continuation after completed tools"
                    )
                )
                .unwrap();
            }
            native.sync_data().unwrap();
            let start = Instant::now();
            assert_eq!(import(&mut conn, &uri, false), 10);
            samples.push(start.elapsed().as_secs_f64() * 1000.);
            assert_eq!(
                conn.query_row(
                    "SELECT count(*) FROM tool_invocation WHERE result_text IS NOT NULL",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                calls as i64
            );
        }
        assert!(library_revision::current(&conn).unwrap().revision > initial_revision.revision);
        cases.push(json!({"historicalCalls":calls,"bytes":fs::metadata(&file).unwrap().len(),"append":percentiles(&samples),"samplesMs":samples}));
    }
    save_report(
        "HESSION_TOOL_APPEND_OUTPUT",
        "TOOL_APPEND_BENCHMARK",
        &json!({"cases":cases}),
    );
}

fn cpu_ticks() -> u64 {
    let stat = fs::read_to_string("/proc/self/stat")
        .expect("Linux /proc is required for idle CPU measurement");
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect();
    fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
}

#[test]
#[ignore = "30s idle native watcher CPU/RSS benchmark; Linux /proc, run alone after compilation"]
fn quiet_watcher_cpu_benchmark() {
    let before = host();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("codex");
    fs::create_dir_all(root.join("sessions")).unwrap();
    for number in 0..100 {
        let mut file = fs::File::create(root.join(format!("sessions/{number}.jsonl"))).unwrap();
        writeln!(file, "{}", json!({"type":"session_meta","payload":{"id":format!("idle-{number}"),"cwd":"/synthetic/project"}})).unwrap();
        for entry in 0..10 {
            writeln!(
                file,
                "{}",
                codex_message(&format!("old-{entry}"), "Quiet existing history")
            )
            .unwrap();
        }
    }
    let database = dir.path().join("library/history.sqlite");
    let conn = crate::db::open_db(&database).unwrap();
    let events = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = events.clone();
    let observer = Observer::start(
        database,
        LocalCatalog::fixture(root),
        Arc::new(move |_| {
            captured.fetch_add(1, Ordering::SeqCst);
        }),
    )
    .unwrap();
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        let count: i64 = conn
            .query_row("SELECT count(*) FROM entry", [], |r| r.get(0))
            .unwrap();
        let status = observer.status();
        if count == 1100
            && status.phase == "active"
            && status.active_source.is_none()
            && !status.may_be_stale
        {
            break;
        }
        assert!(Instant::now() < until, "initial observation did not settle");
        std::thread::sleep(Duration::from_millis(10));
    }
    // Let the final initial import notification finish before measuring quiet work.
    std::thread::sleep(Duration::from_secs(1));
    let revision = library_revision::current(&conn).unwrap();
    let notices = events.load(Ordering::SeqCst);
    let initial_memory = memory();
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    assert!(ticks_per_second > 0);
    let initial_ticks = cpu_ticks();
    let start = Instant::now();
    let mut rss_samples = Vec::new();
    for _ in 0..30 {
        std::thread::sleep(Duration::from_secs(1));
        rss_samples.push(memory());
    }
    let wall = start.elapsed().as_secs_f64();
    let ticks = cpu_ticks() - initial_ticks;
    let cpu_seconds = ticks as f64 / ticks_per_second as f64;
    let percent = cpu_seconds / wall * 100.;
    assert_eq!(library_revision::current(&conn).unwrap(), revision);
    assert_eq!(events.load(Ordering::SeqCst), notices);
    assert_eq!(observer.status().phase, "active");
    assert!(!observer.status().may_be_stale);
    let report = json!({"hostBefore":before,"hostAfter":host(),"corpus":{"sourceFiles":100,"existingEntries":1100},"samplingIntervalSeconds":1,"wallSeconds":wall,"processCpuTicks":ticks,"ticksPerSecond":ticks_per_second,"processCpuSeconds":cpu_seconds,"oneCoreCpuPercent":percent,"initialMemory":initial_memory,"finalMemory":memory(),"rssSamples":rss_samples,"note":"Process CPU includes the watcher and one sleeping benchmark thread, measured across two15-second metadata polling cycles; unchanged library revision and event count assert quiet behavior."});
    drop(observer);
    save_report(
        "HESSION_INDEX_IDLE_OUTPUT",
        "INDEXING_IDLE_BENCHMARK",
        &report,
    );
    assert!(
        percent < 1.,
        "quiet watcher CPU exceeded1% of one logical CPU"
    );
}
