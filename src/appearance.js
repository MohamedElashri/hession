import { storageKey } from './brand.js';

export const THEMES = [
  { value: 'system', label: 'System', description: 'Follow your device' },
  { value: 'dark', label: 'Dark', description: 'Neutral charcoal' },
  { value: 'light', label: 'Light', description: 'Clean and bright' },
  { value: 'midnight', label: 'Midnight', description: 'Deep navy' },
  { value: 'sepia', label: 'Sepia', description: 'Warm paper' },
];

export const ACCENTS = [
  { value: 'green', label: 'Green' },
  { value: 'blue', label: 'Blue' },
  { value: 'violet', label: 'Violet' },
  { value: 'amber', label: 'Amber' },
  { value: 'rose', label: 'Rose' },
];

export const DEFAULT_APPEARANCE = Object.freeze({
  theme: 'system',
  accent: 'green',
  textSize: 'default',
  density: 'comfortable',
  wrapCode: false,
  reduceMotion: false,
});

export function normalizeAppearance(value) {
  const source = value && typeof value === 'object' ? value : {};
  const choice = (key, values) => values.includes(source[key]) ? source[key] : DEFAULT_APPEARANCE[key];
  return {
    theme: choice('theme', THEMES.map(theme => theme.value)),
    accent: choice('accent', ACCENTS.map(accent => accent.value)),
    textSize: choice('textSize', ['small', 'default', 'large']),
    density: choice('density', ['comfortable', 'compact']),
    wrapCode: source.wrapCode === true,
    reduceMotion: source.reduceMotion === true,
  };
}

export function readAppearance(storage) {
  try {
    storage ??= window.localStorage;
    const saved = storage.getItem(storageKey('appearance'));
    if (saved !== null) return normalizeAppearance(JSON.parse(saved));
    const theme = storage.getItem(storageKey('theme'));
    return normalizeAppearance({ theme });
  } catch {
    return { ...DEFAULT_APPEARANCE };
  }
}

function resolveTheme(theme, prefersDark) {
  return theme === 'system' ? (prefersDark ? 'dark' : 'light') : theme;
}

export function applyAppearance(preferences, prefersDark, root = document.documentElement) {
  root.dataset.theme = resolveTheme(preferences.theme, prefersDark);
  root.dataset.accent = preferences.accent;
  root.dataset.textSize = preferences.textSize;
  root.dataset.density = preferences.density;
  root.dataset.wrapCode = String(preferences.wrapCode);
  root.dataset.reduceMotion = String(preferences.reduceMotion);
}
