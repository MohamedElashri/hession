import { memo, useState } from 'react';
import Markdown from './Markdown.jsx';
import { Bot, Brain, Check, ClipboardList, Copy, Info, Terminal, User } from 'lucide-react';
import { parseDate } from '../dates.js';
import ToolActivity from './ToolActivity.jsx';

function DeferredDetails({ summary, children, ...props }) {
  const [open, setOpen] = useState(false);
  return <details {...props} onToggle={event => setOpen(event.currentTarget.open)}>
    <summary>{summary}</summary>
    {open && children()}
  </details>;
}

const timeOf = entry => parseDate(entry?.occurred_at)?.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }) ?? '';
const lineCount = text => String(text ?? '').split('\n').length;

function CopyButton({ text, label }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try { await navigator.clipboard.writeText(text); setCopied(true); setTimeout(() => setCopied(false), 1400); } catch { /* clipboard unavailable */ }
  };
  return <button type="button" className="copy-entry" onClick={copy} title={label} aria-label={label}>{copied ? <Check size={13}/> : <Copy size={13}/>}</button>;
}

/** Injected context (system reminders, environment, metadata), collapsed. */
function ContextChips({ context }) {
  return <div className="context-row">{context.map((c, i) => <DeferredDetails className="context-chip" key={`${c.tag}-${i}`}
    summary={<><Info size={12}/>{c.label}</>}>
    {() => <pre>{c.body}</pre>}
  </DeferredDetails>)}</div>;
}

function Prompt({ prompt }) {
  const copyText = [prompt.command, prompt.text, ...prompt.pasted].filter(Boolean).join('\n\n');
  return <article className="prompt-card" aria-label="Your message">
    <header className="turn-head">
      <span className="turn-avatar"><User size={13}/></span>
      <strong>You</strong>
      <span className="message-time">{timeOf(prompt.entry)}</span>
      <CopyButton text={copyText} label="Copy prompt"/>
    </header>
    {prompt.command && <code className="command-chip">{prompt.command}</code>}
    {prompt.text && <div className="markdown-body prompt-text"><Markdown text={prompt.text} preserveLines/></div>}
    {prompt.pasted.map((paste, i) => <DeferredDetails className="pasted" key={i}
      summary={<><ClipboardList size={13}/>Pasted text · {lineCount(paste)} line{lineCount(paste) === 1 ? '' : 's'}</>}>
      {() => <pre>{paste}</pre>}
    </DeferredDetails>)}
    {prompt.context.length > 0 && <ContextChips context={prompt.context}/>}
  </article>;
}

// Long runs of tool calls show their start; the rest is one click away.
const TOOL_RUN_VISIBLE = 4;
const TOOL_RUN_COLLAPSE = 6;

function ToolCalls({ calls }) {
  const [expanded, setExpanded] = useState(false);
  const collapsible = calls.length > TOOL_RUN_COLLAPSE;
  const shown = collapsible && !expanded ? calls.filter((call, index) => index < TOOL_RUN_VISIBLE ||
    ['error', 'failed', 'failure'].includes(String(call.status ?? '').toLowerCase()) || (call.exit_code != null && call.exit_code !== 0)) : calls;
  const hidden = calls.length - shown.length;
  return <section className="tool-list" aria-label="Tool activity"><header className="tool-list-head"><Terminal size={13}/><strong>Tool activity</strong><span>{calls.length} call{calls.length === 1 ? '' : 's'}</span></header>
  {shown.map(call => <ToolActivity tool={call} key={call.key}/>)}
  {collapsible && <button type="button" className="text-button tool-more" onClick={() => setExpanded(!expanded)}>
    {expanded ? 'Show fewer' : hidden ? `Show ${hidden} more tool calls` : 'Show all tool calls'}
  </button>}
  </section>;
}

function ReplyItem({ item }) {
  if (item.type === 'text') return <div className="reply-text">
    <div className="markdown-body"><Markdown text={item.text} preserveLines/></div>
    <CopyButton text={item.text} label="Copy reply"/>
  </div>;
  if (item.type === 'thinking') return <DeferredDetails className="thinking" summary={<><Brain size={13}/>Thinking</>}>
    {() => <div className="markdown-body"><Markdown text={item.text} preserveLines/></div>}
  </DeferredDetails>;
  if (item.type === 'tools') return <ToolCalls calls={item.calls}/>;
  if (item.type === 'context') return <ContextChips context={item.context}/>;
  return null;
}

/** The dialogue as turns: your prompt, then the agent's reply and actions. */
export const Conversation = memo(function Conversation({ turns, agentName, agentColor }) {
  return turns.map(turn => <section className="turn" id={`turn-${turn.id}`} key={turn.id}>
    {turn.prompt ? <Prompt prompt={turn.prompt}/> : <p className="turn-continued">Continued from earlier in the session</p>}
    {turn.items.length > 0 && <article className="reply" aria-label={`${agentName} response`}>
      <header className="turn-head reply-head">
        <span className={`turn-avatar agent-avatar ${agentColor}`}><Bot size={13}/></span>
        <strong>{agentName}</strong>
        <span className="speaker-label">Agent</span>
      </header>
      {turn.items.map(item => <ReplyItem item={item} key={item.key}/>)}
    </article>}
  </section>);
});

/** Every prompt on the page, to skim a session and jump to a turn. */
export function PromptIndex({ turns, onOpen }) {
  const prompts = turns.filter(t => t.prompt);
  if (!prompts.length) return null;
  return <ol className="prompt-index">{prompts.map((turn, i) => {
    const calls = turn.items.reduce((n, item) => n + (item.type === 'tools' ? item.calls.length : 0), 0);
    const replies = turn.items.filter(item => item.type === 'text').length;
    return <li key={turn.id}><button type="button" onClick={() => onOpen(turn.id)}>
      <span className="turn-number">{i + 1}</span>
      <span className="prompt-index-text">{turn.prompt.command && <code>{turn.prompt.command}</code>} {turn.prompt.text || (turn.prompt.pasted.length ? 'Pasted text' : '')}</span>
      <span className="prompt-index-meta">{timeOf(turn.prompt.entry)} · {replies} repl{replies === 1 ? 'y' : 'ies'}{calls ? ` · ${calls} tool call${calls === 1 ? '' : 's'}` : ''}</span>
    </button></li>;
  })}</ol>;
}
