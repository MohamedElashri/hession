//! Opt-in, synthetic stage timings. Run optimized and without other benchmarks.
use super::*;
use rusqlite::params;
use serde_json::json;
use std::time::Instant;

#[test]
#[ignore = "synthetic search stage profile; run release, serially, with --nocapture"]
fn profile_search_stages_10k() {
    let dir = tempfile::tempdir().unwrap();
    let mut conn = db::open_db(&dir.path().join("profile.sqlite")).unwrap();
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
    conn.execute_batch("PRAGMA optimize").unwrap();
    let mut report = Vec::new();
    for visibility in [
        SearchVisibility::AllIndexedText,
        SearchVisibility::Conversation,
        SearchVisibility::MetadataOnly,
    ] {
        for text in [
            "compiler regression",
            "\"regression needle\"",
            "src/module-42.rs",
            "agent:codex compiler",
        ] {
            let request = SearchRequest {
                query: text.into(),
                visibility,
                ..Default::default()
            };
            let parsed = query::parse(text).unwrap();
            let mut bounds_times = Vec::new();
            let mut execution_times = Vec::new();
            for iteration in 0..13 {
                let tx = conn.transaction().unwrap();
                compiler::register_visible_match(&tx).unwrap();
                let start = Instant::now();
                bounds::check(&tx, &request, &SearchScope::all(), &parsed).unwrap();
                let bounds_ms = start.elapsed().as_secs_f64() * 1000.;
                let start = Instant::now();
                let results =
                    compiler::execute(&tx, &request, &SearchScope::all(), &parsed, 0).unwrap();
                assert!(results.len() <= request.limit as usize + 1);
                let execution_ms = start.elapsed().as_secs_f64() * 1000.;
                tx.commit().unwrap();
                if iteration >= 3 {
                    bounds_times.push(bounds_ms);
                    execution_times.push(execution_ms);
                }
            }
            bounds_times.sort_by(f64::total_cmp);
            execution_times.sort_by(f64::total_cmp);
            report.push(json!({"visibility":format!("{visibility:?}"),"query":text,"boundsMedianMs":(bounds_times[4]+bounds_times[5])/2.,"executionMedianMs":(execution_times[4]+execution_times[5])/2.,"boundsSamplesMs":bounds_times,"executionSamplesMs":execution_times}));
        }
    }
    let report = json!({"sessions":10000,"entries":50000,"profile":"release","queries":report});
    println!(
        "SEARCH_STAGE_PROFILE {}",
        serde_json::to_string_pretty(&report).unwrap()
    );
    if let Ok(output) = std::env::var("HESSION_SEARCH_PROFILE_OUTPUT") {
        std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
