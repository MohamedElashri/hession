import { useEffect, useLayoutEffect, useState } from 'react';
import { storageKey } from './brand.js';
import { applyAppearance, normalizeAppearance, readAppearance } from './appearance.js';

export default function useAppearance() {
  const [preferences, setPreferences] = useState(readAppearance);
  const [prefersDark, setPrefersDark] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches);
  const [saveError, setSaveError] = useState('');

  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    const changed = event => setPrefersDark(event.matches);
    media.addEventListener('change', changed);
    const sync = event => {
      if (event.key === null || event.key === storageKey('appearance') || event.key === storageKey('theme')) {
        setPreferences(readAppearance());
      }
    };
    window.addEventListener('storage', sync);
    return () => {
      media.removeEventListener('change', changed);
      window.removeEventListener('storage', sync);
    };
  }, []);

  useLayoutEffect(() => {
    applyAppearance(preferences, prefersDark);
  }, [preferences, prefersDark]);

  const update = patch => {
    const next = normalizeAppearance({ ...preferences, ...patch });
    setPreferences(next);
    try {
      localStorage.setItem(storageKey('appearance'), JSON.stringify(next));
      setSaveError('');
    } catch {
      setSaveError('Changes apply now, but could not be saved for next time.');
    }
  };

  return { preferences, update, saveError };
}
