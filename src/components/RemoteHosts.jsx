import { useCallback, useEffect, useState } from 'react';
import { Server, Plus, Trash2, LoaderCircle, PlugZap, X } from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { BRAND } from '../brand.js';
import { familyLabel } from '../labels.js';
import useModalDialog from './useModalDialog.js';

const agentList = agents => agents.map(familyLabel).join(', ');
const when = value => value ? new Date(value).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' }) : 'never';

/** Configure SSH hosts whose agent sessions are mirrored and imported on sync. */
export default function RemoteHosts({ onClose, onSyncNow, syncRunning }) {
  const dialog = useModalDialog();
  const [data, setData] = useState(null);
  const [error, setError] = useState('');
  const [custom, setCustom] = useState('');
  const [tests, setTests] = useState({});
  const [saving, setSaving] = useState(false);
  const [changed, setChanged] = useState(false);
  const desktop = Boolean(window.__TAURI_INTERNALS__?.invoke);

  const load = useCallback(async () => {
    if (!desktop) { setError('Remote hosts are available in the desktop app.'); return; }
    try { setData(await invoke('get_remote_hosts')); setError(''); }
    catch (e) { setError(String(e?.message || e)); }
  }, [desktop]);

  useEffect(() => {
    load();
  }, [load]);

  const save = async hosts => {
    setSaving(true); setError('');
    try {
      await invoke('set_remote_hosts', { hosts: hosts.map(({ name, enabled }) => ({ name, enabled })) });
      setChanged(true);
      await load();
    } catch (e) { setError(String(e?.message || e)); }
    finally { setSaving(false); }
  };
  const hosts = data?.hosts || [];
  const add = name => { const n = name.trim(); if (n && !hosts.some(h => h.name === n)) save([...hosts, { name: n, enabled: true }]); };
  const toggle = name => save(hosts.map(h => h.name === name ? { ...h, enabled: !h.enabled } : h));
  const remove = name => save(hosts.filter(h => h.name !== name));
  const test = async name => {
    setTests(t => ({ ...t, [name]: { busy: true } }));
    try { const result = await invoke('test_remote_host', { name }); setTests(t => ({ ...t, [name]: result })); }
    catch (e) { setTests(t => ({ ...t, [name]: { ok: false, error: String(e?.message || e) } })); }
  };
  const testLabel = name => {
    const t = tests[name];
    if (!t) return null;
    if (t.busy) return <span className="remote-test"><LoaderCircle className="spin" size={12}/>Connecting…</span>;
    if (!t.ok) return <span className="remote-test remote-bad">{t.error}</span>;
    if (!t.rsync) return <span className="remote-test remote-bad">Connected, but rsync is not installed on the host.</span>;
    return <span className="remote-test remote-good">Connected · {t.agents.length ? agentList(t.agents) : 'no agent history found'}</span>;
  };
  const close = () => { if (!saving) onClose(); };
  const missingTools = data && (!data.tools.ssh || !data.tools.rsync);

  return <dialog ref={dialog} className="data-dialog remote-dialog" aria-labelledby="remote-title" aria-describedby="remote-description"
    onCancel={event => { event.preventDefault(); close(); }}>
    <div className="export-heading"><h2 id="remote-title"><Server size={18}/>Remote hosts</h2><button type="button" className="icon-button" aria-label="Close remote hosts" disabled={saving} onClick={close}><X size={18}/></button></div>
    <p id="remote-description">Sessions you run on SSH hosts (including through VS Code Remote-SSH) are copied here on each sync and appear next to your local ones. {BRAND.name} connects with your own <code>ssh</code> configuration, without prompting, so hosts need key, agent, or Kerberos login. Only transcript files are copied, using <code>rsync</code>; nothing on the host is changed.</p>
    {missingTools && <p className="data-warning">Install {[!data.tools.ssh && 'ssh', !data.tools.rsync && 'rsync'].filter(Boolean).join(' and ')} on this computer to sync remote hosts.</p>}

    <h3 className="remote-heading">Your hosts</h3>
    {!data && !error && <div className="data-status"><LoaderCircle className="spin" size={14}/>Loading hosts…</div>}
    {data && !hosts.length && <p className="remote-empty">No hosts yet. Add one from the suggestions below or type an SSH destination.</p>}
    <ul className="remote-list">
      {hosts.map(h => <li key={h.name}>
        <label className="data-checkbox"><input type="checkbox" checked={h.enabled} disabled={saving} onChange={() => toggle(h.name)}/><strong>{h.name}</strong></label>
        <span className="remote-meta">Last fetched {when(h.lastSync)}{h.agents.length ? ` · ${agentList(h.agents)}` : ''}</span>
        {testLabel(h.name)}
        <span className="remote-actions">
          <button type="button" className="data-button" disabled={tests[h.name]?.busy} onClick={() => test(h.name)}><PlugZap size={13}/>Test</button>
          <button type="button" className="data-button data-danger" disabled={saving} onClick={() => remove(h.name)} aria-label={`Remove ${h.name}`}><Trash2 size={13}/></button>
        </span>
      </li>)}
    </ul>

    <form className="remote-add" onSubmit={event => { event.preventDefault(); add(custom); setCustom(''); }}>
      <input value={custom} onChange={event => setCustom(event.target.value)} placeholder="SSH alias or user@host" aria-label="SSH host" spellCheck={false} autoComplete="off"/>
      <button type="submit" className="data-button" disabled={!custom.trim() || saving}><Plus size={14}/>Add</button>
    </form>

    {data?.suggestions?.length > 0 && <>
      <h3 className="remote-heading">Suggestions</h3>
      <div className="remote-suggestions">
        {data.suggestions.map(s => <button key={s.name} type="button" className="data-button" disabled={saving} title={s.source === 'vscode' ? 'Recently opened in VS Code Remote-SSH' : 'From ~/.ssh/config'} onClick={() => add(s.name)}>
          <Plus size={12}/>{s.name}{s.source === 'vscode' && <span className="remote-tag">VS Code</span>}
        </button>)}
      </div>
    </>}

    {error && <div className="data-error" role="alert">{error}</div>}
    {changed && <div className="data-actions remote-footer">
      <button type="button" className="data-button" disabled={syncRunning || saving} onClick={() => { onSyncNow(); onClose(); }}>Sync now</button>
      {syncRunning && <span className="remote-meta">A sync is running; changes apply to the next one.</span>}
    </div>}
  </dialog>;
}
