import { useEffect, useState } from 'react';
import { ArrowRight, Settings, X } from 'lucide-react';
import { api } from '../api.js';
import useModalDialog from './useModalDialog.js';
import DeviceSyncSettings from './DeviceSyncSettings.jsx';
import FeatureDocumentation from './FeatureDocumentation.jsx';

export default function SettingsDialog({ initialTab = 'terminal', onClose, onTerminal, onFeaturesChanged, onLibraryChanged }) {
  const dialog = useModalDialog();
  const [tab, setTab] = useState(initialTab);
  const [features, setFeatures] = useState(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [help, setHelp] = useState(null);
  const [message, setMessage] = useState('');
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
    <div className="export-heading"><h2 id="settings-title"><Settings size={18}/>Settings</h2>
      <button type="button" className="icon-button" aria-label="Close settings" disabled={busy} onClick={onClose}><X size={18}/></button>
    </div>
    {!help && <div className="settings-sections" role="group" aria-label="Settings sections">
      {[['terminal', 'Terminal'], ['sync', 'Private sync'], ['continuation', 'Continuation']].map(([value, label]) =>
        <button type="button" className={`btn ${tab === value ? 'primary' : ''}`} aria-pressed={tab === value} key={value} disabled={busy} onClick={() => setTab(value)}>{label}</button>)}
    </div>}
    {help && <FeatureDocumentation topic={help} onClose={() => setHelp(null)}/>}
    {!help && tab === 'terminal' && <section className="settings-section" aria-label="Terminal preferences">
      <h3>Your terminal</h3><p>Choose the terminal used to resume sessions and launch continuations.</p>
      <button type="button" className="btn" onClick={onTerminal}>Choose terminal<ArrowRight size={14}/></button>
      <p className="terminal-settings-hint">Private device sync and cross-harness continuation are optional. Both start disabled and are enabled separately on each device.</p>
    </section>}
    {!help && tab === 'sync' && <DeviceSyncSettings onLibraryChanged={onLibraryChanged} onBusyChange={setBusy} onHelp={() => setHelp('sync')}/>}
    {!help && tab === 'continuation' && <section className="settings-section" aria-label="Cross-harness continuation">
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
    </section>}
    {error && <p className="data-warning" role="alert">{error}</p>}
    {message && <p className="data-success" role="status">{message}</p>}
    <div className="data-actions"><button type="button" className="btn" disabled={busy} onClick={onClose}>Done</button></div>
  </dialog>;
}
