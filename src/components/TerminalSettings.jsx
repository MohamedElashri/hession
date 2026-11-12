import { useEffect, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { Folder, RefreshCw } from 'lucide-react';
import { api } from '../api.js';
import { customTerminalArguments, TERMINAL_ARGUMENTS } from '../terminal-settings.js';

export default function TerminalSettings({ onBusyChange }) {
  const [data, setData] = useState(null);
  const [terminal, setTerminal] = useState('default');
  const [path, setPath] = useState('');
  const [args, setArgs] = useState(TERMINAL_ARGUMENTS.generic.join('\n'));
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let cancelled = false;
    api('list_terminals').then(result => {
      if (cancelled) return;
      setData(result); setTerminal(result.saved || 'default');
      setPath(result.customPath || ''); setArgs((result.customArgs || TERMINAL_ARGUMENTS.generic).join('\n'));
      setError('');
    }).catch(e => { if (!cancelled) setError(e.message); });
    return () => { cancelled = true; };
  }, [revision]);
  const browse = async () => {
    try {
      const selected = await open({ title: 'Choose a terminal executable', multiple: false, directory: false });
      if (typeof selected === 'string') setPath(selected);
    } catch (e) { setError(e.message); }
  };
  const save = async () => {
    setBusy(true); onBusyChange(true); setError(''); setSaved(false);
    try {
      const custom = terminal === 'custom' ? { customPath: path, customArgs: customTerminalArguments(args) } : {};
      await api('set_terminal_preference', { terminal, ...custom });
      setSaved(true);
    } catch (e) { setError(e.message); }
    finally { setBusy(false); onBusyChange(false); }
  };
  return <section aria-label="Terminal preferences" onChange={() => setSaved(false)}>
    <p>Choose the terminal used to resume sessions and launch continuations.</p>
    {!data && !error && <p role="status">Discovering terminals…</p>}
    {data && <fieldset className="terminal-settings-fields" disabled={busy}>
      <label>Terminal<select value={terminal} onChange={event => setTerminal(event.target.value)}>
        <option value="default">Default terminal (automatic)</option>
        {data.terminals.filter(item => !['default', 'custom'].includes(item.id)).map(item =>
          <option key={item.id} value={item.id} disabled={!item.available}>{item.label}{!item.available ? ' — unavailable' : ''}</option>)}
        <option value="custom">Custom executable…</option>
      </select></label>
      <p className="terminal-settings-hint">Automatic uses the platform default when available, then an installed terminal. If a saved choice is unavailable, Resume shows a fallback warning.</p>
      {terminal === 'custom' && <>
        <div className="terminal-executable"><label htmlFor="terminal-executable-path">Executable path</label>
          <div className="terminal-path"><input id="terminal-executable-path" value={path} onChange={event => setPath(event.target.value)} placeholder="/absolute/path/to/terminal"/>
            <button type="button" className="btn" onClick={browse}><Folder size={14}/>Browse</button></div>
        </div>
        <label>Launch convention<select value={Object.keys(TERMINAL_ARGUMENTS).find(key => TERMINAL_ARGUMENTS[key].join('\n') === args) || 'custom'} onChange={event => { if (event.target.value !== 'custom') setArgs(TERMINAL_ARGUMENTS[event.target.value].join('\n')); }}>
          <option value="generic">Generic / Konsole / Alacritty / XTerm (-e)</option>
          <option value="gnome">GNOME Terminal / Kitty / Foot (--)</option>
          <option value="wezterm">WezTerm (start --)</option>
          <option value="xfce">Xfce Terminal (--execute)</option>
          <option value="custom">Custom arguments</option>
        </select></label>
        <label>Launch arguments (one per line)<textarea rows={5} value={args} onChange={event => setArgs(event.target.value)} spellCheck={false}/></label>
        <p className="terminal-settings-hint">Include {'{script}'} once on its own line. It becomes the resume script path. Arguments are passed literally; omit shell quotes. On macOS select the executable inside an app bundle.</p>
      </>}
    </fieldset>}
    {error && <p className="data-warning" role="alert">{error}</p>}
    {saved && <p className="data-success" role="status">Terminal preference saved.</p>}
    <div className="data-actions">
      <button type="button" className="btn" disabled={busy} onClick={() => { setData(null); setError(''); setRevision(revision + 1); }}><RefreshCw size={14}/>Discover again</button>
      <button type="button" className="btn primary" disabled={!data || busy} onClick={save}>{busy ? 'Saving…' : 'Save'}</button>
    </div>
  </section>;
}
