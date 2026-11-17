//! Immutable, owner-configured authorization for CLI and read-only MCP tools.
//! Every operation validates library identity and workspace/path bindings in the
//! same read snapshot used to fetch content. Reused IDs cannot broaden access.
use crate::{
    db,
    library_revision::{self, LibraryRevision},
    projects::{self, Project, ProjectError, ProjectSelector},
    search::{
        self, SearchPage, SearchRequest, SearchScope, SearchVisibility, VISIBLE_CONVERSATION_KINDS,
    },
    service::{Library, Page},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

pub const MAX_READ_LIMIT: u32 = 100;
pub const MAX_READ_BYTES: usize = 1024 * 1024;
const MAX_TEXT_CHARS: usize = 2048;
const MAX_CURSOR_BYTES: usize = 4096;
#[derive(Debug, thiserror::Error)]
pub enum ReadError {
    #[error("{0}")]
    Database(#[from] db::DbError),
    #[error("{0}")]
    Sql(#[from] rusqlite::Error),
    #[error("{0}")]
    Search(#[from] search::SearchError),
    #[error("{0}")]
    Project(#[from] ProjectError),
    #[error("Session or project is unavailable or outside the configured scope")]
    OutsideScope,
    #[error("Configured library or workspace identity changed; restart with a reviewed project allowlist")]
    ScopeChanged,
    #[error("Invalid read request: {0}")]
    Invalid(String),
    #[error("Cursor expired; restart from the first page")]
    StaleCursor,
    #[error("Cursor does not match this request or permission scope")]
    InvalidCursor,
    #[error("Read exceeds safe cell or metadata byte limits; narrow the project scope or repair oversized indexed fields")]
    DataTooLarge,
    #[error("Response exceeds 1 MiB; request a smaller page")]
    ResponseTooLarge,
    #[error("Read cancelled or exceeded its three-second execution budget")]
    Interrupted,
}
#[derive(Clone, Default, Debug)]
pub struct ReadCancellation(Arc<AtomicBool>);
impl ReadCancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct ReadPage {
    pub limit: u32,
    pub cursor: Option<String>,
}
impl Default for ReadPage {
    fn default() -> Self {
        Self {
            limit: 20,
            cursor: None,
        }
    }
}
impl ReadPage {
    fn validate(&self) -> Result<(), ReadError> {
        if self.limit == 0 || self.limit > MAX_READ_LIMIT {
            return Err(ReadError::Invalid("Limit must be 1–100".into()));
        }
        if self
            .cursor
            .as_ref()
            .is_some_and(|c| c.len() > MAX_CURSOR_BYTES)
        {
            return Err(ReadError::InvalidCursor);
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct ScopedReader {
    library: Library,
    scope: SearchScope,
    identity: String,
    bindings: BTreeMap<i64, String>,
    expose_conversation: bool,
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    revision: LibraryRevision,
    fingerprint: String,
    offset: u64,
}
impl ScopedReader {
    /// MCP uses this constructor exclusively. No empty/all-project fallback.
    pub fn projects(
        library: Library,
        selectors: &[ProjectSelector],
        expose_conversation: bool,
    ) -> Result<Self, ReadError> {
        if selectors.is_empty() || selectors.len() > search::MAX_SCOPE_WORKSPACES {
            return Err(ReadError::Invalid(
                "Choose 1–512 explicit indexed projects".into(),
            ));
        }
        let mut conn = db::open_read_only(library.path())?;
        let deadline = Instant::now() + Duration::from_secs(3);
        conn.progress_handler(1000, Some(move || Instant::now() > deadline))?;
        let tx = conn.transaction()?;
        let identity = library_revision::current(&tx)?.identity;
        let mut bindings = BTreeMap::new();
        for selector in selectors {
            let (sql, p): (String, rusqlite::types::Value) = match selector {
                ProjectSelector::WorkspaceId(id) if *id > 0 => (
                    "SELECT id,CASE WHEN octet_length(path)<=4096 THEN path END FROM workspace WHERE id=? AND octet_length(path)<=4096".into(),
                    (*id).into(),
                ),
                ProjectSelector::RecordedPath(path) => {
                    validate_path(path)?;
                    (
                        "SELECT id,path FROM workspace WHERE CASE WHEN octet_length(path)<=4096 THEN path END=?".into(),
                        path.clone().into(),
                    )
                }
                _ => return Err(ReadError::OutsideScope),
            };
            let row: Option<(i64, String)> = tx
                .query_row(&sql, [p], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let (id, path) = row.ok_or(ReadError::OutsideScope)?;
            validate_reported_path(&path)?;
            bindings.insert(id, path);
        }
        let scope = SearchScope::workspaces(bindings.keys().copied())?;
        Ok(Self {
            library,
            scope,
            identity,
            bindings,
            expose_conversation,
        })
    }
    /// Explicit local CLI policy. Never expose this option as an MCP argument.
    pub fn unrestricted(library: Library, expose_conversation: bool) -> Result<Self, ReadError> {
        let conn = db::open_read_only(library.path())?;
        let identity = library_revision::current(&conn)?.identity;
        Ok(Self {
            library,
            scope: SearchScope::all(),
            identity,
            bindings: BTreeMap::new(),
            expose_conversation,
        })
    }
    pub fn library(&self) -> &Library {
        &self.library
    }
    pub fn scope(&self) -> &SearchScope {
        &self.scope
    }
    pub fn exposes_conversation(&self) -> bool {
        self.expose_conversation
    }
    fn ids(&self) -> Result<String, ReadError> {
        serde_json::to_string(&self.scope.workspace_ids())
            .map_err(|e| ReadError::Invalid(e.to_string()))
    }
    fn snapshot<T: Serialize>(
        &self,
        cancel: &ReadCancellation,
        read: impl FnOnce(&Connection, &LibraryRevision) -> Result<T, ReadError>,
    ) -> Result<T, ReadError> {
        if cancel.is_cancelled() {
            return Err(ReadError::Interrupted);
        }
        let mut conn = db::open_read_only(self.library.path())?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let stop = cancel.clone();
        conn.progress_handler(
            1000,
            Some(move || stop.is_cancelled() || Instant::now() > deadline),
        )?;
        let tx = conn.transaction()?;
        let revision = library_revision::current(&tx)?;
        if revision.identity != self.identity {
            return Err(ReadError::ScopeChanged);
        }
        if !self.bindings.is_empty() {
            let encoded = serde_json::to_string(
                &self
                    .bindings
                    .iter()
                    .map(|(id, path)| (*id, path))
                    .collect::<Vec<_>>(),
            )
            .map_err(|e| ReadError::Invalid(e.to_string()))?;
            let found:i64=tx.query_row("SELECT count(*) FROM workspace w JOIN json_each(?) b ON w.id=json_extract(b.value,'$[0]') AND CASE WHEN octet_length(w.path)<=4096 THEN w.path END=json_extract(b.value,'$[1]')",[encoded],|r|r.get(0))?;
            if found != self.bindings.len() as i64 {
                return Err(ReadError::ScopeChanged);
            }
        }
        let value = read(&tx, &revision).map_err(|error| {
            if is_interrupted(&error) {
                ReadError::Interrupted
            } else {
                error
            }
        })?;
        if cancel.is_cancelled() {
            return Err(ReadError::Interrupted);
        }
        if serde_json::to_vec(&value)
            .map_err(|_| ReadError::ResponseTooLarge)?
            .len()
            > MAX_READ_BYTES
        {
            return Err(ReadError::ResponseTooLarge);
        }
        tx.commit()?;
        Ok(value)
    }
    pub fn search(
        &self,
        request: &SearchRequest,
        cancel: &ReadCancellation,
    ) -> Result<SearchPage, ReadError> {
        ReadPage {
            limit: request.limit,
            cursor: request.cursor.clone(),
        }
        .validate()?;
        let parsed = search::query::parse(&request.query).map_err(search::SearchError::from)?;
        let has_text = parsed
            .alternatives
            .iter()
            .flatten()
            .any(|term| term.field.is_none());
        let mut request = request.clone();
        request.visibility = if self.expose_conversation {
            SearchVisibility::Conversation
        } else {
            SearchVisibility::MetadataOnly
        };
        self.snapshot(cancel, |tx, _| {
            metadata_budget(tx, &self.ids()?)?;
            if self.expose_conversation && has_text {
                visible_text_budget(tx, &self.ids()?)?;
            }
            Ok(search::search_snapshot(tx, &request, &self.scope)?)
        })
    }
    /// Explicit local CLI indexed-text policy; MCP must only call `search`.
    pub fn search_indexed(
        &self,
        request: &SearchRequest,
        cancel: &ReadCancellation,
    ) -> Result<SearchPage, ReadError> {
        ReadPage {
            limit: request.limit,
            cursor: request.cursor.clone(),
        }
        .validate()?;
        let parsed = search::query::parse(&request.query).map_err(search::SearchError::from)?;
        let has_text = parsed
            .alternatives
            .iter()
            .flatten()
            .any(|term| term.field.is_none());
        let mut request = request.clone();
        request.visibility = SearchVisibility::AllIndexedText;
        self.snapshot(cancel, |tx, _| {
            metadata_budget(tx, &self.ids()?)?;
            if has_text {
                indexed_text_budget(tx, &self.ids()?)?;
            }
            Ok(search::search_snapshot(tx, &request, &self.scope)?)
        })
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn status(&self, cancel: &ReadCancellation) -> Result<Value, ReadError> {
        self.snapshot(cancel,|tx,revision|{
        let ids=self.ids()?;
        let sessions:i64=tx.query_row("SELECT count(*) FROM session s WHERE (?1='null' OR s.workspace_id IN(SELECT value FROM json_each(?1)))",[&ids],|r|r.get(0))?;
        let entries:i64=tx.query_row("SELECT count(*) FROM entry e JOIN session s ON s.id=e.session_id WHERE (?1='null' OR s.workspace_id IN(SELECT value FROM json_each(?1)))",[&ids],|r|r.get(0))?;
        let projects:i64=tx.query_row("SELECT count(*) FROM workspace w WHERE (?1='null' OR w.id IN(SELECT value FROM json_each(?1)))",[&ids],|r|r.get(0))?;
        let delta:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='index_change')",[],|r|r.get(0))?;
        Ok(json!({"revision":revision,"schemaVersion":db::schema_version(tx)?,"sessions":sessions,"entries":entries,"projects":projects,"durableDeltas":delta,"observerStatus":"not observable from this read-only process"}))
    })
    }
    pub fn session(
        &self,
        id: i64,
        page: &ReadPage,
        cancel: &ReadCancellation,
    ) -> Result<Value, ReadError> {
        positive(id)?;
        page.validate()?;
        self.snapshot(cancel,|tx,revision|{
            let ids=self.ids()?;
            let session=metadata(tx,id,&ids)?;
            if !self.expose_conversation{return Ok(json!({"session":session,"revision":revision,"entries":[],"nextCursor":null,"accessLimited":true,"notice":"Transcript access is disabled by the server owner."}));}
            let fingerprint=self.fingerprint("session",&(id,page.limit))?;
            let offset=decode_cursor(page,&fingerprint,revision)?;
            let kinds=visible_kinds();
            let sql=format!("SELECT e.id,e.ordinal,e.role FROM entry e JOIN session s ON s.id=e.session_id WHERE s.id=?1 AND (?2='null' OR s.workspace_id IN(SELECT value FROM json_each(?2))) AND CASE WHEN octet_length(e.role)<=32 THEN e.role END IN('user','assistant') AND CASE WHEN octet_length(e.kind)<=128 THEN e.kind END IN({kinds}) AND EXISTS(SELECT 1 FROM content_part p WHERE p.entry_id=e.id AND CASE WHEN octet_length(p.kind)<=128 THEN p.kind END='text' AND p.text IS NOT NULL) ORDER BY e.ordinal,e.id LIMIT ?3 OFFSET ?4");
            let mut stmt=tx.prepare(&sql)?;
            let rows=stmt.query_map(params![id,ids,i64::from(page.limit)+1,offset as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let more=rows.len()>page.limit as usize;let mut entries=Vec::new();
            for (entry_id,ordinal,role) in rows.into_iter().take(page.limit as usize){
                let sql=format!("SELECT p.id FROM content_part p JOIN entry e ON e.id=p.entry_id JOIN session s ON s.id=e.session_id WHERE e.id=?1 AND s.id=?2 AND (?3='null' OR s.workspace_id IN(SELECT value FROM json_each(?3))) AND CASE WHEN octet_length(e.role)<=32 THEN e.role END IN('user','assistant') AND CASE WHEN octet_length(e.kind)<=128 THEN e.kind END IN({kinds}) AND CASE WHEN octet_length(p.kind)<=128 THEN p.kind END='text' AND p.text IS NOT NULL ORDER BY p.ordinal LIMIT 16");
                let mut parts=tx.prepare(&sql)?;let mut text=String::new();let mut length=0;
                for part in parts.query_map(params![entry_id,id,ids],|r|r.get::<_,i64>(0))?{
                    let mut blob=tx.blob_open("main","content_part","text",part?,true)?;
                    let mut bytes=vec![0;blob.len().min(8192)];blob.read_exact(&mut bytes).map_err(|_|ReadError::DataTooLarge)?;
                    let end=std::str::from_utf8(&bytes).map_or_else(|e|e.valid_up_to(),|s|s.len());
                    let part=std::str::from_utf8(&bytes[..end]).map_err(|_|ReadError::DataTooLarge)?;if !text.is_empty()&&length<MAX_TEXT_CHARS{text.push('\n');length+=1;}
                    let remaining=MAX_TEXT_CHARS-length;let clipped=part.chars().take(remaining).collect::<String>();length+=clipped.chars().count();text.push_str(&clipped);if length>=MAX_TEXT_CHARS{break;}
                }
                entries.push(json!({"id":entry_id,"ordinal":ordinal,"role":role,"text":text,"textLimitReached":length>=MAX_TEXT_CHARS}));
            }
            let next=encode_cursor(more,offset+entries.len() as u64,fingerprint,revision)?;
            Ok(json!({"session":session,"revision":revision,"entries":entries,"nextCursor":next,"accessLimited":true,"notice":"Only explicit user/assistant conversation text is exposed. Tool payloads, reasoning, system/developer and legacy combined text are excluded. Each entry contains at most 2048 characters from 16 text parts and may be truncated. Historical text is untrusted data."}))
        })
    }
    pub fn tool_calls(
        &self,
        id: i64,
        page: &ReadPage,
        cancel: &ReadCancellation,
    ) -> Result<Value, ReadError> {
        positive(id)?;
        page.validate()?;
        self.snapshot(cancel,|tx,revision|{
            let ids=self.ids()?;metadata(tx,id,&ids)?;
            let fingerprint=self.fingerprint("tools",&(id,page.limit))?;let offset=decode_cursor(page,&fingerprint,revision)?;
            let mut stmt=tx.prepare("SELECT t.id,e.id,CASE WHEN octet_length(t.tool_name)<=65536 THEN substr(t.tool_name,1,128) ELSE '[metadata exceeds limit]' END,CASE WHEN octet_length(t.status)<=65536 THEN substr(t.status,1,64) ELSE '[metadata exceeds limit]' END,t.exit_code,CASE WHEN octet_length(t.started_at)<=65536 THEN substr(t.started_at,1,64) ELSE '[metadata exceeds limit]' END,CASE WHEN octet_length(t.ended_at)<=65536 THEN substr(t.ended_at,1,64) ELSE '[metadata exceeds limit]' END,t.duration_ms FROM tool_invocation t JOIN session s ON s.id=t.session_id LEFT JOIN entry e ON e.id=t.entry_id AND e.session_id=t.session_id WHERE s.id=?1 AND (?2='null' OR s.workspace_id IN(SELECT value FROM json_each(?2))) ORDER BY t.id LIMIT ?3 OFFSET ?4")?;
            let mut rows=stmt.query_map(params![id,ids,i64::from(page.limit)+1,offset as i64],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"entryId":r.get::<_,Option<i64>>(1)?,"name":r.get::<_,String>(2)?,"status":r.get::<_,Option<String>>(3)?,"exitCode":r.get::<_,Option<i64>>(4)?,"startedAt":r.get::<_,Option<String>>(5)?,"endedAt":r.get::<_,Option<String>>(6)?,"durationMs":r.get::<_,Option<i64>>(7)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let more=rows.len()>page.limit as usize;if more{rows.pop();}let next=encode_cursor(more,offset+rows.len() as u64,fingerprint,revision)?;
            Ok(json!({"sessionId":id,"revision":revision,"tools":rows,"nextCursor":next,"accessLimited":true,"notice":"Tool names and recorded status only; raw inputs, commands, outputs and payloads are excluded."}))
        })
    }
    pub fn projects_page(
        &self,
        page: &ReadPage,
        cancel: &ReadCancellation,
    ) -> Result<Value, ReadError> {
        page.validate()?;
        self.snapshot(cancel,|tx,revision|{
            metadata_budget(tx,&self.ids()?)?;
            let fingerprint=self.fingerprint("projects",&page.limit)?;let offset=decode_cursor(page,&fingerprint,revision)?;
            let ids=self.ids()?;let mut stmt=tx.prepare("SELECT w.id FROM workspace w WHERE (?1='null' OR w.id IN(SELECT value FROM json_each(?1))) ORDER BY w.id LIMIT ?2 OFFSET ?3")?;
            let rows=stmt.query_map(params![ids,i64::from(page.limit)+1,offset as i64],|r|r.get::<_,i64>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;let more=rows.len()>page.limit as usize;
            let projects=rows.into_iter().take(page.limit as usize).map(|id|projects::project_on_connection(tx,id,&self.scope).map_err(ReadError::from)).collect::<Result<Vec<_>,_>>()?;
            let next=encode_cursor(more,offset+projects.len() as u64,fingerprint,revision)?;
            Ok(json!({"revision":revision,"projects":projects,"nextCursor":next}))
        })
    }
    pub fn project(&self, id: i64, cancel: &ReadCancellation) -> Result<Project, ReadError> {
        positive(id)?;
        self.snapshot(cancel, |tx, _| {
            if !self.scope.allows_workspace(id) {
                return Err(ReadError::OutsideScope);
            }
            metadata_budget(
                tx,
                &serde_json::to_string(&Some(vec![id])).map_err(|_| ReadError::DataTooLarge)?,
            )?;
            Ok(projects::project_on_connection(tx, id, &self.scope)?)
        })
    }
    pub fn project_activity(
        &self,
        id: i64,
        path: Option<&str>,
        page: &ReadPage,
        cancel: &ReadCancellation,
    ) -> Result<Value, ReadError> {
        positive(id)?;
        page.validate()?;
        if let Some(path) = path {
            validate_reported_path(path)?;
        }
        self.snapshot(cancel, |tx, revision| {
            let fingerprint = self.fingerprint("activity", &(id, path, page.limit))?;
            let offset = decode_cursor(page, &fingerprint, revision)?;
            if !self.scope.allows_workspace(id){return Err(ReadError::OutsideScope);}
            metadata_budget(tx,&serde_json::to_string(&Some(vec![id])).map_err(|_|ReadError::DataTooLarge)?)?;
            let project=projects::project_on_connection(tx,id,&self.scope)?;
            // Select the permission-safe fields in SQL. Never fetch evidence,
            // native cwd/source strings or tool bodies merely to discard them.
            let mut stmt=tx.prepare("SELECT f.id,f.session_id,e.id,e.ordinal,t.id,f.path,
                CASE WHEN f.action='read' THEN 'reported_read' WHEN f.action IN('write','edit','patch','delete','create','rename') THEN 'reported_write_intent' ELSE 'reported_reference' END,
                substr(e.occurred_at,1,128),CASE WHEN octet_length(a.family)<=65536 THEN substr(a.family,1,128) ELSE '[metadata exceeds limit]' END
                FROM file_activity f JOIN session s ON s.id=f.session_id JOIN agent a ON a.id=s.agent_id
                LEFT JOIN entry e ON e.id=f.entry_id AND e.session_id=f.session_id
                LEFT JOIN tool_invocation t ON t.id=f.tool_invocation_id AND t.session_id=f.session_id
                WHERE s.workspace_id=?1 AND (?2 IS NULL OR f.path=?2)
                ORDER BY coalesce(e.occurred_at,s.last_event_at,s.started_at) DESC,f.id DESC LIMIT ?3 OFFSET ?4")?;
            let mut evidence=stmt.query_map(params![id,path,i64::from(page.limit)+1,offset as i64],|r|Ok(json!({
                "id":r.get::<_,i64>(0)?,"source":{"session_id":r.get::<_,i64>(1)?,"entry_id":r.get::<_,Option<i64>>(2)?,"entry_ordinal":r.get::<_,Option<i64>>(3)?,"tool_invocation_id":r.get::<_,Option<i64>>(4)?},
                "reported_path":r.get::<_,String>(5)?,"candidate_workspace_relative_path":null,"category":r.get::<_,String>(6)?,"occurred_at":r.get::<_,Option<String>>(7)?,"agent_family":r.get::<_,String>(8)?
            })))?.collect::<rusqlite::Result<Vec<_>>>()?;
            let more=evidence.len()>page.limit as usize;if more{evidence.pop();}
            let next=encode_cursor(more,offset+evidence.len() as u64,fingerprint,revision)?;
            Ok(json!({"project":project,"revision":revision,"evidence":evidence,"nextCursor":next,"notice":"Permission-safe reported file metadata only; native cwd, tool bodies and free-form evidence are excluded."}))
        })
    }
    pub fn project_files(
        &self,
        id: i64,
        filter: Option<&str>,
        page: &ReadPage,
        cancel: &ReadCancellation,
    ) -> Result<Value, ReadError> {
        positive(id)?;
        page.validate()?;
        if let Some(path) = filter {
            validate_reported_path(path)?;
        }
        self.snapshot(cancel, |tx, revision| {
            let fingerprint = self.fingerprint("files", &(id, filter, page.limit))?;
            let offset = decode_cursor(page, &fingerprint, revision)?;
            if !self.scope.allows_workspace(id) {
                return Err(ReadError::OutsideScope);
            }
            metadata_budget(
                tx,
                &serde_json::to_string(&Some(vec![id])).map_err(|_| ReadError::DataTooLarge)?,
            )?;
            let result = projects::files_on_connection(
                tx,
                id,
                &self.scope,
                Page {
                    limit: page.limit,
                    offset,
                },
                filter,
            )?;
            let next = encode_cursor(
                result.has_more,
                result.next_offset.unwrap_or(offset),
                fingerprint,
                revision,
            )?;
            let mut value =
                serde_json::to_value(result).map_err(|_| ReadError::ResponseTooLarge)?;
            value["nextCursor"] = json!(next);
            Ok(value)
        })
    }
    fn fingerprint(&self, kind: &str, args: &impl Serialize) -> Result<String, ReadError> {
        let bytes = serde_json::to_vec(&(
            kind,
            args,
            &self.scope,
            &self.bindings,
            self.expose_conversation,
        ))
        .map_err(|_| ReadError::InvalidCursor)?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
}
fn positive(id: i64) -> Result<(), ReadError> {
    if id <= 0 {
        Err(ReadError::Invalid("IDs must be positive".into()))
    } else {
        Ok(())
    }
}
fn validate_reported_path(path: &str) -> Result<(), ReadError> {
    if path.is_empty() || path.len() > 4096 || path.contains('\0') {
        Err(ReadError::Invalid(
            "Use a recorded path of 1–4096 bytes without NUL".into(),
        ))
    } else {
        Ok(())
    }
}
fn validate_path(path: &str) -> Result<(), ReadError> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || Path::new(path)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        Err(ReadError::Invalid(
            "Use an indexed path of at most 4096 bytes, without parent traversal or NUL".into(),
        ))
    } else {
        Ok(())
    }
}
fn visible_kinds() -> String {
    VISIBLE_CONVERSATION_KINDS
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(",")
}
fn metadata(conn: &Connection, id: i64, ids: &str) -> Result<Value, ReadError> {
    conn.query_row("SELECT s.id,s.workspace_id,CASE WHEN octet_length(s.title)<=65536 THEN substr(s.title,1,512) ELSE '[metadata exceeds limit]' END,CASE WHEN octet_length(a.family)<=65536 THEN substr(a.family,1,128) ELSE '[metadata exceeds limit]' END,CASE WHEN octet_length(w.path)<=65536 THEN substr(w.path,1,4096) ELSE '[metadata exceeds limit]' END,CASE WHEN octet_length(s.host)<=65536 THEN substr(s.host,1,255) ELSE '[metadata exceeds limit]' END,CASE WHEN octet_length(s.started_at)<=65536 THEN substr(s.started_at,1,64) ELSE '[metadata exceeds limit]' END,CASE WHEN octet_length(s.last_event_at)<=65536 THEN substr(s.last_event_at,1,64) ELSE '[metadata exceeds limit]' END FROM session s JOIN agent a ON a.id=s.agent_id LEFT JOIN workspace w ON w.id=s.workspace_id WHERE s.id=?1 AND (?2='null' OR s.workspace_id IN(SELECT value FROM json_each(?2)))",params![id,ids],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"workspaceId":r.get::<_,Option<i64>>(1)?,"title":r.get::<_,Option<String>>(2)?,"agent":r.get::<_,String>(3)?,"project":r.get::<_,Option<String>>(4)?,"host":r.get::<_,Option<String>>(5)?,"startedAt":r.get::<_,Option<String>>(6)?,"lastEventAt":r.get::<_,Option<String>>(7)?}))).optional()?.ok_or(ReadError::OutsideScope)
}
fn decode_cursor(
    page: &ReadPage,
    fingerprint: &str,
    revision: &LibraryRevision,
) -> Result<u64, ReadError> {
    if let Some(value) = &page.cursor {
        let cursor: Cursor = serde_json::from_str(value).map_err(|_| ReadError::InvalidCursor)?;
        if cursor.fingerprint != fingerprint || cursor.offset > search::MAX_OFFSET {
            return Err(ReadError::InvalidCursor);
        }
        if &cursor.revision != revision {
            return Err(ReadError::StaleCursor);
        }
        Ok(cursor.offset)
    } else {
        Ok(0)
    }
}
fn encode_cursor(
    more: bool,
    offset: u64,
    fingerprint: String,
    revision: &LibraryRevision,
) -> Result<Option<String>, ReadError> {
    if more && offset <= search::MAX_OFFSET {
        Ok(Some(
            serde_json::to_string(&Cursor {
                revision: revision.clone(),
                fingerprint,
                offset,
            })
            .map_err(|_| ReadError::InvalidCursor)?,
        ))
    } else {
        Ok(None)
    }
}

fn is_interrupted(error: &ReadError) -> bool {
    let sql = match error {
        ReadError::Sql(e) => Some(e),
        ReadError::Search(search::SearchError::Sql(e)) => Some(e),
        ReadError::Project(ProjectError::Database(e)) => Some(e),
        _ => None,
    };
    matches!(sql,Some(rusqlite::Error::SqliteFailure(code,message)) if code.code==rusqlite::ErrorCode::OperationInterrupted || message.as_deref().is_some_and(|m|m.contains("HESSION_SEARCH_TIME_BUDGET")))
}

/// Bound inputs to the shared metadata compiler before it materializes its CTE.
/// All identifiers are application constants, and scope is bound independently.
fn metadata_budget(conn: &Connection, ids: &str) -> Result<(), ReadError> {
    let mut total = 0_i64;
    for (from,columns) in [
        ("session s JOIN agent a ON a.id=s.agent_id LEFT JOIN workspace w ON w.id=s.workspace_id","s.title,s.host,s.started_at,s.last_event_at,s.source_uri,a.family,a.product,w.path,w.display_name"),
        ("workspace w LEFT JOIN session s ON s.workspace_id=w.id","w.path,w.display_name"),
        ("file_activity f JOIN session s ON s.id=f.session_id","f.path,f.action"),
        ("entry e JOIN session s ON s.id=e.session_id","e.kind,e.role,e.occurred_at"),
        ("tool_invocation t JOIN session s ON s.id=t.session_id","t.tool_name,t.status,t.started_at,t.ended_at"),
        ("session_context c JOIN session s ON s.id=c.session_id LEFT JOIN model m ON m.id=c.model_id","c.branch,m.model_key"),
        ("session_tag t JOIN session s ON s.id=t.session_id","t.label"),
    ]{
        let bytes=columns.split(',').map(|c|format!("coalesce(octet_length({c}),0)")).collect::<Vec<_>>().join("+");
        let scope=if from.starts_with("workspace "){"w.id"}else{"s.workspace_id"};
        let sql=format!("SELECT coalesce(sum({bytes}),0),coalesce(max({bytes}),0) FROM {from} WHERE (?1='null' OR {scope} IN(SELECT value FROM json_each(?1)))");
        let (sum,max):(i64,i64)=conn.query_row(&sql,[ids],|r|Ok((r.get(0)?,r.get(1)?)))?;
        total=total.saturating_add(sum);if max>65536||total>16*1024*1024{return Err(ReadError::DataTooLarge);}
    }
    Ok(())
}
fn visible_text_budget(conn: &Connection, ids: &str) -> Result<(), ReadError> {
    let sql=format!("SELECT EXISTS(SELECT 1 FROM content_part p JOIN entry e ON e.id=p.entry_id JOIN session s ON s.id=e.session_id WHERE (?1='null' OR s.workspace_id IN(SELECT value FROM json_each(?1))) AND e.role IN('user','assistant') AND e.kind IN({}) AND p.kind='text' AND octet_length(p.text)>1048576)",visible_kinds());
    if conn.query_row(&sql, [ids], |r| r.get::<_, bool>(0))? {
        return Err(ReadError::DataTooLarge);
    }
    Ok(())
}

fn indexed_text_budget(conn: &Connection, ids: &str) -> Result<(), ReadError> {
    let oversized:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM entry e JOIN session s ON s.id=e.session_id WHERE (?1='null' OR s.workspace_id IN(SELECT value FROM json_each(?1))) AND octet_length(e.search_text)>1048576)",[ids],|r|r.get(0))?;
    if oversized {
        return Err(ReadError::DataTooLarge);
    }
    Ok(())
}
