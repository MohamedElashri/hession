import { useState, useRef, useEffect } from 'react';
import { Calendar, ChevronDown, Check, X } from 'lucide-react';

import { localDateKey } from '../dates.js';

const DEFAULT_PRESETS = [
  { value: '', label: 'Any time' },
  { value: 'day', label: 'Past 24 hours' },
  { value: 'week', label: 'Past 7 days' },
  { value: 'month', label: 'Past 30 days' },
  { value: 'year', label: 'Past 12 months' },
];

// All known preset values across the app (sessions + analytics). Anything else
// non-empty is treated as a custom "from" date, so legacy URLs keep working.
const KNOWN_PRESET_VALUES = ['', 'day', 'week', 'month', '3months', 'year'];

const SHORT_LABELS = {
  day: 'Past 24h',
  week: 'Past 7d',
  month: 'Past 30d',
  '3months': 'Past 90d',
  year: 'Past 12m',
};

const isPresetValue = (val) => KNOWN_PRESET_VALUES.includes(val);

export default function DateFilter({ value, onChange, presets = DEFAULT_PRESETS }) {
  const [open, setOpen] = useState(false);
  const [customDate, setCustomDate] = useState(value && !isPresetValue(value) ? value : '');
  const anchorRef = useRef(null);

  // Follow outside changes to the value (adjusting state during render).
  const [seenValue, setSeenValue] = useState(value);
  if (seenValue !== value) {
    setSeenValue(value);
    if (value && !isPresetValue(value)) setCustomDate(value);
    else if (!value) setCustomDate('');
  }

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

  const getLabel = () => {
    if (!value) return 'Date';
    if (SHORT_LABELS[value]) return SHORT_LABELS[value];
    const preset = presets.find((p) => p.value === value);
    if (preset) return preset.label;
    if (value && value.match(/^\d{4}-\d{2}-\d{2}/)) return `From ${value}`;
    return 'Date';
  };

  const handleSelectPreset = (val) => {
    setCustomDate('');
    onChange(val);
    setOpen(false);
  };

  const handleApplyCustom = (e) => {
    e.preventDefault();
    if (customDate) {
      onChange(customDate);
      setOpen(false);
    }
  };

  const isCustomActive = Boolean(value && !isPresetValue(value));

  return (
    <div className="filter-anchor" ref={anchorRef}>
      <button
        type="button"
        className={`filter-button ${open ? 'filter-open' : ''} ${value ? 'filter-has-value' : ''}`}
        onClick={() => setOpen(!open)}
        title="Filter by date"
        aria-expanded={open}
      >
        <Calendar size={14} />
        <span>{getLabel()}</span>
        {value ? <b>1</b> : null}
        <ChevronDown size={12} />
      </button>

      {open && (
        <div className="filter-dropdown-menu date-filter-dropdown">
          <div className="filter-dropdown-header">
            <strong>Date range</strong>
            {value && (
              <button
                type="button"
                className="filter-clear-link"
                onClick={() => {
                  onChange('');
                  setCustomDate('');
                  setOpen(false);
                }}
              >
                Clear
              </button>
            )}
          </div>

          <div className="filter-presets-list">
            {presets.map((p) => {
              const isSelected = value === p.value;
              return (
                <button
                  key={p.value}
                  type="button"
                  className={`filter-menu-item ${isSelected ? 'active' : ''}`}
                  onClick={() => handleSelectPreset(p.value)}
                >
                  <span>{p.label}</span>
                  {isSelected && <Check size={13} />}
                </button>
              );
            })}
          </div>

          <div className="filter-dropdown-divider" />

          <form className="filter-custom-section" onSubmit={handleApplyCustom}>
            <span className="filter-custom-label">Custom from date</span>
            <div className="filter-custom-row">
              <input
                type="date"
                className="filter-date-input"
                value={customDate}
                onChange={(e) => setCustomDate(e.target.value)}
                max={localDateKey()}
              />
              <button
                type="submit"
                className="filter-date-apply-btn"
                disabled={!customDate || customDate === value}
              >
                Apply
              </button>
            </div>
            {isCustomActive && (
              <div className="filter-custom-active-tag">
                <span>Active: from {value}</span>
                <button
                  type="button"
                  onClick={() => {
                    onChange('');
                    setCustomDate('');
                  }}
                  title="Remove custom date"
                >
                  <X size={11} />
                </button>
              </div>
            )}
          </form>
        </div>
      )}
    </div>
  );
}
