//! Measure stored cells before SQLite string functions or matcher UDFs can
//! materialize them. These queries touch approved IDs and metadata lengths;
//! they never load transcript bodies or inspect excluded private content.
use super::*;
use rusqlite::params;

const MAX_METADATA_CELL: i64 = 64 * 1024;
const MAX_TEXT_CELL: i64 = 1024 * 1024;
const MAX_INPUT_BYTES: i64 = 16 * 1024 * 1024;

pub(super) fn check(
    conn: &Connection,
    request: &SearchRequest,
    scope: &SearchScope,
    parsed: &query::Query,
) -> Result<(), SearchError> {
    let ids =
        serde_json::to_string(&scope.workspace_ids()).map_err(|_| SearchError::ResponseTooLarge)?;
    let scope_cte = "WITH approved AS MATERIALIZED (SELECT id FROM session WHERE ?1='null' OR workspace_id IN (SELECT value FROM json_each(?1)))";
    // All identifiers and joins are application constants. The allowlist is a
    // bound JSON array; untrusted requests cannot provide SQL/table names.
    for (from, columns, overhead) in [
        ("session s JOIN approved p ON p.id=s.id JOIN agent a ON a.id=s.agent_id LEFT JOIN workspace w ON w.id=s.workspace_id", &[
            "s.title", "s.host", "s.started_at", "s.last_event_at", "a.family", "a.product", "w.path", "w.display_name"
        ][..], 128),
        ("entry e JOIN approved p ON p.id=e.session_id", &["e.kind", "e.role"][..], 0),
        (
            "content_part c JOIN entry e ON e.id=c.entry_id JOIN approved p ON p.id=e.session_id",
            &["c.kind"][..],
            0,
        ),
        ("file_activity f JOIN approved p ON p.id=f.session_id", &["f.path"][..], 0),
        ("tool_invocation t JOIN approved p ON p.id=t.session_id", &["t.tool_name", "t.status"][..], 0),
        ("session_context c JOIN approved p ON p.id=c.session_id LEFT JOIN model m ON m.id=c.model_id", &["c.branch", "m.model_key"][..], 0),
        ("session_tag t JOIN approved p ON p.id=t.session_id", &["t.label"][..], 0),
    ] {
        let sizes = columns.iter().map(|c| format!("coalesce(octet_length({c}),0)")).collect::<Vec<_>>();
        let maximum = format!("max({})", sizes.join(","));
        // SQLite's one-argument max is an aggregate rather than scalar.
        let maximum = if sizes.len() == 1 { sizes[0].clone() } else { maximum };
        let sql = format!("{scope_cte} SELECT coalesce(max({maximum}),0),coalesce(sum({overhead}+{}),0) FROM {from}",sizes.join("+"));
        let (cell, total): (i64, i64) = conn.query_row(&sql, [&ids], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if cell > MAX_METADATA_CELL || total > MAX_INPUT_BYTES {
            return Err(SearchError::InputTooLarge);
        }
    }
    let searches_text = parsed
        .alternatives
        .iter()
        .flatten()
        .any(|t| t.field.is_none());
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
            ("entry e JOIN approved a ON a.id=e.session_id JOIN content_part p ON p.entry_id=e.id",format!("e.role IN('user','assistant') AND e.kind IN({kinds}) AND p.kind='text'"),"p.text")
        }
        SearchVisibility::AllIndexedText => (
            "entry e JOIN approved a ON a.id=e.session_id",
            "1".into(),
            "e.search_text",
        ),
        SearchVisibility::MetadataOnly => unreachable!(),
    };
    let sql = format!("{scope_cte} SELECT coalesce(max(octet_length({column})),0),coalesce(sum(octet_length({column})),0) FROM {from} WHERE {predicate}");
    let (cell, total): (i64, i64) =
        conn.query_row(&sql, params![ids], |r| Ok((r.get(0)?, r.get(1)?)))?;
    if cell > MAX_TEXT_CELL || total > MAX_INPUT_BYTES {
        return Err(SearchError::InputTooLarge);
    }
    Ok(())
}
