use crate::{mcp, CliError};
use clap::ArgMatches;
use hession_core::{
    indexing::DeltaCursor,
    projects::ProjectSelector,
    reader::{ReadCancellation, ReadPage, ScopedReader},
    search::{SearchFilters, SearchRequest, VISIBLE_CONVERSATION_KINDS},
    service::Library,
};
use serde_json::{json, Value};
use std::{
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

pub(crate) fn terminal_safe(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if (c.is_control() && c != '\n' && c != '\t')
                || matches!(c,'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}')
            {
                format!("\\u{{{:x}}}", c as u32).chars().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}
pub(crate) fn output(value: &Value, json_output: bool) -> Result<(), CliError> {
    let text = if json_output {
        serde_json::to_string(value)?
    } else {
        terminal_safe(&serde_json::to_string_pretty(value)?)
    };
    if text.len() > hession_core::reader::MAX_READ_BYTES {
        return Err(CliError::new(
            5,
            "response_limit",
            "Output exceeds 1 MiB; request a smaller page",
        ));
    }
    let mut out = io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}
pub(crate) fn selector(value: &str) -> Result<ProjectSelector, CliError> {
    if let Some(id) = value
        .strip_prefix("workspace:")
        .or_else(|| value.strip_prefix("id:"))
    {
        let id = id.parse::<i64>().map_err(|_| {
            CliError::new(
                2,
                "project",
                "Use workspace:POSITIVE_ID or an exact indexed path",
            )
        })?;
        if id <= 0 {
            return Err(CliError::new(
                2,
                "project",
                "Workspace IDs must be positive",
            ));
        }
        Ok(ProjectSelector::WorkspaceId(id))
    } else {
        Ok(ProjectSelector::RecordedPath(value.to_owned()))
    }
}
fn reader(
    library: Library,
    project: Option<&String>,
    expose: bool,
) -> Result<ScopedReader, CliError> {
    match project {
        Some(path) => Ok(ScopedReader::projects(library, &[selector(path)?], expose)?),
        None => Ok(ScopedReader::unrestricted(library, expose)?),
    }
}
fn page(args: &ArgMatches) -> ReadPage {
    ReadPage {
        limit: *args.get_one::<u32>("limit").unwrap_or(&20),
        cursor: args.get_one::<String>("cursor").cloned(),
    }
}
fn project_id(reader: &ScopedReader) -> Result<i64, CliError> {
    reader
        .scope()
        .workspace_ids()
        .and_then(|ids| ids.first())
        .copied()
        .ok_or_else(|| CliError::new(4, "project", "Choose one explicit indexed project"))
}
fn render_search(value: &Value, json_output: bool) -> Result<(), CliError> {
    if json_output {
        return output(value, true);
    }
    let mut out = io::stdout().lock();
    for row in value["results"].as_array().into_iter().flatten() {
        let title = row["title"].as_str().unwrap_or("Untitled session");
        let agent = row["agent"].as_str().unwrap_or("");
        writeln!(
            out,
            "{}\t{}\t{}",
            row["sessionId"],
            terminal_safe(agent),
            terminal_safe(title)
        )?;
        if let Some(text) = row["excerpt"].as_str() {
            writeln!(out, "  {}", terminal_safe(text))?;
        }
    }
    if let Some(cursor) = value["nextCursor"].as_str() {
        writeln!(out, "Next cursor: '{cursor}'")?;
    }
    Ok(())
}
fn render_session(value: &Value, format: &str) -> Result<(), CliError> {
    if format == "json" {
        return output(value, true);
    }
    let mut out = io::stdout().lock();
    let title = value["session"]["title"]
        .as_str()
        .unwrap_or("Untitled session");
    writeln!(
        out,
        "{}{}",
        if format == "markdown" { "# " } else { "" },
        terminal_safe(title)
    )?;
    for entry in value["entries"].as_array().into_iter().flatten() {
        writeln!(
            out,
            "\n{}{} (entry {})\n",
            if format == "markdown" { "## " } else { "" },
            terminal_safe(entry["role"].as_str().unwrap_or("message")),
            entry["id"]
        )?;
        writeln!(
            out,
            "{}",
            terminal_safe(entry["text"].as_str().unwrap_or(""))
        )?;
    }
    if let Some(notice) = value["notice"].as_str() {
        writeln!(out, "\n{}", terminal_safe(notice))?;
    }
    if let Some(cursor) = value["nextCursor"].as_str() {
        writeln!(out, "Next cursor: '{cursor}'")?;
    }
    Ok(())
}

pub(crate) async fn execute(
    args: &ArgMatches,
    path: PathBuf,
    json_output: bool,
) -> Result<i32, CliError> {
    let library = Library::new(path);
    let cancel = ReadCancellation::default();
    match args.subcommand() {
        Some(("status", _)) => {
            let data = ScopedReader::unrestricted(library, false)?.status(&cancel)?;
            output(&data, json_output)?;
        }
        Some(("search", command)) => {
            let policy = reader(
                library,
                command.get_one("project"),
                !command.get_flag("metadata-only"),
            )?;
            let page = page(command);
            let request = SearchRequest {
                query: command.get_one::<String>("query").unwrap().clone(),
                limit: page.limit,
                cursor: page.cursor,
                ..Default::default()
            };
            let result = if command.get_flag("metadata-only") {
                policy.search(&request, &cancel)?
            } else {
                policy.search_indexed(&request, &cancel)?
            };
            render_search(&serde_json::to_value(result)?, json_output)?;
        }
        Some(("sessions", commands)) => match commands.subcommand() {
            Some(("list", command)) => {
                let policy = reader(library, command.get_one("project"), false)?;
                let page = page(command);
                let result = policy.search(
                    &SearchRequest {
                        limit: page.limit,
                        cursor: page.cursor,
                        filters: SearchFilters {
                            agent: command
                                .get_one::<String>("agent")
                                .cloned()
                                .unwrap_or_default(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    &cancel,
                )?;
                render_search(&serde_json::to_value(result)?, json_output)?;
            }
            Some(("show", command)) => {
                let policy = reader(
                    library,
                    command.get_one("project"),
                    !command.get_flag("metadata-only"),
                )?;
                let value = policy.session(
                    *command.get_one::<i64>("id").unwrap(),
                    &page(command),
                    &cancel,
                )?;
                render_session(
                    &value,
                    if json_output {
                        "json"
                    } else {
                        command.get_one::<String>("format").unwrap()
                    },
                )?;
            }
            Some(("follow", command)) => {
                return follow(
                    library,
                    *command.get_one::<i64>("id").unwrap(),
                    *command.get_one::<u32>("limit").unwrap(),
                    json_output || command.get_flag("jsonl"),
                )
                .await
            }
            _ => {
                return Err(CliError::new(
                    2,
                    "usage",
                    "Choose sessions list, show or follow",
                ))
            }
        },
        Some(("projects", commands)) => {
            let command = commands.subcommand_matches("list").unwrap();
            let policy = ScopedReader::unrestricted(library, false)?;
            output(&policy.projects_page(&page(command), &cancel)?, json_output)?;
        }
        Some(("project", commands)) => {
            let command = commands.subcommand_matches("show").unwrap();
            let policy = reader(library, command.get_one("project"), false)?;
            output(
                &serde_json::to_value(policy.project(project_id(&policy)?, &cancel)?)?,
                json_output,
            )?;
        }
        Some(("files", commands)) => match commands.subcommand() {
            Some(("history", command)) => {
                let policy = reader(library, command.get_one("project"), false)?;
                output(
                    &policy.project_activity(
                        project_id(&policy)?,
                        Some(command.get_one::<String>("path").unwrap()),
                        &page(command),
                        &cancel,
                    )?,
                    json_output,
                )?;
            }
            Some(("list", command)) => {
                let policy = reader(library, command.get_one("project"), false)?;
                output(
                    &policy.project_files(
                        project_id(&policy)?,
                        command.get_one::<String>("filter").map(String::as_str),
                        &page(command),
                        &cancel,
                    )?,
                    json_output,
                )?;
            }
            _ => return Err(CliError::new(2, "usage", "Choose files list or history")),
        },
        Some(("export", commands)) => {
            let command = commands.subcommand_matches("project").unwrap();
            return export_project(library, command, json_output).await;
        }
        Some(("mcp", commands)) => {
            let command = commands.subcommand_matches("serve").unwrap();
            let selectors = command
                .get_many::<String>("project")
                .unwrap()
                .map(|s| selector(s))
                .collect::<Result<Vec<_>, _>>()?;
            mcp::serve(library, &selectors, command.get_flag("expose-transcripts")).await?;
        }
        _ => {
            return Err(CliError::new(
                2,
                "usage",
                "Choose an explicit headless command; use --help",
            ))
        }
    }
    Ok(0)
}

pub(crate) async fn shutdown_signal() -> i32 {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut interrupt = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(_) => return 130,
        };
        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => return 143,
        };
        tokio::select! {_=interrupt.recv()=>130,_=terminate.recv()=>143}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        130
    }
}
async fn follow(library: Library, id: i64, limit: u32, json_output: bool) -> Result<i32, CliError> {
    let policy = ScopedReader::unrestricted(library.clone(), true)?;
    policy.session(
        id,
        &ReadPage {
            limit: 1,
            cursor: None,
        },
        &ReadCancellation::default(),
    )?;
    let mut cursor: Option<DeltaCursor> = None;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        let source = library.clone();
        let after = cursor.clone();
        let work = tokio::task::spawn_blocking(move || source.session_delta(id, after, limit));
        let value = tokio::select! {
            signal=&mut shutdown=>return Ok(signal),
            result=work=>result.map_err(|_|CliError::new(3,"worker","Delta reader failed"))?.map_err(|e|CliError::new(3,"library",format!("{e}; durable follow requires an indexed current-schema library")))?,
        };
        if value["revision"]["identity"].as_str() != Some(policy.identity()) {
            return Err(CliError::new(
                4,
                "library_changed",
                "The library was replaced; restart follow with a reviewed session ID",
            ));
        }
        cursor = Some(serde_json::from_value(value["cursor"].clone())?);
        let entries = value["entries"]
            .as_array()
            .into_iter()
            .flatten()
            .map(follow_entry)
            .collect::<Vec<_>>();
        if !entries.is_empty() || value["reset"] == true || value["deleted"] == true {
            let envelope = json!({"revision":value["revision"],"cursor":value["cursor"],"reset":value["reset"],"deleted":value["deleted"],"entries":entries,"notice":"Recorded historical updates only; not proof an agent process is running. Raw tool and provider payloads are excluded."});
            tokio::select! {signal=&mut shutdown=>return Ok(signal),result=crate::write_output_async(envelope,json_output)=>result?}
        }
        if value["deleted"] == true {
            return Ok(0);
        }
        if value["hasMore"] != true {
            tokio::select! {signal=&mut shutdown=>return Ok(signal),_=tokio::time::sleep(Duration::from_millis(500))=>{}}
        }
    }
}
fn follow_entry(entry: &Value) -> Value {
    let role = entry["role"].as_str().unwrap_or("");
    let kind = entry["kind"].as_str().unwrap_or("");
    let visible =
        matches!(role, "user" | "assistant") && VISIBLE_CONVERSATION_KINDS.contains(&kind);
    let text = if visible {
        entry["parts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["kind"] == "text")
            .filter_map(|p| p["text"].as_str())
            .take(16)
            .collect::<Vec<_>>()
            .join("\n")
            .chars()
            .take(2048)
            .collect::<String>()
    } else {
        String::new()
    };
    let tools=entry["tools"].as_array().into_iter().flatten().map(|t|json!({"id":t["id"],"name":t["tool_name"].as_str().unwrap_or("").chars().take(128).collect::<String>(),"status":t["status"].as_str().map(|s|s.chars().take(64).collect::<String>()),"exitCode":t["exit_code"]})).collect::<Vec<_>>();
    json!({"id":entry["id"],"ordinal":entry["ordinal"],"role":role.chars().take(32).collect::<String>(),"kind":kind.chars().take(128).collect::<String>(),"text":text,"tools":tools})
}
async fn export_project(
    library: Library,
    command: &ArgMatches,
    json_output: bool,
) -> Result<i32, CliError> {
    use hession_core::project_export::{
        self, ExportCancellation, ExportOptions, ProjectExportRequest,
    };
    let policy = reader(library.clone(), command.get_one("project"), true)?;
    let id = project_id(&policy)?;
    let project = policy.project(id, &ReadCancellation::default())?;
    let request = ProjectExportRequest {
        workspace_id: id,
        query: command.get_one::<String>("query").unwrap().clone(),
        filters: SearchFilters {
            project: project.recorded_path.clone(),
            ..Default::default()
        },
        options: ExportOptions {
            include_statistics: command.get_flag("include-statistics"),
            include_tool_bodies: command.get_flag("include-tool-bodies"),
            include_absolute_paths: command.get_flag("include-absolute-paths"),
            include_git_metadata: command.get_flag("include-git-metadata"),
            redact: !command.get_flag("no-redact"),
            ..Default::default()
        },
        ..Default::default()
    };
    let destination = command.get_one::<PathBuf>("output").unwrap().clone();
    let preview_only = command.get_flag("preview");
    let cancel = ExportCancellation::default();
    let stop = cancel.clone();
    let scope = policy.scope().clone();
    let identity = policy.identity().to_owned();
    let mut work = tokio::task::spawn_blocking(move || -> Result<Value, CliError> {
        let prepared = project_export::prepare(&library, &request, &scope, &destination, &stop)
            .map_err(export_error)?;
        if prepared.preview().project_key != project.project_key
            || prepared.preview().revision.identity != identity
        {
            return Err(CliError::new(
                4,
                "scope_changed",
                "Project identity changed while preparing export; resolve it again",
            ));
        }
        if preview_only {
            return Ok(serde_json::to_value(prepared.preview())?);
        }
        let result = project_export::write(&library, &prepared, &scope, &stop, |_| {})
            .map_err(export_error)?;
        Ok(serde_json::to_value(result)?)
    });
    let result = tokio::select! {
        signal=shutdown_signal()=>{
            if !cancel.cancel(){
                let completed=(&mut work).await.map_err(|_|CliError::new(3,"worker","Export worker failed"))??;
                output(&completed,json_output)?;return Ok(0);
            }
            let cleanup=tokio::time::timeout(Duration::from_secs(5),&mut work).await;
            if cleanup.is_err(){return Err(CliError::new(6,"cancelled","Export cancellation is still finishing; no successful archive was reported"));}
            return Ok(signal);
        },
        result=&mut work=>result.map_err(|_|CliError::new(3,"worker","Export worker failed"))??,
    };
    output(&result, json_output)?;
    Ok(0)
}
fn export_error(error: hession_core::project_export::ExportError) -> CliError {
    use hession_core::project_export::ExportError;
    let (code, kind) = match &error {
        ExportError::Cancelled => (130, "cancelled"),
        ExportError::StalePreview => (5, "stale_preview"),
        ExportError::Limit(_) => (5, "export_limit"),
        ExportError::Invalid(_) => (2, "invalid_export"),
        _ => (3, "export"),
    };
    CliError::new(code, kind, error.to_string())
}
