import { useEffect, useState } from 'react';
import { ChevronLeft, ChevronRight, Search, X } from 'lucide-react';
import { api } from '../api.js';

const PAGE_SIZE = 20;

// FTS supplies control-character delimiters; React escapes all excerpt text.
function Excerpt({ text }) {
  return String(text || '').split('\u0001').map((part, index) => {
    if (!index) return part;
    const end = part.indexOf('\u0002');
    return end < 0 ? part : <span key={index}><mark>{part.slice(0, end)}</mark>{part.slice(end + 1)}</span>;
  });
}

export default function SessionSearch({ sessionId, onOpen }) {
  const [query, setQuery] = useState('');
  const [page, setPage] = useState(0);
  const [result, setResult] = useState(null);
  useEffect(() => {
    if (!query.trim()) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      api('search_session_entries', { id: String(sessionId), q: query, limit: String(PAGE_SIZE), offset: String(page * PAGE_SIZE) })
        .then(data => { if (!cancelled) setResult({ ...data, query, page }); })
        .catch(error => { if (!cancelled) setResult({ error: error.message, query, page }); });
    }, 180);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [query, page, sessionId]);
  const current = result?.query === query && result?.page === page ? result : null;
  const changeQuery = value => { setQuery(value); setPage(0); setResult(null); };
  return <div className="session-search">
    <label className="search-wrap"><Search size={14}/><input type="search" value={query} onChange={event => changeQuery(event.target.value)}
      placeholder="Search this session" aria-label="Search this session" onKeyDown={event => { if (event.key === 'Escape') changeQuery(''); }}/>
    {query && <button type="button" aria-label="Clear session search" onClick={() => changeQuery('')}><X size={14}/></button>}</label>
    {query.trim() && <div className="session-search-results">
      <div role="status" aria-live="polite">{!current ? 'Searching the complete session…' : current.error || `${current.total} matching entries`}</div>
      {current?.entries?.map(entry => <button type="button" className="session-search-hit" key={entry.id} onClick={() => onOpen(entry)}>
        <strong>{entry.role || entry.kind} · Open in full log</strong><span><Excerpt text={entry.excerpt}/></span>
      </button>)}
      {current?.total > PAGE_SIZE && <nav className="pagination-controls" aria-label="Session search pages">
        <button type="button" aria-label="Previous search results" disabled={!page} onClick={() => setPage(page - 1)}><ChevronLeft size={14}/></button>
        <span>Page {page + 1} of {Math.ceil(current.total / PAGE_SIZE)}</span>
        <button type="button" aria-label="Next search results" disabled={(page + 1) * PAGE_SIZE >= current.total} onClick={() => setPage(page + 1)}><ChevronRight size={14}/></button>
      </nav>}
    </div>}
  </div>;
}
