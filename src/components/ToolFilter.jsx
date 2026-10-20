import { useState, useRef, useEffect, useMemo } from 'react';
import { LoaderCircle, Terminal, ChevronDown, Search, X } from 'lucide-react';

export default function ToolFilter({ tools = [], selected = [], onChange, loading = false, error = false, onRetry }) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const anchorRef = useRef(null);

  useEffect(() => {
    if (!open) return;
    const handleClick = (e) => {
      if (anchorRef.current && !anchorRef.current.contains(e.target)) {
        setOpen(false);
      }
    };
    document.addEventListener('pointerdown', handleClick);
    return () => document.removeEventListener('pointerdown', handleClick);
  }, [open]);

  const toolValues = useMemo(() => {
    return tools.map((t) => (typeof t === 'object' && t !== null ? t.value : t)).filter(Boolean);
  }, [tools]);

  const toggleTool = (val) => {
    if (selected.includes(val)) {
      onChange(selected.filter((x) => x !== val));
    } else {
      onChange([...selected, val]);
    }
  };

  const filteredTools = useMemo(() => {
    if (!search.trim()) return toolValues;
    const q = search.toLowerCase();
    return toolValues.filter((t) => t.toLowerCase().includes(q));
  }, [toolValues, search]);

  return (
    <div className="filter-anchor" ref={anchorRef}>
      <button
        type="button"
        className={`filter-button ${open ? 'filter-open' : ''} ${selected.length > 0 ? 'filter-has-value' : ''}`}
        onClick={() => setOpen(!open)}
        title="Filter by tool (multi-select)"
        aria-expanded={open}
        aria-busy={loading}
      >
        <Terminal size={14} />
        <span>Tool</span>
        {selected.length > 0 && <b>{selected.length}</b>}
        {loading ? <LoaderCircle className="spin" size={12} /> : <ChevronDown size={12} />}
      </button>

      {open && (
        <div className="filter-dropdown-menu multi-filter-dropdown">
          {loading && <div className="metadata-status" role="status"><LoaderCircle className="spin" size={12}/>Loading tools…</div>}
          {error && <div className="metadata-status" role="status">Could not load tools<button type="button" onClick={onRetry}>Retry</button></div>}
          <div className="filter-dropdown-header">
            <strong>Tools {selected.length > 0 ? `(${selected.length})` : ''}</strong>
            {selected.length > 0 && (
              <button
                type="button"
                className="filter-clear-link"
                onClick={() => onChange([])}
              >
                Clear all
              </button>
            )}
          </div>

          {toolValues.length > 6 && (
            <div className="filter-search-box">
              <Search size={12} />
              <input
                type="text"
                placeholder="Search tools…"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
              {search && (
                <button type="button" onClick={() => setSearch('')}>
                  <X size={11} />
                </button>
              )}
            </div>
          )}

          <div className="filter-checkbox-list">
            {filteredTools.map((t) => {
              const isChecked = selected.includes(t);
              return (
                <label key={t} className={`filter-checkbox-item ${isChecked ? 'active' : ''}`}>
                  <input
                    type="checkbox"
                    checked={isChecked}
                    onChange={() => toggleTool(t)}
                  />
                  <span className="filter-checkbox-custom" />
                  <span className="filter-item-name">{t}</span>
                </label>
              );
            })}
            {!loading && !error && !filteredTools.length && (
              <div className="filter-empty-text">No tools found</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
