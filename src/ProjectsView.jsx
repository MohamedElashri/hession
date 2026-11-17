import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, ChevronLeft, ChevronRight, FileCode2, Folder, GitBranch, Menu, RefreshCw, Search } from 'lucide-react';
import { api } from './api.js';
import './styles/projects.css';

const PAGE_SIZE = 20;
const evidenceLabels = {
  reported_reference: ['Reported reference', 'A path mentioned by a tool; access or modification has not been verified.'],
  reported_read: ['Reported read', 'The tool reported this read target; file access has not been independently verified.'],
  reported_write_intent: ['Reported write intent', 'A write or patch target is intent, even when a tool reports success.'],
  observed_worktree_change: ['Observed worktree change', 'Current local index or raw file bytes differ; no agent authorship is inferred.'],
  verified_commit_change: ['Verified commit change', 'A change verified in an available Git commit snapshot; no agent authorship is inferred.'],
};

function useRead(command, args, refreshKey = 0) {
  const [result, setResult] = useState(null);
  const [retry, setRetry] = useState(0);
  const key = JSON.stringify([command, args, refreshKey, retry]);
  useEffect(() => {
    let active = true;
    const [name, request] = JSON.parse(key);
    api(name, request).then(data => { if (active) setResult({ key, data }); })
      .catch(error => { if (active) setResult({ key, error: error.message }); });
    return () => { active = false; };
  }, [key]);
  return { data: result?.key === key ? result.data : null, error: result?.key === key ? result.error : null,
    loading: result?.key !== key, retry: () => setRetry(value => value + 1) };
}

function changeLabel(status) {
  if (status === 'missing') return 'Tracked file is currently missing';
  if (status === 'raw_bytes_differ') return 'Raw file bytes differ from the index';
  const staged = status.startsWith('staged_');
  const code = staged ? status.slice(7) : status;
  const label = { A: 'Added', M: 'Modified', D: 'Deleted', R: 'Renamed', C: 'Copied', T: 'File type changed', U: 'Unmerged' }[code[0]] || code;
  return `${staged ? 'Staged: ' : ''}${label}`;
}

function EvidenceLabel({ category }) {
  const [label, description] = evidenceLabels[category] || evidenceLabels.reported_reference;
  return <span className={`project-evidence project-evidence-${category}`} title={description}>{label}</span>;
}
function ReadState({ read, empty, children }) {
  if (read.loading) return <p role="status">Loading recorded history…</p>;
  if (read.error) return <div role="alert" className="project-error"><p>{read.error}</p><button type="button" onClick={read.retry}>Retry</button></div>;
  return empty ? <p className="project-empty">No recorded evidence matches this selection.</p> : children;
}
function Pages({ label, offset, hasMore, nextOffset, onChange, busy = false }) {
  return <nav className="project-pages" aria-label={label}>
    <button type="button" disabled={!offset || busy} aria-label={`Previous ${label}`} onClick={() => onChange(Math.max(0, offset - PAGE_SIZE))}><ChevronLeft size={14}/>Previous</button>
    <span>Page {Math.floor(offset / PAGE_SIZE) + 1}</span>
    <button type="button" disabled={nextOffset == null || busy} aria-label={`Next ${label}`} onClick={() => onChange(nextOffset)}>Next<ChevronRight size={14}/></button>
    {hasMore && nextOffset == null && <span>Result limit reached; narrow the selection.</span>}
  </nav>;
}
function SourceButton({ source, positions, onSelectSession }) {
  const position = source.entry_id != null ? positions?.[source.entry_id] : null;
  return <button type="button" className="project-source" onClick={() => onSelectSession(source.session_id, { entryId: position != null ? source.entry_id : null, position })}>
    {position != null ? `Open session ${source.session_id}, event ${source.entry_ordinal ?? source.entry_id}` : `Open session ${source.session_id}`}
  </button>;
}
function Activity({ workspaceId, reportedPath, refreshKey, onSelectSession }) {
  const [offset, setOffset] = useState(0);
  const command = reportedPath == null ? 'project_activity' : 'file_history';
  const read = useRead(command, { workspaceId, page: { limit: PAGE_SIZE, offset }, ...(reportedPath == null ? {} : { reportedPath }) }, refreshKey);
  const rows = read.data?.evidence || [];
  return <section className="project-card" aria-label="Source activity">
    <h2>{reportedPath == null ? 'Recent path references' : 'File history'}</h2>
    <p className="project-subtext">Reported paths link to recorded events. They do not prove a final file modification.</p>
    <ReadState read={read} empty={!rows.length}>
      <ul className="project-events">{rows.map(item => <li key={item.id}>
        <div><code>{item.reported_path}</code><EvidenceLabel category={item.category}/></div>
        <p>{item.agent_family}{item.tool_name && ` · ${item.tool_name}`}{item.occurred_at && ` · ${item.occurred_at}`}</p>
        {item.candidate_workspace_relative_path ? <p>Workspace path candidate: <code>{item.candidate_workspace_relative_path}</code> · unverified</p> : <p>Unresolved recorded path · no filesystem access</p>}
        <SourceButton source={item.source} positions={read.data?.entry_positions} onSelectSession={onSelectSession}/>
      </li>)}</ul>
    </ReadState>
    <Pages label="activity pages" offset={offset} hasMore={read.data?.has_more} nextOffset={read.data?.next_offset} onChange={setOffset} busy={read.loading}/>
  </section>;
}
function GitContext({ project, refreshKey, onSelectSession }) {
  const [offset, setOffset] = useState(0);
  const recorded = useRead('recorded_git', { workspaceId: project.workspace_id, page: { limit: PAGE_SIZE, offset } }, refreshKey);
  const [inspection, setInspection] = useState(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const localAvailable = !project.host && !project.ambiguous_host && project.local_session_count > 0;
  // This component is keyed by workspace: an unmounted checkout cannot paint another project's UI.
  const pending = useRef({ active: true, busy: false });
  useEffect(() => { const state = pending.current; state.active = true; return () => { state.active = false; }; }, []);
  const inspect = async () => {
    if (pending.current.busy) return;
    pending.current.busy = true;
    setBusy(true); setError('');
    try { const result = await api('inspect_git', { workspaceId: project.workspace_id }); if (pending.current.active) setInspection(result); }
    catch (failure) { if (pending.current.active) setError(failure.message); }
    finally { pending.current.busy = false; if (pending.current.active) setBusy(false); }
  };
  const rows = recorded.data?.contexts || [];
  return <section className="project-card" aria-label="Git context">
    <h2><GitBranch size={17}/>Recorded Git context</h2>
    <ReadState read={recorded} empty={!rows.length}>
      <ul className="project-events">{rows.map(item => <li key={item.context_id}>
        <p>Recorded branch: <code>{item.branch || 'not recorded'}</code> · Recorded commit: <code>{item.commit_sha || 'not recorded'}</code></p>
        <p>{item.observed_at || 'Observation time not recorded'}</p>
        <SourceButton source={item.source} positions={recorded.data?.entry_positions} onSelectSession={onSelectSession}/>
      </li>)}</ul>
    </ReadState>
    <Pages label="recorded Git pages" offset={offset} hasMore={recorded.data?.has_more} nextOffset={recorded.data?.next_offset} onChange={setOffset} busy={recorded.loading}/>
    <div className="project-git-inspection">
      <h3>Current local checkout</h3>
      <p className="project-subtext">Inspect explicitly to compare current local metadata with recorded context. Git inspection never contacts SSH or a Git remote.</p>
      <button type="button" disabled={!localAvailable || busy} onClick={inspect}><GitBranch size={14}/>{busy ? 'Inspecting local Git…' : 'Inspect local Git'}</button>
      {!localAvailable && <p>Local inspection is unavailable for remote, ambiguous or synced-only history.</p>}
      {error && <p role="alert" className="project-error">{error}</p>}
      {inspection && <div>
        <dl className="project-metadata"><dt>Observed at</dt><dd>{new Date(inspection.observed_at_unix_ms).toLocaleString()}</dd>
          <dt>Root</dt><dd><code>{inspection.canonical_root}</code></dd><dt>Worktree</dt><dd><code>{inspection.worktree_git_dir}</code></dd>
          <dt>Current branch</dt><dd>{inspection.detached ? 'Detached HEAD' : inspection.branch || 'No branch available'}</dd><dt>Current commit</dt><dd><code>{inspection.head || 'No commit available'}</code></dd></dl>
        {rows[0] && ((rows[0].branch && rows[0].branch !== inspection.branch) || (rows[0].commit_sha && rows[0].commit_sha !== inspection.head)) && <p className="project-mismatch">Current Git metadata differs from the latest displayed recorded context.</p>}
        {!inspection.worktree_scan_complete && <p role="status">Tracked-file inspection is incomplete: {inspection.inspected_tracked_paths} examined, {inspection.skipped_paths} skipped{inspection.worktree_scan_truncated ? ', path limit reached' : ''}.</p>}
        <p className="project-subtext">Raw bytes are compared with index blobs. Attribute conversions, clean filters, untracked files, symlink targets and submodule working trees are excluded. No commit is attributed to an agent.</p>
        <ul className="project-events">{inspection.changes.map((change, index) => <li key={`${change.commit_sha}:${change.path.bytes_hex}:${index}`}>
          <div><EvidenceLabel category={change.category}/><code>{change.previous_path && `${change.previous_path.display} → `}{change.path.display}</code></div><p>{changeLabel(change.status)}{change.commit_sha && ` · ${change.commit_sha}`}</p>
        </li>)}</ul>
        {!inspection.changes.length && <p>No differences were found within the inspected bounds.</p>}
      </div>}
    </div>
  </section>;
}
function ProjectDetails({ project, refreshKey, onSelectSession }) {
  const [filename, setFilename] = useState('');
  const [offset, setOffset] = useState(0);
  const [selectedPath, setSelectedPath] = useState(null);
  const files = useRead('project_files', { workspaceId: project.workspace_id, page: { limit: PAGE_SIZE, offset }, filenameFilter: filename || null }, refreshKey);
  const rows = files.data?.files || [];
  return <div className="project-detail">
    <header className="project-heading"><span className="project-eyebrow">Recorded workspace</span><h1>{project.display_name || project.recorded_path}</h1><code>{project.recorded_path}</code>
      <p>{project.session_count} sessions · {project.host ? `SSH host ${project.host}` : `${project.local_session_count} local, ${project.synced_session_count} synced`}{project.ambiguous_host && ' · ambiguous host evidence'}</p></header>
    <section className="project-card" aria-label="Reported files">
      <h2><FileCode2 size={17}/>Reported files</h2>
      <label className="project-search"><Search size={14}/><input type="search" aria-label="Filter reported filenames" value={filename} onChange={event => { setFilename(event.target.value); setOffset(0); setSelectedPath(null); }} placeholder="Filter recorded paths"/></label>
      <ReadState read={files} empty={!rows.length}><div className="project-table-wrap"><table className="project-table"><thead><tr><th>Recorded path</th><th>Reads</th><th>Write intents</th><th>References</th><th>Sessions</th></tr></thead>
        <tbody>{rows.map(file => <tr key={file.reported_path}><th scope="row"><button type="button" aria-pressed={selectedPath === file.reported_path} onClick={() => setSelectedPath(file.reported_path)}><code>{file.reported_path}</code></button></th>
          <td>{file.reported_read_count}</td><td>{file.reported_write_intent_count}</td><td>{file.reported_reference_count}</td><td>{file.session_count}</td></tr>)}</tbody></table></div></ReadState>
      <p className="project-subtext">Counts deduplicate repeated references from the same event and tool. File history retains individual evidence rows.</p>
      <Pages label="file pages" offset={offset} hasMore={files.data?.has_more} nextOffset={files.data?.next_offset} onChange={setOffset} busy={files.loading}/>
    </section>
    {selectedPath != null && <div className="project-selected"><code>{selectedPath}</code><button type="button" onClick={() => setSelectedPath(null)}><ArrowLeft size={14}/>All activity</button></div>}
    <Activity key={selectedPath ?? 'all'} workspaceId={project.workspace_id} reportedPath={selectedPath} refreshKey={refreshKey} onSelectSession={onSelectSession}/>
    <GitContext project={project} refreshKey={refreshKey} onSelectSession={onSelectSession}/>
  </div>;
}

export default function ProjectsView({ onSelectSession, onOpenNav, refreshKey = 0 }) {
  const [offset, setOffset] = useState(0);
  const [selectedId, setSelectedId] = useState(null);
  const [projectFilter, setProjectFilter] = useState('');
  const [refresh, setRefresh] = useState(0);
  const readRefresh = JSON.stringify([refreshKey, refresh]);
  const catalog = useRead('project_catalog', { page: { limit: PAGE_SIZE, offset }, projectFilter: projectFilter || null }, readRefresh);
  const projects = catalog.data?.projects || [];
  const selected = projects.find(project => project.workspace_id === selectedId) || projects[0];
  return <div className="projects-view">
    <header className="projects-toolbar"><button className="icon-button mobile-menu" type="button" aria-label="Open navigation" onClick={onOpenNav}><Menu size={18}/></button><h1>Files and Git</h1><button type="button" onClick={() => setRefresh(value => value + 1)}><RefreshCw size={14}/>Refresh recorded history</button></header>
    <div className="project-browser"><aside className="project-picker" aria-label="Recorded workspaces"><h2><Folder size={16}/>Workspaces</h2>
      <label className="project-search"><Search size={14}/><input type="search" aria-label="Filter recorded workspaces" value={projectFilter} onChange={event => { setProjectFilter(event.target.value); setOffset(0); setSelectedId(null); }} placeholder="Find a workspace"/></label>
      <ReadState read={catalog} empty={!projects.length}><ul>{projects.map(project => <li key={project.workspace_id}><button type="button" aria-pressed={selected?.workspace_id === project.workspace_id} onClick={() => setSelectedId(project.workspace_id)}><strong>{project.display_name || project.recorded_path}</strong><span>{project.recorded_path}</span>{project.host && <span>SSH · {project.host}</span>}</button></li>)}</ul></ReadState>
      <Pages label="workspace pages" offset={offset} hasMore={catalog.data?.has_more} nextOffset={catalog.data?.next_offset} onChange={value => { setOffset(value); setSelectedId(null); }} busy={catalog.loading}/>
    </aside>{selected ? <ProjectDetails key={selected.workspace_id} project={selected} refreshKey={readRefresh} onSelectSession={onSelectSession}/> : <div className="project-empty">Select a recorded workspace to explore its file evidence.</div>}</div>
  </div>;
}
