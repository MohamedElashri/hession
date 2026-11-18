//! Classification of background histories without discarding their transcripts.
use serde_json::Value;

pub fn is_subagent(family: &str, source_uri: &str, metadata: &Value) -> bool {
    match family {
        "codex" => {
            let explicit = metadata["source"]
                .as_object()
                .is_some_and(|source| source.get("subagent").is_some_and(|value| !value.is_null()));
            // Older imports let the inherited parent header overwrite id and
            // source, but retained the child's nickname and fork marker. This
            // identifies those rows even when the native log is now offline.
            let inherited_parent = metadata["agent_nickname"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
                && metadata["forked_from_id"].as_str().is_some_and(|parent| {
                    !parent.is_empty() && metadata["id"].as_str() == Some(parent)
                });
            explicit || inherited_parent
        }
        "claude" => {
            source_uri.contains("/subagents/")
                || source_uri
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.starts_with("agent-"))
                || metadata["isSidechain"] == true
        }
        "opencode" => metadata["parent_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        _ => false,
    }
}

pub(crate) fn migrate(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch("ALTER TABLE session ADD COLUMN is_subagent INTEGER NOT NULL DEFAULT 0 CHECK(is_subagent IN (0,1));")?;
    let mut statement = conn.prepare("SELECT s.id,a.family,s.source_uri,s.raw_metadata_json FROM session s JOIN agent a ON a.id=s.agent_id")?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let family: String = row.get(1)?;
        let uri: String = row.get(2)?;
        let raw: Option<String> = row.get(3)?;
        let metadata = raw
            .as_deref()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or(Value::Null);
        if is_subagent(&family, &uri, &metadata) {
            conn.execute("UPDATE session SET is_subagent=1 WHERE id=?", [id])?;
        }
    }
    drop(rows);
    drop(statement);
    Ok(())
}

pub(crate) fn migrate_indexes(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch("DROP INDEX session_activity_idx;
        CREATE INDEX session_activity_idx ON session(COALESCE(last_event_at,started_at) DESC,id DESC,agent_id,workspace_id,is_subagent);
        CREATE INDEX session_workspace_activity_idx ON session(workspace_id,COALESCE(last_event_at,started_at) DESC,id DESC,agent_id,is_subagent);
        CREATE INDEX session_agent_activity_idx ON session(agent_id,COALESCE(last_event_at,started_at) DESC,id DESC,workspace_id,is_subagent);
        CREATE INDEX session_main_activity_idx ON session(COALESCE(last_event_at,started_at) DESC,id DESC,workspace_id,agent_id,is_subagent) WHERE is_subagent=0;
        CREATE INDEX session_main_workspace_activity_idx ON session(workspace_id,COALESCE(last_event_at,started_at) DESC,id DESC,agent_id,is_subagent) WHERE is_subagent=0;
        CREATE INDEX session_main_agent_activity_idx ON session(agent_id,COALESCE(last_event_at,started_at) DESC,id DESC,workspace_id,is_subagent) WHERE is_subagent=0;
        CREATE INDEX context_model_session_idx ON session_context(model_id,session_id);
        CREATE INDEX context_branch_session_idx ON session_context(branch,session_id);")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_helpers_without_hiding_user_forks_or_repeated_titles() {
        for (family, uri, metadata, expected) in [
            (
                "codex",
                "codex:///rollout.jsonl",
                json!({"source":{"subagent":{"other":"guardian"}}}),
                true,
            ),
            (
                "codex",
                "codex:///rollout.jsonl",
                json!({"source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}}),
                true,
            ),
            (
                "codex",
                "codex:///rollout.jsonl",
                json!({"source":"cli","forked_from_id":"parent"}),
                false,
            ),
            (
                "codex",
                "codex:///rollout.jsonl",
                json!({"source":"cli","id":"parent","forked_from_id":"parent","agent_nickname":"Ada"}),
                true,
            ),
            (
                "codex",
                "codex:///rollout.jsonl",
                json!({"source":"cli","id":"user-fork","forked_from_id":"parent","agent_nickname":"Ada"}),
                false,
            ),
            (
                "codex",
                "codex:///rollout.jsonl",
                json!({"source":{"subagent":null}}),
                false,
            ),
            (
                "claude",
                "claude:///parent/subagents/agent-child.jsonl",
                Value::Null,
                true,
            ),
            (
                "claude",
                "claude:///project/agent-child.jsonl",
                Value::Null,
                true,
            ),
            (
                "claude",
                "claude:///project/session.jsonl",
                json!({"isSidechain":true}),
                true,
            ),
            (
                "claude",
                "claude:///project/session.jsonl",
                json!({"title":"Repeated prompt"}),
                false,
            ),
            (
                "opencode",
                "opencode:///db#child",
                json!({"parent_id":"parent"}),
                true,
            ),
            (
                "opencode",
                "opencode:///db#main",
                json!({"parent_id":null}),
                false,
            ),
            (
                "opencode",
                "opencode:///db#main",
                json!({"parent_id":""}),
                false,
            ),
            (
                "pi",
                "pi:///session.jsonl",
                json!({"title":"Repeated prompt"}),
                false,
            ),
        ] {
            assert_eq!(
                is_subagent(family, uri, &metadata),
                expected,
                "{family} {metadata}"
            );
        }
    }
}
