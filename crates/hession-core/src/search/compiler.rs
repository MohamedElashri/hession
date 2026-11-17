use super::{
    query::{self, Field, Query, Term},
    *,
};
use rusqlite::{params_from_iter, types::Value};

fn visible_entry_predicate() -> String {
    // Only application-owned constants become SQL. Never accept an alias or
    // kind list from a query, cursor or adapter request.
    let kinds = VISIBLE_CONVERSATION_KINDS
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(",");
    format!("e.role IN('user','assistant') AND e.kind IN({kinds})")
}
struct Compiler {
    visibility: SearchVisibility,
    params: Vec<Value>,
    ctes: Vec<String>,
    positives: Vec<(usize, Term)>,
    text_count: usize,
}
impl Compiler {
    fn bind(&mut self, value: impl Into<Value>) -> String {
        self.params.push(value.into());
        format!("?{}", self.params.len())
    }
    fn contains(&mut self, column: &str, value: &str) -> String {
        let p = self.bind(value.to_owned());
        format!("instr(lower(coalesce({column},'')),lower({p}))>0")
    }
    fn metadata(&mut self, t: &Term) -> String {
        match t.field.as_ref().unwrap() {
            Field::Agent => { let a=self.contains("s.family",&t.value); let b=self.contains("s.product",&t.value); format!("({a} OR {b})") },
            Field::Project => { let a=self.contains("s.project",&t.value); let b=self.contains("s.display_name",&t.value); format!("({a} OR {b})") },
            Field::File => { let c=self.contains("f.path",&t.value); format!("EXISTS(SELECT 1 FROM file_activity f INDEXED BY file_session_idx WHERE f.session_id=s.id AND {c})") },
            Field::Tool => { let c=self.contains("t.tool_name",&t.value); format!("EXISTS(SELECT 1 FROM tool_invocation t INDEXED BY tool_session_idx WHERE t.session_id=s.id AND {c})") },
            Field::Branch => { let c=self.contains("c.branch",&t.value); format!("EXISTS(SELECT 1 FROM session_context c INDEXED BY context_session_latest_idx WHERE c.session_id=s.id AND {c})") },
            Field::Host if t.value=="@local" => "s.host IS NULL".into(),
            Field::Host => { let p=self.bind(t.value.clone()); format!("s.host={p} COLLATE NOCASE") },
            Field::After|Field::Before => { let p=self.bind(t.value.clone()); let op=if t.field==Some(Field::After) { ">=" } else { "<" }; format!("julianday(s.activity_at){op}julianday({p})") },
            Field::Errors => "EXISTS(SELECT 1 FROM tool_invocation t INDEXED BY tool_session_idx WHERE t.session_id=s.id AND (t.exit_code<>0 OR lower(t.status) IN ('error','failed','failure')))".into(),
        }
    }
    fn term(&mut self, t: &Term, mode: SearchMode) -> String {
        let predicate = if t.field.is_some() {
            self.metadata(t)
        } else {
            let index = self.text_count;
            self.text_count += 1;
            let cte = if self.visibility == SearchVisibility::MetadataOnly {
                format!("h{index} AS MATERIALIZED (SELECT e.id,e.session_id,0.0 AS rank FROM entry e WHERE 0)")
            } else if self.visibility == SearchVisibility::Conversation {
                let p = self.bind(t.value.clone());
                let literal = i32::from(query::is_literal(&t.value));
                format!("h{index} AS MATERIALIZED (SELECT _hession_hit_budget_v1(e.id) AS id,e.session_id,-0.000001 AS rank FROM scoped s CROSS JOIN entry e INDEXED BY entry_session_order_idx ON e.session_id=s.id WHERE {} AND EXISTS(SELECT 1 FROM content_part p INDEXED BY sqlite_autoindex_content_part_1 WHERE p.entry_id=e.id AND p.kind='text' AND _hession_visible_match_v1(CASE WHEN e.session_id IN(SELECT id FROM scoped) THEN CASE WHEN {} THEN CASE WHEN p.kind='text' THEN p.text END END END,{p},{literal})))",visible_entry_predicate(),visible_entry_predicate())
            } else if query::is_literal(&t.value) {
                let p = self.bind(t.value.clone());
                format!("h{index} AS MATERIALIZED (SELECT _hession_hit_budget_v1(e.id) AS id,e.session_id,-0.000001 AS rank FROM scoped s CROSS JOIN entry e INDEXED BY entry_session_order_idx ON e.session_id=s.id WHERE _hession_visible_match_v1(CASE WHEN e.session_id IN(SELECT id FROM scoped) THEN e.search_text END,{p},1)>0)")
            } else {
                let p = self.bind(query::fts_literal(&t.value));
                // FTS reads its index first; rowid scope is checked before any
                // content-table payload is selected by this service.
                format!("h{index} AS MATERIALIZED (SELECT _hession_hit_budget_v1(e.id) AS id,e.session_id,bm25(entry_fts) AS rank FROM entry_fts JOIN entry e ON e.id=entry_fts.rowid JOIN scoped s ON s.id=e.session_id WHERE entry_fts MATCH {p})")
            };
            self.ctes.push(cte);
            if !t.excluded {
                self.positives.push((index, t.clone()));
            }
            let entry_match = match mode {
                SearchMode::Sessions => {
                    format!("EXISTS(SELECT 1 FROM h{index} h WHERE h.session_id=s.id)")
                }
                SearchMode::Messages => format!("e.id IN(SELECT id FROM h{index})"),
            };
            if mode == SearchMode::Messages {
                // Every unfielded message term (including exclusions) belongs
                // to this entry. Only explicit fields may match its session.
                entry_match
            } else {
                let title = self.contains("s.title", &t.value);
                let project = self.contains("s.project", &t.value);
                let display = self.contains("s.display_name", &t.value);
                let family = self.contains("s.family", &t.value);
                let file = self.contains("f.path", &t.value);
                format!("({entry_match} OR {title} OR {project} OR {display} OR {family} OR EXISTS(SELECT 1 FROM file_activity f INDEXED BY file_session_idx WHERE f.session_id=s.id AND {file}))")
            }
        };
        if t.excluded {
            format!("NOT coalesce(({predicate}),0)")
        } else {
            format!("coalesce(({predicate}),0)")
        }
    }
    fn filters(&mut self, f: &SearchFilters) -> Vec<String> {
        let mut out = Vec::new();
        for (column, value) in [("s.family", &f.agent), ("s.project", &f.project)] {
            if !value.is_empty() {
                let p = self.bind(value.clone());
                out.push(format!("{column}={p}"));
            }
        }
        if !f.host.is_empty() {
            out.push(self.metadata(&Term {
                field: Some(Field::Host),
                value: f.host.clone(),
                excluded: false,
                phrase: false,
                position: 0,
            }));
        }
        if !f.branch.is_empty() {
            let c = self.contains("c.branch", &f.branch);
            out.push(format!(
                "EXISTS(SELECT 1 FROM session_context c INDEXED BY context_session_latest_idx WHERE c.session_id=s.id AND {c})"
            ));
        }
        for (values,sql) in [(&f.models,"SELECT 1 FROM session_context c INDEXED BY context_session_latest_idx JOIN model m ON m.id=c.model_id WHERE c.session_id=s.id AND m.model_key"),(&f.tools,"SELECT 1 FROM tool_invocation t INDEXED BY tool_session_idx WHERE t.session_id=s.id AND t.tool_name")] {
            if !values.is_empty() { let ps=values.iter().map(|v|self.bind(v.clone())).collect::<Vec<_>>().join(",");out.push(format!("EXISTS({sql} IN({ps}))")); }
        }
        for tag in &f.tags {
            let p = self.bind(tag.clone());
            out.push(format!("EXISTS(SELECT 1 FROM session_tag t NOT INDEXED WHERE t.session_id=s.id AND CASE WHEN t.session_id=s.id THEN t.label END={p} COLLATE NOCASE)"));
        }
        if f.starred {
            out.push("EXISTS(SELECT 1 FROM session_annotation n WHERE n.session_id=s.id AND n.starred=1)".into());
        }
        for (value, op) in [(&f.since, ">="), (&f.before, "<")] {
            if !value.is_empty() {
                let p = self.bind(value.clone());
                out.push(format!("julianday(s.activity_at){op}julianday({p})"));
            }
        }
        out
    }
}

pub(super) fn execute(
    conn: &Connection,
    r: &SearchRequest,
    scope: &SearchScope,
    q: &Query,
    offset: u64,
) -> Result<Vec<SearchResult>, SearchError> {
    if r.mode == SearchMode::Messages {
        bounds::check_message_window(offset, r.limit)?;
    }
    bounds::register_hit_budget(conn)?;
    let mut c = Compiler {
        visibility: r.visibility,
        params: Vec::new(),
        ctes: Vec::new(),
        positives: Vec::new(),
        text_count: 0,
    };
    let scope_sql = if let Some(ids) = scope.workspace_ids() {
        if ids.is_empty() {
            "0".into()
        } else {
            let ps = ids
                .iter()
                .map(|id| c.bind(*id))
                .collect::<Vec<_>>()
                .join(",");
            format!("s.workspace_id IN({ps})")
        }
    } else {
        "1".into()
    };
    c.ctes.push(format!("scoped AS MATERIALIZED (SELECT s.id,s.workspace_id,s.title,s.host,coalesce(s.last_event_at,s.started_at) AS activity_at,a.family,a.product,w.path AS project,w.display_name FROM session s NOT INDEXED JOIN agent a ON a.id=s.agent_id LEFT JOIN workspace w ON w.id=s.workspace_id WHERE {scope_sql})"));
    let alternatives = q
        .alternatives
        .iter()
        .map(|group| {
            if group.is_empty() {
                "1".into()
            } else {
                group
                    .iter()
                    .map(|t| format!("({})", c.term(t, r.mode)))
                    .collect::<Vec<_>>()
                    .join(" AND ")
            }
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut predicates = c.filters(&r.filters);
    predicates.push(format!("({alternatives})"));
    if r.mode == SearchMode::Messages && r.visibility == SearchVisibility::Conversation {
        predicates.push(format!(
            "{} AND EXISTS(SELECT 1 FROM content_part p INDEXED BY sqlite_autoindex_content_part_1 WHERE p.entry_id=e.id AND p.kind='text')",
            visible_entry_predicate()
        ));
    }
    let mut score_parts = Vec::new();
    let mut hit_unions = Vec::new();
    let mut metadata_locations = Vec::new();
    for (index, term) in c.positives.clone() {
        let target = if r.mode == SearchMode::Messages {
            " AND h.id=e.id"
        } else {
            ""
        };
        // Lower is better: sum best BM25 term ranks; phrase, exact path and
        // title boosts are deliberately small constants, not probabilities.
        score_parts.push(format!(
            "coalesce((SELECT min(rank) FROM h{index} h WHERE h.session_id=s.id{target}),0)"
        ));
        if term.phrase {
            score_parts.push(format!("CASE WHEN EXISTS(SELECT 1 FROM h{index} h WHERE h.session_id=s.id{target}) THEN -0.000002 ELSE 0 END"));
        }
        let title = c.contains("s.title", &term.value);
        score_parts.push(format!("CASE WHEN {title} THEN -0.000004 ELSE 0 END"));
        let p = c.bind(term.value.clone());
        score_parts.push(format!("CASE WHEN EXISTS(SELECT 1 FROM file_activity f INDEXED BY file_session_idx WHERE f.session_id=s.id AND f.path={p} COLLATE NOCASE) THEN -0.000008 ELSE 0 END"));
        metadata_locations.push(("title", title));
        let project = c.contains("s.project", &term.value);
        metadata_locations.push(("project", project));
        let file = c.contains("f.path", &term.value);
        metadata_locations.push((
            "file",
            format!("EXISTS(SELECT 1 FROM file_activity f INDEXED BY file_session_idx WHERE f.session_id=s.id AND {file})"),
        ));
        hit_unions.push(format!(
            "SELECT _hession_hit_budget_v1(id) AS id,session_id,rank FROM h{index}"
        ));
    }
    let (join, entry_select) = if r.mode == SearchMode::Messages {
        (
            "JOIN entry e INDEXED BY entry_session_order_idx ON e.session_id=s.id".to_owned(),
            "e.id AS entry_id,e.ordinal".to_owned(),
        )
    } else if !hit_unions.is_empty() {
        c.ctes.push(format!(
            "all_hits AS MATERIALIZED ({})",
            hit_unions.join(" UNION ALL ")
        ));
        ("LEFT JOIN entry e ON e.id=(SELECT h.id FROM all_hits h JOIN entry candidate ON candidate.id=h.id WHERE h.session_id=s.id GROUP BY h.id ORDER BY count(*) DESC,min(h.rank),candidate.ordinal,h.id LIMIT 1)".into(),"e.id AS entry_id,e.ordinal".into())
    } else {
        (String::new(), "NULL AS entry_id,NULL AS ordinal".into())
    };
    let score = if score_parts.is_empty() {
        "0.0".into()
    } else {
        score_parts.join("+")
    };
    for term in q
        .alternatives
        .iter()
        .flatten()
        .filter(|t| t.field.is_some() && !t.excluded)
    {
        let name = match term.field.as_ref().unwrap() {
            Field::Agent => "agent",
            Field::Project => "project",
            Field::File => "file",
            Field::Tool => "tool",
            Field::Branch => "branch",
            Field::Host => "host",
            Field::After | Field::Before => "date",
            Field::Errors => "errors",
        };
        let predicate = c.metadata(term);
        metadata_locations.push((name, predicate));
    }
    if r.mode == SearchMode::Messages && !c.positives.is_empty() {
        let hits = c
            .positives
            .iter()
            .map(|(i, _)| format!("EXISTS(SELECT 1 FROM h{i} h WHERE h.id=e.id)"))
            .collect::<Vec<_>>()
            .join(" OR ");
        metadata_locations.push(("entry", format!("({hits})")));
    }
    let locations = if metadata_locations.is_empty() {
        "''".into()
    } else {
        metadata_locations
            .iter()
            .map(|(name, p)| format!("CASE WHEN {p} THEN '{name},' ELSE '' END"))
            .collect::<Vec<_>>()
            .join("||")
    };
    let limit = c.bind(i64::from(r.limit) + 1);
    let offset = c.bind(offset as i64);
    // Select bounded metadata/IDs first, never whole transcripts. Excerpts are
    // fetched below by authorized IDs inside this same scoped snapshot.
    let metadata="s.id,s.workspace_id,substr(s.title,1,512),substr(s.family,1,128),substr(s.project,1,2048),substr(s.host,1,255),substr(s.activity_at,1,64)";
    let sql = if r.mode == SearchMode::Sessions {
        // Rank/filter before finding an excerpt entry. A broad query should
        // not sort every session's transcript just to return twenty rows.
        c.ctes.push(format!("ranked AS MATERIALIZED (SELECT s.*,({score}) AS score,({locations}) AS locations FROM scoped s WHERE {} ORDER BY score ASC,julianday(s.activity_at) DESC,s.id DESC LIMIT {limit} OFFSET {offset})",predicates.join(" AND ")));
        let entry_location = if !hit_unions.is_empty() {
            "||CASE WHEN e.id IS NOT NULL THEN 'entry,' ELSE '' END"
        } else {
            ""
        };
        format!("WITH {} SELECT {metadata},{entry_select},s.score,s.locations{entry_location} FROM ranked s {join} ORDER BY s.score ASC,julianday(s.activity_at) DESC,s.id DESC,ordinal ASC,entry_id ASC",c.ctes.join(","))
    } else {
        c.ctes.push(format!("ranked_messages AS MATERIALIZED (SELECT s.id AS session_id,e.id AS entry_id,e.ordinal,({score}) AS score,julianday(s.activity_at) AS activity_sort FROM scoped s {join} WHERE {} ORDER BY score ASC,activity_sort DESC,s.id DESC,e.ordinal ASC,e.id ASC LIMIT {limit} OFFSET {offset})",predicates.join(" AND ")));
        format!("WITH {} SELECT {metadata},{entry_select},ranked.score,({locations}) AS locations FROM ranked_messages ranked JOIN scoped s ON s.id=ranked.session_id JOIN entry e ON e.id=ranked.entry_id ORDER BY ranked.score ASC,ranked.activity_sort DESC,s.id DESC,e.ordinal ASC,e.id ASC",c.ctes.join(","))
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(c.params.iter()), |row| {
        Ok(SearchResult {
            session_id: row.get(0)?,
            workspace_id: row.get(1)?,
            title: row.get(2)?,
            agent: row.get(3)?,
            project: row.get(4)?,
            host: row.get(5)?,
            activity_at: row.get(6)?,
            entry_id: row.get(7)?,
            ordinal: row.get(8)?,
            position: None,
            starred: false,
            tags: Vec::new(),
            score: row.get(9)?,
            excerpt: None,
            highlights: Vec::new(),
            matching_locations: row
                .get::<_, String>(10)?
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    })?;
    let mut results = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut annotations = std::collections::HashMap::new();
    for result in &mut results {
        if let std::collections::hash_map::Entry::Vacant(slot) =
            annotations.entry(result.session_id)
        {
            let starred = conn.query_row(
                "SELECT coalesce((SELECT starred FROM session_annotation WHERE session_id=?),0)",
                [result.session_id],
                |r| r.get::<_, bool>(0),
            )?;
            let mut statement=conn.prepare("SELECT substr(CASE WHEN session_id=?1 THEN label END,1,80) FROM session_tag NOT INDEXED WHERE session_id=?1 ORDER BY CASE WHEN session_id=?1 THEN label END COLLATE NOCASE LIMIT 50")?;
            let tags = statement
                .query_map([result.session_id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            slot.insert((starred, tags));
        }
        let (starred, tags) = &annotations[&result.session_id];
        result.starred = *starred;
        result.tags = tags.clone();
        if let Some(ordinal) = result.ordinal {
            result.position = Some(conn.query_row(
                "SELECT count(*) FROM entry WHERE session_id=?1 AND ordinal<?2",
                rusqlite::params![result.session_id, ordinal],
                |row| row.get(0),
            )?);
        }
        result.matching_locations.sort();
        result.matching_locations.dedup();
        if r.visibility != SearchVisibility::MetadataOnly {
            if let Some(id) = result.entry_id {
                use rusqlite::OptionalExtension;
                for (_, term) in &c.positives {
                    let literal = i32::from(query::is_literal(&term.value));
                    let sql=match r.visibility {
                        SearchVisibility::AllIndexedText=>"SELECT substr(CASE WHEN e.id=?1 AND e.session_id=?4 THEN e.search_text END,max(1,_hession_visible_match_v1(CASE WHEN e.id=?1 AND e.session_id=?4 THEN e.search_text END,?2,?3)-120),480) FROM entry e WHERE e.id=?1 AND e.session_id=?4 AND _hession_visible_match_v1(CASE WHEN e.id=?1 AND e.session_id=?4 THEN e.search_text END,?2,?3)>0".to_owned(),
                        SearchVisibility::Conversation=>{
                            let text=format!("CASE WHEN e.id=?1 AND e.session_id=?4 THEN CASE WHEN {} THEN CASE WHEN p.kind='text' THEN p.text END END END",visible_entry_predicate());
                            format!("SELECT substr({text},max(1,_hession_visible_match_v1({text},?2,?3)-120),480) FROM entry e JOIN content_part p INDEXED BY sqlite_autoindex_content_part_1 ON p.entry_id=e.id WHERE e.id=?1 AND e.session_id=?4 AND {} AND p.kind='text' AND _hession_visible_match_v1({text},?2,?3)>0 ORDER BY p.ordinal LIMIT 1",visible_entry_predicate())
                        },
                        SearchVisibility::MetadataOnly=>unreachable!(),
                    };
                    let excerpt: Option<String> = conn
                        .query_row(
                            &sql,
                            rusqlite::params![id, term.value, literal, result.session_id],
                            |row| row.get(0),
                        )
                        .optional()?;
                    if let Some(excerpt) = excerpt {
                        result.highlights = highlights(&excerpt, &c.positives);
                        result.excerpt = Some(excerpt);
                        break;
                    }
                }
            }
        }
    }
    Ok(results)
}
fn highlights(text: &str, terms: &[(usize, Term)]) -> Vec<HighlightSpan> {
    // Scalar-based lowercase keeps offsets valid even when Unicode case folds
    // expand (e.g. İ). Accents absent from literal excerpt matches are left plain.
    use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
    let fold = |c: char, literal: bool| {
        if literal {
            c.to_lowercase().collect::<String>()
        } else {
            c.to_string()
                .nfd()
                .filter(|c| !is_combining_mark(*c))
                .flat_map(char::to_lowercase)
                .collect::<String>()
        }
    };
    let chars: Vec<char> = text.chars().collect();
    let folded = chars.iter().map(|c| fold(*c, false)).collect::<Vec<_>>();
    let literal_chars = chars.iter().map(|c| fold(*c, true)).collect::<Vec<_>>();
    let mut spans = Vec::new();
    for (_, term) in terms {
        let literal = query::is_literal(&term.value);
        let needle = term
            .value
            .chars()
            .map(|c| fold(c, literal))
            .collect::<Vec<_>>();
        let haystack = if literal { &literal_chars } else { &folded };
        if needle.is_empty() || needle.len() > haystack.len() {
            continue;
        }
        for (start, window) in haystack.windows(needle.len()).enumerate() {
            if window == needle {
                spans.push(HighlightSpan {
                    start,
                    end: start + needle.len(),
                });
            }
        }
    }
    spans.sort_by_key(|s| (s.start, s.end));
    let mut merged: Vec<HighlightSpan> = Vec::new();
    for span in spans {
        if let Some(last) = merged.last_mut() {
            if span.start <= last.end {
                last.end = last.end.max(span.end);
                continue;
            }
        }
        merged.push(span);
    }
    merged
}

/// No temporary tables or content writes: safe visible-text matching is a pure
/// SQL function. SQLite caches the compiled constant needle with auxiliary data;
/// input is streamed and memory is bounded by the 4096-byte query.
pub(super) fn register_visible_match(conn: &Connection) -> rusqlite::Result<()> {
    use rusqlite::functions::FunctionFlags;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    conn.create_scalar_function(
        "_hession_visible_match_v1",
        3,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        move |ctx| {
            let text = ctx.get_raw(0).as_str().unwrap_or("");
            let literal = ctx.get::<i64>(2)? != 0;
            let cached = ctx.get_aux::<MatchPattern>(1)?;
            let pattern = if let Some(cached) = cached.filter(|p| p.literal == literal) {
                cached
            } else {
                ctx.set_aux(
                    1,
                    MatchPattern::new(ctx.get_raw(1).as_str().unwrap_or(""), literal),
                )?
            };
            pattern
                .offset(text, deadline)
                .map(|p| p.map_or(0, |p| p as i64 + 1))
                .map_err(|()| {
                    rusqlite::Error::UserFunctionError(Box::new(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "HESSION_SEARCH_TIME_BUDGET",
                    )))
                })
        },
    )
}
struct MatchPattern {
    literal: bool,
    chars: Vec<char>,
    words: Vec<String>,
    prefix: Vec<usize>,
    max_chars: usize,
    ascii_literal: Option<Vec<u8>>,
}
fn prefix<T: Eq>(pattern: &[T]) -> Vec<usize> {
    let mut prefix = vec![0; pattern.len()];
    for i in 1..pattern.len() {
        let mut j = prefix[i - 1];
        while j > 0 && pattern[i] != pattern[j] {
            j = prefix[j - 1];
        }
        if pattern[i] == pattern[j] {
            j += 1;
        }
        prefix[i] = j;
    }
    prefix
}
impl MatchPattern {
    fn new(needle: &str, literal: bool) -> Self {
        use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
        if literal {
            let chars = needle
                .chars()
                .flat_map(char::to_lowercase)
                .collect::<Vec<_>>();
            let prefix = prefix(&chars);
            let ascii_literal = chars
                .iter()
                .all(char::is_ascii)
                .then(|| chars.iter().map(|c| *c as u8).collect());
            Self {
                literal,
                chars,
                words: Vec::new(),
                prefix,
                max_chars: 0,
                ascii_literal,
            }
        } else {
            let words = needle
                .split(|c: char| !c.is_alphanumeric() && !is_combining_mark(c))
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.nfd()
                        .filter(|c| !is_combining_mark(*c))
                        .flat_map(char::to_lowercase)
                        .collect::<String>()
                })
                .collect::<Vec<_>>();
            let prefix = prefix(&words);
            let max_chars = words.iter().map(|s| s.chars().count()).max().unwrap_or(0);
            Self {
                literal,
                chars: Vec::new(),
                words,
                prefix,
                max_chars,
                ascii_literal: None,
            }
        }
    }
    fn offset(&self, text: &str, deadline: std::time::Instant) -> Result<Option<usize>, ()> {
        use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
        let length = if self.literal {
            self.chars.len()
        } else {
            self.words.len()
        };
        if length == 0 {
            return Ok(None);
        }
        if let Some(pattern) = self.ascii_literal.as_ref().filter(|_| text.is_ascii()) {
            let mut matched = 0;
            for (index, byte) in text.bytes().enumerate() {
                if index.is_multiple_of(8192) && std::time::Instant::now() > deadline {
                    return Err(());
                }
                let byte = byte.to_ascii_lowercase();
                while matched > 0 && byte != pattern[matched] {
                    matched = self.prefix[matched - 1];
                }
                if byte == pattern[matched] {
                    matched += 1;
                }
                if matched == pattern.len() {
                    return Ok(Some(index + 1 - pattern.len()));
                }
            }
            return Ok(None);
        }
        let mut positions = std::collections::VecDeque::with_capacity(length);
        let mut j = 0;
        if self.literal {
            for (scalar, ch) in text
                .chars()
                .enumerate()
                .flat_map(|(i, c)| c.to_lowercase().map(move |c| (i, c)))
            {
                if scalar.is_multiple_of(8192) && std::time::Instant::now() > deadline {
                    return Err(());
                }
                positions.push_back(scalar);
                if positions.len() > length {
                    positions.pop_front();
                }
                while j > 0 && ch != self.chars[j] {
                    j = self.prefix[j - 1];
                }
                if ch == self.chars[j] {
                    j += 1;
                }
                if j == length {
                    return Ok(positions.front().copied());
                }
            }
        } else {
            for (index, word) in text
                .split(|c: char| !c.is_alphanumeric() && !is_combining_mark(c))
                .filter(|s| !s.is_empty())
                .enumerate()
            {
                if index.is_multiple_of(1024) && std::time::Instant::now() > deadline {
                    return Err(());
                }
                let byte = word.as_ptr() as usize - text.as_ptr() as usize;
                positions.push_back(byte);
                if positions.len() > length {
                    positions.pop_front();
                }
                let normalized = if word.is_ascii() {
                    None
                } else {
                    Some(
                        word.nfd()
                            .filter(|c| !is_combining_mark(*c))
                            .flat_map(char::to_lowercase)
                            .take(self.max_chars + 1)
                            .collect::<String>(),
                    )
                };
                let equal = |p: &str| {
                    normalized
                        .as_ref()
                        .map_or_else(|| word.eq_ignore_ascii_case(p), |w| w == p)
                };
                while j > 0 && !equal(&self.words[j]) {
                    j = self.prefix[j - 1];
                }
                if equal(&self.words[j]) {
                    j += 1;
                }
                if j == length {
                    return Ok(positions.front().map(|byte| text[..*byte].chars().count()));
                }
            }
        }
        Ok(None)
    }
}
