import { useState, useRef, useEffect, useMemo } from 'react';
import { LoaderCircle, GitBranch, ChevronDown, Check, Search, X } from 'lucide-react';

export default function BranchFilter({ branches = [], selected = '', onChange, loading = false, error = false, onRetry }) {
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

  const branchValues = useMemo(() => {
    return branches.map((b) => (typeof b === 'object' && b !== null ? b.value : b)).filter(Boolean);
  }, [branches]);

  const filteredBranches = useMemo(() => {
    if (!search.trim()) return branchValues;
    const q = search.toLowerCase();
    return branchValues.filter((b) => b.toLowerCase().includes(q));
  }, [branchValues, search]);

  const handleSelect = (val) => {
    if (selected === val) {
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
        className={`filter-button ${open ? 'filter-open' : ''} ${selected ? 'filter-has-value' : ''}`}
        onClick={() => setOpen(!open)}
        title="Filter by git branch"
        aria-expanded={open}
        aria-busy={loading}
      >
        <GitBranch size={14} />
        <span className="filter-button-label">
          {selected ? (selected.length > 14 ? `${selected.slice(0, 12)}…` : selected) : 'Branch'}
        </span>
        {selected ? <b>1</b> : null}
        {loading ? <LoaderCircle className="spin" size={12} /> : <ChevronDown size={12} />}
      </button>

      {open && (
        <div className="filter-dropdown-menu">
          {loading && <div className="metadata-status" role="status"><LoaderCircle className="spin" size={12}/>Loading branches…</div>}
          {error && <div className="metadata-status" role="status">Could not load branches<button type="button" onClick={onRetry}>Retry</button></div>}
          <div className="filter-dropdown-header">
            <strong>Branches</strong>
            {selected && (
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

          {branchValues.length > 6 && (
            <div className="filter-search-box">
              <Search size={12} />
              <input
                type="text"
                placeholder="Search branches…"
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
              className={`filter-menu-item ${!selected ? 'active' : ''}`}
              onClick={() => handleSelect('')}
            >
              <span>Any branch</span>
              {!selected && <Check size={13} />}
            </button>
            {filteredBranches.map((b) => {
              const isSelected = selected === b;
              return (
                <button
                  key={b}
                  type="button"
                  className={`filter-menu-item ${isSelected ? 'active' : ''}`}
                  onClick={() => handleSelect(b)}
                >
                  <span className="filter-item-name">{b}</span>
                  {isSelected && <Check size={13} />}
                </button>
              );
            })}
            {!loading && !error && !filteredBranches.length && (
              <div className="filter-empty-text">No branches found</div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
