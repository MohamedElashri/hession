import { useId, useMemo, useState, useTransition } from 'react';
import { AlertCircle, Braces, Check, CheckCircle2, ChevronRight, Code2, Copy, FileOutput, Terminal, XCircle } from 'lucide-react';
import { toolPresentation } from '../tool-presentation.js';
import { textPreview } from '../text-preview.js';
import { canHighlight, recordedSyntax, syntaxLanguage, SYNTAX_LANGUAGES } from '../syntax.js';
import HighlightedCode from './HighlightedCode.jsx';
import '../styles/tool-activity.css';

/** Keep the original text intact, even when the initial rendering is bounded. */
export function TextPanel({ label, text, language, empty = 'Empty output.' }) {
  const id = useId();
  const PanelIcon = /output/i.test(label) ? FileOutput : label === 'Arguments' ? Braces : label === 'Code' ? Code2 : Terminal;
  const [expanded, setExpanded] = useState(false);
  const [wrap, setWrap] = useState(true);
  const [selectedLanguage, setSelectedLanguage] = useState(null);
  const [copied, setCopied] = useState(null);
  const [pending, startTransition] = useTransition();
  const content = String(text ?? '');
  const { preview, lines } = useMemo(() => {
    let lines = 1;
    for (let i = 0; i < content.length; i++) if (content[i] === '\n') lines++;
    return { preview: textPreview(content, 12000, 80), lines };
  }, [content]);
  const shortened = preview.length < content.length;
  const displayed = expanded ? content : preview;
  const inferredLanguage = useMemo(() => syntaxLanguage(language ?? recordedSyntax(content)), [content, language]);
  const syntax = selectedLanguage ?? inferredLanguage;
  const copy = async () => {
    try { await navigator.clipboard.writeText(content); setCopied({ content, status: 'Copied' }); }
    catch { setCopied({ content, status: 'Copy failed' }); }
  };
  const copyState = copied?.content === content ? copied.status : '';
  return <section className="tool-panel" data-panel={label.toLowerCase()} aria-labelledby={id}>
    <header className="tool-panel-head">
      <strong id={id}><PanelIcon size={14}/>{label}</strong><span className="tool-panel-size">{lines.toLocaleString()} line{lines === 1 ? '' : 's'}</span>
      <div className="tool-panel-actions">
        <select className="syntax-select" title="Syntax highlighting" aria-label={`Syntax for ${label.toLowerCase()}`} value={syntax} onChange={event => setSelectedLanguage(event.target.value)}>
          {SYNTAX_LANGUAGES.map(([value, name]) => <option key={value} value={value}>{name}</option>)}
        </select>
        <button type="button" aria-label={`Wrap ${label.toLowerCase()}`} aria-pressed={wrap} onClick={() => setWrap(value => !value)}>Wrap lines</button>
        <button type="button" aria-label={`Copy ${label.toLowerCase()}`} onClick={copy}>{copyState === 'Copied' ? <Check size={12}/> : <Copy size={12}/>} {copyState === 'Copied' ? 'Copied' : 'Copy'}</button>
      </div>
      <span className={`tool-copy-status ${copyState === 'Copied' ? 'tool-sr-only' : ''}`} role="status">{copyState}</span>
      {syntax !== 'plaintext' && displayed && !canHighlight(displayed, syntax) && <span className="syntax-limit">Large block · plain text</span>}
    </header>
    {content ? <pre className={`tool-panel-text ${wrap ? 'is-wrapped' : 'is-unwrapped'}`} tabIndex={0} aria-label={`${label} text`}><HighlightedCode text={displayed} language={syntax}/></pre> : <p className="tool-empty">{empty}</p>}
    {shortened && <footer className="tool-panel-footer"><span>{expanded ? 'Showing complete text' : `Preview · ${preview.length.toLocaleString()} of ${content.length.toLocaleString()} characters`}</span>
      <button type="button" disabled={pending} aria-expanded={expanded} onClick={() => startTransition(() => setExpanded(value => !value))}>{pending ? 'Opening…' : expanded ? 'Show preview' : `Show all ${content.length.toLocaleString()} characters`}</button>
    </footer>}
  </section>;
}

function ToolArguments({ view }) {
  const extra = view.command != null || view.code != null;
  const [open, setOpen] = useState(!extra);
  return <details className="tool-arguments" open={open} onToggle={event => setOpen(event.currentTarget.open)}>
    <summary>{extra ? 'All arguments' : 'Input arguments'}</summary>
    {open && <TextPanel label="Arguments" text={view.args} language={view.argsLanguage}/>}
  </details>;
}

export default function ToolActivity({ tool, initiallyOpen = false }) {
  const [open, setOpen] = useState(initiallyOpen);
  const view = useMemo(() => toolPresentation(tool), [tool]);
  const argumentsPanel = view.hasArguments && <ToolArguments view={view}/>;
  return <details className={`tool-activity ${view.failed ? 'tool-failed' : ''}`} open={open} onToggle={event => setOpen(event.currentTarget.open)}>
    <summary className="tool-activity-head">
      <ChevronRight size={14} className="tool-chevron"/><span className="tool-activity-icon">{view.failed ? <AlertCircle size={15}/> : <Terminal size={15}/>}</span>
      <span className="tool-activity-description"><span className="tool-activity-title"><span className="tool-activity-label">Tool call</span><strong>{view.action}</strong>
        {view.action !== view.name && <code className="tool-technical-name">{view.name}</code>}</span>
        {view.summary && <code className="tool-activity-preview">{view.summary}</code>}</span>
      <span className={`tool-outcome ${view.failed ? 'is-failed' : view.completed ? 'is-completed' : ''}`}>
        {view.failed ? <XCircle size={13}/> : view.completed ? <CheckCircle2 size={13}/> : null}{view.outcome}
        {view.exit != null && <span className="tool-exit-code"> · exit {view.exit}</span>}
      </span>
    </summary>
    {open && <div className="tool-activity-body">
      {view.command != null && <TextPanel label="Command" text={view.command} language={view.commandLanguage}/>}
      {view.code != null && <TextPanel label="Code" text={view.code} language={view.codeLanguage}/>}
      {view.command == null && view.code == null && argumentsPanel}
      {view.result != null ? <TextPanel label="Output" text={view.result} language={view.resultLanguage}/> : <p className="tool-empty">No output recorded.</p>}
      {(view.command != null || view.code != null) && argumentsPanel}
    </div>}
  </details>;
}
