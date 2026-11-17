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
  const [busy, setBusy] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const started = useRef(false), starting = useRef(null), currentEnabled = useRef(enabled), change = useRef(onChanged);
  const revision = useRef(null);
  useEffect(() => { change.current = onChanged; }, [onChanged]);
  useEffect(() => { currentEnabled.current = enabled; }, [enabled]);
  useEffect(() => {
    if (!ready || !available) return;
    let active = true;
    const update = async () => {
      try {
        if (!enabled && !started.current && !starting.current) { setBusy(false); return; }
        setBusy(true);
        if (starting.current) await starting.current;
        if (enabled) {
          starting.current = api('start_indexing').then(next => { started.current = true; return next; });
          let next;
          try { next = await starting.current; } finally { starting.current = null; }
          // A preference can change while startup is still in flight.
          if (!currentEnabled.current || next.phase === 'paused') next = await api('pause_indexing', { paused: !currentEnabled.current });
          if (active) setStatus(next);
        } else {
          const next = await api('pause_indexing', { paused: true });
          if (active) setStatus(next);
        }
        if (active) setError('');
      } catch (e) { if (active) setError(e.message); }
      finally { if (active) setBusy(false); }
    };
    update();
    return () => { active = false; };
  }, [available, ready, enabled, attempt]);
  useEffect(() => {
    if (!available || !ready) return;
    let active = true, polling = false, timer, unlisten;
    const poll = async () => {
      if (polling) return;
      polling = true;
      try {
        const [next, current] = await Promise.all([api('indexing_status'), api('observed_library_revision')]);
        if (active) {
          setStatus(next);
          const token = JSON.stringify(current);
          if (revision.current !== token) { revision.current = token; change.current?.(); }
        }
      }
      catch { /* Keep the last status; a later poll recovers. */ }
      finally { polling = false; }
    };
    const interval = setInterval(poll, 3000);
    listen('library-changed', () => {
      if (!active || timer) return;
      timer = setTimeout(() => { timer = null; if (active) change.current?.(); }, 750);
    }).then(stop => { if (active) unlisten = stop; else stop(); }).catch(e => { if (active) setError(e.message); });
    return () => { active = false; clearInterval(interval); clearTimeout(timer); unlisten?.(); };
  }, [available, ready]);
  const toggle = useCallback(() => {
    const next = !currentEnabled.current;
    currentEnabled.current = next;
    try { localStorage.setItem(storageKey('background-indexing'), String(next)); }
    catch { setError('Could not save the automatic indexing preference.'); }
    setEnabled(next);
  }, []);
  const retry = useCallback(() => setAttempt(value => value + 1), []);
  return { available, enabled, status, error, busy, toggle, retry };
}
