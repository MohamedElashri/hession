import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';

/** One in-flight IPC plus a coalesced dirty flag, even while selection/filters change. */
export default function useLivePolling({ identity, enabled, load, interval = 2000 }) {
  const state = useRef({ active: false, enabled: false, generation: 0, busy: false, dirty: false, timer: null, due: 0, load: null, interval });
  const [eventError, setEventError] = useState('');
  const poll = useCallback(async function run() {
    const current = state.current;
    current.timer = null; current.due = 0;
    if (!current.active || !current.enabled) return;
    if (document.hidden) { current.dirty = true; return; }
    if (current.busy) { current.dirty = true; return; }
    current.busy = true; current.dirty = false;
    const generation = current.generation;
    let more = false;
    try { more = await current.load(() => current.active && current.enabled && current.generation === generation); }
    finally {
      current.busy = false;
      if (current.active && current.enabled && !document.hidden) {
        const delay = current.dirty ? 150 : more ? 100 : current.interval;
        clearTimeout(current.timer);
        current.due = Date.now() + delay;
        current.timer = setTimeout(run, delay);
      }
    }
  }, []);
  const refresh = useCallback((delay = 250) => {
    const current = state.current;
    if (!current.active || !current.enabled) return;
    current.dirty = true;
    if (current.busy || document.hidden) return;
    const due = Date.now() + delay;
    if (current.timer && current.due <= due) return;
    clearTimeout(current.timer); current.due = due;
    current.timer = setTimeout(poll, delay);
  }, [poll]);
  useEffect(() => { state.current.load = load; }, [load]);
  useEffect(() => {
    const current = state.current;
    current.active = true; current.enabled = enabled; current.interval = interval; current.generation++;
    refresh(0);
    return () => { current.generation++; current.enabled = false; clearTimeout(current.timer); current.timer = null; current.due = 0; };
  }, [identity, enabled, interval, refresh]);
  useEffect(() => {
    if (!enabled) return;
    let active = true, unlisten;
    listen('library-changed', () => refresh()).then(stop => {
      if (active) { unlisten = stop; setEventError(''); } else stop();
    }).catch(() => { if (active) setEventError('Live notifications are unavailable; polling continues.'); });
    return () => { active = false; unlisten?.(); };
  }, [enabled, refresh]);
  useEffect(() => {
    const current = state.current;
    const visible = () => {
      if (document.hidden) {
        clearTimeout(current.timer); current.timer = null; current.due = 0;
      } else refresh(0);
    };
    document.addEventListener('visibilitychange', visible);
    return () => { document.removeEventListener('visibilitychange', visible); current.active = false; };
  }, [refresh]);
  return { refresh, eventError };
}
