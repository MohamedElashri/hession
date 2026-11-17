use super::{
    transcript::{cell, entry_id, tool_id, Metadata},
    *,
};
use crate::projects::EvidenceCategory;
use rusqlite::params;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Seek, SeekFrom},
};
use zip::{write::SimpleFileOptions, ZipWriter};

struct GuardedFile<'a> {
    file: &'a mut File,
    work: &'a Work<'a>,
}
impl Write for GuardedFile<'_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.work.check().map_err(io::Error::other)?;
        let length = data.len().min(64 * 1024);
        if self.file.stream_position()?.saturating_add(length as u64)
            > self.work.limits.max_archive_bytes
        {
            return Err(io::Error::other("Compressed archive exceeds byte bound"));
        }
        self.file.write(&data[..length])
    }
    fn flush(&mut self) -> io::Result<()> {
        self.work.check().map_err(io::Error::other)?;
        self.file.flush()
    }
}
impl Seek for GuardedFile<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.work.check().map_err(io::Error::other)?;
        let target = match position {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => self.file.stream_position()? as i128 + n as i128,
            SeekFrom::End(n) => self.file.metadata()?.len() as i128 + n as i128,
        };
        if target < 0 || target > self.work.limits.max_archive_bytes as i128 {
            return Err(io::Error::other("Archive seek exceeds byte bound"));
        }
        self.file.seek(SeekFrom::Start(target as u64))
    }
}
type Archive<'a> = ZipWriter<GuardedFile<'a>>;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Member {
    path: String,
    sha256: String,
    uncompressed_bytes: u64,
}
struct MemberOutput<'a, 'b> {
    zip: &'a mut Archive<'b>,
    work: &'a Work<'b>,
    total: &'a mut u64,
    hash: Sha256,
    bytes: u64,
}
impl Write for MemberOutput<'_, '_> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.work.check().map_err(io::Error::other)?;
        let n = data.len().min(64 * 1024);
        if self.bytes + n as u64 > self.work.limits.max_member_bytes
            || self.total.saturating_add(n as u64) > self.work.limits.max_archive_bytes
        {
            return Err(io::Error::other(
                "Uncompressed archive or member exceeds byte bound",
            ));
        }
        let n = self.zip.write(&data[..n])?;
        self.hash.update(&data[..n]);
        self.bytes += n as u64;
        *self.total += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.zip.flush()
    }
}
fn member<'work>(
    zip: &mut Archive<'work>,
    work: &Work<'work>,
    total: &mut u64,
    names: &mut BTreeSet<String>,
    name: &str,
    write: impl FnOnce(&mut dyn Write) -> Result<(), ExportError>,
) -> Result<Member, ExportError> {
    if name.starts_with('/')
        || name.contains('\\')
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._/".contains(&b))
        || name
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || !names.insert(name.into())
    {
        return Err(ExportError::Invalid(
            "Invalid or duplicate archive member name".into(),
        ));
    }
    if names.len() > work.limits.max_sessions as usize * 2 + 5 {
        return Err(ExportError::Limit("archive member count"));
    }
    zip.start_file(
        name,
        SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(3))
            .unix_permissions(0o600),
    )?;
    let mut output = MemberOutput {
        zip,
        work,
        total,
        hash: Sha256::new(),
        bytes: 0,
    };
    {
        let mut buffered = std::io::BufWriter::with_capacity(64 * 1024, &mut output);
        write(&mut buffered)?;
        buffered.flush()?;
    }
    Ok(Member {
        path: name.into(),
        sha256: format!("{:x}", output.hash.finalize()),
        uncompressed_bytes: output.bytes,
    })
}
fn array_item(output: &mut dyn Write, first: &mut bool, value: &Value) -> Result<(), ExportError> {
    if !*first {
        output.write_all(b",")?;
    }
    *first = false;
    serde_json::to_writer(output, value)?;
    Ok(())
}
fn relative_path(
    plan: &PreparedProjectExport,
    reported: &str,
    cwd: Option<&str>,
) -> Option<String> {
    let root = if let Some(host) = &plan.project.host {
        plan.project
            .recorded_path
            .strip_prefix(&format!("{host}:"))
            .unwrap_or(&plan.project.recorded_path)
    } else {
        &plan.project.recorded_path
    };
    projects::relative_candidate(reported, cwd, root, false)
}
fn source(session: &SessionPlan, ordinal: Option<i64>, tool: Option<String>) -> Value {
    json!({"sessionId":session.portable_id,"snapshotId":session.snapshot_id,"entryId":ordinal.map(|o|entry_id(&session.portable_id,o)),"entryOrdinal":ordinal,"toolInvocationId":tool})
}
fn files(
    work: &Work<'_>,
    plan: &PreparedProjectExport,
    output: &mut dyn Write,
) -> Result<(), ExportError> {
    output.write_all(b"{\"schemaVersion\":1,\"evidence\":[")?;
    let mut first = true;
    let mut count = 0;
    let mut privacy = Sanitizer::new(&plan.request.options);
    for session in &plan.sessions {
        let cwd = cell(
            work,
            "session",
            "cwd",
            session.local_id,
            work.limits.max_cell_bytes,
        )?;
        let mut after = 0;
        loop {
            work.check()?;
            let mut stmt=work.conn.prepare(&format!("SELECT f.id,{},{},{},{},{} FROM file_activity f INDEXED BY file_session_idx LEFT JOIN entry e ON e.id=f.entry_id AND e.session_id=f.session_id WHERE f.session_id=?1 AND f.id>?2 ORDER BY f.id LIMIT 100",Metadata::EntryOrdinal.sql(),Metadata::FileToolId.sql(),Metadata::Action.sql(),Metadata::Occurred.sql(),Metadata::Evidence.sql()))?;
            let rows = stmt
                .query_map(params![session.local_id, after], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            if rows.is_empty() {
                break;
            }
            for (id, ordinal, tool, action, occurred, evidence) in rows {
                after = id;
                count += 1;
                if count > work.limits.max_activity_rows {
                    return Err(ExportError::Limit("file evidence rows"));
                }
                let reported = cell(
                    work,
                    "file_activity",
                    "path",
                    id,
                    work.limits.max_cell_bytes,
                )?
                .unwrap_or_default();
                let candidate = relative_path(plan, &reported, cwd.as_deref());
                let path = if !plan.request.options.include_absolute_paths {
                    candidate.clone().unwrap_or_else(|| privacy.text(&reported))
                } else {
                    privacy.text(&reported)
                };
                let category = match action.as_deref() {
                    Some("read") => EvidenceCategory::ReportedRead,
                    Some("write" | "edit" | "patch" | "delete" | "create" | "rename") => {
                        EvidenceCategory::ReportedWriteIntent
                    }
                    _ => EvidenceCategory::ReportedReference,
                };
                let tool = tool
                    .map(|id| tool_id(work.conn, &session.portable_id, session.local_id, id))
                    .transpose()?
                    .flatten();
                let mut row = json!({"source":source(session,ordinal,tool),"path":path,"candidateWorkspaceRelativePath":candidate,"resolution":if candidate.is_some(){"lexical_candidate_unverified"}else{"unresolved_recorded_path"},"category":category,"occurredAt":occurred,"evidence":evidence});
                privacy.value(&mut row)?;
                array_item(output, &mut first, &row)?;
            }
        }
    }
    output.write_all(b"]}\n")?;
    Ok(())
}
fn git(
    work: &Work<'_>,
    plan: &PreparedProjectExport,
    output: &mut dyn Write,
) -> Result<(), ExportError> {
    output.write_all(b"{\"schemaVersion\":1,\"kind\":\"recorded_context_only\",\"contexts\":[")?;
    let mut first = true;
    let mut count = 0;
    let mut privacy = Sanitizer::new(&plan.request.options);
    for session in &plan.sessions {
        let mut after = 0;
        loop {
            work.check()?;
            let mut stmt=work.conn.prepare(&format!("SELECT c.id,{},{},{} FROM session_context c INDEXED BY context_session_latest_idx LEFT JOIN entry e ON e.id=c.entry_id AND e.session_id=c.session_id WHERE c.session_id=?1 AND c.id>?2 ORDER BY c.id LIMIT 100",Metadata::EntryOrdinal.sql(),Metadata::Observed.sql(),Metadata::RepositoryId.sql()))?;
            let rows = stmt
                .query_map(params![session.local_id, after], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            if rows.is_empty() {
                break;
            }
            for (id, ordinal, observed, repository) in rows {
                after = id;
                count += 1;
                if count > work.limits.max_activity_rows {
                    return Err(ExportError::Limit("recorded Git rows"));
                }
                let mut row = json!({"source":source(session,ordinal,None),"observedAt":observed,"branch":cell(work,"session_context","branch",id,work.limits.max_cell_bytes)?,"commitSha":cell(work,"session_context","commit_sha",id,work.limits.max_cell_bytes)?});
                if plan.request.options.include_absolute_paths {
                    row["cwd"] = serde_json::to_value(cell(
                        work,
                        "session_context",
                        "cwd",
                        id,
                        work.limits.max_cell_bytes,
                    )?)?;
                    if let Some(repository) = repository {
                        row["commonDirectory"] = serde_json::to_value(cell(
                            work,
                            "repository",
                            "common_dir",
                            repository,
                            work.limits.max_cell_bytes,
                        )?)?;
                    }
                }
                // Remote URLs are intentionally never exported: credentials can hide outside userinfo.
                privacy.value(&mut row)?;
                array_item(output, &mut first, &row)?;
            }
        }
    }
    output.write_all(b"]}\n")?;
    Ok(())
}
const README:&str="# Hession project history\n\nThis ZIP is a portable reading archive, not a restorable SQLite backup or a repository checkout. No source files, private reasoning, system/developer messages, raw event payloads, attachments or remote credentials are copied.\n\nmanifest.json records schema version 1, options, selection, portable session identities and sanitized-content snapshot identities. Session member names are generated hashes. Entry IDs use portable session identity plus recorded ordinal. File evidence references the matching snapshot, session, entry and tool identity; it records reported reads and write intent, not verified agent modifications. Optional Git data contains recorded context only; exporting never runs Git or contacts a remote.\n\nMarkdown/JSON transcripts use Hession's shared streaming formatter. Tool names/status can be included while tool bodies remain off by default. Statistics are optional existing cached estimates. Absolute metadata paths are off by default; freeform text is masked with conservative path patterns. Pattern redaction cannot find every secret or encoded path: inspect all members before sharing.\n\nSnapshot IDs hash the sanitized canonical JSON transcript with its export timestamp excluded. Identical sanitized snapshots share a member; distinct snapshots of one logical session retain its portable identity.

The manifest lists SHA-256 checksums of uncompressed members written before it. manifest.json excludes itself from that list to avoid a circular checksum. ZIP CRCs cover every member. This archive is not encrypted.\n";

fn publish_new(temporary: &Path, target: &Path) -> Result<(), ExportError> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let temporary = CString::new(temporary.as_os_str().as_bytes())
        .map_err(|_| ExportError::Invalid("Invalid temporary archive path".into()))?;
    let target = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| ExportError::Invalid("Invalid archive destination".into()))?;
    // SAFETY: both C strings live through this call. Atomic exclusive rename cannot follow or
    // overwrite an existing destination. The source is our fully synced same-directory tempfile.
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            temporary.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    let result =
        unsafe { libc::renamex_np(temporary.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

pub(super) fn write_archive(
    library: &Library,
    plan: &PreparedProjectExport,
    scope: &SearchScope,
    cancel: &ExportCancellation,
    progress: impl Fn(ExportProgress),
) -> Result<ExportResult, ExportError> {
    if library.path().canonicalize()? != plan.database
        || !scope.allows_workspace(plan.request.workspace_id)
    {
        return Err(ExportError::Invalid(
            "The prepared export belongs to another library or project scope".into(),
        ));
    }
    let target = destination(library.path(), &plan.destination)?;
    let parent = target
        .parent()
        .ok_or_else(|| ExportError::Invalid("Invalid archive destination".into()))?;
    let (mut conn, deadline) = open(library, cancel, plan.request.limits)?;
    let tx = conn.transaction()?;
    if library_revision::current(&tx)? != plan.preview.revision {
        return Err(ExportError::StalePreview);
    }
    projects::project_on_connection(&tx, plan.request.workspace_id, scope)?;
    let exported_at = tx.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ','now')", [], |r| {
        r.get::<_, String>(0)
    })?;
    let work = Work {
        conn: &tx,
        cancel,
        limits: plan.request.limits,
        deadline,
    };
    work.check()?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".hession-project-export-")
        .suffix(".tmp")
        .tempfile_in(parent)?;
    let mut total = 0;
    let mut names = BTreeSet::new();
    let mut members = Vec::new();
    let mut manifest_sessions = Vec::new();
    let mut unique = BTreeSet::new();
    let mut completed = 0;
    let mut entries = 0;
    {
        let mut zip = ZipWriter::new(GuardedFile {
            file: temporary.as_file_mut(),
            work: &work,
        });
        members.push(member(
            &mut zip,
            &work,
            &mut total,
            &mut names,
            "README.md",
            |out| {
                out.write_all(README.as_bytes())?;
                Ok(())
            },
        )?);
        for session in &plan.sessions {
            work.check()?;
            if !unique.insert(session.snapshot_id.clone()) {
                continue;
            }
            let mut paths = Vec::new();
            for format in match plan.request.options.format {
                TranscriptFormat::Markdown => &["markdown"][..],
                TranscriptFormat::Json => &["json"][..],
                TranscriptFormat::Both => &["markdown", "json"][..],
            } {
                let extension = if *format == "json" { "json" } else { "md" };
                let path = format!("sessions/{}.{extension}", session.snapshot_id);
                paths.push(path.clone());
                members.push(member(
                    &mut zip,
                    &work,
                    &mut total,
                    &mut names,
                    &path,
                    |out| {
                        let mut privacy = Sanitizer::new(&plan.request.options);
                        transcript::write_transcript(
                            &work,
                            session.local_id,
                            &session.portable_id,
                            transcript::TranscriptStyle {
                                format,
                                exported_at: &exported_at,
                            },
                            out,
                            &mut privacy,
                            session.statistics.as_ref(),
                        )?;
                        Ok(())
                    },
                )?);
            }
            entries += session.exported_entries;
            completed += 1;
            manifest_sessions.push(json!({"portableSessionId":session.portable_id,"snapshotId":session.snapshot_id,"members":paths,"entryCount":session.exported_entries,"canonicalJsonBytes":session.canonical_bytes,"identityBasis":session.identity_basis}));
            progress(ExportProgress {
                stage: ExportStage::Writing,
                completed_sessions: completed,
                total_sessions: plan.preview.session_count,
                entries,
            });
        }
        members.push(member(
            &mut zip,
            &work,
            &mut total,
            &mut names,
            "activity/files.json",
            |out| files(&work, plan, out),
        )?);
        if plan.request.options.include_git_metadata {
            members.push(member(
                &mut zip,
                &work,
                &mut total,
                &mut names,
                "activity/git.json",
                |out| git(&work, plan, out),
            )?);
        }
        if plan.request.options.include_statistics {
            let mut seen = BTreeSet::new();
            let mut statistics = BTreeMap::<String, f64>::new();
            let mut missing = 0;
            for session in &plan.sessions {
                if !seen.insert(&session.snapshot_id) {
                    continue;
                }
                if let Some(stats) = &session.statistics {
                    for (key, value) in stats.as_object().into_iter().flatten() {
                        if let Some(value) = value.as_f64() {
                            *statistics.entry(key.clone()).or_default() += value;
                        }
                    }
                } else {
                    missing += 1;
                }
            }
            members.push(member(&mut zip,&work,&mut total,&mut names,"analytics/summary.json",|out|{serde_json::to_writer(out,&json!({"schemaVersion":1,"kind":"cached_estimates","sessionCount":plan.preview.session_count,"missingStatisticsSessions":missing,"totals":statistics}))?;Ok(())})?);
        }
        let mut included = vec![
            "conversation_text",
            "tool_metadata",
            "reported_file_evidence",
        ];
        let mut omitted = vec![
            "source_files",
            "source_uris",
            "raw_payloads",
            "private_reasoning",
            "system_developer_messages",
            "attachments",
            "remote_credentials",
            "current_git_inspection",
        ];
        for (flag, category) in [
            (
                plan.request.options.include_tool_bodies,
                "tool_inputs_outputs",
            ),
            (
                plan.request.options.include_absolute_paths,
                "absolute_machine_paths",
            ),
            (
                plan.request.options.include_git_metadata,
                "recorded_git_context",
            ),
            (plan.request.options.include_statistics, "cached_statistics"),
        ] {
            if flag {
                included.push(category)
            } else {
                omitted.push(category)
            }
        }
        let mut project =
            json!({"key":plan.preview.project_key,"displayName":plan.preview.project_display_name});
        if plan.request.options.include_absolute_paths {
            project["recordedWorkspace"] = Value::String(plan.project.recorded_path.clone());
            project["host"] = serde_json::to_value(&plan.project.host)?;
        }
        let mut manifest = json!({"schemaVersion":1,"producerVersion":env!("CARGO_PKG_VERSION"),"producerName":crate::brand::NAME,"exportedAt":exported_at,"preparedAt":plan.exported_at,"project":project,"query":plan.request.query,"filters":plan.request.filters,"selectedSessionCount":plan.preview.selected_session_count,"sessionCount":plan.preview.session_count,"sessions":manifest_sessions,"pathsPolicy":if plan.request.options.include_absolute_paths{"recorded_absolute_paths_opted_in"}else{"metadata_absolute_paths_omitted_and_freeform_paths_masked"},"options":plan.request.options,"includedCategories":included,"omittedCategories":omitted,"warnings":plan.preview.warnings,"checksums":members,"checksumsExclude":["manifest.json"]});
        Sanitizer::new(&plan.request.options).value(&mut manifest)?;
        member(
            &mut zip,
            &work,
            &mut total,
            &mut names,
            "manifest.json",
            |out| {
                serde_json::to_writer(out, &manifest)?;
                Ok(())
            },
        )?;
        zip.finish()?;
    }
    work.check()?;
    tx.commit()?;
    temporary.as_file().sync_all()?;
    let archive_bytes = temporary.as_file().metadata()?.len();
    progress(ExportProgress {
        stage: ExportStage::Finalizing,
        completed_sessions: completed,
        total_sessions: plan.preview.session_count,
        entries,
    });
    let mut saved = cancel
        .0
        .saved
        .lock()
        .map_err(|_| ExportError::Invalid("Export finalization lock was poisoned".into()))?;
    cancel.check()?;
    // Atomic create-new publication prevents overwrite races, symlink following and source damage.
    publish_new(temporary.path(), &target)?;
    *saved = true;
    drop(saved);
    let mut warnings = plan.preview.warnings.clone();
    if let Err(error) = File::open(parent).and_then(|directory| directory.sync_all()) {
        warnings.push(format!(
            "Archive saved; directory durability sync was unavailable: {error}"
        ));
    }

    progress(ExportProgress {
        stage: ExportStage::Saved,
        completed_sessions: completed,
        total_sessions: plan.preview.session_count,
        entries,
    });
    Ok(ExportResult {
        destination: target,
        session_count: completed,
        archive_bytes,
        warnings,
    })
}
