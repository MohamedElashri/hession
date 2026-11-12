import { useEffect, useRef, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { ArrowRightLeft, Check, Folder, LoaderCircle, RefreshCw, Trash2, X } from 'lucide-react';
import { api } from '../api.js';
import useModalDialog from './useModalDialog.js';
import '../styles/continuation.css';

export default function ContinuationDialog({ sessionId, onClose }) {
  // A different source owns a different draft and async request lifecycle.
  // Never leave the previous source's reviewed context actionable while a new
  // source is loading, even when this component is reused by its parent.
  return <ContinuationForm key={String(sessionId)} sessionId={sessionId} onClose={onClose}/>;
}

function ContinuationForm({ sessionId, onClose }) {
  const dialog = useModalDialog();
  const request = useRef(0);
  const reviewedRequest = useRef(null);
  const launching = useRef(false);
  const [target, setTarget] = useState('codex');
  const [cwd, setCwd] = useState('');
  const [note, setNote] = useState('');
  const [includeTools, setIncludeTools] = useState(false);
  const [info, setInfo] = useState(null);
  const [busy, setBusy] = useState('preview');
  const [error, setError] = useState('');
  const [result, setResult] = useState(null);
  const [cleanupMessage, setCleanupMessage] = useState('');

  useEffect(() => {
    let cancelled = false;
    const id = ++request.current;
    api('get_continuation_info', { id: String(sessionId), target: 'codex', cwd: null, note: null, includeTools: false })
      .then(value => {
        if (cancelled || id !== request.current) return;
        reviewedRequest.current = id;
        setInfo(value); setCwd(value.workspace.path);
      })
      .catch(error => { if (!cancelled && id === request.current) setError(error.message); })
      .finally(() => { if (!cancelled && id === request.current) setBusy(''); });
    return () => { cancelled = true; };
  }, [sessionId]);

  const invalidate = () => {
    request.current++;
    reviewedRequest.current = null;
    setInfo(null); setError(''); setResult(null);
  };
  const refresh = async () => {
    const id = ++request.current;
    setBusy('preview'); setInfo(null); setError(''); setResult(null);
    try {
      const value = await api('get_continuation_info', {
        id: String(sessionId), target, cwd: cwd || null, note, includeTools,
      });
      if (id !== request.current) return;
      reviewedRequest.current = id;
      setInfo(value); setCwd(value.workspace.path);
    } catch (error) { if (id === request.current) setError(error.message); }
    finally { if (id === request.current) setBusy(''); }
  };
  const browse = async () => {
    setError('');
    try {
      const path = await open({ title: 'Choose the local workspace for this continuation', directory: true, multiple: false });
      if (typeof path === 'string') { invalidate(); setCwd(path); }
    } catch (error) { setError(error.message); }
  };
  const launch = async () => {
    if (launching.current || reviewedRequest.current !== request.current || !info?.canLaunch || busy) return;
    launching.current = true;
    reviewedRequest.current = null;
    setBusy('launch'); setError('');
    try {
      const value = await api('launch_continuation', {
        id: String(sessionId), target, cwd: info.workspace.path, note, includeTools,
        checkpoint: info.checkpoint, terminal: null,
      });
      setResult(value);
    } catch (error) { setError(error.message); setInfo(null); }
    finally { launching.current = false; setBusy(''); }
  };
  const cleanup = async () => {
    setBusy('cleanup'); setError(''); setCleanupMessage('');
    try {
      const value = await api('cleanup_continuation_packages', { all: true });
      setCleanupMessage(`Deleted ${value.removed} saved handoff ${value.removed === 1 ? 'package' : 'packages'}.`);
    } catch (error) { setError(error.message); }
    finally { setBusy(''); }
  };

  return <dialog ref={dialog} className="data-dialog continuation-dialog" aria-labelledby="continuation-title"
    aria-describedby="continuation-description" onCancel={event => { event.preventDefault(); if (!busy) onClose(); }}>
    <div className="export-heading">
      <h2 id="continuation-title"><ArrowRightLeft size={18}/>Continue in another harness</h2>
      <button type="button" className="icon-button" disabled={Boolean(busy)} aria-label="Close continuation" onClick={onClose}><X size={18}/></button>
    </div>
    <p id="continuation-description">Start a new session with a reviewed handoff from your indexed history. Your original session stays available.</p>
    <fieldset className="continuation-fields" disabled={Boolean(busy) || Boolean(result)}>
      <label>Destination harness<select value={target} onChange={event => { invalidate(); setTarget(event.target.value); }}>
        <option value="codex">Codex</option><option value="claude">Claude Code</option>
      </select></label>
      <label>Local workspace<div className="terminal-path">
        <input value={cwd} onChange={event => { invalidate(); setCwd(event.target.value); }} placeholder="Choose the matching local checkout" spellCheck={false}/>
        <button type="button" className="btn" onClick={browse}><Folder size={14}/>Browse</button>
      </div></label>
      <label>Handoff note <span className="continuation-hint">Optional: task, constraints, decisions, or next steps</span>
        <textarea rows={4} maxLength={8000} value={note} onChange={event => { invalidate(); setNote(event.target.value); }} placeholder="Continue fixing the login issue. Preserve the public API and run the regression test first."/>
      </label>
      <label className="continuation-checkbox"><input type="checkbox" checked={includeTools} onChange={event => { invalidate(); setIncludeTools(event.target.checked); }}/>
        Include recent tool inputs and outputs</label>
      <p className="continuation-hint">Tool context can contain source code and secrets. Inspect the exact package before launching. Hession does not summarize through a model service.</p>
    </fieldset>
    {busy === 'preview' && <p role="status"><LoaderCircle className="spin" size={14}/> Preparing local preview…</p>}
    {info && !result && <div className="continuation-preview">
      <p><strong>{info.source.title}</strong> · {info.source.family} → {target === 'claude' ? 'Claude Code' : 'Codex'}</p>
      <p className="continuation-hint">Indexed checkpoint: {info.source.importedAt}{info.cliVersion ? ` · Destination CLI: ${info.cliVersion}` : ''}</p>
      {info.warnings.length > 0 && <ul className="continuation-warnings">{info.warnings.map(warning => <li key={warning}>{warning}</li>)}</ul>}
      {info.workspace.repositoryRoot && <details><summary>Destination repository state{info.workspace.dirty ? ' · uncommitted changes' : ''}</summary>
        <pre>{info.workspace.repositoryRoot}{'\n'}Branch: {info.workspace.branch || '(detached / unknown)'}{'\n'}Commit: {info.workspace.commit || '(unborn / unknown)'}{'\n'}{info.workspace.statusKnown === false ? 'Working tree status unavailable' : info.workspace.status || 'Working tree clean'}</pre>
      </details>}
      <details className="continuation-context"><summary>Review exact handoff package</summary><pre>{info.content}</pre></details>
      <details><summary>What transfers and what is omitted</summary><ul>{info.omissions.map(omission => <li key={omission}>{omission}</li>)}</ul></details>
      <p className="continuation-provider">{info.providerNotice}</p>
    </div>}
    {result && <div className="data-success" role="status"><Check size={16}/> Opened a new session in {result.terminal}.
      <p className="continuation-hint">The destination may ask permission to read the handoff file. Its native session appears after the next history refresh. This launch record does not claim a confirmed destination session ID.</p>
      <p className="continuation-package-path">Local context: {result.packagePath}</p>
    </div>}
    {error && <p className="data-error" role="alert">{error}</p>}
    <div className="data-actions">
      <button type="button" className="btn" disabled={Boolean(busy)} onClick={refresh}><RefreshCw size={14}/>Refresh preview</button>
      {result ? <button type="button" className="btn primary" onClick={onClose}>Done</button>
        : <button type="button" className="btn primary" disabled={Boolean(busy) || !info?.canLaunch} onClick={launch}>
          {busy === 'launch' ? <LoaderCircle className="spin" size={14}/> : <ArrowRightLeft size={14}/>}{busy === 'launch' ? 'Opening…' : `Continue in ${target === 'claude' ? 'Claude Code' : 'Codex'}`}
        </button>}
    </div>
    <details className="continuation-storage"><summary>Saved handoff packages</summary>
      <p>Packages are private local files containing the reviewed context. Packages older than seven days are removed when you next launch a continuation. Delete them after destination sessions have read them; original histories are unaffected.</p>
      <button type="button" className="btn" disabled={Boolean(busy)} onClick={cleanup}><Trash2 size={14}/>Delete saved packages</button>
      {cleanupMessage && <p role="status">{cleanupMessage}</p>}
    </details>
  </dialog>;
}
