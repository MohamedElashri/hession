import { useEffect, useState } from 'react';
import { ChevronLeft, ChevronRight, Folder, LoaderCircle, RefreshCw, Star } from 'lucide-react';
import { api } from '../api.js';
import { filterSince, parseDate } from '../dates.js';
import { familyLabel } from '../labels.js';
import '../styles/developer-search.css';

// Resolve relative presets once when starting a query. Reusing a cursor must
// reuse the identical absolute cutoff, even if midnight passes between pages.
function absoluteFilters(filters) {
  const days = { day: 1, week: 7, month: 30, year: 365 }[filters.since];
  return { ...filters, since: days ? new Date(Date.now() - days * 86_400_000).toISOString() : filterSince(filters.since || '') };
}

function HighlightedText({ text, spans = [] }) {
  const characters = Array.from(String(text || ''));
  const ranges = spans.filter(span => Number.isInteger(span.start) && Number.isInteger(span.end)
    && span.start >= 0 && span.start < span.end && span.end <= characters.length)
    .sort((a, b) => a.start - b.start || a.end - b.end);
  const pieces = [];
  let offset = 0;
  for (const range of ranges) {
    if (range.start < offset) continue;
    pieces.push(characters.slice(offset, range.start).join(''));
    pieces.push(<mark key={`${range.start}:${range.end}`}>{characters.slice(range.start, range.end).join('')}</mark>);
    offset = range.end;
  }
  pieces.push(characters.slice(offset).join(''));
  return pieces;
}

function SearchPage({ query, filters, mode, onModeChange, selected, onOpen, onToggleStar, starPending, refreshKey }) {
  const [requestFilters, setRequestFilters] = useState(() => absoluteFilters(filters));
  const [paging, setPaging] = useState({ cursor: null, previous: [] });
  const [attempt, setAttempt] = useState(0);
  const [response, setResponse] = useState(null);
  const requestKey = JSON.stringify([query, requestFilters, mode, paging.cursor, refreshKey, attempt]);
  useEffect(() => {
    let active = true;
    const timer = setTimeout(() => {
      api('developer_search', { request: { query, filters: requestFilters, mode, limit: 20,
        cursor: paging.cursor, visibility: 'all_indexed_text' } })
        .then(data => { if (active) setResponse({ key: requestKey, data }); })
        .catch(error => { if (active) setResponse(previous => ({ key: requestKey, error, data: previous?.data })); });
    }, 180);
    return () => { active = false; clearTimeout(timer); };
  }, [query, requestFilters, mode, paging.cursor, refreshKey, attempt, requestKey]);
  const current = response?.key === requestKey ? response : null;
  const loading = !current;
  const data = current?.error ? null : current?.data;
  const error = current?.error;
  const restart = () => {
    setPaging({ cursor: null, previous: [] });
    setRequestFilters(absoluteFilters(filters));
    setAttempt(value => value + 1);
  };
  const results = (data || response?.data)?.results || [];
  const changeMode = value => { setPaging({ cursor: null, previous: [] }); setResponse(null); onModeChange(value); };
  const stale = error?.kind === 'stale_cursor';
  return <>
    <div className="developer-search-controls">
      <div className="segmented" role="group" aria-label="Search result type">
        <button type="button" aria-pressed={mode === 'sessions'} onClick={() => changeMode('sessions')}>Sessions</button>
        <button type="button" aria-pressed={mode === 'messages'} onClick={() => changeMode('messages')}>Messages</button>
      </div>
      <button type="button" className="icon-button" aria-label="Restart developer search" onClick={restart} disabled={loading}><RefreshCw size={14}/></button>
    </div>
    <p className="developer-search-hint" id="developer-search-help">Words use AND. Use OR, &quot;phrases&quot;, -exclusions and field:value.</p>
    <details className="developer-search-syntax"><summary>Query syntax</summary><p>Use agent:, project:, file:, tool:, branch:, host:, after:, before: and has:errors. Quote values containing spaces. Dates use YYYY-MM-DD: after includes that UTC date; before excludes it. C++, foo::bar and paths keep their punctuation. Boolean groups and regexes are unsupported.</p><p>Results rank by relevance. Session words can match different entries; message words must match the same entry. Sidebar and toolbar filters also apply.</p></details>
    <div id="developer-search-feedback" className="developer-search-feedback" role={error ? 'alert' : 'status'} aria-live="polite">
      {loading ? <><LoaderCircle size={14} className="spin"/>Searching indexed history…</> : error ? <>
        <span>{stale ? 'The library changed. Restart this search to use its latest results.' : error.message || 'Could not search indexed history.'}
          {error.kind === 'syntax' && Number.isInteger(error.position) && <span className="developer-search-position"> Query position: byte {error.position}.</span>}</span>
        {error.kind !== 'syntax' && <button type="button" className="text-button" onClick={restart}>{stale ? 'Restart search' : 'Retry search'}</button>}
      </> : `${data.countLowerBound?.toLocaleString() ?? results.length} ${data.nextCursor ? 'or more ' : ''}${mode === 'messages' ? 'messages' : 'sessions'} · Relevance`}
    </div>
    <div className="session-list developer-search-results" data-session-list aria-label="Developer search results" aria-busy={loading}>
      {!loading && !error && !results.length && <div className="empty-list"><strong>No matching {mode}</strong><p>Try another query or clear a filter.</p></div>}
      {results.map(result => <div className="developer-search-card" key={`${result.sessionId}:${mode === 'messages' ? result.entryId : 'session'}`}>
        <button type="button" className={`developer-search-hit ${String(selected) === String(result.sessionId) ? 'session-selected' : ''}`}
          data-session-navigation data-session-id={result.sessionId} aria-current={String(selected) === String(result.sessionId) ? 'true' : undefined}
          aria-disabled={loading || Boolean(error)} onClick={() => { if (!loading && !error) onOpen(result); }}>
          <span className="developer-search-meta">{familyLabel(result.agent)}{result.host ? ` · ${result.host}` : ''}{result.activityAt ? ` · ${parseDate(result.activityAt)?.toLocaleDateString() || ''}` : ''}</span>
          <strong>{result.title || 'Untitled session'}</strong>
          {result.project && <span className="developer-search-project"><Folder size={12}/>{result.project}</span>}
          {result.excerpt != null && <span className="developer-search-excerpt"><HighlightedText text={result.excerpt} spans={result.highlights}/></span>}
          <span className="developer-search-location">{result.entryId != null ? `Open matching entry${result.position != null ? ` ${result.position + 1}` : ''} in full log` : 'Open session · metadata match'}{result.matchingLocations?.length ? ` · ${result.matchingLocations.join(', ')}` : ''}</span>
          {!!result.tags?.length && <span className="developer-search-tags">{result.tags.map(tag => <span className="tag-chip" key={tag}>{tag}</span>)}</span>}
        </button>
        {onToggleStar && <button type="button" className={`developer-search-star icon-button ${result.starred ? 'is-starred' : ''}`} aria-label={`${result.starred ? 'Unstar' : 'Star'} ${result.title || 'session'}`}
          aria-pressed={Boolean(result.starred)} disabled={loading || Boolean(error) || starPending?.has(result.sessionId)} onClick={() => onToggleStar({ id: result.sessionId, starred: result.starred })}><Star size={14} fill={result.starred ? 'currentColor' : 'none'}/></button>}
      </div>)}
    </div>
    <nav className="list-footer developer-search-pagination" aria-label="Developer search pages">
      <span>Page {paging.previous.length + 1}</span><div className="pagination-controls">
        <button type="button" aria-label="Previous developer results" disabled={loading || Boolean(error) || !paging.previous.length} onClick={() => setPaging(current => ({ cursor: current.previous.at(-1), previous: current.previous.slice(0, -1) }))}><ChevronLeft size={14}/></button>
        <button type="button" aria-label="Next developer results" disabled={loading || Boolean(error) || !data?.nextCursor} onClick={() => setPaging(current => ({ cursor: data.nextCursor, previous: [...current.previous, current.cursor] }))}><ChevronRight size={14}/></button>
      </div>
    </nav>
  </>;
}

export default function DeveloperSearch(props) {
  const [mode, setMode] = useState('sessions');
  const key = JSON.stringify([props.query, props.filters]);
  return <SearchPage {...props} key={key} mode={mode} onModeChange={setMode}/>;
}
