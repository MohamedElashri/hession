import { useCallback, useEffect, useRef, useState } from 'react';
import { Copy, LoaderCircle, RefreshCw, Square } from 'lucide-react';
import { api } from '../api.js';
import { canEnableDeviceSync, canDisconnectDeviceSync } from '../device-sync-settings.js';

export default function DeviceSyncSettings({ onLibraryChanged, onBusyChange, onHelp }) {
  const [settings, setSettings] = useState(null);
  const [relayUrl, setRelayUrl] = useState('');
  const [deviceName, setDeviceName] = useState('');
  const [scope, setScope] = useState([]);
  const [mode, setMode] = useState('create');
  const [bootstrapToken, setBootstrapToken] = useState('');
  const [invitation, setInvitation] = useState('');
  const [recoveryInput, setRecoveryInput] = useState('');
  const [recoveryKey, setRecoveryKey] = useState('');
  const [recoverySaved, setRecoverySaved] = useState(false);
  const [pairing, setPairing] = useState(null);
  const [fingerprintConfirmed, setFingerprintConfirmed] = useState(false);
  const [createdHere, setCreatedHere] = useState(false);
  const [preview, setPreview] = useState(null);
  const [busy, setBusy] = useState('');
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const operation = useRef(false);
  const applySettings = useCallback(value => {
    setSettings(value); setRelayUrl(value.relayUrl || '');
    setDeviceName(value.deviceName || ''); setScope(value.scope || []);
  }, []);
  useEffect(() => {
    let active = true;
    api('get_device_sync_settings').then(value => { if (active) applySettings(value); })
      .catch(e => { if (active) setError(e.message); });
    return () => { active = false; };
  }, [applySettings]);
  useEffect(() => {
    onBusyChange(!!busy);
    return () => onBusyChange(false);
  }, [busy, onBusyChange]);
  useEffect(() => {
    if (busy !== 'sync') return;
    let active = true;
    const timer = window.setInterval(() => {
      api('get_device_sync_settings').then(value => { if (active) setSettings(value); }).catch(() => {});
    }, 800);
    return () => { active = false; window.clearInterval(timer); };
  }, [busy]);
  const perform = async (action, task) => {
    // React batches state updates; guard before starting IPC so repeated
    // activation cannot release another operation's busy state prematurely.
    if (operation.current) return;
    operation.current = true;
    setBusy(action); setError(''); setMessage('');
    try { await task(); }
    catch (e) {
      setError(e.message);
      // Setup may have committed on the relay before a network interruption.
      // Keep its locally saved key reachable rather than inviting a blind retry.
      if (action === 'configure') {
        try { applySettings(await api('get_device_sync_settings')); } catch { /* preserve setup error */ }
        setBootstrapToken(''); setInvitation(''); setRecoveryInput('');
      }
    }
    finally { operation.current = false; setBusy(''); }
  };
  const previewScope = () => perform('preview', async () => {
    const value = await api('preview_device_sync', { workspaces: scope });
    setPreview({ ...value, scope: JSON.stringify(scope) });
  });
  const previewValid = preview?.scope === JSON.stringify(scope);
  const enableAllowed = canEnableDeviceSync(settings, { previewValid, createdHere, recoverySaved, fingerprintConfirmed });
  const disconnectAllowed = canDisconnectDeviceSync(settings, { recoveryVisible: !!recoveryKey, recoverySaved });
  const configure = () => perform('configure', async () => {
    if (mode === 'create') {
      const value = await api('setup_device_sync', { relayUrl, bootstrapToken, deviceName, workspaces: scope });
      applySettings(value.settings); setRecoveryKey(value.recoveryKey); setRecoverySaved(false);
      setCreatedHere(true);
      setMessage('Library created. Save your recovery key, then enable sync. No history has been uploaded.');
    } else {
      applySettings(await api('join_device_sync', { relayUrl, invitation, recoveryKey: recoveryInput, deviceName, workspaces: scope }));
      setFingerprintConfirmed(false);
      setCreatedHere(false);
      setMessage('Device connected. Verify the library fingerprint with your trusted device, then enable sync.');
    }
    setBootstrapToken(''); setInvitation(''); setRecoveryInput('');
  });
  const toggleEnabled = () => perform('enable', async () => {
    applySettings(await api('set_device_sync_enabled', { enabled: !settings.enabled, workspaces: scope }));
    setMessage(settings.enabled ? 'Device sync disabled. Existing local and relay copies are kept.' : 'Device sync enabled. Choose Sync now to transfer records.');
  });
  const syncNow = () => perform('sync', async () => {
    try {
      const value = await api('run_device_sync');
      setSettings(value);
      if (value.status?.error) throw new Error(value.status.error);
      setMessage(`Sync complete: ${value.status?.uploaded || 0} uploaded, ${value.status?.downloaded || 0} downloaded.`);
    } finally {
      // Cancellation or a late request failure may still leave completed imports.
      try { setSettings(await api('get_device_sync_settings')); } catch { /* retain last local status */ }
      await onLibraryChanged();
    }
  });
  const copy = value => perform('copy', async () => {
    await navigator.clipboard.writeText(value); setMessage('Copied. Share secrets only through a trusted channel.');
  });
  return <section className="settings-section" aria-label="Private device sync">
    <h3>Your devices, your relay</h3>
    <p>Sync selected histories through your self-hosted relay. Content is encrypted on this device and searched locally. Sync is manual and starts disabled.</p>
    <button type="button" className="btn" disabled={!!busy} onClick={onHelp}>Setup and self-hosting guide</button>
    {!settings && !error && <p role="status">Loading device settings…</p>}
    {settings && <>
      {settings.setupPending && <p className="data-warning" role="alert">Setup was interrupted. Your encryption key is saved securely on this device. Show and save the recovery key before disconnecting. If the relay already created the library, create an invitation with relay administrator access and join it again.</p>}
      {!settings.configured && !settings.setupPending && <>
        <div className="settings-inline" role="group" aria-label="Library setup">
          <button type="button" className={`btn ${mode === 'create' ? 'primary' : ''}`} aria-pressed={mode === 'create'} disabled={!!busy} onClick={() => setMode('create')}>Create library</button>
          <button type="button" className={`btn ${mode === 'join' ? 'primary' : ''}`} aria-pressed={mode === 'join'} disabled={!!busy} onClick={() => setMode('join')}>Join library</button>
        </div>
        <label>Relay address<input type="url" value={relayUrl} disabled={!!busy} onChange={event => setRelayUrl(event.target.value)} placeholder="https://sync.example.com" autoComplete="off"/></label>
        <button type="button" className="btn" disabled={!!busy || !relayUrl.trim()} onClick={() => perform('test', async () => {
          const value = await api('test_device_sync_relay', { relayUrl });
          setMessage(`Relay reachable. ${value.initialized ? 'An existing library is ready to join.' : 'Ready to create a library.'} No history was sent.`);
        })}>Test connection</button>
        <label>Device name<input value={deviceName} disabled={!!busy} maxLength={80} onChange={event => setDeviceName(event.target.value)} placeholder="My laptop" autoComplete="off"/></label>
        {mode === 'create' ? <label>Relay setup token<input type="password" value={bootstrapToken} disabled={!!busy} onChange={event => setBootstrapToken(event.target.value)} autoComplete="new-password"/></label> : <>
          <label>Pairing invitation<input type="password" value={invitation} disabled={!!busy} onChange={event => setInvitation(event.target.value)} autoComplete="new-password"/></label>
          <label>Library recovery key<input type="password" value={recoveryInput} disabled={!!busy} onChange={event => setRecoveryInput(event.target.value)} autoComplete="new-password"/></label>
          <p className="terminal-settings-hint">Get an invitation from an enabled trusted device and use your saved recovery key. Compare the library fingerprint before transferring history.</p>
        </>}
      </>}
      {settings.configured && <>
        <p><strong>{settings.enabled ? 'Enabled' : 'Disabled'}</strong> · {settings.deviceName}<br/><span className="terminal-settings-hint">{settings.relayUrl}</span></p>
        <p className="terminal-settings-hint">Library fingerprint: <code>{settings.keyFingerprint}</code></p>
        {!settings.enabled && !createdHere && <label className="settings-switch"><input type="checkbox" checked={fingerprintConfirmed} disabled={!!busy} onChange={event => setFingerprintConfirmed(event.target.checked)}/><span>I verified this fingerprint against my trusted device or saved recovery information</span></label>}
      </>}
      <fieldset className="terminal-settings-fields" disabled={!!busy || settings.enabled}>
        <legend>Histories this device may upload</legend>
        <p className="terminal-settings-hint">Only selected workspaces are uploaded, including their existing history. Select none for download-only sync. Disable sync before changing this selection.</p>
        <div className="settings-scope">
          {(settings.workspaces || []).map(workspace => <label className="settings-switch" key={workspace.path}>
            <input type="checkbox" checked={scope.includes(workspace.path)} onChange={event => {
              setScope(value => event.target.checked ? [...value, workspace.path] : value.filter(path => path !== workspace.path)); setPreview(null);
            }}/><span>{workspace.path} <small>({workspace.count} sessions)</small></span>
          </label>)}
          {!settings.workspaces?.length && <p className="terminal-settings-hint">No imported workspaces yet. Refresh histories before selecting uploads.</p>}
        </div>
      </fieldset>
      <button type="button" className="btn" disabled={!!busy} onClick={previewScope}>Preview selected histories</button>
      {previewValid && <p className="terminal-settings-hint" role="status">{preview.sessions} sessions · {preview.entries} entries · approximately {Math.ceil(preview.estimatedBytes / 1024).toLocaleString()} KiB before encryption. Tool payloads, images, and private reasoning are omitted. Redaction is best-effort; conversation text may still contain private code or secrets.</p>}
      {!settings.configured && !settings.setupPending && <button type="button" className="btn primary" disabled={!!busy || !previewValid || !relayUrl.trim() || !deviceName.trim() || (mode === 'create' ? !bootstrapToken.trim() : !invitation.trim() || !recoveryInput.trim())} onClick={configure}>{mode === 'create' ? 'Create encrypted library' : 'Connect this device'}</button>}
      {recoveryKey && <>
        <p className="data-warning">Save this recovery key somewhere safe. The relay cannot recover it. You can show it again while this device can access its credential store.</p>
        <pre className="settings-secret">{recoveryKey}</pre>
        <button type="button" className="btn" disabled={!!busy} onClick={() => copy(recoveryKey)}><Copy size={14}/>Copy recovery key</button>
        <label className="settings-switch"><input type="checkbox" checked={recoverySaved} disabled={!!busy} onChange={event => setRecoverySaved(event.target.checked)}/><span>I have saved my recovery key</span></label>
      </>}
      {settings.configured && <div className="settings-inline">
        <button type="button" className={`btn ${settings.enabled ? '' : 'primary'}`} disabled={!!busy || (!settings.enabled && !enableAllowed)} onClick={toggleEnabled}>{settings.enabled ? 'Disable device sync' : 'Enable device sync'}</button>
        {settings.enabled && <button type="button" className="btn primary" disabled={!!busy} onClick={syncNow}><RefreshCw size={14}/>Sync now</button>}
        {busy === 'sync' && <button type="button" className="btn" onClick={() => api('stop_device_sync').catch(e => setError(e.message))}><Square size={13}/>Stop</button>}
      </div>}
      {settings.enabled && <button type="button" className="btn" disabled={!!busy} onClick={() => perform('invite', async () => setPairing(await api('create_device_sync_invitation')))}>Pair another device</button>}
      {(settings.configured || settings.setupPending) && <div className="settings-inline">
        <button type="button" className="btn" disabled={!!busy} onClick={() => perform('recovery', async () => {
          const value = await api('export_device_sync_recovery_key'); setRecoveryKey(value.recoveryKey); setRecoverySaved(false);
        })}>Show recovery key</button>
        <button type="button" className="btn" disabled={!!busy || !disconnectAllowed} onClick={() => perform('disconnect', async () => {
          applySettings(await api('disconnect_device_sync')); setRecoveryKey(''); setRecoverySaved(false); setPairing(null); setPreview(null); setFingerprintConfirmed(false); setCreatedHere(false);
          setMessage('Relay disconnected on this device. Local histories and records on the relay remain.');
        })}>Disconnect relay</button>
      </div>}
      {settings.configured && <details><summary>Repair after restoring a relay backup</summary>
        <p className="terminal-settings-hint">If you restored an older relay backup, disable device sync, reset its local checkpoint, then re-enable and sync. This replays available records and republishes selected local histories. Local copies and queued transfers are kept.</p>
        <button type="button" className="btn" disabled={!!busy || settings.enabled} onClick={() => perform('repair', async () => {
          applySettings(await api('reset_device_sync_checkpoint'));
          setMessage('Checkpoint reset. Review your scope, re-enable device sync, and choose Sync now.');
        })}>Replay after relay restore</button>
      </details>}
      {pairing && <>
        <p>On the other device, choose <strong>Join library</strong>. Use this invitation and your saved recovery key.</p>
        <pre className="settings-secret">{pairing.invitation}</pre>
        <p className="terminal-settings-hint">Expires {new Date(typeof pairing.expiresAt === 'number' ? pairing.expiresAt * 1000 : pairing.expiresAt).toLocaleString()}. Verify fingerprint <code>{pairing.keyFingerprint}</code> on both devices.</p>
        <button type="button" className="btn" disabled={!!busy} onClick={() => copy(pairing.invitation)}><Copy size={14}/>Copy invitation</button>
      </>}
    </>}
    {busy && <p className="data-status" role="status"><LoaderCircle className="spin" size={14}/>{busy === 'sync' ? `Syncing · ${settings?.status?.phase || 'connecting'} · ${settings?.status?.uploaded || 0} uploaded · ${settings?.status?.downloaded || 0} downloaded` : 'Working…'}</p>}
    {error && <p className="data-warning" role="alert">{error}</p>}
    {message && <p className="data-success" role="status">{message}</p>}
  </section>;
}
