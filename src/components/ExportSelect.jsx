import { useEffect, useRef, useState } from 'react';
import { Check, ChevronDown } from 'lucide-react';

export default function ExportSelect({ label, value, options, onChange, disabled }) {
  const [menuOpen, setOpen] = useState(false);
  const open = menuOpen && !disabled;
  const anchor = useRef(null);
  const trigger = useRef(null);
  useEffect(() => {
    if (!open) return;
    const outside = event => { if (!anchor.current?.contains(event.target)) setOpen(false); };
    document.addEventListener('pointerdown', outside);
    const active = anchor.current?.querySelector('.filter-menu-item.active:not(:disabled)') || anchor.current?.querySelector('.filter-menu-item:not(:disabled)');
    active?.focus();
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);
  const choose = option => { onChange(option.value); setOpen(false); trigger.current?.focus(); };
  return <div className="export-field"><span>{label}</span><div className="filter-anchor export-select" ref={anchor} onKeyDown={event => {
    if (event.key === 'Escape' && open) { event.stopPropagation(); setOpen(false); trigger.current?.focus(); }
    if (event.key === 'Tab') setOpen(false);
    if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
      event.preventDefault();
      if (!open) { setOpen(true); return; }
      const items = Array.from(anchor.current.querySelectorAll('.filter-menu-item:not(:disabled)'));
      const index = items.indexOf(document.activeElement);
      items[event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length]?.focus();
    }
  }}><button ref={trigger} type="button" className={`filter-button ${open ? 'filter-open' : ''}`} disabled={disabled} aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(!open)}><span>{options.find(option => option.value === value)?.label}</span><ChevronDown size={13}/></button>{open && <div className="filter-dropdown-menu" role="menu" aria-label={label}>{options.map(option => <button type="button" role="menuitemradio" aria-checked={value === option.value} key={option.value} disabled={option.disabled} className={`filter-menu-item ${value === option.value ? 'active' : ''}`} onClick={() => choose(option)}><span>{option.label}</span>{value === option.value && <Check size={13}/>}</button>)}</div>}</div></div>;
}
