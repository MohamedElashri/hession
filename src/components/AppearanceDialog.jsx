import { Palette, RotateCcw, X } from 'lucide-react';
import { ACCENTS, DEFAULT_APPEARANCE, THEMES } from '../appearance.js';
import useModalDialog from './useModalDialog.js';

export default function AppearanceDialog({ preferences, onChange, saveError, onClose }) {
  const dialog = useModalDialog();

  return <dialog ref={dialog} className="data-dialog appearance-dialog" aria-labelledby="appearance-title"
    onCancel={event => { event.preventDefault(); onClose(); }}>
    <div className="export-heading">
      <h2 id="appearance-title"><Palette size={18}/>Appearance</h2>
      <button type="button" className="icon-button" aria-label="Close appearance" onClick={onClose}><X size={18}/></button>
    </div>
    <p>Make your library comfortable to read. Changes apply immediately and save on this device.</p>

    <fieldset className="appearance-section">
      <legend>Theme</legend>
      <div className="theme-options">
        {THEMES.map(theme => <label key={theme.value} className={`theme-option ${preferences.theme === theme.value ? 'selected' : ''}`}>
          <input type="radio" name="theme" value={theme.value} checked={preferences.theme === theme.value}
            onChange={() => onChange({ theme: theme.value })}/>
          <span className="theme-preview" data-preview={theme.value} aria-hidden="true"><i/><span><b/><b/><b/></span></span>
          <strong>{theme.label}</strong><small>{theme.description}</small>
        </label>)}
      </div>
    </fieldset>

    <fieldset className="appearance-section">
      <legend>Accent color</legend>
      <div className="accent-options">
        {ACCENTS.map(accent => <label key={accent.value} className="accent-option">
          <input type="radio" name="accent" value={accent.value} checked={preferences.accent === accent.value}
            onChange={() => onChange({ accent: accent.value })}/>
          <span className="accent-swatch" data-swatch={accent.value} aria-hidden="true"/>{accent.label}
        </label>)}
      </div>
    </fieldset>

    <div className="appearance-selects">
      <label>Text size<select value={preferences.textSize} onChange={event => onChange({ textSize: event.target.value })}>
        <option value="small">Small</option><option value="default">Default</option><option value="large">Large</option>
      </select></label>
      <label>Spacing<select value={preferences.density} onChange={event => onChange({ density: event.target.value })}>
        <option value="comfortable">Comfortable</option><option value="compact">Compact</option>
      </select></label>
    </div>
    <div className="appearance-toggles">
      <label><input type="checkbox" checked={preferences.wrapCode} onChange={event => onChange({ wrapCode: event.target.checked })}/>
        <span>Wrap code and tool output<small>Read long lines without horizontal scrolling.</small></span></label>
      <label><input type="checkbox" checked={preferences.reduceMotion} onChange={event => onChange({ reduceMotion: event.target.checked })}/>
        <span>Reduce motion<small>Your device’s reduced motion setting is always respected.</small></span></label>
    </div>
    {saveError && <div className="data-warning" role="status">{saveError}</div>}
    <div className="data-actions">
      <button type="button" className="btn" onClick={() => onChange(DEFAULT_APPEARANCE)}><RotateCcw size={14}/>Reset to defaults</button>
      <button type="button" className="btn primary" onClick={onClose}>Done</button>
    </div>
  </dialog>;
}
