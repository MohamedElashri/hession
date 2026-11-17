import { act, fireEvent, render, renderHook, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { api, hasTauriRuntime } from './api.js';
import useIndexing from './useIndexing.js';
import IndexingControl from './components/IndexingControl.jsx';
import { storageKey } from './brand.js';

const events = vi.hoisted(() => ({ callback: null, stop: vi.fn() }));
vi.mock('./api.js', () => ({ api: vi.fn(), hasTauriRuntime: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async (_name, callback) => { events.callback = callback; return events.stop; }) }));
beforeEach(() => {
  vi.useFakeTimers();
  const saved = new Map();
  vi.stubGlobal('localStorage', { getItem: key => saved.get(key) ?? null, setItem: (key, value) => saved.set(key, String(value)) });
  hasTauriRuntime.mockReturnValue(true); events.stop.mockClear(); events.callback = null;
  api.mockReset();
  api.mockImplementation(async (command, args) => {
    if (command === 'start_indexing' || command === 'indexing_status') return { phase: 'active', roots: ['/synthetic'], errors: [] };
    if (command === 'pause_indexing') return { phase: args.paused ? 'paused' : 'active' };
    if (command === 'observed_library_revision') return { identity: 'fixture', revision: 1 };
    throw new Error(`Unexpected IPC ${command}`);
  });
});
afterEach(() => { vi.useRealTimers(); });

test('automatic observation waits for readiness, coalesces invalidations and releases listeners', async () => {
  const onChanged = vi.fn();
  const hook = renderHook(({ ready }) => useIndexing({ ready, onChanged }), { initialProps: { ready: false } });
  expect(api).not.toHaveBeenCalled();
  await act(async () => hook.rerender({ ready: true }));
  expect(api.mock.calls.filter(([command]) => command === 'start_indexing')).toHaveLength(1);
  await act(async () => { for (let n = 0; n < 100; n++) events.callback({ payload: { revision: n } }); });
  expect(onChanged).not.toHaveBeenCalled();
  await act(async () => vi.advanceTimersByTimeAsync(750));
  expect(onChanged).toHaveBeenCalledTimes(1);
  hook.unmount(); expect(events.stop).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(6000));
  expect(onChanged).toHaveBeenCalledTimes(1);
});

test('legacy opt-out stays private and explicit resume then pause persists', async () => {
  localStorage.setItem(storageKey('auto-sync'), 'false');
  const hook = renderHook(() => useIndexing({ ready: true }));
  await act(async () => {});
  expect(api.mock.calls.some(([command]) => command === 'start_indexing')).toBe(false);
  await act(async () => hook.result.current.toggle());
  expect(hook.result.current.status.phase).toBe('active');
  await act(async () => hook.result.current.toggle());
  expect(hook.result.current.status.phase).toBe('paused');
  expect(localStorage.getItem(storageKey('background-indexing'))).toBe('false');
});

test('a pause requested during startup is applied after admission', async () => {
  let resolve;
  api.mockImplementation((command, args) => command === 'start_indexing'
    ? new Promise(done => { resolve = done; }) : Promise.resolve({ phase: args?.paused ? 'paused' : 'active' }));
  const hook = renderHook(() => useIndexing({ ready: true }));
  await act(async () => hook.result.current.toggle());
  await act(async () => resolve({ phase: 'active' }));
  expect(hook.result.current.enabled).toBe(false);
  expect(hook.result.current.busy).toBe(false);
  expect(hook.result.current.status.phase).toBe('paused');
  expect(api).toHaveBeenCalledWith('pause_indexing', { paused: true });
});

test('failed control never claims a successful pause and offers retry', async () => {
  const toggle = vi.fn(), retry = vi.fn();
  render(<IndexingControl observer={{ available: true, enabled: false, busy: false, error: 'Library busy', status: { phase: 'active' }, toggle, retry }} detailed/>);
  expect(screen.getByRole('status').textContent).toBe('Indexing control needs attention');
  expect(screen.getByRole('alert').textContent).toBe('Library busy');
  fireEvent.click(screen.getByRole('button', { name: 'Retry indexing control' }));
  expect(retry).toHaveBeenCalledOnce();
});

test('browser preview never starts native observation', async () => {
  hasTauriRuntime.mockReturnValue(false);
  const hook = renderHook(() => useIndexing({ ready: true }));
  await act(async () => vi.advanceTimersByTimeAsync(6000));
  expect(api).not.toHaveBeenCalled(); expect(hook.result.current.available).toBe(false);
});

test('the development IPC preview does not enable native observation', async () => {
  const previous = window.__TAURI_INTERNALS__;
  window.__TAURI_INTERNALS__ = { isPreview: true };
  try {
    const hook = renderHook(() => useIndexing({ ready: true }));
    await act(async () => vi.advanceTimersByTimeAsync(6000));
    expect(api).not.toHaveBeenCalled();
    expect(hook.result.current.available).toBe(false);
    hook.unmount();
  } finally { window.__TAURI_INTERNALS__ = previous; }
});

test('revision polling observes another process even when this watcher cannot start', async () => {
  let revision = 1;
  const normal = api.getMockImplementation();
  api.mockImplementation(async (command, args) => {
    if (command === 'start_indexing') throw new Error('Another watcher owns this library');
    if (command === 'observed_library_revision') return { identity: 'fixture', revision };
    return normal(command, args);
  });
  const changed = vi.fn();
  const hook = renderHook(() => useIndexing({ ready: true, onChanged: changed }));
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(hook.result.current.error).toContain('Another watcher');
  expect(changed).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(changed).toHaveBeenCalledTimes(1);
  revision = 2;
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(changed).toHaveBeenCalledTimes(2);
});
