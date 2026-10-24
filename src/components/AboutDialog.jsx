import { useEffect, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Check, Copy, ExternalLink, LoaderCircle, RefreshCw, Terminal, X } from 'lucide-react';
import { api } from '../api.js';
import { BRAND } from '../brand.js';
import BrandMark from './BrandMark.jsx';
import useModalDialog from './useModalDialog.js';

const dateOf = value => (value ? new Date(value).toLocaleDateString([], { dateStyle: 'medium' }) : '');

/**
 * App version and a manual update check. Nothing is checked until the user
 * clicks, and updating always runs the official installer in a visible terminal.
 */
export default function AboutDialog({ onClose }) {
  const dialog = useModalDialog();
  const [version, setVersion] = useState('');
  const [state, setState] = useState({ status: 'idle' }); // idle | checking | done | error
  const [busy, setBusy] = useState('');
  const [message, setMessage] = useState('');
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    api('app_version').then(info => setVersion(info.version)).catch(() => {});
  }, []);

  const check = async () => {
    setState({ status: 'checking' }); setMessage('');
    try { setState({ status: 'done', info: await api('check_for_update') }); }
    catch (e) { setState({ status: 'error', error: e.message }); }
  };
  const act = async (label, fn) => {
    setBusy(label); setMessage('');
    try { await fn(); } catch (e) { setMessage(e.message); } finally { setBusy(''); }
  };
  const info = state.info;
  const update = () => act('update', async () => {
    const result = await api('run_update', { tag: info.tag });
    setMessage(`The installer is running in ${result.terminal}. Restart ${BRAND.name} when it finishes.`);
  });
  const copy = () => act('copy', async () => {
    await navigator.clipboard.writeText(await api('update_command', { tag: info.tag }));
    setCopied(true); setTimeout(() => setCopied(false), 1400);
  });
  const openPage = () => act('open', () => api('open_release_page', { tag: info.tag }));

  return <dialog ref={dialog} className="data-dialog about-dialog" aria-labelledby="about-title" onCancel={event => { event.preventDefault(); onClose(); }}>
    <div className="export-heading">
      <h2 id="about-title"><span className="brand-mark"><BrandMark size={18}/></span>{BRAND.name}</h2>
      <button type="button" className="icon-button" aria-label="Close" onClick={onClose}><X size={18}/></button>
    </div>
    <p className="about-version">Version {version || '…'}</p>
    <p>{BRAND.tagline}</p>

    <section className="about-updates" aria-live="polite">
      <h3 className="remote-heading">Updates</h3>
      <p>{BRAND.name} never checks or updates on its own. A check sends one request to GitHub for the latest release; nothing about you or your library is shared.</p>
      {state.status !== 'done' && <button type="button" className="btn" disabled={state.status === 'checking'} onClick={check}>
        {state.status === 'checking' ? <LoaderCircle className="spin" size={14}/> : <RefreshCw size={14}/>}
        {state.status === 'checking' ? 'Checking…' : 'Check for updates'}
      </button>}
      {state.status === 'error' && <div className="data-error" role="alert">{state.error}</div>}
      {state.status === 'done' && !info.newer && <div className="data-success"><Check size={14}/>You have the latest version ({info.current}).</div>}
      {state.status === 'done' && info.newer && <div className="update-available">
        <strong>Version {info.latest} is available</strong>
        <span className="remote-meta">You have {info.current}{info.publishedAt ? ` · released ${dateOf(info.publishedAt)}` : ''}</span>
        {info.notes && <div className="update-notes markdown-body"><ReactMarkdown remarkPlugins={[remarkGfm]}>{info.notes}</ReactMarkdown></div>}
        <p className="remote-meta">Updating opens a terminal running this release&apos;s install script, which verifies the download before installing. You may be asked for your password.</p>
        <div className="data-actions">
          <button type="button" className="btn" disabled={Boolean(busy)} onClick={openPage}><ExternalLink size={14}/>View release</button>
          <button type="button" className="btn" disabled={Boolean(busy)} onClick={copy}>{copied ? <Check size={14}/> : <Copy size={14}/>}{copied ? 'Copied' : 'Copy command'}</button>
          <button type="button" className="btn primary" disabled={Boolean(busy)} onClick={update}>{busy === 'update' ? <LoaderCircle className="spin" size={14}/> : <Terminal size={14}/>}Update in terminal</button>
        </div>
      </div>}
      {message && <div className="data-status" role="status">{message}</div>}
    </section>
  </dialog>;
}
