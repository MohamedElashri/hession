//! Regenerable per-session read tokens. No transcript text participates in a
//! token, and unrelated session writes do not invalidate a reader or sync cache.
use rusqlite::{Connection, OptionalExtension};
use std::{
    collections::BTreeMap,
    sync::{Mutex, OnceLock},
};

fn touch(session: &str, condition: &str) -> String {
    format!("INSERT INTO index_read_revision(session_id,nonce,generation)
        SELECT {session},lower(hex(randomblob(16))),1 WHERE {condition}
        ON CONFLICT(session_id) DO UPDATE SET
        nonce=CASE WHEN generation=9223372036854775807 OR octet_length(nonce)<>32 THEN excluded.nonce ELSE nonce END,
        generation=CASE WHEN generation=9223372036854775807 THEN 0 ELSE generation+1 END;")
}

fn triggers() -> &'static BTreeMap<String, String> {
    static TRIGGERS: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    TRIGGERS.get_or_init(|| {
        let mut result=BTreeMap::new();
        for table in ["session","entry","content_part","session_context","tool_invocation","file_activity","session_annotation","session_tag","agent","workspace","model","repository"] {
            for action in ["INSERT","UPDATE","DELETE"] {
                let name=format!("index_read_{table}_{}",action.to_lowercase());
                let body=if ["agent","workspace","model","repository"].contains(&table) {
                    touch("0","1")
                } else if table=="session" && action=="DELETE" {
                    "DELETE FROM index_read_revision WHERE session_id=OLD.id;".into()
                } else if table=="session" && action=="INSERT" {
                    "INSERT OR REPLACE INTO index_read_revision VALUES(NEW.id,lower(hex(randomblob(16))),0);".into()
                } else {
                    let row=if action=="DELETE" {"OLD"} else {"NEW"};
                    let session=|row:&str| if table=="session" {format!("{row}.id")} else if table=="content_part" {
                        format!("(SELECT session_id FROM entry WHERE id={row}.entry_id)")
                    } else {format!("{row}.session_id")};
                    let id=session(row);
                    let mut body=touch(&id,&format!("EXISTS(SELECT 1 FROM session WHERE id={id})"));
                    if action=="UPDATE" {
                        let old=session("OLD"); let new=session("NEW");
                        if table=="session" {body.push_str("DELETE FROM index_read_revision WHERE session_id=OLD.id AND OLD.id<>NEW.id;");}
                        else {body.push_str(&touch(&old,&format!("{old} IS NOT {new} AND EXISTS(SELECT 1 FROM session WHERE id={old})")));}
                    }
                    body
                };
                result.insert(name.clone(),format!("CREATE TRIGGER {name} AFTER {action} ON {table} BEGIN {body} END"));
            }
        }
        result
    })
}

pub(crate) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS index_read_revision(session_id INTEGER PRIMARY KEY,nonce TEXT NOT NULL,generation INTEGER NOT NULL CHECK(typeof(generation)='integer' AND generation>=0));
        INSERT OR IGNORE INTO index_read_revision SELECT id,lower(hex(randomblob(16))),0 FROM session;")?;
    for sql in triggers().values() {
        conn.execute_batch(&sql.replacen("CREATE TRIGGER", "CREATE TRIGGER IF NOT EXISTS", 1))?;
    }
    Ok(())
}

fn trusted(conn: &Connection, identity: &str, cookie: i64) -> rusqlite::Result<bool> {
    type Cache = BTreeMap<(String, String, i64), bool>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    let key = (
        conn.path().unwrap_or(":memory:").to_owned(),
        identity.to_owned(),
        cookie,
    );
    if let Some(value) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Ok(*value);
    }
    let mut statement=conn.prepare("SELECT name,sql FROM sqlite_schema WHERE type='trigger' AND name GLOB 'index_read_*' ORDER BY name")?;
    let actual = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    let valid = &actual == triggers();
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= 128 {
        cache.clear();
    }
    cache.insert(key, valid);
    Ok(valid)
}

/// Read within the caller's WAL snapshot. Missing/corrupt derived state or
/// changed trigger definitions disable reuse; the caller reads actual history.
pub fn token(conn: &Connection, id: i64) -> Option<String> {
    (||->rusqlite::Result<Option<String>> {
        let identity:Option<String>=conn.query_row("SELECT CASE WHEN octet_length(identity)<=128 THEN identity END FROM library_revision WHERE id=1",[],|row|row.get(0))?;
        let Some(identity)=identity else {return Ok(None)};
        let cookie:i64=conn.query_row("PRAGMA schema_version",[],|row|row.get(0))?;
        if !trusted(conn,&identity,cookie)? {return Ok(None);}
        type State=(i64,Option<String>,Option<i64>,Option<String>,Option<i64>);
        let state:Option<State>=conn.query_row("SELECT m.epoch,r.nonce,r.generation,g.nonce,g.generation FROM session s CROSS JOIN index_maintenance m
            LEFT JOIN index_read_revision r ON r.session_id=s.id LEFT JOIN index_read_revision g ON g.session_id=0 WHERE s.id=? AND m.id=1
            AND (r.session_id IS NULL OR (octet_length(r.nonce)=32 AND typeof(r.generation)='integer' AND r.generation>=0))
            AND (g.session_id IS NULL OR (octet_length(g.nonce)=32 AND typeof(g.generation)='integer' AND g.generation>=0))",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
        let Some((epoch,nonce,generation,global_nonce,global))=state else {return Ok(None)};
        if nonce.as_ref().is_some_and(|value|value.len()!=32)||global_nonce.as_ref().is_some_and(|value|value.len()!=32) {return Ok(None);}
        Ok(Some(format!("s1:{identity}:{cookie}:{epoch}:{id}:{}:{}:{}:{}",nonce.unwrap_or_default(),generation.unwrap_or(0),global_nonce.unwrap_or_default(),global.unwrap_or(0))))
    })().ok().flatten()
}
