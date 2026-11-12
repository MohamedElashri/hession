import { useEffect, useState, useSyncExternalStore } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Check, Download, ExternalLink, LoaderCircle, RefreshCw, RotateCcw, ShieldCheck } from 'lucide-react';
import { api } from '../api.js';
import { BRAND } from '../brand.js';
import { updates } from '../updates.js';

const working = new Set(['checking', 'downloading', 'verifying', 'installing', 'restarting']);

export default function SoftwareUpdate({ syncRunning }) {
  const state = useSyncExternalStore(updates.subscribe, updates.getSnapshot);
  const [linkError, setLinkError] = useState('');
  useEffect(() => { void updates.loadInfo(); }, []);
  const { phase, info, release, downloaded, total } = state;
  const busy = working.has(phase);
  const unsupported = info && !info.supported;
  const failed = state.error || state.metadataError || linkError;
  const percent = total > 0 ? Math.min(100, Math.round(downloaded / total * 100)) : undefined;
  const title = {
    idle: failed ? 'Couldn’t check for updates' : 'Software update',
    checking: 'Checking for updates…',
    current: 'You’re up to date',
    available: state.error ? 'Update couldn’t finish' : `${BRAND.name} ${release?.version} is available`,
    downloading: 'Downloading update…',
    verifying: 'Verifying update…',
    installing: 'Installing update…',
    ready: 'Ready to restart',
    restarting: `Restarting ${BRAND.name}…`,
  }[phase];
  const description = {
    idle: failed ? 'Try again, or download the latest release below.' : 'Check for the latest stable release when you’re ready.',
    checking: 'Looking for the latest stable release.',
    current: `You’re running the latest available version, ${info?.version}.`,
    available: info?.needsAuthorization ? 'Review the release notes, then download and install. Your system will ask for administrator authorization.' : 'Review the release notes, then download and install.',
    downloading: 'You can leave Settings while the download continues.',
    verifying: 'Checking the download’s signature and release version.',
    installing: `Keep ${BRAND.name} open until installation finishes.`,
    ready: `${BRAND.name} ${release?.version} is installed. Restart to use it. Your session library stays in place.`,
    restarting: 'Opening the updated app.',
  }[phase];
  const primaryAction = () => {
    setLinkError('');
    if (phase === 'ready') void updates.restart();
    else if (phase === 'available') void updates.install();
    else void updates.check();
  };
  const openReleases = async () => {
    setLinkError('');
    try { await api('open_release_page'); } catch (error) { setLinkError(error.message); }
  };
  const Icon = busy ? LoaderCircle : ['current', 'ready'].includes(phase) ? Check : Download;

  return <section aria-label="Software update">
    <p className="about-version">{BRAND.name} · Version {info?.version || '…'}</p>
    <section className={`update-status-card${failed ? ' has-error' : ''}`} aria-busy={busy}>
      <span className="update-status-icon"><Icon size={24} className={busy ? 'spin' : undefined}/></span>
      <div className="update-status-content">
        <div role="status" aria-live="polite"><h3>{unsupported ? 'Update this installation manually' : title}</h3><p>{unsupported ? info.reason : description}</p></div>
        {phase === 'downloading' && <div className="update-download-progress">
          <progress aria-label="Update download" max={100} value={percent}/>
          <span>{percent === undefined ? `${(downloaded / 1048576).toFixed(1)} MB downloaded` : `${percent}% · ${(downloaded / 1048576).toFixed(1)} MB`}</span>
        </div>}
        {!busy && !unsupported && <button type="button" className="btn primary" disabled={phase === 'ready' && syncRunning} onClick={primaryAction}>
          {phase === 'ready' ? <RotateCcw size={14}/> : phase === 'available' ? <Download size={14}/> : <RefreshCw size={14}/>}
          {phase === 'ready' ? `Restart ${BRAND.name}` : phase === 'available' ? (state.error ? 'Try again' : 'Download and install') : 'Check for updates'}
        </button>}
        {phase === 'ready' && syncRunning && <p>Wait for session sync to finish before restarting.</p>}
        {failed && <div className="data-error" role="alert">{failed}</div>}
      </div>
    </section>
    {release?.notes && <section className="update-release-notes" aria-label="Release notes">
      <h3>What’s new in {release.version}</h3>
      <div className="update-notes markdown-body"><ReactMarkdown remarkPlugins={[remarkGfm]}>{release.notes}</ReactMarkdown></div>
    </section>}
    <div className="update-footer"><span><ShieldCheck size={14}/>Signed updates · Checked only when you ask</span><button type="button" className="btn" onClick={openReleases}><ExternalLink size={14}/>All releases</button></div>
  </section>;
}
