//! Advisory fixture-file cache experiment. Never evicts global OS caches or any
//! user library. Run only in an explicitly coordinated quiet benchmark window.
#[cfg(target_os = "linux")]
mod linux {
    use hession_core::{
        db,
        search::{SearchError, SearchRequest, SearchScope, SearchVisibility},
        service::Library,
    };
    use rusqlite::{params, Connection};
    use serde_json::{json, Value};
    use std::{
        fs::OpenOptions,
        io,
        os::{
            fd::AsRawFd,
            unix::fs::{OpenOptionsExt, PermissionsExt},
        },
        path::Path,
        time::{Duration, Instant},
    };

    fn fixture(path: &Path) {
        // Bypass open_db's process-local keepalive: this experiment must close
        // EVERY SQLite handle before advising fixture pages out of the cache.
        let mut conn = Connection::open(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        conn.busy_timeout(Duration::from_millis(db::BUSY_TIMEOUT_MS))
            .unwrap();
        conn.set_prepared_statement_cache_capacity(128);
        conn.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA trusted_schema=ON; PRAGMA wal_autocheckpoint=4096;",
        )
        .unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        db::init_schema(&conn).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute_batch("INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex');INSERT INTO workspace(id,path) VALUES(1,'/fixture/project');").unwrap();
        // Exact original warm benchmark corpus. This separate experiment leaves
        // benchmark_10k and its sampling/fixture untouched.
        for id in 1..=10_000_i64 {
            tx.execute("INSERT INTO session(id,agent_id,workspace_id,source_uri,title,started_at) VALUES(?,1,1,?,?,?)",params![id,format!("synthetic:{id}"),format!("Compiler task {id}"),format!("2026-01-{:02}T12:00:00Z",id%28+1)]).unwrap();
            for ordinal in 0..5_i64 {
                let text=format!("compiler implementation session {id} message {ordinal} Rust C++ src/module-{}.rs {}",id%100,if id%25==0{"regression needle"}else{"routine work"});
                tx.execute("INSERT INTO entry(session_id,ordinal,kind,role,search_text) VALUES(?,?,'message','assistant',?)",params![id,ordinal,text]).unwrap();
                tx.execute("INSERT INTO content_part(entry_id,ordinal,kind,text) VALUES(last_insert_rowid(),0,'text',?)",[text]).unwrap();
            }
        }
        tx.commit().unwrap();
        conn.execute_batch("PRAGMA optimize").unwrap();
        let checkpoint: (i64, i64, i64) = conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(checkpoint.0, 0, "fixture checkpoint busy");
        conn.close().unwrap();
    }
    fn usage() -> io::Result<libc::rusage> {
        // SAFETY: rusage is a C numeric struct; getrusage receives a valid
        // writable pointer and never retains it beyond this call.
        let mut result: libc::rusage = unsafe { std::mem::zeroed() };
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut result) } == 0 {
            Ok(result)
        } else {
            Err(io::Error::last_os_error())
        }
    }
    fn read_bytes() -> Option<u64> {
        std::fs::read_to_string("/proc/self/io")
            .ok()?
            .lines()
            .find_map(|line| {
                line.strip_prefix("read_bytes:")
                    .and_then(|v| v.trim().parse().ok())
            })
    }
    fn cpu_seconds(value: &libc::rusage) -> f64 {
        value.ru_utime.tv_sec as f64
            + value.ru_utime.tv_usec as f64 / 1e6
            + value.ru_stime.tv_sec as f64
            + value.ru_stime.tv_usec as f64 / 1e6
    }
    fn evict_owned_fixture_file(path: &Path, label: &str) -> io::Result<Option<Value>> {
        // O_NOFOLLOW prevents a replaced sidecar link from extending the
        // experiment beyond its private fixture directory.
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "fixture file must be regular",
            ));
        }
        file.sync_all()?;
        // SAFETY: this live File owns the fd. Zero offset/length advises the
        // entire owned fixture file only; no global cache control is invoked.
        let code =
            unsafe { libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED) };
        Ok(Some(
            json!({"file":label,"bytes":file.metadata()?.len(),"returnCode":code,"error":if code==0{None}else{Some(io::Error::from_raw_os_error(code).to_string())}}),
        ))
    }
    pub fn run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("benchmark.sqlite");
        fixture(&path);
        let library = Library::new(&path);
        let wal = dir.path().join("benchmark.sqlite-wal");
        eprintln!(
            "{}",
            json!({"scenario":"advisory fixture file cache","notice":"Only owned fixture DB/WAL are fsynced and advised DONTNEED. Advisory acceptance and process I/O counters do not prove every page was OS-cache-cold or attribute all I/O exclusively to this DB. No global cache dropped.","dbBytes":std::fs::metadata(&path).unwrap().len(),"loadBefore":std::fs::read_to_string("/proc/loadavg").ok()})
        );
        let mut successful = 0_usize;
        let mut timed_out = 0_usize;
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
                // All SQLite handles from earlier requests have closed. Nothing
                // writes this catalog after fixture construction/checkpoint.
                let advice_start = Instant::now();
                let mut advice = Vec::new();
                for (file, label) in [(&path, "db"), (&wal, "wal")] {
                    if let Some(result) = evict_owned_fixture_file(file, label).unwrap() {
                        advice.push(result);
                    }
                }
                assert!(!advice.is_empty());
                let advice_ms = advice_start.elapsed().as_secs_f64() * 1000.0;
                let before_io = read_bytes();
                let before = usage().unwrap();
                let start = Instant::now();
                // Library::search opens a fresh db::open_read_only connection.
                let result = library.search(
                    &SearchRequest {
                        query: query.into(),
                        visibility,
                        ..Default::default()
                    },
                    &SearchScope::all(),
                );
                let elapsed = start.elapsed();
                let after = usage().unwrap();
                let after_io = read_bytes();
                let physical_bytes = before_io.zip(after_io).map(|(a, b)| b.saturating_sub(a));
                eprintln!(
                    "{}",
                    json!({"visibility":format!("{visibility:?}"),"query":query,"advice":advice,"adviceMs":advice_ms,"queryMs":elapsed.as_secs_f64()*1000.0,"cpuMs":(cpu_seconds(&after)-cpu_seconds(&before))*1000.0,"processReadBytesDelta":physical_bytes,"processStorageReadObserved":physical_bytes.map(|n|n>0),"inputBlocksDelta":after.ru_inblock.saturating_sub(before.ru_inblock),"majorFaultsDelta":after.ru_majflt.saturating_sub(before.ru_majflt),"rows":result.as_ref().ok().map(|p|p.results.len()),"error":result.as_ref().err().map(ToString::to_string)})
                );
                match result {
                    Ok(_) => successful += 1,
                    Err(SearchError::TimedOut) => timed_out += 1,
                    Err(error) => panic!("Unexpected advisory experiment error: {error}"),
                }
            }
        }
        eprintln!(
            "{}",
            json!({"experimentCompleted":true,"samples":successful+timed_out,"successes":successful,"timeouts":timed_out,"allQueriesSucceeded":timed_out==0,"coldCacheProven":false,"loadAfter":std::fs::read_to_string("/proc/loadavg").ok()})
        );
        assert_eq!(successful + timed_out, 12);
        // Keep the entire test-owned directory alive until every fd/request has
        // closed; no path supplied by a user can enter the advisory routine.
        drop(library);
        drop(dir);
    }
}

#[test]
#[ignore = "advisory per-file cache experiment; coordinate a quiet window and run explicitly"]
fn advisory_file_cache_10k() {
    #[cfg(target_os = "linux")]
    linux::run();
    #[cfg(not(target_os = "linux"))]
    eprintln!("No supported posix_fadvise experiment on this platform; use the documented first-connection sample without claiming OS-cache coldness.");
}
