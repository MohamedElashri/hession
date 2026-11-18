import { useCallback, useEffect, useRef, useState } from 'react';
import { Download, Folder, X } from 'lucide-react';
import { save } from '@tauri-apps/plugin-dialog';
import { listen } from '@tauri-apps/api/event';
import { api } from '../api.js';
import useModalDialog from './useModalDialog.js';
import '../styles/project-export.css';

const defaults = { format: 'markdown', includeStatistics: false, includeToolBodies: false, includeAbsolutePaths: false, redact: true, includeGitMetadata: false };
const bytes = value => `${(Number(value || 0) / 1024).toLocaleString(undefined, { maximumFractionDigits: 1 })} KiB`;

export function ProjectExportButton({ project, query = '', filters = {} }) {
  const [open, setOpen] = useState(false);
  return <><button type="button" className="btn" disabled={!project?.workspace_id} onClick={() => setOpen(true)}><Download size={14}/>Export project ZIP</button>
    {open && <ProjectExportDialog key={project.workspace_id} project={project} query={query} filters={filters} onClose={() => setOpen(false)}/>}</>;
}
export default function ProjectExportDialog(props) {
  return <ProjectExportForm key={`${props.project.workspace_id}:${props.project.project_key}`} {...props}/>;
}
function ProjectExportForm({ project, query = '', filters = {}, onClose }) {
  const dialog = useModalDialog();
  const alive = useRef(true), working = useRef(false), job = useRef(null), cancellationRequested = useRef(false);
  const releasing = useRef(Promise.resolve()), cancelling = useRef(null);
  const [initialFilters] = useState(() => JSON.parse(JSON.stringify(filters)));
  const [queryText, setQuery] = useState(query);
  const [options, setOptions] = useState(defaults);
  const [target, setTarget] = useState('');
  const [preview, setPreview] = useState(null);
  const [busy, setBusy] = useState('');
  const [progress, setProgress] = useState(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [result, setResult] = useState(null);
  const releaseId = useCallback(id => {
    if (id) releasing.current = releasing.current.then(() => api('release_project_archive', { jobId: id })).catch(() => {});
    return releasing.current;
  }, []);
  const release = useCallback(() => {
    const id = job.current; job.current = null;
    return releaseId(id);
  }, [releaseId]);
  useEffect(() => {
    alive.current = true;
    let active = true, unlisten;
    listen('project-export-progress', event => { if (active && event.payload.jobId === job.current) setProgress(event.payload); })
      .then(dispose => { if (active) unlisten = dispose; else dispose(); }).catch(() => {});
    return () => { active = false; alive.current = false; unlisten?.(); release(); };
  }, [release]);
  const invalidate = () => { release(); setPreview(null); setResult(null); setProgress(null); setError(''); setNotice(''); };
  const update = (key, value) => { invalidate(); setOptions(current => ({ ...current, [key]: value })); };
  const browse = async () => {
    if (working.current) return; working.current = true; setBusy('choose');
    try {
      const path = await save({ title: 'Save a new project history archive', defaultPath: `project-history-${new Date().toISOString().replaceAll(':', '-')}.zip`, filters: [{ name: 'Project history ZIP', extensions: ['zip'] }] });
      if (alive.current && path) { invalidate(); setTarget(path); }
    } catch (failure) { if (alive.current) setError(failure.message || String(failure)); }
    finally { working.current = false; if (alive.current) setBusy(''); }
  };
  const prepare = async () => {
    if (working.current || !target) return; working.current = true; cancellationRequested.current = false; setBusy('prepare'); setError(''); setNotice(''); setPreview(null); setResult(null);
    let id;
    try {
      await release();
      if (!alive.current) return;
      if (cancellationRequested.current) { setNotice('Project preview cancelled. No archive was saved.'); return; }
      id = crypto.randomUUID(); job.current = id;
      const response = await api('prepare_project_archive', { jobId: id, request: { workspaceId: project.workspace_id, query: queryText, filters: initialFilters, options }, expectedProjectKey: project.project_key, target });
      if (!alive.current || job.current !== id) {
        // A close can release before native admission; clean up again after its acknowledgement.
        await releaseId(id);
      } else if (cancellationRequested.current) { await release(); setNotice('Project preview cancelled. No archive was saved.'); }
      else setPreview(response.preview);
    } catch (failure) { if (alive.current && job.current === id) { setError(failure.message); await release(); } }
    finally { working.current = false; if (alive.current) setBusy(''); }
  };
  const write = async () => {
    if (working.current || !preview || !job.current) return; working.current = true; cancellationRequested.current = false; setBusy('write'); setError(''); setNotice('');
    try { const saved = await api('write_project_archive', { jobId: job.current }); if (alive.current) setResult(saved); }
    catch (failure) { if (alive.current) { setPreview(null); setError(failure.message); } }
    finally { release(); working.current = false; if (alive.current) setBusy(''); }
  };
  const cancel = () => {
    cancellationRequested.current = true;
    const id = job.current;
    if (!id) { if (alive.current && working.current) setNotice('Stopping export safely…'); return; }
    if (cancelling.current?.id === id) return cancelling.current.promise;
    const pending = Promise.resolve().then(() => api('cancel_project_archive', { jobId: id })).then(cancelled => {
      if (alive.current && working.current && job.current === id) setNotice(cancelled ? 'Stopping export safely…' : 'The archive is already saved; finishing…');
    }).catch(failure => {
      if (alive.current && working.current && job.current === id) setError(failure.message);
    }).finally(() => { if (cancelling.current?.promise === pending) cancelling.current = null; });
    cancelling.current = { id, promise: pending };
    return pending;
  };
  return <dialog ref={dialog} className="data-dialog project-export-dialog" aria-labelledby="project-export-title" aria-describedby="project-export-description"
    onCancel={event => { event.preventDefault(); if (busy && busy !== 'choose') cancel(); else if (!busy) onClose(); }}>
    <div className="export-heading"><h2 id="project-export-title"><Download size={18}/>Export project history</h2><button type="button" className="icon-button" aria-label="Close project export" disabled={Boolean(busy)} onClick={onClose}><X size={18}/></button></div>
    <p id="project-export-description">Create a portable reading archive for <strong>{project.display_name || project.recorded_path}</strong>. Original agent logs and repository files stay untouched.</p>
    <fieldset className="project-export-fields" disabled={Boolean(busy) || Boolean(result)}>
      <label>Developer query<input value={queryText} maxLength={4096} onChange={event => { invalidate(); setQuery(event.target.value); }} placeholder="Optional phrases, exclusions, and metadata filters"/></label>
      <label>Transcript format<select value={options.format} onChange={event => update('format', event.target.value)}><option value="markdown">Markdown</option><option value="json">JSON</option><option value="both">Markdown and JSON</option></select></label>
      <label className="project-export-checkbox"><input type="checkbox" checked={options.redact} onChange={event => update('redact', event.target.checked)}/>Redact common secret patterns</label>
      <label className="project-export-checkbox"><input type="checkbox" checked={options.includeToolBodies} onChange={event => update('includeToolBodies', event.target.checked)}/>Include tool inputs and outputs</label>
      <label className="project-export-checkbox"><input type="checkbox" checked={options.includeAbsolutePaths} onChange={event => update('includeAbsolutePaths', event.target.checked)}/>Include absolute machine paths</label>
      <label className="project-export-checkbox"><input type="checkbox" checked={options.includeGitMetadata} onChange={event => update('includeGitMetadata', event.target.checked)}/>Include recorded Git metadata</label>
      <label className="project-export-checkbox"><input type="checkbox" checked={options.includeStatistics} onChange={event => update('includeStatistics', event.target.checked)}/>Include cached statistics</label>
      <div className="project-export-target"><span>New archive destination</span><code>{target || 'Choose a new ZIP filename'}</code><button type="button" className="btn" onClick={browse}><Folder size={14}/>Choose destination</button></div>
    </fieldset>
    <p className="project-export-hint">Tool bodies and paths can contain private code or credentials. Pattern redaction cannot find every secret or encoded path. This ZIP is not encrypted and cannot restore a SQLite backup.</p>
    {preview && !result && <section className="project-export-preview" aria-label="Project export preview"><h3>Reviewed selection</h3><p>{preview.sessionCount} session snapshots ({preview.selectedSessionCount} selected histories) · about {bytes(preview.estimatedUncompressedBytes)} uncompressed</p><p>{preview.exportedEntryCount} exported entries · {preview.redactions.secretPatterns} secret-pattern matches · {preview.redactions.pathPatterns} path-pattern matches</p><code>{preview.intendedOutput}</code><ul>{preview.warnings.map(warning => <li key={warning}>{warning}</li>)}</ul></section>}
    {busy && <p role="status" aria-live="polite">{busy === 'prepare' ? 'Preparing the project preview…' : busy === 'choose' ? 'Choosing destination…' : progress ? `Saving ${progress.completedSessions} of ${progress.totalSessions} sessions…` : 'Saving project history…'}</p>}
    {notice && <p role="status">{notice}</p>}{error && <p role="alert" className="project-export-error">{error}</p>}
    {result && <section aria-label="Saved project archive"><h3>Project archive saved</h3><code>{result.destination}</code><p>{result.sessionCount} snapshots · {bytes(result.archiveBytes)}</p><ul>{result.warnings.map(warning => <li key={warning}>{warning}</li>)}</ul></section>}
    <div className="data-actions">{busy && busy !== 'choose' ? <button type="button" className="btn" onClick={cancel}>Cancel export</button> : <><button type="button" className="btn" onClick={onClose}>{result ? 'Done' : 'Close'}</button>{!result && <><button type="button" className="btn" disabled={!target || Boolean(busy)} onClick={prepare}>Prepare preview</button><button type="button" className="btn btn-primary" disabled={!preview || Boolean(busy)} onClick={write}>Save reviewed ZIP</button></>}</>}</div>
  </dialog>;
}
