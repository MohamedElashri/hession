import { useId, useMemo, useState, useTransition } from 'react';
import { AlertCircle, Check, ChevronRight, Copy, Terminal } from 'lucide-react';
import { toolPresentation } from '../tool-presentation.js';
import { textPreview } from '../text-preview.js';
import '../styles/tool-activity.css';

/** Keep the original text intact, even when the initial rendering is bounded. */
export function TextPanel({ label, text, empty = 'Empty output.' }) {
  const id = useId();
  const [expanded, setExpanded] = useState(false);
  const [wrap, setWrap] = useState(true);
  const [copied, setCopied] = useState(null);
  const [pending, startTransition] = useTransition();
  const content = String(text ?? '');
  const { preview, lines } = useMemo(() => {
    let lines = 1;
    for (let i = 0; i < content.length; i++) if (content[i] === '\n') lines++;
    return { preview: textPreview(content, 12000, 80), lines };
  }, [content]);
  const shortened = preview.length < content.length;
  const copy = async () => {
    try { await navigator.clipboard.writeText(content); setCopied({ content, status: 'Copied' }); }
    catch { setCopied({ content, status: 'Copy failed' }); }
  };
  const copyState = copied?.content === content ? copied.status : '';
  return <section className="tool-panel" aria-labelledby={id}>
    <header className="tool-panel-head">
      <strong id={id}>{label}</strong><span className="tool-panel-size">{lines.toLocaleString()} line{lines === 1 ? '' : 's'}</span>
      <div className="tool-panel-actions">
        <button type="button" aria-label={`Wrap ${label.toLowerCase()}`} aria-pressed={wrap} onClick={() => setWrap(value => !value)}>Wrap</button>
        <button type="button" aria-label={`Copy ${label.toLowerCase()}`} onClick={copy}>{copyState === 'Copied' ? <Check size={12}/> : <Copy size={12}/>}Copy</button>
      </div>
      <span className="tool-copy-status" role="status">{copyState}</span>
    </header>
    {content ? <pre className={`tool-panel-text ${wrap ? 'is-wrapped' : 'is-unwrapped'}`} tabIndex={0} aria-label={`${label} text`}><code>{expanded ? content : preview}</code></pre> : <p className="tool-empty">{empty}</p>}
    {shortened && <footer className="tool-panel-footer"><span>{expanded ? 'Showing complete text' : `Preview · ${preview.length.toLocaleString()} of ${content.length.toLocaleString()} characters`}</span>
      <button type="button" disabled={pending} aria-expanded={expanded} onClick={() => startTransition(() => setExpanded(value => !value))}>{pending ? 'Opening…' : expanded ? 'Show preview' : `Show all ${content.length.toLocaleString()} characters`}</button>
    </footer>}
  </section>;
}

export default function ToolActivity({ tool, initiallyOpen = false }) {
  const [open, setOpen] = useState(initiallyOpen);
  const view = useMemo(() => toolPresentation(tool), [tool]);
  return <details className={`tool-activity ${view.failed ? 'tool-failed' : ''}`} open={open} onToggle={event => setOpen(event.currentTarget.open)}>
    <summary className="tool-activity-head">
      <ChevronRight size={14} className="tool-chevron"/><span className="tool-activity-icon">{view.failed ? <AlertCircle size={15}/> : <Terminal size={15}/>}</span>
      <span className="tool-activity-description"><span className="tool-activity-title"><span className="tool-activity-label">Tool call</span><strong>{view.name}</strong></span>
        {view.summary && <code className="tool-activity-preview">{view.summary}</code>}</span>
      <span className={`tool-outcome ${view.failed ? 'is-failed' : ''}`}>{view.outcome}</span>
    </summary>
    {open && <div className="tool-activity-body">
      {view.command != null && <TextPanel label="Command" text={view.command}/>}
      {view.code != null && <TextPanel label="Code" text={view.code}/>}
      {view.args && <details className="tool-arguments" open={view.command == null && view.code == null}><summary>{view.command != null || view.code != null ? 'All arguments' : 'Input arguments'}</summary><TextPanel label="Arguments" text={view.args}/></details>}
      {view.result != null ? <TextPanel label="Output" text={view.result}/> : <p className="tool-empty">No output recorded.</p>}
    </div>}
  </details>;
}
