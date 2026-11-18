import { useEffect, useRef, useSyncExternalStore } from 'react';
import { Maximize2, Minimize2, PanelLeft, PanelLeftClose, Search, SlidersHorizontal } from 'lucide-react';
import { READING_DEFAULTS, WIDTH_LIMITS } from '../reading-layout.js';

export function PanelResize({ layout, setting, label, className = '' }) {
  const settings = useSyncExternalStore(layout.subscribe, layout.getSnapshot);
  const drag = useRef(null);
  const [min, max] = WIDTH_LIMITS[setting];
  const finish = event => {
    if (!drag.current || (event && event.pointerId !== drag.current.id)) return;
    drag.current = null; layout.save();
    if (event?.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
  };
  useEffect(() => () => { if (drag.current) layout.save(); }, [layout]);
  return <div className={`panel-resize ${className}`} role="separator" tabIndex={0} aria-label={label}
    aria-orientation="vertical" aria-valuemin={min} aria-valuemax={max} aria-valuenow={settings[setting]} aria-valuetext={`${settings[setting]} pixels`}
    title={`${label}. Drag or use arrow keys; double-click to reset.`}
    onDoubleClick={() => layout.update({ [setting]: READING_DEFAULTS[setting] })}
    onPointerDown={event => {
      if (event.button !== 0) return;
      event.preventDefault(); event.currentTarget.focus();
      drag.current = { id: event.pointerId, x: event.clientX, width: event.currentTarget.previousElementSibling.getBoundingClientRect().width };
      event.currentTarget.setPointerCapture?.(event.pointerId);
    }}
    onPointerMove={event => { const start = drag.current; if (start && start.id === event.pointerId) layout.update({ [setting]: start.width + event.clientX - start.x }, { persist: false }); }}
    onPointerUp={finish} onPointerCancel={finish} onLostPointerCapture={() => { if (drag.current) { drag.current = null; layout.save(); } }}
    onKeyDown={event => {
      const change = { ArrowLeft: -10, ArrowRight: 10 }[event.key];
      if (change != null || ['Home', 'End'].includes(event.key)) {
        event.preventDefault(); event.stopPropagation();
        layout.update({ [setting]: event.key === 'Home' ? min : event.key === 'End' ? max : settings[setting] + change * (event.shiftKey ? 5 : 1) });
      }
    }}/>;
}

export default function ReadingControls({ layout, title, onSearch, onBrowse, canFocus = true }) {
  const settings = useSyncExternalStore(layout.subscribe, layout.getSnapshot);
  const menu = useRef(null);
  useEffect(() => {
    const outside = event => { if (menu.current?.open && !menu.current.contains(event.target)) menu.current.open = false; };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, []);
  const toggle = key => layout.update({ [key]: !settings[key] });
  return <div className="reading-bar" aria-label="Reading controls">
    {onBrowse && <button type="button" className="reader-button reader-mobile-back" aria-label="Browse sessions" onClick={onBrowse}><PanelLeft size={15}/></button>}
    <button type="button" className="reader-button" disabled={!canFocus} aria-pressed={settings.focusMode}
      title="Focus mode (Ctrl/Cmd+Shift+F; Escape to leave)" onClick={() => layout.update({ focusMode: !settings.focusMode, searchVisible: false })}>
      {settings.focusMode ? <Minimize2 size={15}/> : <Maximize2 size={15}/>}<span>{settings.focusMode ? 'Exit focus' : 'Focus mode'}</span>
    </button>
    <span className="reading-title" role={settings.focusMode || settings.headerHidden ? 'heading' : undefined} aria-level={settings.focusMode || settings.headerHidden ? 2 : undefined} title={title}>{title}</span>
    {onSearch && <button type="button" className="icon-button" aria-label="Search conversation" onClick={onSearch}><Search size={15}/></button>}
    <button type="button" className="icon-button reader-list-toggle" aria-label={settings.listHidden ? 'Show session list' : 'Hide session list'}
      aria-pressed={settings.listHidden} onClick={() => layout.update({ listHidden: !settings.listHidden, focusMode: false })}>{settings.listHidden ? <PanelLeft size={15}/> : <PanelLeftClose size={15}/>}</button>
    <details className="reading-layout-menu" ref={menu} onKeyDown={event => {
      if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); menu.current.open = false; menu.current.querySelector('summary').focus(); }
    }}>
      <summary className="reader-button" aria-label="Reading layout"><SlidersHorizontal size={15}/><span>Layout</span></summary>
      <div className="reading-layout-options">
        <strong>Reading layout</strong>
        {Object.entries({ navHidden: 'Hide navigation', listHidden: 'Hide session list', filtersHidden: 'Hide search and filters', headerHidden: 'Hide session details', toolsHidden: 'Hide view controls' }).map(([key, label]) =>
          <label className="reader-check" key={key}><input type="checkbox" checked={settings[key]} onChange={() => toggle(key)}/>{label}</label>)}
        <label className="reader-check"><input type="checkbox" checked={settings.fullWidth} onChange={() => toggle('fullWidth')}/>Use available reading width</label>
        {Object.entries({ navWidth: 'Navigation width', listWidth: 'Session list width', readingWidth: 'Reading width' }).map(([key, label]) =>
          <label className="reader-width" key={key}><span>{label}<output>{settings[key]} px</output></span>
            <input type="range" aria-label={label} min={WIDTH_LIMITS[key][0]} max={WIDTH_LIMITS[key][1]} step="10" value={settings[key]} disabled={key === 'readingWidth' && settings.fullWidth}
              onChange={event => layout.update({ [key]: Number(event.target.value) }, { persist: false })}
              onPointerUp={() => layout.save()} onKeyUp={() => layout.save()} onBlur={() => layout.save()}/></label>)}
        <button type="button" className="btn" onClick={() => layout.reset()}>Reset layout</button>
        {layout.getSaveError() && <p role="status">{layout.getSaveError()}</p>}
      </div>
    </details>
  </div>;
}
