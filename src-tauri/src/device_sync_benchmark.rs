//! Synthetic, opt-in probes. Run optimized and alone after other work finishes.
use super::*;
use std::time::Instant;

fn samples(mut operation: impl FnMut()) -> Value {
    let mut values = Vec::new();
    for run in 0..10 {
        let start = Instant::now();
        operation();
        if run >= 3 {
            values.push(start.elapsed().as_secs_f64() * 1000.);
        }
    }
    values.sort_by(f64::total_cmp);
    json!({"medianMs":values[3],"maxMs":values[6],"samplesMs":values})
}

#[test]
#[ignore = "synthetic private-sync CPU/quiet-capture benchmark; release, serial, no network"]
fn private_sync_performance() {
    let mut cases = Vec::new();
    for count in [1000, 10_000] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.sqlite");
        let mut conn = db::open_db(&path).unwrap();
        db::configure_bulk_writer(&conn).unwrap();
        init_schema(&conn).unwrap();
        let key = LibraryKey::generate();
        let config = Config {
            enabled: true,
            vault_id: "performance-vault".into(),
            device_id: "performance-device".into(),
            device_name: "Synthetic".into(),
            credential_ref: "not-an-os-credential".into(),
            relay_url: "https://invalid.example".into(),
            key_fingerprint: key.fingerprint(),
            scope: vec!["/synthetic/project".into()],
            last_sync: None,
        };
        save(&path, &config).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');
            INSERT INTO workspace(id,path) VALUES(1,'/synthetic/project');
            INSERT INTO session(id,agent_id,workspace_id,source_uri,title,cwd) VALUES(1,1,1,'fixture://private-sync','Synthetic history','/synthetic/project');").unwrap();
        let text = format!(
            "Unicode 🌍 and ordinary conversation text {}",
            "x".repeat(1000)
        );
        for ordinal in 0..count {
            tx.execute("INSERT INTO entry(id,session_id,ordinal,kind,role,search_text,payload_json) VALUES(?,1,?,'message','assistant',?,'{\"private\":\"never copied\"}')",params![ordinal+1,ordinal,text]).unwrap();
            tx.execute(
                "INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(?,0,'text',?)",
                params![ordinal + 1, text],
            )
            .unwrap();
            tx.execute("INSERT INTO tool_invocation(session_id,entry_id,tool_name,status,result_text) VALUES(1,?,'Read','completed','private tool body never copied')",[ordinal+1]).unwrap();
        }
        tx.commit().unwrap();
        let expected = content_digest(&content(&conn, 1).unwrap()).unwrap();
        let content_time = samples(|| {
            assert_eq!(
                content_digest(&content(&conn, 1).unwrap()).unwrap(),
                expected
            );
        });
        let epoch = hession_core::indexing::epoch(&conn).unwrap();
        let start = Instant::now();
        capture(&path, &config, &key, epoch).unwrap();
        let first = start.elapsed().as_secs_f64() * 1000.;
        let queued: i64 = conn
            .query_row("SELECT count(*) FROM replication_outbox", [], |r| r.get(0))
            .unwrap();
        let quiet = samples(|| {
            capture(&path, &config, &key, epoch).unwrap();
        });
        assert_eq!(
            conn.query_row("SELECT count(*) FROM replication_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            queued
        );
        conn.execute(
            "UPDATE content_part SET text='Edited conversation 🌍' WHERE entry_id=1",
            [],
        )
        .unwrap();
        let start = Instant::now();
        capture(&path, &config, &key, epoch).unwrap();
        let edit = start.elapsed().as_secs_f64() * 1000.;
        assert!(
            conn.query_row("SELECT count(*) FROM replication_outbox", [], |r| r
                .get::<_, i64>(0))
                .unwrap()
                > queued
        );
        cases.push(json!({"events":count,"contentDigest":expected,"contentAndDigest":content_time,"firstCaptureMs":first,"quietCapture":quiet,"editedCaptureMs":edit}));
    }
    let report = json!({"profile":"release","network":false,"cases":cases});
    if let Ok(path) = std::env::var("HESSION_PRIVATE_SYNC_BENCH_OUTPUT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
