import { useEffect, useId, useRef, useState } from 'react';
import { Search, X } from 'lucide-react';
import { api } from '../api.js';
import { shortcutLabel } from '../commands.js';
import useModalDialog from './useModalDialog.js';
import { trapDialogFocus } from './dialog-focus.js';

const LIMIT = 12;

function readSessions(value) {
  if (!Array.isArray(value?.sessions)) throw new Error('Invalid session search response.');
  return value.sessions.slice(0, LIMIT).map(session => {
    const validId = typeof session?.id === 'number' ? Number.isSafeInteger(session.id) && session.id > 0
      : typeof session?.id === 'string' && /^[1-9]\d*$/.test(session.id);
    if (!validId || (session.title != null && typeof session.title !== 'string')) throw new Error('Invalid session search response.');
    return { id: session.id, title: session.title,
      cwd: typeof session.cwd === 'string' ? session.cwd : '', project: typeof session.project === 'string' ? session.project : '' };
  });
}

export default function CommandPalette({ registry, context, projects, onSession, onProject, onExecute, onClose }) {
  const dialog = useModalDialog();
  const input = useRef(null);
  const listId = useId();
  const [query, setQuery] = useState('');
  const [cursor, setCursor] = useState(0);
  const [result, setResult] = useState(null);
  useEffect(() => { input.current?.focus(); }, []);
  useEffect(() => {
    let active = true;
    const timer = setTimeout(() => {
      api('search_sessions', { q: query.trim(), limit: String(LIMIT), offset: '0', sort: 'recent' })
        .then(value => { if (active) setResult({ query, sessions: readSessions(value) }); })
        .catch(error => { if (active) setResult({ query, error: error?.message || 'Session search failed.' }); });
    }, 180);
    return () => { active = false; clearTimeout(timer); };
  }, [query]);
  const needle = query.trim().toLowerCase();
  const commands = registry.commands.filter(command => !command.hidden && command.title.toLowerCase().includes(needle)).slice(0, 30);
  const current = result?.query === query ? result : null;
  const items = [
    ...commands.map(command => ({ id: `command-${command.id}`, title: command.title, kind: 'Command',
      reason: command.disabled?.(context), shortcut: command.shortcuts?.[0], run: () => onExecute(command.id) })),
    ...projects.filter(project => `${project.display_name || ''} ${project.path}`.toLowerCase().includes(needle)).slice(0, LIMIT)
      .map(project => ({ id: `project-${project.path}`, title: project.display_name || project.path, detail: project.path, kind: 'Workspace', run: () => onProject(project) })),
    ...(current?.sessions || []).map(session => ({ id: `session-${session.id}`, title: session.title || 'Untitled session',
      detail: session.cwd || session.project, kind: 'Session', run: () => onSession(session.id) })),
  ];
  const activeIndex = Math.min(cursor, Math.max(0, items.length - 1));
  const activeItem = items[activeIndex];
  const itemId = index => `${listId}-${index}`;
  const choose = item => { if (item && !item.reason) item.run(); };
  useEffect(() => {
    document.getElementById(`${listId}-${activeIndex}`)?.scrollIntoView?.({ block: 'nearest', behavior: 'instant' });
  }, [listId, activeIndex]);
  const handleKeys = event => {
    if (event.isComposing || event.nativeEvent?.isComposing || event.keyCode === 229) return;
    if (trapDialogFocus(event, dialog.current)) return;
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    if (!event.shiftKey && event.target === input.current && ['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault();
      setCursor(event.key === 'Home' ? 0 : event.key === 'End' ? Math.max(0, items.length - 1)
        : Math.max(0, Math.min(items.length - 1, activeIndex + (event.key === 'ArrowDown' ? 1 : -1))));
    } else if (!event.shiftKey && !event.repeat && event.target === input.current && event.key === 'Enter') { event.preventDefault(); choose(activeItem); }
    else if (!event.repeat && event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); onClose(); }
  };
  return <dialog ref={dialog} className="data-dialog command-palette" aria-labelledby={`${listId}-title`}
    onKeyDown={handleKeys} onCancel={event => { event.preventDefault(); onClose(); }}
    onClick={event => { if (event.target === event.currentTarget) onClose(); }}>
    <div className="command-palette-heading"><h2 id={`${listId}-title`}>Command palette</h2><button type="button" className="icon-button" aria-label="Close command palette" onClick={onClose}><X size={18}/></button></div>
    <label className="command-search"><Search size={17}/><input ref={input} role="combobox" aria-label="Search commands, sessions, and workspaces"
      aria-autocomplete="list" aria-expanded="true" aria-controls={listId} aria-activedescendant={activeItem ? itemId(activeIndex) : undefined}
      value={query} maxLength={512} placeholder="Commands, sessions, workspaces…" onChange={event => { setQuery(event.target.value); setCursor(0); }}/></label>
    <div className="command-results" role="listbox" id={listId} aria-label="Commands, sessions, and workspaces">
      {items.map((item, index) => <div key={item.id} id={itemId(index)} role="option" aria-selected={index === activeIndex} aria-disabled={!!item.reason}
        className="command-option" onMouseDown={event => event.preventDefault()} onClick={() => choose(item)}>
        <div><span className="command-kind">{item.kind}</span><strong>{item.title}</strong>{(item.reason || item.detail) && <small>{item.reason || item.detail}</small>}</div>
        {item.shortcut && <kbd>{shortcutLabel(item.shortcut, context.mac)}</kbd>}
      </div>)}
    </div>
    <p className="command-status" role="status">{!current ? 'Searching sessions…' : current.error ? `Session search unavailable: ${current.error}`
      : !items.length ? 'No matching commands, sessions, or workspaces.' : `${items.length} results · ↑/↓ to choose · Enter to open · Esc to close`}</p>
  </dialog>;
}
