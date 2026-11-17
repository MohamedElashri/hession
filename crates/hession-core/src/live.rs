//! Passive recorded local activity. No process discovery, agent launch, remote
//! refresh or inference of success/permission state occurs here.
use crate::{
    db, library_revision,
    service::{Library, ServiceError, MAX_OFFSET, MAX_RESPONSE_BYTES},
    sql::query_all,
};
use rusqlite::types::Value as SqlValue;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct RecentRequest {
    pub agent: String,
    pub project: String,
    pub since: String,
    pub limit: u32,
    pub offset: u64,
    pub unhealthy: bool,
}
impl Default for RecentRequest {
    fn default() -> Self {
        Self {
            agent: String::new(),
            project: String::new(),
            since: String::new(),
            limit: 50,
            offset: 0,
            unhealthy: false,
        }
    }
}

fn query_error(error: impl std::fmt::Display) -> ServiceError {
    ServiceError::Query(error.to_string())
}

fn source_available(uri: &str) -> bool {
    if uri.len() > 8192 {
        return false;
    }
    let Some((family, path)) = uri.split_once("://") else {
        return false;
    };
    if ![
        "claude",
        "codex",
        "opencode",
        "pi",
        "commandcode",
        "muse",
        "hermes",
        "antigravity",
        "cline",
    ]
    .contains(&family)
        || !Path::new(path).is_absolute()
    {
        return false;
    }
    let regular = |path: &str| std::fs::metadata(path).is_ok_and(|m| m.is_file());
    if regular(path) {
        return true;
    }
    // Legacy SQLite session URIs append a native session fragment. This is an
    // availability hint only; never open/read the candidate's content.
    matches!(family, "opencode" | "hermes")
        && path.rsplit_once('#').is_some_and(|(file, _)| regular(file))
}

impl Library {
    pub fn recent_local_sessions(&self, request: &RecentRequest) -> Result<Value, ServiceError> {
        if request.limit == 0
            || request.limit > 100
            || request.offset > MAX_OFFSET
            || request.agent.len() > 64
            || request.project.len() > 4096
            || request.since.len() > 40
        {
            return Err(ServiceError::Query(
                "Use a recent-session limit of 1–100, offset at most 1000000 and bounded filters"
                    .into(),
            ));
        }
        let mut conn = db::open_read_only(self.path())?;
        let deadline = Instant::now() + Duration::from_secs(3);
        conn.progress_handler(1000, Some(move || Instant::now() >= deadline))
            .map_err(query_error)?;
        let tx = conn.transaction().map_err(query_error)?;
        if !request.since.is_empty() {
            if request.since.len() < 10
                || !request
                    .since
                    .bytes()
                    .all(|b| b.is_ascii_digit() || b"-Tt:+. Zz".contains(&b))
            {
                return Err(ServiceError::Query(
                    "Use an absolute ISO date/time cutoff".into(),
                ));
            }
            let date: Option<f64> = tx
                .query_row("SELECT julianday(?)", [&request.since], |r| r.get(0))
                .map_err(query_error)?;
            if date.is_none() {
                return Err(ServiceError::Query(
                    "Invalid recent-session date/time cutoff".into(),
                ));
            }
        }
        // All string functions below operate only after this byte-count
        // preflight. Direct octet_length(column) does not load a TEXT value.
        let columns = [
            "s.title",
            "s.cwd",
            "s.last_event_at",
            "s.started_at",
            "s.source_uri",
            "a.family",
            "a.product",
            "w.path",
            "e.kind",
            "e.role",
            "e.occurred_at",
        ];
        let sizes = columns
            .iter()
            .map(|c| format!("coalesce(octet_length({c}),0)"))
            .collect::<Vec<_>>()
            .join(",");
        let max: i64 = tx.query_row(&format!("SELECT coalesce(max(max({sizes})),0) FROM session s JOIN agent a ON a.id=s.agent_id LEFT JOIN workspace w ON w.id=s.workspace_id LEFT JOIN entry e ON e.id=(SELECT id FROM entry WHERE session_id=s.id ORDER BY ordinal DESC LIMIT 1) WHERE s.host IS NULL"),[],|r|r.get(0)).map_err(query_error)?;
        if max > 64 * 1024 {
            return Err(ServiceError::ResponseTooLarge);
        }
        let kinds = crate::search::VISIBLE_CONVERSATION_KINDS
            .iter()
            .map(|kind| format!("'{kind}'"))
            .collect::<Vec<_>>()
            .join(",");
        let sql=format!("SELECT s.id,substr(s.title,1,240) AS title,a.family AS agent,substr(a.product,1,80) AS product,
            substr(COALESCE(w.path,s.cwd,''),1,4096) AS project,s.workspace_id AS workspaceId,
            substr(s.last_event_at,1,64) AS lastEventAt,substr(s.source_uri,1,8193) AS nativeSource,
            e.id AS entryId,e.ordinal AS ordinal,substr(e.kind,1,80) AS kind,substr(e.role,1,80) AS role,
            substr(e.occurred_at,1,64) AS occurredAt,
            (SELECT COUNT(*) FROM entry prior WHERE prior.session_id=s.id AND prior.ordinal<e.ordinal) AS position,
            CASE WHEN e.role IN('user','assistant') AND e.kind IN({kinds}) THEN
                (SELECT cp.id FROM content_part cp WHERE cp.entry_id=e.id AND CASE WHEN octet_length(cp.kind)<=64 THEN cp.kind='text' ELSE 0 END AND cp.text IS NOT NULL ORDER BY cp.ordinal LIMIT 1) ELSE NULL END AS summaryPartId,
            CASE WHEN julianday(COALESCE(e.occurred_at,s.last_event_at,s.started_at)) BETWEEN julianday('now','-15 minutes') AND julianday('now','+5 minutes') THEN 1 ELSE 0 END AS isRecent
            FROM session s JOIN agent a ON a.id=s.agent_id LEFT JOIN workspace w ON w.id=s.workspace_id
            LEFT JOIN entry e ON e.id=(SELECT latest.id FROM entry latest WHERE latest.session_id=s.id ORDER BY latest.ordinal DESC LIMIT 1)
            WHERE s.host IS NULL AND s.source_uri NOT LIKE 'hession-sync://%'
            AND a.family IN('claude','codex','opencode','pi','commandcode','muse','hermes','antigravity','cline')
            AND (?1='' OR a.family=?1) AND (?2='' OR COALESCE(w.path,s.cwd,'')=?2)
            AND (?3='' OR julianday(COALESCE(s.last_event_at,s.started_at))>=julianday(?3))
            ORDER BY COALESCE(s.last_event_at,s.started_at) DESC,s.id DESC LIMIT ?4 OFFSET ?5");
        let mut rows = query_all(
            &tx,
            &sql,
            &[
                SqlValue::Text(request.agent.clone()),
                SqlValue::Text(request.project.clone()),
                SqlValue::Text(request.since.clone()),
                SqlValue::Integer(i64::from(request.limit) + 1),
                SqlValue::Integer(request.offset as i64),
            ],
        )
        .map_err(ServiceError::Query)?;
        let has_more = rows.len() > request.limit as usize;
        rows.truncate(request.limit as usize);
        let mut sessions = Vec::with_capacity(rows.len());
        for row in rows {
            let summary = row["summaryPartId"]
                .as_i64()
                .map(|id| {
                    crate::text_read::read_prefix(
                        &tx,
                        crate::text_read::TextColumn::ContentPartText,
                        id,
                        960,
                        240,
                    )
                })
                .transpose()
                .map_err(query_error)?;
            let summary_truncated = summary.as_ref().is_some_and(|s| s.truncated);
            let summary = summary.map_or_else(String::new, |s| s.text);
            let status = if !source_available(row["nativeSource"].as_str().unwrap_or("")) {
                "unavailable"
            } else if request.unhealthy {
                "stale"
            } else if row["isRecent"].as_i64() == Some(1) {
                "recent"
            } else {
                "quiet"
            };
            let latest = if row["entryId"].is_null() {
                Value::Null
            } else {
                json!({"id":row["entryId"],"ordinal":row["ordinal"],"position":row["position"],"kind":row["kind"],"role":row["role"],"summary":summary,"summaryTruncated":summary_truncated,"occurredAt":row["occurredAt"]})
            };
            sessions.push(json!({"id":row["id"],"title":row["title"],"agent":row["agent"],"product":row["product"],"project":row["project"],"workspaceId":row["workspaceId"],"lastEventAt":row["lastEventAt"],"status":status,"latestEvent":latest}));
        }
        let next = (has_more && request.offset + u64::from(request.limit) <= MAX_OFFSET)
            .then_some(request.offset + u64::from(request.limit));
        let result = json!({"sessions":sessions,"hasMore":has_more,"nextOffset":next,"revision":library_revision::current(&tx).map_err(query_error)?});
        if serde_json::to_vec(&result).map_err(query_error)?.len() > MAX_RESPONSE_BYTES {
            return Err(ServiceError::ResponseTooLarge);
        }
        Ok(result)
    }
}
