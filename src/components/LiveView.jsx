import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { Activity, ArrowDown, ArrowRight, LoaderCircle, Menu, Pause, Play, RefreshCw } from 'lucide-react';
import { api, hasTauriRuntime } from '../api.js';
import { familyLabel, shortPath } from '../labels.js';
import { parseDate } from '../dates.js';
import { LIVE_PAGE_SIZE, liveEntrySummary, liveStatusLabel, MAX_LIVE_ENTRIES, mergeLiveEntries } from '../live-state.js';
import useLivePolling from '../useLivePolling.js';
import '../styles/live.css';

const SESSION_PAGE_SIZE = 50;
const text = value => typeof value === 'string' ? value : '';
const timeLabel = value => parseDate(value)?.toLocaleString() || 'Time not recorded';
const location = entry => ({ entryId: entry.id, position: Number.isSafeInteger(entry.position) && entry.position >= 0 ? entry.position : null });

function LiveEntry({ entry, expanded, onToggle, onOpen }) {
  return <article className="live-entry" data-live-entry={entry.id}>
    <div className="live-entry-heading"><button type="button" className="live-entry-toggle" aria-expanded={expanded} disabled={entry.oversized} onClick={onToggle}>
      <strong>{entry.oversized ? 'Large recorded event' : `${text(entry.role) || 'event'} · ${text(entry.kind) || 'Recorded event'}`}</strong><span>{timeLabel(entry.occurred_at)}</span>
    </button><button type="button" className="text-button" onClick={onOpen}>Open full history<ArrowRight size={12}/></button></div>
    {expanded ? <div className="live-entry-content">
      {(entry.parts || []).map((part, index) => <div key={part.id || index}><span className="live-part-kind">{text(part.kind)}</span><pre>{text(part.text).slice(0, 12000)}{text(part.text).length > 12000 ? '\n… Open full history for the complete content.' : ''}</pre></div>)}
      {!(entry.parts || []).length && <pre>{text(entry.search_text).slice(0, 12000)}</pre>}
      {(entry.tools || []).map((tool, index) => <div key={tool.id || tool.call_id || index} className="live-tool"><strong>{text(tool.tool_name) || 'Recorded tool call'}</strong>
        {tool.exit_code != null && <span> · Recorded exit code {tool.exit_code}</span>}
        {tool.arguments_json && <pre>{text(tool.arguments_json).slice(0, 8000)}</pre>}{tool.result_text && <pre>{text(tool.result_text).slice(0, 12000)}</pre>}
      </div>)}
    </div> : <p className="live-entry-preview">{liveEntrySummary(entry)}</p>}
  </article>;
}

function emptyTail(sessionId) { return { sessionId, entries: [], cursor: null, loaded: false, pending: 0, updated: 0, trimmed: 0, reset: false, deleted: false, error: '' }; }

export function LiveTail({ session, available, onSelectSession }) {
  const sessionId = session?.id ?? null;
  const [tail, setTail] = useState(() => emptyTail(null));
  const store = useRef(emptyTail(null));
  const viewport = useRef(null), scrollSnapshot = useRef(null), followingRef = useRef(false);
  const [follow, setFollow] = useState({ sessionId: null, enabled: false });
  const [pause, setPause] = useState({ sessionId: null, enabled: false });
  const [expanded, setExpanded] = useState({ sessionId: null, ids: new Set() });
  const [scrollNotice, setScrollNotice] = useState({ sessionId: null, text: '' });
  const following = follow.sessionId === sessionId && follow.enabled;
  const paused = pause.sessionId === sessionId && pause.enabled;
  const current = tail.sessionId === sessionId ? tail : emptyTail(sessionId);
  useEffect(() => { followingRef.current = following; }, [following]);
  const captureScroll = () => {
    const area = viewport.current;
    if (!area || followingRef.current) return null;
    const top = area.getBoundingClientRect().top;
    const anchor = Array.from(area.querySelectorAll('[data-live-entry]')).find(element => element.getBoundingClientRect().bottom > top);
    return { id: anchor?.dataset.liveEntry, offset: anchor ? anchor.getBoundingClientRect().top - top : null, scrollTop: area.scrollTop };
  };
  const load = async isCurrent => {
    if (store.current.sessionId !== sessionId) store.current = emptyTail(sessionId);
    const previous = store.current;
    try {
      const result = await api('session_delta', { sessionId, cursor: previous.cursor, limit: LIVE_PAGE_SIZE });
      if (!isCurrent()) return false;
      if (!result || !Array.isArray(result.entries) || !result.cursor || typeof result.hasMore !== 'boolean') throw new Error('Invalid live event response.');
      if (result.hasMore && JSON.stringify(result.cursor) === JSON.stringify(previous.cursor)) throw new Error('Live cursor did not advance. Retry this session.');
      if (result.oversizedEntries != null && !Array.isArray(result.oversizedEntries)) throw new Error('Invalid live event response.');
      const omitted = (result.oversizedEntries || []).map(item => ({ id: item.id, ordinal: item.ordinal, position: item.position, oversized: true }));
      const merged = mergeLiveEntries(previous.entries, [...result.entries, ...omitted], !!result.reset || !!result.deleted);
      scrollSnapshot.current = previous.loaded && !result.reset && !result.deleted ? captureScroll() : { initial: true };
      const next = { sessionId, ...merged, cursor: result.cursor, loaded: true, error: '', reset: !!result.reset && previous.loaded, deleted: !!result.deleted,
        pending: !previous.loaded || result.reset || result.deleted || followingRef.current ? 0 : store.current.pending + merged.added };
      if (result.deleted) next.entries = [];
      store.current = next; setTail(next);
      const retained = new Set(next.entries.map(entry => entry.id));
      setExpanded(value => result.reset || result.deleted || value.sessionId !== sessionId ? { sessionId, ids: new Set() }
        : [...value.ids].every(id => retained.has(id)) ? value : { sessionId, ids: new Set([...value.ids].filter(id => retained.has(id))) });
      return result.hasMore;
    } catch (error) {
      if (isCurrent()) { const next = { ...store.current, error: error?.message || 'Could not read local session updates.' }; store.current = next; setTail(next); }
      return false;
    }
  };
  const { refresh, eventError } = useLivePolling({ identity: String(sessionId), enabled: available && !!sessionId && !paused, load });
  const scrollToLatest = () => {
    const area = viewport.current;
    area?.scrollTo?.({ top: area.scrollHeight, behavior: 'instant' });
    if (area) area.scrollTop = area.scrollHeight;
    if (store.current.sessionId === sessionId) { const next = { ...store.current, pending: 0 }; store.current = next; setTail(next); }
    setScrollNotice({ sessionId, text: '' });
  };
  useLayoutEffect(() => {
    const area = viewport.current;
    if (!area) return;
    if (following) { area.scrollTop = area.scrollHeight; return; }
    const snapshot = scrollSnapshot.current;
    scrollSnapshot.current = null;
    if (!snapshot) return;
    if (snapshot.initial) { area.scrollTop = 0; return; }
    const anchor = snapshot.id && area.querySelector(`[data-live-entry="${snapshot.id}"]`);
    area.scrollTop = anchor && snapshot.offset != null
      ? area.scrollTop + anchor.getBoundingClientRect().top - area.getBoundingClientRect().top - snapshot.offset : snapshot.scrollTop;
  }, [current.entries, following]);
  const toggleFollow = () => {
    const enabled = !following;
    followingRef.current = enabled; setFollow({ sessionId, enabled });
    if (enabled) scrollToLatest();
  };
  const togglePause = () => {
    setPause({ sessionId, enabled: !paused });
    if (!paused) { followingRef.current = false; setFollow({ sessionId, enabled: false }); }
  };
  if (!session) return <section className="live-tail live-tail-empty" aria-label="Recorded events"><p>Select a local session to inspect its recorded events.</p></section>;
  return <section className="live-tail" aria-label="Recorded events">
    <header className="live-tail-heading"><div><h2>{session.title || 'Untitled session'}</h2><p>{familyLabel(session.agent)} · {liveStatusLabel(session.status)}</p></div>
      <button type="button" className="btn" onClick={() => onSelectSession(session.id, session.latestEvent ? location(session.latestEvent) : undefined)}>Open full history<ArrowRight size={14}/></button>
    </header>
    <div className="live-tail-controls"><button type="button" className={`btn ${following ? 'primary' : ''}`} disabled={paused || !current.loaded || current.deleted} aria-pressed={following} onClick={toggleFollow}><ArrowDown size={14}/>{following ? 'Following latest events' : 'Follow latest events'}</button>
      <button type="button" className="btn" aria-pressed={paused} onClick={togglePause}>{paused ? <Play size={14}/> : <Pause size={14}/>} {paused ? 'Resume live updates' : 'Pause live updates'}</button>
      <button type="button" className="text-button" onClick={scrollToLatest} disabled={!current.entries.length}>{current.pending ? `${current.pending} new event${current.pending === 1 ? '' : 's'} · Jump to latest` : 'Jump to latest'}</button>
    </div>
    <p className="live-reading-note" role="status">{paused ? 'Live updates paused. Automatic indexing is unchanged.' : (scrollNotice.sessionId === sessionId && scrollNotice.text) || (following ? 'Following is on. Scroll away to pause following.' : 'Following is off. New events do not move your reading position.')}</p>
    {eventError && <p className="live-warning">{eventError}</p>}
    {current.error && <div className="live-error" role="alert"><span>{current.error}</span><button type="button" className="btn" disabled={paused} onClick={() => refresh(0)}>Retry live events</button></div>}
    {current.reset && current.loaded && <p className="live-warning" role="status">The source history changed. The live window was replaced with its latest recorded events.</p>}
    {current.updated > 0 && <p className="live-reading-note" role="status">{current.updated} earlier recorded event{current.updated === 1 ? '' : 's'} updated.</p>}
    {current.trimmed > 0 && <p className="live-window-note">Older events left the bounded live window. Open full history to keep reading them.</p>}
    <div ref={viewport} className="live-event-scroll" tabIndex={0} role="region" aria-label="Live transcript" onScroll={event => {
      const area = event.currentTarget;
      if (followingRef.current && area.scrollHeight - area.clientHeight - area.scrollTop > 48) {
        followingRef.current = false; setFollow({ sessionId, enabled: false }); setScrollNotice({ sessionId, text: 'Following paused after you scrolled away. Automatic indexing is unchanged.' });
      }
    }}>
      {!current.loaded && !current.error && <p role="status"><LoaderCircle className="spin" size={16}/>Loading recorded events…</p>}
      {current.deleted ? <p role="status">This session is no longer indexed.</p> : current.loaded && !current.entries.length ? <p role="status">No events are recorded for this session.</p> : current.entries.map(entry => <LiveEntry key={entry.id} entry={entry}
        expanded={expanded.sessionId === sessionId && expanded.ids.has(entry.id)} onToggle={() => setExpanded(value => {
          const ids = new Set(value.sessionId === sessionId ? value.ids : []); if (ids.has(entry.id)) ids.delete(entry.id); else ids.add(entry.id); return { sessionId, ids };
        })} onOpen={() => onSelectSession(session.id, location(entry))}/>)}
    </div>
    <p className="live-window-note">Live keeps up to {MAX_LIVE_ENTRIES} events and a bounded amount of content. Full history contains the complete indexed transcript.</p>
  </section>;
}

export default function LiveView({ agents = [], projects = [], observer, onSelectSession, onOpenNav, available = hasTauriRuntime(), focusOnMount = false }) {
  const heading = useRef(null);
  useEffect(() => { if (focusOnMount && !document.querySelector('dialog[open],[aria-modal="true"]')) heading.current?.focus(); }, [focusOnMount]);
  const [agent, setAgent] = useState(''), [project, setProject] = useState(''), [window, setWindow] = useState('15m');
  const filters = JSON.stringify([agent, project, window]);
  const [paging, setPaging] = useState({ filters: '', offset: 0 });
  const offset = paging.filters === filters ? paging.offset : 0;
  const identity = JSON.stringify([filters, offset]);
  const [list, setList] = useState({ identity: '', sessions: [], loaded: false, error: '', hasMore: false, nextOffset: null });
  const [selection, setSelection] = useState({ filters: '', session: null });
  const current = list.identity === identity ? list : { sessions: [], loaded: false, error: '', hasMore: false };
  const session = selection.filters === filters ? selection.session : null;
  const load = async isCurrent => {
    try {
      const result = await api('recent_local_sessions', { agent, project, since: window === '15m' ? new Date(Date.now() - 15 * 60 * 1000).toISOString() : '', limit: SESSION_PAGE_SIZE, offset });
      if (!isCurrent()) return false;
      if (!Array.isArray(result?.sessions) || result.sessions.length > SESSION_PAGE_SIZE) throw new Error('Invalid local activity response.');
      setList({ identity, sessions: result.sessions, loaded: true, error: '', hasMore: !!result.hasMore, nextOffset: result.nextOffset });
      setSelection(previous => previous.filters === filters && previous.session
        ? { filters, session: result.sessions.find(value => value.id === previous.session.id) || previous.session }
        : { filters, session: result.sessions[0] || null });
    } catch (error) {
      if (isCurrent()) setList(previous => ({ ...(previous.identity === identity ? previous : { sessions: [], loaded: false, hasMore: false }), identity, error: error?.message || 'Could not read local activity.' }));
    }
    return false;
  };
  const { refresh, eventError } = useLivePolling({ identity, enabled: available, load, interval: 3000 });
  const indexingLabel = observer?.error ? 'Local indexing needs attention.' : !observer?.available ? 'Indexing status is not available here.'
    : !observer.enabled ? 'Automatic local indexing is paused.' : observer.status?.phase === 'degraded' ? 'Indexing is using recovery polling; results may be stale.'
      : observer.status?.phase === 'active' ? 'Automatic local indexing is on.' : `Local indexing: ${observer.status?.phase || 'starting'}.`;
  return <div className="live-view">
    <header className="live-header"><div className="live-title-row">{onOpenNav && <button type="button" className="icon-button mobile-menu" onClick={onOpenNav} aria-label="Open navigation"><Menu size={18}/></button>}<Activity size={20}/><h1 ref={heading} tabIndex={-1}>Live local activity</h1></div>
      <p>Passive updates from locally recorded histories. Recent changes do not prove an agent process is running, blocked, or finished. SSH histories use manual refresh.</p>
      {globalThis.window?.__TAURI_INTERNALS__?.isPreview && <p className="live-warning">Browser preview shows recorded sample data. Monitoring runs in the desktop app.</p>}
      <div className="live-health"><span role="status">{indexingLabel}</span>{observer && <button type="button" className="btn" disabled={!observer.available || observer.busy} onClick={observer.toggle}>{observer.enabled ? 'Pause automatic indexing' : 'Resume automatic indexing'}</button>}</div>
      {observer?.error && <div className="live-error" role="alert"><span>{observer.error}</span>{observer.retry && <button type="button" className="btn" disabled={observer.busy} onClick={observer.retry}>Retry automatic indexing</button>}</div>}
      <div className="live-filters"><label>Agent<select aria-label="Live agent" value={agent} onChange={event => setAgent(event.target.value)}><option value="">All agents</option>{agents.map(value => <option key={value.family} value={value.family}>{value.product || familyLabel(value.family)}</option>)}</select></label>
        <label>Project<select aria-label="Live project" value={project} onChange={event => setProject(event.target.value)}><option value="">All local projects</option>{projects.map(value => <option key={value.path} value={value.path}>{value.display_name || shortPath(value.path)}</option>)}</select></label>
        <label>Window<select aria-label="Live time window" value={window} onChange={event => setWindow(event.target.value)}><option value="15m">Last 15 minutes</option><option value="all">All recorded activity</option></select></label>
        <button type="button" className="btn" disabled={!available} onClick={() => refresh(0)}><RefreshCw size={14}/>Refresh local activity</button>
      </div>
    </header>
    {!available ? <p className="live-unavailable" role="status">Live local activity is available in the desktop app.</p> : <div className="live-grid">
      <section className="live-sessions" aria-label="Local sessions"><div className="live-session-list" data-session-list tabIndex={0} role="region" aria-label="Live session list">
        {eventError && <p className="live-warning">{eventError}</p>}
        {current.error && <div className="live-error" role="alert"><p>{current.error}</p><button type="button" className="btn" onClick={() => refresh(0)}>Retry local activity</button></div>}
        {!current.loaded && !current.error && <p role="status">Loading local activity…</p>}
        {current.loaded && !current.sessions.length && <p role="status">No local sessions recorded in this window.</p>}
        {current.sessions.map(value => <div key={value.id} className="live-session-row"><button type="button" data-session-navigation aria-current={session?.id === value.id ? 'true' : undefined} className="live-session-card" onClick={() => setSelection({ filters, session: value })}>
          <span>{value.product || familyLabel(value.agent)} · <span className={`live-status live-status-${value.status}`}>{liveStatusLabel(value.status)}</span></span><strong>{value.title || 'Untitled session'}</strong>
          <span>{shortPath(value.project)} · {timeLabel(value.lastEventAt)}</span>{value.latestEvent?.summary && <p>{text(value.latestEvent.summary).slice(0, 300)}</p>}
        </button></div>)}
      </div><nav className="live-pagination" aria-label="Local activity pages"><button type="button" className="btn" disabled={!offset} onClick={() => setPaging({ filters, offset: Math.max(0, offset - SESSION_PAGE_SIZE) })}>Previous</button><span>Page {Math.floor(offset / SESSION_PAGE_SIZE) + 1}</span><button type="button" className="btn" disabled={!current.hasMore || !Number.isSafeInteger(current.nextOffset) || current.nextOffset <= offset} onClick={() => setPaging({ filters, offset: current.nextOffset })}>Next</button></nav></section>
      <LiveTail session={session} available={available} onSelectSession={onSelectSession}/>
    </div>}
  </div>;
}
