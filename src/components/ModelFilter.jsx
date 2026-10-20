import React, { useState, useRef, useEffect, useMemo } from 'react';
import { LoaderCircle, Sparkles, ChevronDown, Search, X } from 'lucide-react';

export default function ModelFilter({ models = [], selected = [], onChange, loading = false, error = false, onRetry }) {
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

  const toggleModel = (val) => {
    if (selected.includes(val)) {
      onChange(selected.filter((x) => x !== val));
    } else {
      onChange([...selected, val]);
    }
  };

  const filteredModels = useMemo(() => {
    if (!search.trim()) return models;
    const q = search.toLowerCase();
    return models.filter(
      (m) =>
        (m.value || '').toLowerCase().includes(q) ||
        (m.provider || '').toLowerCase().includes(q)
    );
  }, [models, search]);

  return (
    <div className="filter-anchor" ref={anchorRef}>
      <button
        type="button"
        className={`filter-button ${open ? 'filter-open' : ''} ${selected.length > 0 ? 'filter-has-value' : ''}`}
        onClick={() => setOpen(!open)}
        title="Filter by model (multi-select)"
        aria-expanded={open}
        aria-busy={loading}
      >
        <Sparkles size={14} />
        <span>Model</span>
        {selected.length > 0 && <b>{selected.length}</b>}
        {loading ? <LoaderCircle className="spin" size={12} /> : <ChevronDown size={12} />}
      </button>

      {open && (
        <div className="filter-dropdown-menu multi-filter-dropdown">
          {loading && <div className="metadata-status" role="status"><LoaderCircle className="spin" size={12}/>Loading models…</div>}
          {error && <div className="metadata-status" role="status">Could not load models<button type="button" onClick={onRetry}>Retry</button></div>}
          <div className="filter-dropdown-header">
            <strong>Models {selected.length > 0 ? `(${selected.length})` : ''}</strong>
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

          {models.length > 6 && (
            <div className="filter-search-box">
              <Search size={12} />
              <input
                type="text"
                placeholder="Search models…"
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
            {filteredModels.map((m) => {
              const isChecked = selected.includes(m.value);
              return (
                <label key={`${m.provider || ''}:${m.value}`} className={`filter-checkbox-item ${isChecked ? 'active' : ''}`}>
                  <input
                    type="checkbox"
                    checked={isChecked}
                    onChange={() => toggleModel(m.value)}
                  />
                  <span className="filter-checkbox-custom" />
                  <span className="filter-item-name">{m.value}</span>
                  {m.provider && <span className="filter-item-badge">{m.provider}</span>}
                </label>
              );
            })}
            {!loading && !error && !filteredModels.length && (
              <div className="filter-empty-text">No models found</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
