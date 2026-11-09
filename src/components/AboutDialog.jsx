import { useEffect, useState } from 'react';
import { Download, X } from 'lucide-react';
import { api } from '../api.js';
import { BRAND } from '../brand.js';
import BrandMark from './BrandMark.jsx';
import useModalDialog from './useModalDialog.js';

export default function AboutDialog({ onClose, onUpdates }) {
  const dialog = useModalDialog();
  const [version, setVersion] = useState('');
  useEffect(() => {
    api('app_version').then(info => setVersion(info.version)).catch(() => {});
  }, []);
  return <dialog ref={dialog} className="data-dialog about-dialog" aria-labelledby="about-title" onCancel={event => { event.preventDefault(); onClose(); }}>
    <div className="export-heading">
      <h2 id="about-title"><span className="brand-mark"><BrandMark size={18}/></span>{BRAND.name}</h2>
      <button type="button" className="icon-button" aria-label="Close" onClick={onClose}><X size={18}/></button>
    </div>
    <p className="about-version">Version {version || '…'}</p>
    <p>{BRAND.tagline}</p>
    <button type="button" className="btn" onClick={onUpdates}><Download size={14}/>Software update</button>
  </dialog>;
}
