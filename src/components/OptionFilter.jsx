import { useId, useState } from 'react';
import { Check, ChevronDown, LoaderCircle, Search, X } from 'lucide-react';
import useDropdown from './useDropdown.js';

/** Options use { value, label, badge?, title? }; a multi-select value is an array. */
export default function OptionFilter({ label, plural, icon: Icon, options, value, onChange, multiple = false, anyLabel, loading, error, onRetry }) {
  const { open, setOpen, anchorRef } = useDropdown();
  const [search, setSearch] = useState('');
  const id = useId();
  const selected = multiple ? value : value ? [value] : [];
  const current = options.find(option => option.value === value);
  const buttonLabel = multiple ? label : current?.label || value || label;
  const query = search.trim().toLowerCase();
  const filtered = options.filter(option => [option.value, option.label, option.badge].some(text => text?.toLowerCase().includes(query)));
  const choose = next => {
    onChange(multiple
      ? selected.includes(next) ? selected.filter(item => item !== next) : [...selected, next]
      : value === next ? '' : next);
    if (!multiple) setOpen(false);
  };

  return <div className="filter-anchor" ref={anchorRef}>
    <button type="button" className={`filter-button ${open ? 'filter-open' : ''} ${selected.length ? 'filter-has-value' : ''}`}
      onClick={() => setOpen(!open)} title={`Filter by ${label.toLowerCase()}${multiple ? ' (multi-select)' : ''}`}
      aria-expanded={open} aria-controls={open ? id : undefined} aria-busy={loading}>
      <Icon size={14}/><span className="filter-button-label">{buttonLabel}</span>
      {selected.length > 0 && <b>{selected.length}</b>}
      {loading ? <LoaderCircle className="spin" size={12}/> : <ChevronDown size={12}/>}
    </button>
    {open && <div className="filter-dropdown-menu" id={id}>
      {loading && <div className="metadata-status" role="status"><LoaderCircle className="spin" size={12}/>Loading {plural.toLowerCase()}…</div>}
      {error && <div className="metadata-status" role="status">Could not load {plural.toLowerCase()}{onRetry && <button type="button" onClick={onRetry}>Retry</button>}</div>}
      <div className="filter-dropdown-header"><strong>{plural}{multiple && selected.length ? ` (${selected.length})` : ''}</strong>
        {selected.length > 0 && <button type="button" className="filter-clear-link" onClick={() => { onChange(multiple ? [] : ''); if (!multiple) setOpen(false); }}>{multiple ? 'Clear all' : 'Clear'}</button>}
      </div>
      {options.length > 6 && <div className="filter-search-box">
        <Search size={12}/><input type="search" placeholder={`Search ${plural.toLowerCase()}…`} aria-label={`Search ${plural.toLowerCase()}`} value={search} onChange={event => setSearch(event.target.value)}/>
        {search && <button type="button" aria-label="Clear search" onClick={() => setSearch('')}><X size={11}/></button>}
      </div>}
      <div className={multiple ? 'filter-checkbox-list' : 'filter-options-list'}>
        {!multiple && <button type="button" className={`filter-menu-item ${!value ? 'active' : ''}`} aria-pressed={!value} onClick={() => { onChange(''); setOpen(false); }}><span>{anyLabel}</span>{!value && <Check size={13}/>}</button>}
        {filtered.map(option => {
          const active = selected.includes(option.value);
          const content = <><span className="filter-item-name">{option.label}</span>{option.badge && <span className="filter-item-badge">{option.badge}</span>}</>;
          return multiple
            ? <label key={option.value} className={`filter-checkbox-item ${active ? 'active' : ''}`} title={option.title}>
              <input type="checkbox" checked={active} onChange={() => choose(option.value)}/><span className="filter-checkbox-custom" aria-hidden="true">{active && <Check size={11}/>}</span>{content}
            </label>
            : <button key={option.value} type="button" className={`filter-menu-item ${active ? 'active' : ''}`} aria-pressed={active} title={option.title} onClick={() => choose(option.value)}>{content}{active && <Check size={13}/>}</button>;
        })}
        {!loading && !error && !filtered.length && <div className="filter-empty-text">No {plural.toLowerCase()} found</div>}
      </div>
    </div>}
  </div>;
}
