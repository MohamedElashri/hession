//! Match Codex's separate name stores to imported rollouts by thread ID.
use super::*;
use hession_core::{jsonl::Fingerprint, source};
use rusqlite::OpenFlags;
use std::cell::RefCell;
use std::collections::HashMap;

const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_NAME_BYTES: usize = 4096;
pub(crate) const INDEX_FILE: &str = "session_index.jsonl";
const STATE_FILE: &str = "state_5.sqlite";
const STATE_WAL: &str = "state_5.sqlite-wal";
type Names = Arc<HashMap<String, String>>;
type Stamp = [Option<Fingerprint>; 3];

thread_local! {
    static CACHE: RefCell<HashMap<PathBuf, (Stamp, Names)>> = RefCell::new(HashMap::new());
}

pub(super) fn rollout_root(file: &Path) -> Option<&Path> {
    file.ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "sessions"))?
        .parent()
}

fn names(root: &Path) -> Result<Names, String> {
    let paths = [
        root.join(INDEX_FILE),
        root.join(STATE_FILE),
        root.join(STATE_WAL),
    ];
    let [index, state, wal] = paths.each_ref().map(|path| match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(Some(Fingerprint::of(&metadata))),
        Ok(_) => Err("Codex name store is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    });
    let stamp = [index?, state?, wal?];
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let previous = cache.get(root);
        if let Some((previous_stamp, names)) = previous {
            if previous_stamp == &stamp {
                return Ok(names.clone());
            }
        }
        let mut names = HashMap::new();
        if stamp[0].is_some() {
            let contents = source::read_bounded(&paths[0], MAX_INDEX_BYTES, &stop_requested)?;
            for line in contents.split_inclusive(|byte| *byte == b'\n') {
                if stop_requested() {
                    return Err("Import stopped".into());
                }
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let row: Value = match serde_json::from_slice(line) {
                    Ok(row) => row,
                    Err(_) if !line.ends_with(b"\n") => break,
                    Err(_) => return Err("Invalid Codex session name index record".into()),
                };
                let Some(id) = row["id"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 128)
                else {
                    continue;
                };
                if let Some(name) =
                    title_text(&row["thread_name"]).filter(|name| name.len() <= MAX_NAME_BYTES)
                {
                    names.insert(id.to_owned(), name.to_owned());
                } else {
                    names.remove(id);
                }
            }
        }
        if stamp[1].is_some() {
            read_state_names(&paths[1], &mut names)?;
        }
        // Activity also updates the state database. Reuse the same map when
        // the metadata changed but the actual names stayed the same.
        let names = if let Some((_, previous_names)) =
            previous.filter(|(_, previous_names)| previous_names.as_ref() == &names)
        {
            previous_names.clone()
        } else {
            Arc::new(names)
        };
        if cache.len() >= 8 && !cache.contains_key(root) {
            cache.clear();
        }
        cache.insert(root.to_path_buf(), (stamp, names.clone()));
        Ok(names)
    })
}

fn read_state_names(path: &Path, names: &mut HashMap<String, String>) -> Result<(), String> {
    let mut conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    formats::configure_source(&mut conn)?;
    let has_names = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('threads') WHERE name='name')",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    if !has_names {
        return Ok(());
    }
    // The state DB is authoritative for newer threads. Older state DBs lack
    // this column and expose a prompt-derived title, so use the name index.
    let mut stmt = conn
        .prepare(
            "SELECT id,name FROM threads WHERE typeof(id)='text' AND octet_length(id)<=128
         AND typeof(name)='text' AND octet_length(name) BETWEEN 1 AND ?1 LIMIT ?2",
        )
        .map_err(|error| error.to_string())?;
    let mut rows = stmt
        .query(params![MAX_NAME_BYTES as i64, MAX_FILES as i64 + 1])
        .map_err(|error| error.to_string())?;
    let mut count = 0;
    let mut remaining = MAX_INDEX_BYTES as usize;
    while let Some(row) = rows.next().map_err(|error| error.to_string())? {
        if stop_requested() {
            return Err("Import stopped".into());
        }
        count += 1;
        let id: String = row.get(0).map_err(|error| error.to_string())?;
        let name: String = row.get(1).map_err(|error| error.to_string())?;
        remaining = remaining
            .checked_sub(id.len() + name.len())
            .filter(|_| count <= MAX_FILES)
            .ok_or("Codex session name store exceeds the import limit")?;
        if !name.trim().is_empty() {
            names.insert(id, name.trim().to_owned());
        }
    }
    Ok(())
}

pub(super) fn title(
    conn: &Connection,
    file: &Path,
    source_uri: &str,
    metadata: &Value,
) -> Result<Option<String>, String> {
    let (Some(root), Some(id)) = (rollout_root(file), metadata["id"].as_str()) else {
        return Ok(None);
    };
    match names(root) {
        Ok(names) => Ok(names.get(id).cloned()),
        // The transcript is usable while optional metadata is unavailable.
        Err(_) => conn
            .query_row(
                "SELECT title FROM session WHERE source_uri=?",
                [source_uri],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(|error| error.to_string()),
    }
}

/// Apply title-only revisions without rebuilding transcript entries or their
/// checkpoints. Bounds on the source URI keep equally named thread IDs from
/// different Codex stores or SSH hosts separate.
pub(crate) fn reconcile(
    conn: &mut Connection,
    root: &Path,
    epoch: i64,
    applied: &mut Option<Names>,
) -> Result<Vec<i64>, String> {
    if !root.join("sessions").is_dir() {
        return Ok(Vec::new());
    }
    let names = names(root)?;
    if applied
        .as_ref()
        .is_some_and(|previous| Arc::ptr_eq(previous, &names))
    {
        return Ok(Vec::new());
    }
    let sessions = native_source_uri("codex", &root.join("sessions"))?;
    let changes = {
        let mut stmt = conn
            .prepare(
                "SELECT id,external_id,title,source_uri FROM session
             WHERE source_uri>=?1 AND source_uri<?2 LIMIT ?3",
            )
            .map_err(|error| error.to_string())?;
        let mut rows = stmt
            .query(params![
                format!("{sessions}/"),
                format!("{sessions}0"),
                MAX_FILES as i64 + 1
            ])
            .map_err(|error| error.to_string())?;
        let mut fallback = conn
            .prepare_cached(
                "SELECT CASE WHEN json_valid(summary_json) THEN
                 COALESCE(NULLIF(trim(json_extract(summary_json,'$.metadata.title')),''),
                          json_extract(summary_json,'$.title')) END
             FROM index_checkpoint WHERE source_uri=? AND invalidated=0",
            )
            .map_err(|error| error.to_string())?;
        let mut changes = Vec::new();
        let mut count = 0;
        while let Some(row) = rows.next().map_err(|error| error.to_string())? {
            count += 1;
            if stop_requested() || count > MAX_FILES {
                return Err(
                    "Codex name reconciliation stopped or exceeded the source limit".into(),
                );
            }
            let id: i64 = row.get(0).map_err(|error| error.to_string())?;
            let external: Option<String> = row.get(1).map_err(|error| error.to_string())?;
            let current: Option<String> = row.get(2).map_err(|error| error.to_string())?;
            let uri: String = row.get(3).map_err(|error| error.to_string())?;
            let title = match external.as_ref().and_then(|id| names.get(id)) {
                Some(title) => Some(title.clone()),
                None => match fallback
                    .query_row([&uri], |row| row.get(0))
                    .optional()
                    .map_err(|error| error.to_string())?
                {
                    Some(fallback) => fallback,
                    None => continue,
                },
            };
            if current != title {
                changes.push(SessionTitleUpdate {
                    id,
                    source_uri: uri,
                    external_id: external,
                    previous_title: current,
                    title,
                });
            }
        }
        changes
    };
    let changed = commit_session_title_updates(conn, epoch, &mut ImportPricing::new(), changes)?;
    *applied = Some(names);
    Ok(changed)
}
