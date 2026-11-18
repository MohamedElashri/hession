//! Opt-in benchmarks against disposable, deterministic catalogs. Run serially
//! after compilation; preserve the executable for before/after comparisons.
use super::*;
use rusqlite::params;
use std::{
    fs,
    hash::{Hash, Hasher},
    time::Instant,
};

fn cpu_ms() -> f64 {
    // SAFETY: the OS writes a numeric struct through this valid pointer.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
    (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.
        + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.
}

fn sample(name: &str, mut operation: impl FnMut() -> Value) -> Value {
    let mut wall = Vec::new();
    let mut cpu = Vec::new();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for iteration in 0..11 {
        let before_cpu = cpu_ms();
        let start = Instant::now();
        let value = operation();
        let elapsed = start.elapsed().as_secs_f64() * 1000.;
        let consumed = cpu_ms() - before_cpu;
        if iteration >= 3 {
            wall.push(elapsed);
            cpu.push(consumed);
            serde_json::to_vec(&value).unwrap().hash(&mut hash);
        }
    }
    let mut sorted = wall.clone();
    sorted.sort_by(f64::total_cmp);
    json!({"scenario":name,"medianMs":(sorted[3]+sorted[4])/2.,"p95Ms":sorted[7],
        "wallSamplesMs":wall,"cpuSamplesMs":cpu,"resultHash":format!("{:016x}",hash.finish())})
}

fn seed(path: &Path, size: i64) {
    let mut conn = open_db(path).unwrap();
    crate::db::configure_bulk_writer(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute_batch(
        "INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
        INSERT INTO workspace(id,path,display_name) VALUES(1,'/fixture/project','Fixture');
        INSERT INTO model(id,provider,model_key) VALUES(1,'fixture','fixture-model');",
    )
    .unwrap();
    for id in 1..=size {
        let time = chrono::DateTime::from_timestamp(1_767_225_600 + id, 0)
            .unwrap()
            .to_rfc3339();
        // Helpers are newer than the primary conversations: first-page queries
        // must not walk 90,000 discarded rows to find twenty conversations.
        tx.execute("INSERT INTO session(id,agent_id,workspace_id,external_id,source_uri,title,started_at,last_event_at,raw_metadata_json,is_subagent)
            VALUES(?,1,1,?,?,?,?,?,?,?)", params![id,format!("fixture-{id}"),format!("catalog-benchmark://{id}"),format!("Compiler task {id}"),time,time,json!({"fixture":"x".repeat(1024)}).to_string(),id>size/10]).unwrap();
        tx.execute("INSERT INTO entry(id,session_id,ordinal,kind,role,search_text) VALUES(?,?,0,'message','user',?)",
            params![id,id,if id%100==0 {"compiler needle"} else {"compiler ordinary"}]).unwrap();
        tx.execute(
            "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'text',?)",
            params![
                id,
                if id % 100 == 0 {
                    "compiler needle"
                } else {
                    "compiler ordinary"
                }
            ],
        )
        .unwrap();
        tx.execute(
            "INSERT INTO session_context(session_id,entry_id,model_id,branch) VALUES(?,?,1,'main')",
            params![id, id],
        )
        .unwrap();
        tx.execute("INSERT INTO tool_invocation(session_id,entry_id,tool_name,status) VALUES(?,?,'Read','success')",params![id,id]).unwrap();
        tx.execute("INSERT INTO file_activity(session_id,entry_id,path,action) VALUES(?,?,'src/main.rs','read')",params![id,id]).unwrap();
    }
    tx.commit().unwrap();
    conn.execute_batch("PRAGMA optimize; PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
}

#[test]
#[ignore = "synthetic library browsing/startup benchmark; run release, serially, after builds finish"]
fn library_catalog_performance() {
    let root = PathBuf::from(
        std::env::var_os("HESSION_CATALOG_BENCH_DIR")
            .expect("Set a private synthetic fixture directory"),
    );
    fs::create_dir_all(&root).unwrap();
    let sizes =
        std::env::var("HESSION_CATALOG_BENCH_SIZES").unwrap_or_else(|_| "10000,100000".into());
    let mut report = Vec::new();
    for size in sizes.split(',').map(|size| size.parse::<i64>().unwrap()) {
        assert!((100..=100_000).contains(&size));
        let path = root.join(format!("catalog-{size}.sqlite"));
        if !path.exists() {
            seed(&path, size);
        }
        let conn = open_db(&path).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM session WHERE source_uri LIKE 'catalog-benchmark://%'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            size
        );
        let mut cases = Vec::new();
        for (name, filters, offset) in [
            ("visible-recent", SessionFilters::default(), 0),
            (
                "all-recent",
                SessionFilters {
                    include_subagents: true,
                    ..Default::default()
                },
                0,
            ),
            (
                "visible-oldest",
                SessionFilters {
                    sort: "oldest".into(),
                    ..Default::default()
                },
                0,
            ),
            (
                "visible-deep-page",
                SessionFilters::default(),
                (size / 20).min(1000),
            ),
            (
                "visible-most-messages",
                SessionFilters {
                    sort: "messages".into(),
                    ..Default::default()
                },
                0,
            ),
            (
                "visible-transcript-search",
                SessionFilters {
                    q: "needle".into(),
                    ..Default::default()
                },
                0,
            ),
            (
                "missing-transcript-search",
                SessionFilters {
                    q: "nonexistentword".into(),
                    ..Default::default()
                },
                0,
            ),
            (
                "visible-project",
                SessionFilters {
                    project: "/fixture/project".into(),
                    ..Default::default()
                },
                0,
            ),
        ] {
            cases.push(sample(name, || {
                search_sessions_inner(&path, &filters, 20, offset).unwrap()
            }));
        }
        for (name, operation) in [
            (
                "startup-projects",
                get_projects_inner as fn(&Path) -> Result<Value, String>,
            ),
            ("startup-stats", get_stats_inner),
            ("startup-facets", get_facets_inner),
        ] {
            cases.push(sample(name, || operation(&path).unwrap()));
        }
        cases.push(sample("developer-visible", || {
            let request = hession_core::search::SearchRequest {
                query: "compiler needle".into(),
                visibility: hession_core::search::SearchVisibility::AllIndexedText,
                filters: hession_core::search::SearchFilters {
                    exclude_subagents: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            match hession_core::service::Library::new(&path)
                .search(&request, &hession_core::search::SearchScope::all())
            {
                Ok(page) => json!({"results":page.results}),
                Err(error) => json!({"error":error.to_string()}),
            }
        }));
        report.push(json!({"sessions":size,"helpers":size-size/10,"cases":cases}));
    }
    let output =
        std::env::var_os("HESSION_CATALOG_BENCH_OUTPUT").expect("Set benchmark report path");
    fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}
