import React, { useCallback, useEffect, useRef, useState } from 'react';
import { Database, Download, Upload, Trash2, LoaderCircle, RefreshCw, X } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { open, save } from '@tauri-apps/plugin-dialog';
import { BRAND } from '../brand.js';

const emptyFilters = () => ({ query: '', agent: '', workspace: '', since: '', until: '', starred: false, tag: '' });
const errorMessage = error => error instanceof Error ? error.message : String(error);
const fileLabel = path => path.replace(/\\/g, '/').split('/').at(-1);
const bytes = size => size < 1024 * 1024 ? `${(size / 1024).toFixed(1)} KB` : `${(size / 1024 / 1024).toFixed(1)} MB`;
const filtersForCommand = filters => {
  const boundary = (value, nextDay) => {
    if (!value) return '';
    const [year, month, day] = value.split('-').map(Number);
    return new Date(year, month - 1, day + (nextDay ? 1 : 0)).toISOString();
  };
  return { ...filters, since: boundary(filters.since, false), until: boundary(filters.until, true) };
};

export default function DataManagement({ onClose, onChanged, syncRunning }) {
  const dialog = useRef(null);
  const requestId = useRef(0);
  const operation = useRef(false);
  const [tab, setTab] = useState('saved');
  const [filters, setFilters] = useState(emptyFilters);
  const [importPath, setImportPath] = useState('');
  const [preview, setPreview] = useState(null);
  const [options, setOptions] = useState({ agents: [], workspaces: [], tags: [] });
  const [previewLoading, setPreviewLoading] = useState(true);
  const [busy, setBusy] = useState('');
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [revision, setRevision] = useState(0);
  const [mode, setMode] = useState('keep');
  const [includePricing, setIncludePricing] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [confirmation, setConfirmation] = useState('');
  const [resetDatabase, setResetDatabase] = useState(false);
  const filtered = Object.values(filters).some(Boolean);
  const desktop = Boolean(window.__TAURI_INTERNALS__?.invoke);
  const actionsDisabled = Boolean(busy || previewLoading || !preview || syncRunning || !desktop);
  const confirmationWord = resetDatabase ? 'RESET' : 'DELETE';

  useEffect(() => {
    const previous = document.activeElement;
    dialog.current.showModal();
    dialog.current.querySelector('button')?.focus();
    return () => { requestId.current++; if (previous?.isConnected) previous.focus(); };
  }, []);

  useEffect(() => {
    const id = ++requestId.current;
    setPreview(null); setError(''); setPreviewLoading(true);
    if (!desktop) { setError('Manage saved data is available in the desktop app.'); setPreviewLoading(false); return; }
    if (tab === 'import' && !importPath) { setPreviewLoading(false); return; }
    const timer = setTimeout(async () => {
      try {
        const args = { filters: filtersForCommand(filters), ...(tab === 'import' ? { path: importPath } : {}) };
        const result = await invoke(tab === 'import' ? 'preview_data_import' : 'preview_saved_data', args);
        if (id !== requestId.current) return;
        setPreview(result); setOptions(result.options);
      } catch (error) { if (id === requestId.current) setError(errorMessage(error)); }
      finally { if (id === requestId.current) setPreviewLoading(false); }
    }, filters.query ? 250 : 0);
    return () => { clearTimeout(timer); requestId.current++; };
  }, [tab, importPath, filters, revision, desktop]);

  const changeFilters = patch => {
    requestId.current++; setPreview(null); setPreviewLoading(true);
    setFilters(current => ({ ...current, ...patch }));
    setDeleteOpen(false); setConfirmation(''); setMessage('');
  };
  const changeTab = value => {
    requestId.current++; setPreview(null); setOptions({ agents: [], workspaces: [], tags: [] });
    setTab(value); setFilters(emptyFilters()); setDeleteOpen(false); setConfirmation(''); setResetDatabase(false); setMessage('');
  };
  const refreshPreview = useCallback(() => { setPreview(null); setPreviewLoading(true); setRevision(value => value + 1); }, []);
  const perform = async (label, action) => {
    if (operation.current) return;
    operation.current = true; setBusy(label); setError(''); setMessage('');
    try { await action(); }
    catch (error) { setError(errorMessage(error)); }
    finally { operation.current = false; setBusy(''); }
  };
  const chooseImport = () => perform('Choosing backup…', async () => {
    const path = await open({ title: `Import ${BRAND.name} database backup`, multiple: false, directory: false, filters: [{ name: 'SQLite backup', extensions: ['sqlite'] }] });
    if (typeof path !== 'string') return;
    requestId.current++; setPreview(null); setOptions({ agents: [], workspaces: [], tags: [] });
    setImportPath(path); setFilters(emptyFilters()); setRevision(value => value + 1);
  });
  const exportData = () => perform('Exporting backup…', async () => {
    const stamp = new Date().toISOString().replace(/[:.]/g, '-');
    const path = await save({ title: `Export ${BRAND.name} database backup`, defaultPath: `${BRAND.slug}-${filtered ? 'filtered' : 'all'}-${stamp}.sqlite`, filters: [{ name: 'SQLite backup', extensions: ['sqlite'] }] });
    if (!path) return;
    const result = await invoke('export_saved_data', { path, filters: filtersForCommand(filters), expectedToken: preview.token });
    setMessage(`Exported ${result.sessions.toLocaleString()} sessions to ${fileLabel(result.path)}.`);
  });
  const importData = () => perform('Importing backup…', async () => {
    const result = await invoke('import_saved_data', { path: importPath, filters: filtersForCommand(filters), mode, includePricing, expectedToken: preview.token });
    setMessage(`Import complete: ${result.inserted.toLocaleString()} added, ${result.replaced.toLocaleString()} replaced, ${result.skipped.toLocaleString()} kept.${includePricing ? ` ${result.pricingOverrides} pricing overrides imported.` : ''}`);
    refreshPreview(); await onChanged();
  });
  const deleteData = () => perform(resetDatabase ? 'Resetting database…' : 'Deleting saved sessions…', async () => {
    const result = await invoke('delete_saved_data', { filters: filtersForCommand(filters), expectedToken: preview.token, confirmation, resetDatabase });
    setDeleteOpen(false); setConfirmation(''); setResetDatabase(false);
    setMessage(`${result.reset ? 'Database reset.' : `Deleted ${result.deleted.toLocaleString()} saved sessions.`}${result.warning ? ` ${result.warning}` : ''}`);
    refreshPreview(); await onChanged();
  });

  return <dialog ref={dialog} className="data-dialog" aria-labelledby="data-title" aria-describedby="data-description"
    onCancel={event => { event.preventDefault(); if (!operation.current) onClose(); }}
    onClick={event => { if (event.target === dialog.current && !operation.current) { const rect = dialog.current.getBoundingClientRect(); if (event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) onClose(); } }}>
    <div className="export-heading"><h2 id="data-title"><Database size={18}/>Manage saved data</h2><button type="button" className="icon-button" aria-label="Close data management" disabled={Boolean(busy)} onClick={onClose}><X size={18}/></button></div>
    <p id="data-description">Back up, restore, or remove the data saved by {BRAND.name}. Backups include full transcripts, tool activity, file activity, stars, tags, and pricing overrides.</p>
    <div className="data-tabs" role="tablist" aria-label="Data operations" onKeyDown={event => {
      if (busy || !['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
      event.preventDefault();
      const tabs = Array.from(event.currentTarget.querySelectorAll('[role="tab"]'));
      const next = event.key === 'Home' ? tabs[0] : event.key === 'End' ? tabs[1] : tabs[tab === 'saved' ? 1 : 0];
      next.click(); next.focus();
    }}>
      <button type="button" role="tab" id="data-saved-tab" aria-controls="data-panel" aria-selected={tab === 'saved'} tabIndex={tab === 'saved' ? 0 : -1} disabled={Boolean(busy)} onClick={() => changeTab('saved')}>Saved data</button>
      <button type="button" role="tab" id="data-import-tab" aria-controls="data-panel" aria-selected={tab === 'import'} tabIndex={tab === 'import' ? 0 : -1} disabled={Boolean(busy)} onClick={() => changeTab('import')}>Import backup</button>
    </div>
    <div id="data-panel" role="tabpanel" aria-labelledby={tab === 'saved' ? 'data-saved-tab' : 'data-import-tab'}>
      {tab === 'import' && <div className="data-file"><button type="button" className="data-button" disabled={Boolean(busy) || !desktop} onClick={chooseImport}><Upload size={14}/>{importPath ? 'Choose another backup' : 'Choose SQLite backup'}</button>{importPath && <span title={importPath}>{fileLabel(importPath)}</span>}<p>Use a .sqlite backup from this dialog. Markdown and transcript JSON exports cannot be restored.</p></div>}
      {(tab === 'saved' || importPath) && <>
        <fieldset className="data-filters" disabled={Boolean(busy) || !desktop}>
          <legend>{tab === 'import' ? <>Filter incoming&nbsp;sessions</> : <>Filter saved&nbsp;sessions</>}</legend>
          <label className="data-search">Search<input type="search" value={filters.query} placeholder="Title, session ID, or transcript text" onChange={event => changeFilters({ query: event.target.value })}/></label>
          <label>Agent<select value={filters.agent} onChange={event => changeFilters({ agent: event.target.value })}><option value="">All agents</option>{filters.agent&&!options.agents.includes(filters.agent)&&<option value={filters.agent}>{filters.agent}</option>}{options.agents.map(agent => <option key={agent} value={agent}>{agent}</option>)}</select></label>
          <label>Workspace<select value={filters.workspace} onChange={event => changeFilters({ workspace: event.target.value })}><option value="">All workspaces</option>{filters.workspace&&!options.workspaces.includes(filters.workspace)&&<option value={filters.workspace}>{filters.workspace}</option>}{options.workspaces.map(path => <option key={path} value={path}>{path}</option>)}</select></label>
          <label>Last activity from<input type="date" value={filters.since} max={filters.until || undefined} onChange={event => changeFilters({ since: event.target.value })}/></label>
          <label>Through<input type="date" value={filters.until} min={filters.since || undefined} onChange={event => changeFilters({ until: event.target.value })}/></label>
          <label>Tag<select value={filters.tag} onChange={event => changeFilters({ tag: event.target.value })}><option value="">All tags</option>{filters.tag&&!options.tags.includes(filters.tag)&&<option value={filters.tag}>{filters.tag}</option>}{options.tags.map(tag => <option key={tag} value={tag}>{tag}</option>)}</select></label>
          <label className="data-checkbox"><input type="checkbox" checked={filters.starred} onChange={event => changeFilters({ starred: event.target.checked })}/>Starred only</label>
          <div className="data-filter-note"><span>Dates include whole local calendar days. Undated sessions appear only without a date filter.</span><button type="button" disabled={!filtered} onClick={() => changeFilters(emptyFilters())}>Clear filters</button></div>
        </fieldset>
        <div className="data-summary" aria-live="polite" aria-busy={previewLoading}>
          {previewLoading ? <span><LoaderCircle className="spin" size={14}/>Reading preview…</span> : preview ? <><strong>{preview.sessions.toLocaleString()} {tab === 'import' ? 'incoming' : 'matching'} sessions</strong><span>{preview.entries.toLocaleString()} entries · {preview.tools.toLocaleString()} tool calls</span>{tab === 'import' && <span>{preview.newSessions.toLocaleString()} new · {preview.matchingSessions.toLocaleString()} matching saved&nbsp;sessions</span>}</> : <span>Preview unavailable</span>}
          <button type="button" className="icon-button" aria-label="Refresh data preview" title="Refresh preview" disabled={Boolean(busy) || previewLoading || !desktop} onClick={refreshPreview}><RefreshCw size={14}/></button>
        </div>
      </>}
      {tab === 'saved' ? <>
        {preview && <p className="data-location" title={preview.databasePath}>{preview.totalSessions.toLocaleString()} sessions in database · {bytes(preview.databaseBytes)}<br/>{preview.databasePath}</p>}
        <p>Exports contain all related records for the matching sessions, plus pricing settings. Backup files may contain private prompts, code, and local paths.</p>
        <div className="data-actions"><button type="button" className="data-button" disabled={actionsDisabled} onClick={exportData}><Download size={14}/>Export SQLite backup</button><button type="button" className="data-button data-danger" disabled={actionsDisabled} aria-expanded={deleteOpen} onClick={() => { setDeleteOpen(current => !current); setConfirmation(''); setResetDatabase(false); }}><Trash2 size={14}/>Delete / reset…</button></div>
        {deleteOpen && <div className="data-delete">
          <strong>{resetDatabase ? 'Reset the entire saved database' : `Delete ${preview?.sessions.toLocaleString() || 0} matching sessions`}</strong>
          <p>This permanently removes saved transcripts, tool and file activity, stars, and tags. Original agent files stay on disk; deleted sessions can return on your next sync.</p>
          {!filtered && <label className="data-checkbox"><input type="checkbox" checked={resetDatabase} disabled={Boolean(busy)} onChange={event => { setResetDatabase(event.target.checked); setConfirmation(''); }}/>Reset everything, including pricing overrides, cached prices, and sync history</label>}
          <p>Export a backup first if you may need to restore this data.</p>
          <label className="data-confirm">Type {confirmationWord} to confirm<input value={confirmation} autoComplete="off" spellCheck={false} disabled={Boolean(busy)} onChange={event => setConfirmation(event.target.value)}/></label>
          <button type="button" className="data-button data-danger" disabled={actionsDisabled || confirmation !== confirmationWord || (!resetDatabase && !preview?.sessions)} onClick={deleteData}><Trash2 size={14}/>{resetDatabase ? 'Reset database' : 'Delete matching sessions'}</button>
        </div>}
      </> : importPath && <>
        <fieldset className="data-import-options" disabled={Boolean(busy)}><legend>When sessions match</legend>
          <label><input type="radio" name="data-import-mode" value="keep" checked={mode === 'keep'} onChange={() => setMode('keep')}/><span><strong>Keep existing, add new only</strong><small>Matching sessions and their stars and tags stay as they are.</small></span></label>
          <label><input type="radio" name="data-import-mode" value="replace" checked={mode === 'replace'} onChange={() => setMode('replace')}/><span><strong>Replace existing matches</strong><small>Replace matching sessions completely, including stars and tags, with the backup’s records.</small></span></label>
          <label className="data-checkbox"><input type="checkbox" checked={includePricing} onChange={event => setIncludePricing(event.target.checked)}/><span>Also import pricing overrides ({preview?.pricingOverrides ?? '…'})<small>Session filters do not limit pricing overrides. The same keep / replace choice applies.</small></span></label>
        </fieldset>
        <p>Sessions match by source URI, or by agent and external session ID. Ambiguous matches stop the import. All changes are applied together; an invalid backup leaves saved data intact.</p>
        <button type="button" className="data-button" disabled={actionsDisabled || (!preview?.sessions && !(includePricing && preview?.pricingOverrides)) || (mode === 'keep' && !preview?.newSessions && !(includePricing && preview?.pricingOverrides))} onClick={importData}><Upload size={14}/>{mode === 'replace' ? 'Import and replace matches' : 'Import new sessions'}</button>
      </>}
    </div>
    {syncRunning && <p className="data-warning" role="status">Session sync is running. Close this dialog and stop sync before exporting, importing, or deleting data.</p>}
    {busy && <div className="data-status" role="status"><LoaderCircle className="spin" size={14}/>{busy} Keep the app open until this finishes.</div>}
    {error && <div className="data-error" role="alert">{error}<button type="button" disabled={Boolean(busy) || !desktop} onClick={refreshPreview}>Refresh preview</button></div>}
    {message && <div className="data-success" role="status">{message}</div>}
  </dialog>;
}
