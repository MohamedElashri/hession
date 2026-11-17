import { useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import userGuide from '../../docs/user-guide.md?raw';
import hostingGuide from '../../docs/self-hosting.md?raw';
import protocolGuide from '../../docs/sync-protocol.md?raw';

// Settings and the repository share one source for each topic. Promote its
// heading for the focused help page without including unrelated guide sections.
function section(heading) {
  const start = userGuide.indexOf(`## ${heading}\n`);
  if (start < 0) throw new Error(`Missing help section: ${heading}`);
  const end = userGuide.indexOf('\n## ', start + 1);
  return userGuide.slice(start, end < 0 ? undefined : end).replace(/^#(#+) /gm, '$1 ');
}

const guides = {
  sync: section('Private device sync'), hosting: hostingGuide,
  continuation: section('Cross-harness continuation'), security: section('Security and privacy'),
  terminal: section('Terminal commands and read-only MCP'), protocol: protocolGuide,
};
const paths = {
  'self-hosting.md': 'hosting', 'sync-protocol.md': 'protocol',
  '#private-device-sync': 'sync', '#cross-harness-continuation': 'continuation',
  '#security-and-privacy': 'security', '#mcp-server-permissions': 'terminal',
  '#terminal-commands-and-read-only-mcp': 'terminal',
};
const plugins = [remarkGfm];

function helpPage(href = '') {
  return paths[href.replace(/^user-guide\.md(?=#)/, '')]
    || paths[href.split('#')[0]];
}

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
      a: ({ href, children }) => helpPage(href)
        ? <a href={`#guide-${helpPage(href)}`} onClick={event => { event.preventDefault(); setPage(helpPage(href)); }}>{children}</a>
        : <span title={href}>{children}</span>,
    }}>{guides[page]}</ReactMarkdown></div>
  </section>;
}
