//! Shared-catalog A/B benchmark. No real agent histories or global cache eviction.
use hession_core::{
    db,
    search::{SearchRequest, SearchScope, SearchVisibility},
    service::Library,
};
use rusqlite::params;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, time::Instant};

fn cpu_ms() -> f64 {
    // SAFETY: getrusage writes this valid numeric C struct and retains no pointer.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) }, 0);
    (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64 * 1000.
        + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1000.
}

#[test]
#[ignore = "synthetic shared-catalog benchmark; run release, serially, with explicit output"]
fn benchmark_shared_catalog() {
    let path = PathBuf::from(
        std::env::var_os("HESSION_SEARCH_BENCH_DB").expect("Set a private synthetic fixture path"),
    );
    if std::env::var_os("HESSION_SEARCH_BENCH_CREATE").is_some() {
        assert!(!path.exists(), "Never replace an existing library");
        let mut conn = db::open_db(&path).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex'); INSERT INTO workspace(id,path) VALUES(1,'/fixture/project');").unwrap();
        for id in 1..=10_000_i64 {
            tx.execute("INSERT INTO session(id,agent_id,workspace_id,source_uri,title,started_at) VALUES(?,1,1,?,?,?)",params![id,format!("synthetic:{id}"),format!("Compiler task {id}"),format!("2026-01-{:02}T12:00:00Z",id%28+1)]).unwrap();
            for ordinal in 0..5_i64 {
                let text=format!("compiler implementation session {id} message {ordinal} Rust C++ src/module-{}.rs {}",id%100,if id%25==0 {"regression needle"}else{"routine work"});
                tx.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text) VALUES(?,?,'message','assistant',?)",params![id,ordinal,text]).unwrap();
                tx.execute("INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(last_insert_rowid(),0,'text',?)",[text]).unwrap();
            }
        }
        tx.commit().unwrap();
        conn.execute_batch("PRAGMA optimize; PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        return;
    }
    let conn = db::open_read_only(&path).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM session WHERE source_uri LIKE 'synthetic:%'",
            [],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        10000
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM entry", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        50000
    );
    drop(conn);
    let library = Library::new(&path);
    let mut cases = Vec::new();
    for visibility in [
        SearchVisibility::AllIndexedText,
        SearchVisibility::Conversation,
        SearchVisibility::MetadataOnly,
    ] {
        for query in [
            "compiler regression",
            "\"regression needle\"",
            "src/module-42.rs",
            "agent:codex compiler",
        ] {
            let request = SearchRequest {
                query: query.into(),
                visibility,
                ..Default::default()
            };
            let mut wall = Vec::new();
            let mut cpu = Vec::new();
            let mut digest = Sha256::new();
            for sample in 0..11 {
                let before_cpu = cpu_ms();
                let start = Instant::now();
                let result = library.search(&request, &SearchScope::all()).unwrap();
                let elapsed = start.elapsed().as_secs_f64() * 1000.;
                let consumed = cpu_ms() - before_cpu;
                if sample >= 3 {
                    wall.push(elapsed);
                    cpu.push(consumed);
                    digest.update(serde_json::to_vec(&result).unwrap());
                }
            }
            wall.sort_by(f64::total_cmp);
            cpu.sort_by(f64::total_cmp);
            cases.push(json!({"visibility":format!("{visibility:?}"),"query":query,"medianWallMs":(wall[3]+wall[4])/2.,"medianCpuMs":(cpu[3]+cpu[4])/2.,"wallSamplesMs":wall,"cpuSamplesMs":cpu,"resultSha256":format!("{:x}",digest.finalize())}));
        }
    }
    let report = json!({"profile":"release","sessions":10000,"entries":50000,"queries":cases});
    let output =
        std::env::var_os("HESSION_SEARCH_BENCH_OUTPUT").expect("Set benchmark output path");
    std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}
