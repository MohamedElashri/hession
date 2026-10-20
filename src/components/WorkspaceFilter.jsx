import React, { useState, useRef, useEffect, useMemo } from 'react';
import { LoaderCircle, Layers, ChevronDown, Check, Search, X } from 'lucide-react';

const shortPath = (p) => String(p ?? '').replace(/\\/g, '/').split('/').filter(Boolean).at(-1) || 'Local workspace';

export default function WorkspaceFilter({ projects = [], workspaces, value = '', selected, onChange, loading = false, error = false, onRetry }) {
  const list = workspaces ?? projects ?? [];
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
    return (list || [])
      .map((p) => {
        if (typeof p === 'string') return { path: p, display_name: shortPath(p) };
        const path = p.path || p.value || '';
        return { path, display_name: p.display_name || shortPath(path) };
      })
      .filter((p) => p.path);
  }, [list]);

  const filtered = useMemo(() => {
    if (!search.trim()) return normalized;
    const q = search.toLowerCase();
    return normalized.filter(
      (p) => (p.display_name || '').toLowerCase().includes(q) || p.path.toLowerCase().includes(q)
    );
  }, [normalized, search]);

  const selectedProject = normalized.find((p) => p.path === current);
  const buttonLabel = selectedProject
    ? (selectedProject.display_name || shortPath(selectedProject.path))
    : 'Workspace';
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
        title="Filter by workspace"
        aria-expanded={open}
        aria-busy={loading}
      >
        <Layers size={14} />
        <span className="filter-button-label">{truncatedLabel}</span>
        {current ? <b>1</b> : null}
        {loading ? <LoaderCircle className="spin" size={12} /> : <ChevronDown size={12} />}
      </button>

      {open && (
        <div className="filter-dropdown-menu">
          {loading && <div className="metadata-status" role="status"><LoaderCircle className="spin" size={12} />Loading workspaces…</div>}
          {error && (
            <div className="metadata-status" role="status">
              Could not load workspaces
              {onRetry && <button type="button" onClick={onRetry}>Retry</button>}
            </div>
          )}
          <div className="filter-dropdown-header">
            <strong>Workspaces</strong>
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
                placeholder="Search workspaces…"
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
              <span>All Workspaces</span>
              {!current && <Check size={13} />}
            </button>
            {filtered.map((p) => {
              const isSelected = current === p.path;
              const showBadge = p.display_name && p.display_name !== p.path;
              return (
                <button
                  key={p.path}
                  type="button"
                  className={`filter-menu-item ${isSelected ? 'active' : ''}`}
                  onClick={() => handleSelect(p.path)}
                  title={p.path}
                >
                  <span className="filter-item-name">{p.display_name || shortPath(p.path)}</span>
                  {showBadge && <span className="filter-item-badge">{shortPath(p.path)}</span>}
                  {isSelected && <Check size={13} />}
                </button>
              );
            })}
            {!loading && !error && !filtered.length && (
              <div className="filter-empty-text">No workspaces found</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
