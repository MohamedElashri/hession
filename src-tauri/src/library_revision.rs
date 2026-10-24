use rusqlite::{Connection, OptionalExtension};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

pub(crate) const DATA_TABLES: &[&str] = &[
    "agent",
    "workspace",
    "repository",
    "model",
    "session",
    "entry",
    "content_part",
    "session_context",
    "tool_invocation",
    "file_activity",
    "session_annotation",
    "session_tag",
];
pub(crate) const SETTINGS_TABLES: &[&str] = &[
    "model_price_overrides",
    "model_pricing_cache",
    "sync_state",
    "sync_checkpoint",
];

fn triggers() -> &'static BTreeMap<String, String> {
    static TRIGGERS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    TRIGGERS.get_or_init(|| {
        let mut triggers = BTreeMap::new();
        for table in DATA_TABLES.iter().chain(SETTINGS_TABLES) {
            for action in ["INSERT", "UPDATE", "DELETE"] {
                let name = format!("library_revision_{table}_{}", action.to_lowercase());
                triggers.insert(name.clone(), format!("CREATE TRIGGER \"{name}\" AFTER {action} ON \"{table}\" BEGIN UPDATE library_revision SET revision=revision+1 WHERE id=1; END"));
            }
        }
        triggers
    })
}

pub(crate) fn install(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("SAVEPOINT install_library_revision;
        CREATE TABLE IF NOT EXISTS library_revision(id INTEGER PRIMARY KEY CHECK(id=1), identity TEXT NOT NULL, revision INTEGER NOT NULL);
        INSERT OR IGNORE INTO library_revision VALUES(1,lower(hex(randomblob(16))),0);")?;
    let result = (|| {
        for statement in triggers().values() {
            conn.execute_batch(&statement.replacen(
                "CREATE TRIGGER",
                "CREATE TRIGGER IF NOT EXISTS",
                1,
            ))?;
        }
        conn.execute_batch("RELEASE install_library_revision")
    })();
    if result.is_err() {
        let _ = conn.execute_batch(
            "ROLLBACK TO install_library_revision; RELEASE install_library_revision",
        );
    }
    result
}

pub(crate) fn mark_managed(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TEMP TABLE library_revision_trusted(id INTEGER)")
}

pub(crate) fn managed_token(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Option<String>> {
    let trusted: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM temp.sqlite_master WHERE name='library_revision_trusted')",
        [],
        |row| row.get(0),
    )?;
    if !trusted {
        return Ok(None);
    }
    snapshot_token(conn, ids)
}

// Only active app connections and fresh copies produced by SQLite's backup API
// use revision tokens. Incoming backups always retain their full content hash.
pub(crate) fn snapshot_token(conn: &Connection, ids: &[i64]) -> rusqlite::Result<Option<String>> {
    let mut statement = conn.prepare_cached("SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND name GLOB 'library_revision_*' ORDER BY name")?;
    let actual = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    if &actual != triggers() {
        return Ok(None);
    }
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT identity,revision FROM library_revision WHERE id=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    ids.hash(&mut hash);
    let mut schema =
        conn.prepare_cached("SELECT name,sql FROM sqlite_schema WHERE type='table' ORDER BY name")?;
    let mut rows = schema.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(0)?;
        if DATA_TABLES.contains(&name.as_str()) || SETTINGS_TABLES.contains(&name.as_str()) {
            name.hash(&mut hash);
            row.get::<_, String>(1)?.hash(&mut hash);
        }
    }
    Ok(row.map(|(identity, revision)| format!("r1:{identity}:{revision}:{:016x}", hash.finish())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revisions_track_edits_rollbacks_and_selection_without_reading_payloads() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        assert_eq!(managed_token(&conn, &[]).unwrap(), None);
        mark_managed(&conn).unwrap();
        let original = managed_token(&conn, &[])
            .unwrap()
            .expect("managed fast path");
        assert!(original.starts_with("r1:"));
        conn.execute(
            "INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex')",
            [],
        )
        .unwrap();
        let inserted = managed_token(&conn, &[]).unwrap().unwrap();
        assert_ne!(original, inserted);
        let tx = conn.transaction().unwrap();
        tx.execute("UPDATE agent SET product='Changed' WHERE id=1", [])
            .unwrap();
        assert_ne!(managed_token(&tx, &[]).unwrap().unwrap(), inserted);
        tx.rollback().unwrap();
        assert_eq!(managed_token(&conn, &[]).unwrap().unwrap(), inserted);
        assert_ne!(
            managed_token(&conn, &[1]).unwrap(),
            managed_token(&conn, &[2]).unwrap()
        );
        conn.execute_batch("ALTER TABLE entry ADD COLUMN future_metadata TEXT")
            .unwrap();
        assert_ne!(managed_token(&conn, &[]).unwrap().unwrap(), inserted);
        conn.execute("DELETE FROM agent", []).unwrap();
        assert_ne!(managed_token(&conn, &[]).unwrap().unwrap(), inserted);
        conn.execute_batch("DROP TRIGGER library_revision_entry_update")
            .unwrap();
        assert_eq!(managed_token(&conn, &[]).unwrap(), None);
    }

    #[test]
    fn independent_writers_and_backup_copies_keep_snapshot_revisions_consistent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("library.sqlite");
        let mut conn = crate::db::open_db(&path).unwrap();
        mark_managed(&conn).unwrap();
        let original = managed_token(&conn, &[]).unwrap().unwrap();
        let tx = conn.transaction().unwrap();
        assert_eq!(managed_token(&tx, &[]).unwrap().unwrap(), original);
        let other = Connection::open(&path).unwrap();
        other
            .execute(
                "INSERT INTO agent(id,family,product) VALUES(1,'codex','Codex')",
                [],
            )
            .unwrap();
        assert_eq!(managed_token(&tx, &[]).unwrap().unwrap(), original);
        tx.commit().unwrap();
        let changed = managed_token(&conn, &[]).unwrap().unwrap();
        assert_ne!(original, changed);
        let backup_path = directory.path().join("backup.sqlite");
        conn.backup(rusqlite::MAIN_DB, &backup_path, None).unwrap();
        let backup = Connection::open(&backup_path).unwrap();
        assert_eq!(snapshot_token(&backup, &[]).unwrap().unwrap(), changed);
        assert_eq!(managed_token(&backup, &[]).unwrap(), None);
        other
            .execute("UPDATE agent SET product='Changed'", [])
            .unwrap();
        assert_ne!(managed_token(&conn, &[]).unwrap().unwrap(), changed);
    }
}
