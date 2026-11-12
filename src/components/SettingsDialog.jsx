import { lazy, Suspense, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { Database, Info, LoaderCircle, Palette, RefreshCw, Server, Settings, ShieldCheck, Sparkles, Terminal, X } from 'lucide-react';
import { api } from '../api.js';
import { updates } from '../updates.js';
import useModalDialog from './useModalDialog.js';
import DeviceSyncSettings from './DeviceSyncSettings.jsx';
import FeatureDocumentation from './FeatureDocumentation.jsx';
const AppearanceSettings = lazy(() => import('./AppearanceSettings.jsx'));
const TerminalSettings = lazy(() => import('./TerminalSettings.jsx'));
const RemoteHosts = lazy(() => import('./RemoteHosts.jsx'));
const DataManagement = lazy(() => import('./DataManagement.jsx'));
const AboutSettings = lazy(() => import('./AboutSettings.jsx'));

const sections = [
  { id: 'appearance', label: 'Appearance', icon: Palette, description: 'Make your library comfortable to read.' },
  { id: 'terminal', label: 'Terminal', icon: Terminal, description: 'Choose where sessions and continuations open.' },
  { id: 'history', label: 'History refresh', icon: RefreshCw, description: 'Control when Hession reads your agent histories.' },
  { id: 'remotes', label: 'Remote hosts', icon: Server, description: 'Bring histories from your SSH hosts into this library.' },
  { id: 'sync', label: 'Private sync', icon: ShieldCheck, description: 'Optionally share your library between your own devices.' },
  { id: 'continuation', label: 'Continuation', icon: Sparkles, description: 'Optionally continue a session with another harness.' },
  { id: 'data', label: 'Saved data', icon: Database, description: 'Back up, restore, or remove your saved library.' },
  { id: 'about', label: 'About', icon: Info, description: 'Version information and signed software updates.' },
];

export default function SettingsDialog({ initialTab = 'appearance', onClose, onSectionChange, onFeaturesChanged, onLibraryChanged,
  onManagedDataChanged, onRemotesChanged, preferences, onAppearanceChange, appearanceSaveError, autoSync, onAutoSyncChange,
  onRefreshHistories, syncRunning }) {
  const dialog = useModalDialog();
  const initialSection = sections.some(section => section.id === initialTab) ? initialTab : 'appearance';
  const [tab, setTab] = useState(initialSection);
  const [visited, setVisited] = useState(() => new Set([initialSection]));
  const content = useRef(null);
  const { phase } = useSyncExternalStore(updates.subscribe, updates.getSnapshot);
  const [features, setFeatures] = useState(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [help, setHelp] = useState(null);
  const [message, setMessage] = useState('');
  const section = sections.find(item => item.id === tab);
  const selectSection = value => {
    if (busy) return;
    setTab(value); setHelp(null);
    setVisited(current => new Set([...current, value]));
    onSectionChange?.(value);
    content.current?.scrollTo?.(0, 0);
  };
  const navigateSections = event => {
    if (busy || !['ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const index = sections.findIndex(item => item.id === tab);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? sections.length - 1
      : (index + (['ArrowUp', 'ArrowLeft'].includes(event.key) ? -1 : 1) + sections.length) % sections.length;
    selectSection(sections[next].id);
    event.currentTarget.querySelectorAll('[role="tab"]')[next]?.focus();
  };
  useEffect(() => {
    let active = true;
    api('get_optional_features').then(value => { if (active) setFeatures(value); })
      .catch(e => { if (active) setError(e.message); });
    return () => { active = false; };
  }, []);
  const toggleContinuation = async enabled => {
    setBusy(true); setError(''); setMessage('');
    try {
      const updated = await api('set_continuation_enabled', { enabled });
      setFeatures(updated); onFeaturesChanged(updated);
    } catch (e) { setError(e.message); }
    finally { setBusy(false); }
  };
  return <dialog ref={dialog} className="data-dialog settings-dialog" aria-labelledby="settings-title"
    onCancel={event => { event.preventDefault(); if (!busy) onClose(); }}>
    <div className="settings-heading"><h2 id="settings-title"><Settings size={18}/>Settings</h2>
      <button type="button" className="icon-button" aria-label="Close settings" disabled={busy} onClick={onClose}><X size={18}/></button>
    </div>
    <div className="settings-layout">
    <nav className="settings-navigation" aria-label="Settings sections">
      <div role="tablist" aria-label="Settings sections" aria-orientation="vertical" onKeyDown={navigateSections}>
        {sections.map(({ id, label, icon: Icon }) => <button type="button" role="tab" key={id} id={`settings-tab-${id}`}
          aria-controls={`settings-panel-${id}`} aria-selected={tab === id} tabIndex={tab === id ? 0 : -1}
          disabled={busy} onClick={() => selectSection(id)}>
          <Icon size={16}/><span>{label}</span>
          {id === 'about' && ['ready', 'available'].includes(phase) && <span className="settings-update-dot" aria-hidden="true" title={phase === 'ready' ? 'Ready to restart' : 'Update available'}/>}
        </button>)}
      </div>
      <p className="settings-device-note">Preferences are saved on this device.</p>
    </nav>
    <div className="settings-content" ref={content}>
      <header className="settings-section-heading"><h3>{section.label}</h3><p>{section.description}</p></header>
      <Suspense fallback={<div className="data-status" role="status"><LoaderCircle size={16} className="spin"/>Loading settings…</div>}>
      {sections.map(({ id }) => <div key={id} role="tabpanel" id={`settings-panel-${id}`} aria-labelledby={`settings-tab-${id}`}
        hidden={tab !== id || !!help} tabIndex={0}>
        {visited.has(id) && <>
          {id === 'appearance' && <AppearanceSettings preferences={preferences} onChange={onAppearanceChange} saveError={appearanceSaveError}/>}
          {id === 'terminal' && <TerminalSettings onBusyChange={setBusy}/>}
          {id === 'history' && <section aria-label="History refresh preferences">
            <label className="settings-switch"><input type="checkbox" checked={!!autoSync} onChange={onAutoSyncChange}/><span>Refresh histories on startup</span></label>
            <p className="terminal-settings-hint">Read new sessions from local files and enabled SSH hosts when Hession opens. Private device sync is separate and always starts manually.</p>
            <button type="button" className="btn" disabled={syncRunning} onClick={onRefreshHistories}><RefreshCw size={14}/>{syncRunning ? 'Refreshing histories…' : 'Refresh histories'}</button>
          </section>}
          {id === 'remotes' && <RemoteHosts onSyncNow={onRefreshHistories} syncRunning={syncRunning} onBusyChange={setBusy} onChanged={onRemotesChanged}/>}
          {id === 'sync' && <DeviceSyncSettings onLibraryChanged={onLibraryChanged} onBusyChange={setBusy} onHelp={() => setHelp('sync')}/>}
          {id === 'data' && <DataManagement onChanged={onManagedDataChanged} syncRunning={syncRunning} onBusyChange={setBusy}/>}
          {id === 'about' && <AboutSettings syncRunning={syncRunning}/>}
          {id === 'continuation' && <section className="settings-section" aria-label="Cross-harness continuation">
            <h3>Continue with another harness</h3>
            <p>Start a new Claude Code or Codex session with a reviewable package of context from an existing session. Your original session stays intact.</p>
            <label className="settings-switch"><input type="checkbox" checked={!!features?.continuationEnabled} disabled={!features || busy} onChange={event => toggleContinuation(event.target.checked)}/>
              <span>Enable cross-harness continuation</span></label>
            <p className="terminal-settings-hint">Each handoff shows the content and destination before you launch. The chosen harness may send that content to its model provider. Enabling this feature does not enable device sync or upload history.</p>
            <p className="terminal-settings-hint">After enabling, open a session and choose <strong>Continue in…</strong>. Native Resume remains available independently.</p>
            <button type="button" className="btn" onClick={() => setHelp('continuation')}>Read the continuation guide</button>
            <p className="terminal-settings-hint">Saved handoff packages contain local context. Delete them after destination sessions have read them. Cleanup remains available when continuation is disabled.</p>
            <button type="button" className="btn" disabled={busy} onClick={async () => {
              setBusy(true); setError(''); setMessage('');
              try {
                const result = await api('cleanup_continuation_packages', { all: true });
                setMessage(`Deleted ${result.removed} saved handoff ${result.removed === 1 ? 'package' : 'packages'}. Original histories are kept.`);
              } catch (e) { setError(e.message); }
              finally { setBusy(false); }
            }}>Delete saved handoff packages</button>
            {error && <p className="data-warning" role="alert">{error}</p>}
            {message && <p className="data-success" role="status">{message}</p>}
          </section>}
        </>}
      </div>)}
      {help && <FeatureDocumentation key={help} topic={help} onClose={() => setHelp(null)}/>}
      </Suspense>
    </div>
    </div>
    <div className="settings-footer"><span>{busy ? 'Finishing changes…' : 'Changes apply immediately unless a Save button is shown.'}</span><button type="button" className="btn" disabled={busy} onClick={onClose}>Done</button></div>
  </dialog>;
}
