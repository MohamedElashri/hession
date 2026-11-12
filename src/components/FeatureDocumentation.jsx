import { useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import syncGuide from '../../docs/private-sync.md?raw';
import hostingGuide from '../../docs/self-hosting.md?raw';
import continuationGuide from '../../docs/continuation.md?raw';
import securityGuide from '../../docs/sync-security.md?raw';

const guides = {
  sync: syncGuide, hosting: hostingGuide, continuation: continuationGuide, security: securityGuide,
};
const paths = { 'private-sync.md': 'sync', 'self-hosting.md': 'hosting', 'continuation.md': 'continuation', 'sync-security.md': 'security' };
const plugins = [remarkGfm];

export default function FeatureDocumentation({ topic, onClose }) {
  const [page, setPage] = useState(topic);
  return <section className="settings-section" aria-label="Setup documentation">
    <div className="settings-inline">
      <button type="button" className="btn" onClick={onClose}>Back to settings</button>
      {topic === 'sync' && <>
        <button type="button" className="btn" onClick={() => setPage(page === 'hosting' ? 'sync' : 'hosting')}>{page === 'hosting' ? 'Device setup' : 'Self-hosting guide'}</button>
        <button type="button" className="btn" onClick={() => setPage(page === 'security' ? 'sync' : 'security')}>{page === 'security' ? 'Device setup' : 'Security and privacy'}</button>
      </>}
    </div>
    <div className="markdown-body feature-documentation"><ReactMarkdown remarkPlugins={plugins} components={{
      a: ({ href, children }) => paths[href]
        ? <a href={`#guide-${paths[href]}`} onClick={event => { event.preventDefault(); setPage(paths[href]); }}>{children}</a>
        : <span title={href}>{children}</span>,
    }}>{guides[page]}</ReactMarkdown></div>
  </section>;
}
