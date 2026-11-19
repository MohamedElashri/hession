import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { api, hasTauriRuntime } from './api.js';
import { storageKey } from './brand.js';

export default function useIndexing({ ready, onChanged }) {
  const available = hasTauriRuntime() && !globalThis.window?.__TAURI_INTERNALS__?.isPreview;
  const [enabled, setEnabled] = useState(() => {
    try { return (localStorage.getItem(storageKey('background-indexing')) ?? localStorage.getItem(storageKey('auto-sync'))) !== 'false'; }
    catch { return true; }
  });
  const [status, setStatus] = useState({ phase: 'stopped', mayBeStale: true });
  const [error, setError] = useState('');
  const [preferenceError, setPreferenceError] = useState(''), [notificationError, setNotificationError] = useState('');
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const started = useRef(false), starting = useRef(null), currentEnabled = useRef(enabled), change = useRef(onChanged);
  const revision = useRef(null), controlEpoch = useRef(0), reads = useRef({ busy: false, resume: null });
  useEffect(() => { change.current = onChanged; }, [onChanged]);
  useEffect(() => { currentEnabled.current = enabled; }, [enabled]);
  useEffect(() => {
    if (!ready || !available) return;
    let active = true;
    const update = async () => {
      try {
        if (!enabled && !started.current && !starting.current) { setBusy(false); return; }
        setBusy(true);
        if (!starting.current) {
          controlEpoch.current++;
          starting.current = (async () => {
            let next, applied;
            if (enabled) {
              next = await api('start_indexing'); started.current = true;
              applied = next.phase !== 'paused';
            } else {
              next = await api('pause_indexing', { paused: true }); applied = false;
            }
            // One admitted control operation converges to the latest preference.
            // StrictMode replay and rapid toggles share it, including late pauses.
            while (applied !== currentEnabled.current) {
              const paused = !currentEnabled.current;
              next = await api('pause_indexing', { paused }); applied = !paused;
            }
            return next;
          })();
        }
        const pending = starting.current;
        let next;
        try { next = await pending; } finally { if (starting.current === pending) starting.current = null; }
        if (active) setStatus(next);
        if (active) setError('');
      } catch (e) { if (active) setError(e.message); }
      finally { if (active) setBusy(false); }
    };
    update();
    return () => { active = false; };
  }, [available, ready, enabled, attempt]);
  useEffect(() => {
    if (!available || !ready) return;
    const ownership = reads.current;
    let active = true, dirty = false, timer, unlisten;
    const resume = () => {
      if (active && dirty && !document.hidden && !timer) timer = setTimeout(() => { timer = null; poll(); }, 750);
    };
    ownership.resume = resume;
    const poll = async () => {
      if (!active || ownership.busy || document.hidden) return;
      clearTimeout(timer); timer = null;
      ownership.busy = true; dirty = false;
      const control = controlEpoch.current;
      try {
        const results = await Promise.allSettled([api('indexing_status'), api('observed_library_revision')]);
        const [health, version] = results;
        if (active) {
          if (health.status === 'fulfilled' && control === controlEpoch.current && !starting.current) setStatus(previous => JSON.stringify(previous) === JSON.stringify(health.value) ? previous : health.value);
          if (version.status === 'rejected') throw version.reason;
          const current = version.value;
          if (typeof current?.identity !== 'string' || !current.identity || !Number.isSafeInteger(current?.revision) || current.revision < 0) throw new Error('Invalid library revision');
          const token = JSON.stringify([current.identity, current.revision]);
          if (revision.current !== token) {
            await change.current?.();
            if (active) revision.current = token;
          }
        }
      }
      catch { /* Keep the last status; a later poll recovers. */ }
      finally {
        ownership.busy = false;
        ownership.resume?.();
      }
    };
    let interval = document.hidden ? null : setInterval(poll, 3000);
    const visible = () => {
      clearInterval(interval); interval = null;
      if (document.hidden) { clearTimeout(timer); timer = null; }
      else { dirty = true; poll(); interval = setInterval(poll, 3000); }
    };
    document.addEventListener('visibilitychange', visible);
    listen('library-changed', () => {
      if (!active) return;
      dirty = true;
      if (!document.hidden && !timer) timer = setTimeout(() => { timer = null; poll(); }, 750);
    }).then(stop => { if (active) { unlisten = stop; setNotificationError(''); } else stop(); }).catch(e => { if (active) setNotificationError(e.message || 'Live notifications are unavailable; polling continues.'); });
    return () => {
      active = false; document.removeEventListener('visibilitychange', visible); clearInterval(interval); clearTimeout(timer); unlisten?.();
      if (ownership.resume === resume) ownership.resume = null;
    };
  }, [available, ready, attempt]);
  const persist = useCallback(value => {
    try { localStorage.setItem(storageKey('background-indexing'), String(value)); setPreferenceError(''); }
    catch { setPreferenceError('Could not save the automatic indexing preference.'); }
  }, []);
  const toggle = useCallback(() => {
    const next = !currentEnabled.current;
    currentEnabled.current = next;
    persist(next);
    setEnabled(next);
  }, [persist]);
  const retry = useCallback(() => { persist(currentEnabled.current); setAttempt(value => value + 1); }, [persist]);
  return { available, enabled, status, error: [...new Set([error, preferenceError, notificationError].filter(Boolean))].join(' '), busy, toggle, retry };
}
