//! Bound values before SQLite materializes strings, and count the fixed-width
//! hit records actually emitted into materialized CTEs. Transcript bytes are
//! streamed one eligible cell at a time; their corpus total is not a memory cap.
use super::*;
use rusqlite::functions::FunctionFlags;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

const MAX_METADATA_CELL: i64 = 64 * 1024;
const MAX_TEXT_CELL: i64 = 1024 * 1024;
const MAX_SCOPED_METADATA_BYTES: i64 = 16 * 1024 * 1024;
const SCOPED_ROW_OVERHEAD: i64 = 128;
// A hit has only integer IDs and a floating-point rank. Budget both its hCTE
// record and any all_hits copy; no transcript strings enter either structure.
// This is a conservative row estimate, not an allocator-byte measurement.
const MAX_MATERIALIZED_HITS: usize = 16 * 1024 * 1024 / 96;
const HIT_LIMIT_MARKER: &str = "HESSION_SEARCH_MATERIALIZATION_LIMIT";

pub(super) fn check(
    conn: &Connection,
    request: &SearchRequest,
    scope: &SearchScope,
    parsed: &query::Query,
) -> Result<(), SearchError> {
    let ids =
        serde_json::to_string(&scope.workspace_ids()).map_err(|_| SearchError::ResponseTooLarge)?;
    // Cap this permission fence itself before constructing an unbounded ID CTE.
    // The extra row necessarily exceeds the scoped metadata overhead budget.
    let scope_cte=format!("WITH approved AS MATERIALIZED (SELECT id FROM session NOT INDEXED WHERE ?1='null' OR workspace_id IN (SELECT value FROM json_each(?1)) LIMIT {})",MAX_SCOPED_METADATA_BYTES/SCOPED_ROW_OVERHEAD+1);
    // Approved-first loops and persistent indexes prevent SQLite from building
    // an automatic covering index containing oversized or denied TEXT before
    // octet_length/CASE can guard it. Automatic indexes remain useful on the
    // bounded numeric hit CTEs and are not disabled connection-wide.
    let terms = parsed.alternatives.iter().flatten().collect::<Vec<_>>();
    let searches_text = terms.iter().any(|t| t.field.is_none());
    let field = |wanted: query::Field| terms.iter().any(|t| t.field.as_ref() == Some(&wanted));
    let conversation = request.visibility == SearchVisibility::Conversation
        && (searches_text || request.mode == SearchMode::Messages);
    let mut checks=vec![("approved p CROSS JOIN session s ON s.id=p.id JOIN agent a ON a.id=s.agent_id LEFT JOIN workspace w ON w.id=s.workspace_id",vec!["s.title","s.host","s.started_at","s.last_event_at","a.family","a.product","w.path","w.display_name"],true)];
    if conversation {
        checks.push(("approved a CROSS JOIN entry e INDEXED BY entry_session_order_idx ON e.session_id=a.id LEFT JOIN content_part p INDEXED BY sqlite_autoindex_content_part_1 ON p.entry_id=e.id",vec!["e.kind","e.role","p.kind"],false));
    }
    if searches_text || field(query::Field::File) {
        checks.push((
            "approved p CROSS JOIN file_activity f INDEXED BY file_session_idx ON f.session_id=p.id",
            vec!["f.path"],
            false,
        ));
    }
    let mut tool_columns = Vec::new();
    if field(query::Field::Tool) || !request.filters.tools.is_empty() {
        tool_columns.push("t.tool_name");
    }
    if field(query::Field::Errors) {
        tool_columns.push("t.status");
    }
    if !tool_columns.is_empty() {
        checks.push((
            "approved p CROSS JOIN tool_invocation t INDEXED BY tool_session_idx ON t.session_id=p.id",
            tool_columns,
            false,
        ));
    }
    let mut context_columns = Vec::new();
    if field(query::Field::Branch) || !request.filters.branch.is_empty() {
        context_columns.push("c.branch");
    }
    if !request.filters.models.is_empty() {
        context_columns.push("m.model_key");
    }
    if !context_columns.is_empty() {
        checks.push(("approved p CROSS JOIN session_context c INDEXED BY context_session_latest_idx ON c.session_id=p.id LEFT JOIN model m ON m.id=c.model_id",context_columns,false));
    }
    // Returned annotations always read labels, even without a tag filter.
    checks.push((
        "session_tag t NOT INDEXED JOIN approved p ON p.id=t.session_id",
        vec!["t.label"],
        false,
    ));
    let checks = checks
        .into_iter()
        .map(|(from, columns, materialized)| {
            let sizes = columns
                .iter()
                .map(|c| format!("coalesce(octet_length({c}),0)"))
                .collect::<Vec<_>>();
            let maximum = if sizes.len() == 1 {
                sizes[0].clone()
            } else {
                format!("max({})", sizes.join(","))
            };
            let total = if materialized {
                format!("coalesce(sum({SCOPED_ROW_OVERHEAD}+{}),0)", sizes.join("+"))
            } else {
                "0".into()
            };
            format!("SELECT coalesce(max({maximum}),0) AS cell,{total} AS materialized FROM {from}")
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    let sql =
        format!("{scope_cte}, checks AS ({checks}) SELECT max(cell),sum(materialized) FROM checks");
    let (cell, total): (i64, i64) = conn.query_row(&sql, [&ids], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if cell > MAX_METADATA_CELL || total > MAX_SCOPED_METADATA_BYTES {
        return Err(SearchError::InputTooLarge);
    }
    if !searches_text || request.visibility == SearchVisibility::MetadataOnly {
        return Ok(());
    }
    let (from, predicate, column) = match request.visibility {
        SearchVisibility::Conversation => {
            let kinds = VISIBLE_CONVERSATION_KINDS
                .iter()
                .map(|k| format!("'{k}'"))
                .collect::<Vec<_>>()
                .join(",");
            ("approved a CROSS JOIN entry e INDEXED BY entry_session_order_idx ON e.session_id=a.id JOIN content_part p INDEXED BY sqlite_autoindex_content_part_1 ON p.entry_id=e.id",format!("e.role IN('user','assistant') AND e.kind IN({kinds}) AND p.kind='text'"),"p.text")
        }
        SearchVisibility::AllIndexedText => (
            "approved a CROSS JOIN entry e INDEXED BY entry_session_order_idx ON e.session_id=a.id",
            "1".into(),
            "e.search_text",
        ),
        SearchVisibility::MetadataOnly => unreachable!(),
    };
    // Only an individual cell can be materialized by the matcher/excerpt path.
    // Total history size says nothing about the size of its emitted hit CTEs.
    let sql=format!("{scope_cte} SELECT EXISTS(SELECT 1 FROM {from} WHERE {predicate} AND octet_length({column})>{MAX_TEXT_CELL})");
    if conn.query_row(&sql, [ids], |r| r.get::<_, bool>(0))? {
        return Err(SearchError::InputTooLarge);
    }
    Ok(())
}

pub(super) fn register_hit_budget(conn: &Connection) -> rusqlite::Result<()> {
    let rows = Arc::new(AtomicUsize::new(0));
    conn.create_scalar_function(
        "_hession_hit_budget_v1",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_INNOCUOUS,
        move |ctx| {
            let id = ctx.get::<i64>(0)?;
            if rows.fetch_add(1, Ordering::Relaxed) >= MAX_MATERIALIZED_HITS {
                return Err(rusqlite::Error::UserFunctionError(Box::new(
                    std::io::Error::other(HIT_LIMIT_MARKER),
                )));
            }
            Ok(id)
        },
    )
}
pub(super) fn map_execution_error(error: SearchError) -> SearchError {
    if matches!(&error,SearchError::Sql(rusqlite::Error::SqliteFailure(_,Some(message))) if message.contains(HIT_LIMIT_MARKER))
    {
        SearchError::InputTooLarge
    } else {
        error
    }
}

// ORDER BY/LIMIT retains at most offset+page rows. Message ranking stores only
// IDs, numeric score and numeric date, never duplicated session strings.
pub(super) fn check_message_window(offset: u64, limit: u32) -> Result<(), SearchError> {
    if offset.saturating_add(u64::from(limit) + 1) > MAX_MATERIALIZED_HITS as u64 {
        return Err(SearchError::InputTooLarge);
    }
    Ok(())
}
