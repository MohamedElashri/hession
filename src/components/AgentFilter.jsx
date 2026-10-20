import React, { useState, useRef, useEffect, useMemo } from 'react';
import { LoaderCircle, Cpu, ChevronDown, Check, Search, X } from 'lucide-react';

export default function AgentFilter({ agents = [], value = '', selected, onChange, loading = false, error = false, onRetry }) {
  const current = selected ?? value ?? '';
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
    const handleKey = (e) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('pointerdown', handleClick);
    document.addEventListener('keydown', handleKey);
    return () => {
      document.removeEventListener('pointerdown', handleClick);
      document.removeEventListener('keydown', handleKey);
    };
  }, [open ]);

  const normalized = useMemo(() => {
    return (agents || [])
      .map((a) => {
        if (typeof a === 'string') return { family: a, product: a };
        return { family: a.family || a.value || '', product: a.product || a.family || a.value || '' };
      })
      .filter((a) => a.family);
  }, [agents]);

  const filtered = useMemo(() => {
    if (!search.trim()) return normalized;
    const q = search.toLowerCase();
    return normalized.filter(
      (a) => a.family.toLowerCase().includes(q) || (a.product || '').toLowerCase().includes(q)
    );
  }, [normalized, search]);

  const selectedAgent = normalized.find((a) => a.family === current);
  const buttonLabel = selectedAgent
    ? (selectedAgent.product || selectedAgent.family)
    : 'Agent';
  const truncatedLabel = buttonLabel.length > 14 ? `${buttonLabel.slice(0, 12)}…` : buttonLabel;

  const handleSelect = (val) => {
    if (current === val) {
      onChange('');
    } else {
      onChange(val);
    }
    setOpen(false);
  };

  return (
    <div className="filter-anchor" ref={anchorRef}>
      <button
        type="button"
        className={`filter-button ${open ? 'filter-open' : ''} ${current ? 'filter-has-value' : ''}`}
        onClick={() => setOpen(!open)}
        title="Filter by agent"
        aria-expanded={open}
        aria-busy={loading}
      >
        <Cpu size={14} />
        <span className="filter-button-label">{truncatedLabel}</span>
        {current ? <b>1</b> : null}
        {loading ? <LoaderCircle className="spin" size={12} /> : <ChevronDown size={12} />}
      </button>

      {open && (
        <div className="filter-dropdown-menu">
          {loading && <div className="metadata-status" role="status"><LoaderCircle className="spin" size={12} />Loading agents…</div>}
          {error && (
            <div className="metadata-status" role="status">
              Could not load agents
              {onRetry && <button type="button" onClick={onRetry}>Retry</button>}
            </div>
          )}
          <div className="filter-dropdown-header">
            <strong>Agents</strong>
            {current && (
              <button
                type="button"
                className="filter-clear-link"
                onClick={() => {
                  onChange('');
                  setOpen(false);
                }}
              >
                Clear
              </button>
            )}
          </div>

          {normalized.length > 6 && (
            <div className="filter-search-box">
              <Search size={12} />
              <input
                type="text"
                placeholder="Search agents…"
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

          <div className="filter-options-list">
            <button
              type="button"
              className={`filter-menu-item ${!current ? 'active' : ''}`}
              onClick={() => handleSelect('')}
            >
              <span>All Agents</span>
              {!current && <Check size={13} />}
            </button>
            {filtered.map((a) => {
              const isSelected = current === a.family;
              const showBadge = a.product && a.product !== a.family;
              return (
                <button
                  key={a.family}
                  type="button"
                  className={`filter-menu-item ${isSelected ? 'active' : ''}`}
                  onClick={() => handleSelect(a.family)}
                  title={a.product && a.product !== a.family ? `${a.product} (${a.family})` : a.family}
                >
                  <span className="filter-item-name">{a.product || a.family}</span>
                  {showBadge && <span className="filter-item-badge">{a.family}</span>}
                  {isSelected && <Check size={13} />}
                </button>
              );
            })}
            {!loading && !error && !filtered.length && (
              <div className="filter-empty-text">No agents found</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
