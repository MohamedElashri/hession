import { useState, useSyncExternalStore } from 'react';
import { api } from '../api.js';
import { BRAND } from '../brand.js';
import { updates } from '../updates.js';
import BrandMark from './BrandMark.jsx';
import SoftwareUpdate from './SoftwareUpdate.jsx';

export default function AboutSettings({ syncRunning }) {
  const { info } = useSyncExternalStore(updates.subscribe, updates.getSnapshot);
  const [linkError, setLinkError] = useState('');

  const openLink = async (target) => {
    setLinkError('');
    try {
      await api('open_project_link', { target });
    } catch (error) {
      setLinkError(error.message);
    }
  };

  return <>
    <div className="settings-about-brand">
      <span className="brand-mark"><BrandMark size={24}/></span>
      <div><h3>{BRAND.name}</h3><p>{info?.version ? `Version ${info.version}` : 'Loading version…'}</p></div>
    </div>
    <p>{BRAND.tagline}</p>
    <nav className="settings-about-links" aria-label="Project links">
      <button type="button" className="btn" onClick={() => openLink('website')}>Project website</button>
      <button type="button" className="btn" onClick={() => openLink('source')}>Source code</button>
      <button type="button" className="btn" onClick={() => openLink('issues')}>Report an issue</button>
      <button type="button" className="btn" onClick={() => openLink('releases')}>Release notes</button>
    </nav>
    <div className="settings-about-credits">
      <p>Maintained by {BRAND.author}</p>
      <button type="button" className="btn" onClick={() => openLink('license')}>MIT license</button>
    </div>
    <div className="settings-about-support">
      <button type="button" className="btn" onClick={() => openLink('support')}>Support on Ko-fi</button>
      {linkError && <p className="data-error" role="alert">{linkError}</p>}
    </div>
    <SoftwareUpdate syncRunning={syncRunning} showVersion={false}/>
  </>;
}
