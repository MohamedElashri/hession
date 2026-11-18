import { storageKey } from './brand.js';

export const READING_DEFAULTS = Object.freeze({ navWidth: 248, listWidth: 360, readingWidth: 860,
  navHidden: false, listHidden: false, filtersHidden: false, headerHidden: false, toolsHidden: false, fullWidth: false,
  focusMode: false, searchVisible: false });
export const WIDTH_LIMITS = Object.freeze({ navWidth: [180, 360], listWidth: [220, 560], readingWidth: [560, 1600] });
const visibilityKeys = ['navHidden', 'listHidden', 'filtersHidden', 'headerHidden', 'toolsHidden', 'focusMode', 'searchVisible'];
function normalize(value) {
  const next = { ...READING_DEFAULTS };
  for (const key of Object.keys(next)) {
    if (key in WIDTH_LIMITS) {
      const [min, max] = WIDTH_LIMITS[key];
      if (Number.isFinite(value?.[key])) next[key] = Math.round(Math.max(min, Math.min(max, value[key])));
    } else if (typeof value?.[key] === 'boolean') next[key] = value[key];
  }
  return next;
}

/** Width subscribers update CSS directly; dragging never rerenders the transcript. */
export function createReadingLayout(storage) {
  if (storage === undefined) { try { storage = globalThis.localStorage; } catch { storage = null; } }
  let initial;
  try { initial = JSON.parse(storage?.getItem(storageKey('reading-layout')) || 'null'); } catch { /* defaults */ }
  let state = normalize(initial);
  state.focusMode = false; state.searchVisible = false;
  let visibility = Object.fromEntries(visibilityKeys.map(key => [key, state[key]]));
  const listeners = new Set();
  let saveError = '';
  const save = () => {
    const previousError = saveError;
    const saved = { ...state }; delete saved.focusMode; delete saved.searchVisible;
    try { storage?.setItem(storageKey('reading-layout'), JSON.stringify(saved)); saveError = ''; }
    catch { saveError = 'Could not save the reading layout. Changes still apply to this window.'; }
    if (saveError !== previousError) { state = { ...state }; listeners.forEach(listener => listener()); }
  };
  return {
    getSnapshot: () => state,
    getVisibility: () => visibility,
    subscribe: listener => { listeners.add(listener); return () => listeners.delete(listener); },
    update(patch, { persist = true } = {}) {
      const next = normalize({ ...state, ...patch });
      if (visibilityKeys.some(key => next[key] !== state[key])) visibility = Object.fromEntries(visibilityKeys.map(key => [key, next[key]]));
      state = next;
      if (persist) save();
      listeners.forEach(listener => listener());
    },
    save,
    getSaveError: () => saveError,
    reset() { this.update(READING_DEFAULTS); },
  };
}

export function applyReadingWidths(element, settings) {
  if (!element) return;
  element.style.setProperty('--sidebar-w', `${settings.navWidth}px`);
  element.style.setProperty('--list-w', `${settings.listWidth}px`);
  element.style.setProperty('--reading-w', settings.fullWidth ? 'none' : `${settings.readingWidth}px`);
}
