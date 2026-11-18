import { StrictMode } from 'react';
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


test('StrictMode startup shares one admitted observer start', async () => {
  let finish;
  const normal = api.getMockImplementation();
  api.mockImplementation((command, args) => command === 'start_indexing' ? new Promise(resolve => { finish = resolve; }) : normal(command, args));
  const hook = renderHook(() => useIndexing({ ready: true }), { wrapper: StrictMode });
  expect(api.mock.calls.filter(([command]) => command === 'start_indexing')).toHaveLength(1);
  await act(async () => finish({ phase: 'active' }));
  expect(api.mock.calls.filter(([command]) => command === 'start_indexing')).toHaveLength(1);
  expect(hook.result.current.busy).toBe(false); expect(hook.result.current.status.phase).toBe('active');
});
test('a late pause converges to a newer resume without overlapping native controls', async () => {
  let finishPause, busy = 0, peak = 0;
  const normal = api.getMockImplementation();
  api.mockImplementation(async (command, args) => {
    if (command !== 'pause_indexing') return normal(command, args);
    busy++; peak = Math.max(peak, busy);
    if (args.paused) await new Promise(resolve => { finishPause = resolve; });
    busy--; return { phase: args.paused ? 'paused' : 'active' };
  });
  const hook = renderHook(() => useIndexing({ ready: true })); await act(async () => {});
  await act(async () => hook.result.current.toggle());
  await act(async () => hook.result.current.toggle());
  expect(api.mock.calls.filter(([command]) => command === 'start_indexing')).toHaveLength(1);
  await act(async () => finishPause());
  expect(peak).toBe(1); expect(hook.result.current.enabled).toBe(true);
  expect(hook.result.current.status.phase).toBe('active'); expect(hook.result.current.busy).toBe(false);
  expect(api.mock.calls.filter(([command]) => command === 'pause_indexing').map(([, args]) => args.paused)).toEqual([true, false]);
});
test('events and revision polling refresh once per observed snapshot', async () => {
  const changed = vi.fn(); renderHook(() => useIndexing({ ready: true, onChanged: changed }));
  await act(async () => { for (let i=0;i<100;i++) events.callback({payload:{revision:{identity:'fixture',revision:1}}}); });
  await act(async () => vi.advanceTimersByTimeAsync(750)); expect(changed).toHaveBeenCalledOnce();
  await act(async () => vi.advanceTimersByTimeAsync(5250)); expect(changed).toHaveBeenCalledOnce();
  await act(async () => { events.callback({payload:{}}); await vi.advanceTimersByTimeAsync(750); });
  expect(changed).toHaveBeenCalledOnce();
});
test('a failed poll waits for its other read before admitting another poll', async () => {
  let finishStatus;
  const normal = api.getMockImplementation();
  api.mockImplementation((command,args) => {
    if (command === 'indexing_status') return new Promise(resolve => { finishStatus = resolve; });
    if (command === 'observed_library_revision') return Promise.reject(Error('Synthetic read failure'));
    return normal(command,args);
  });
  const hook = renderHook(() => useIndexing({ ready: true }));
  await act(async () => vi.advanceTimersByTimeAsync(9000));
  expect(api.mock.calls.filter(([command]) => command === 'indexing_status')).toHaveLength(1);
  await act(async () => finishStatus({phase:'active'})); hook.unmount();
});
test('invalidations coalesce one follow-up while readers keep making progress', async () => {
  let finishFirst; const normal = api.getMockImplementation(); let reads=0;
  api.mockImplementation((command,args) => command === 'observed_library_revision'
    ? (++reads === 1 ? new Promise(resolve => { finishFirst=resolve; }) : Promise.resolve({identity:'fixture',revision:2})) : normal(command,args));
  const changed=vi.fn(); renderHook(() => useIndexing({ready:true,onChanged:changed}));
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  await act(async () => { for(let i=0;i<100;i++) events.callback({payload:{}}); await vi.advanceTimersByTimeAsync(750); });
  await act(async () => finishFirst({identity:'fixture',revision:1}));
  expect(changed).toHaveBeenCalledOnce();
  await act(async () => vi.advanceTimersByTimeAsync(750));
  expect(changed).toHaveBeenCalledTimes(2); expect(reads).toBe(2);
});
test('refresh work is awaited and failures can retry the same revision', async () => {
  let finish; const changed=vi.fn().mockImplementationOnce(() => new Promise(resolve => { finish=resolve; })).mockRejectedValueOnce(Error('Synthetic refresh failure')).mockResolvedValue(undefined);
  let revision=1; const normal=api.getMockImplementation();
  api.mockImplementation((command,args) => command==='observed_library_revision' ? Promise.resolve({identity:'fixture',revision}) : normal(command,args));
  renderHook(() => useIndexing({ready:true,onChanged:changed}));
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  revision=2;
  await act(async () => { events.callback({payload:{}}); await vi.advanceTimersByTimeAsync(6000); });
  expect(changed).toHaveBeenCalledOnce();
  await act(async () => finish());
  await act(async () => vi.advanceTimersByTimeAsync(750)); expect(changed).toHaveBeenCalledTimes(2);
  await act(async () => vi.advanceTimersByTimeAsync(3000)); expect(changed).toHaveBeenCalledTimes(3);
});

test('a status read started before a control change cannot overwrite its result', async () => {
  let finishStatus; const normal=api.getMockImplementation();
  api.mockImplementation((command,args) => command==='indexing_status' ? new Promise(resolve => { finishStatus=resolve; }) : normal(command,args));
  const hook=renderHook(() => useIndexing({ready:true})); await act(async()=>{});
  await act(async()=>vi.advanceTimersByTimeAsync(3000));
  await act(async()=>hook.result.current.toggle());
  expect(hook.result.current.status.phase).toBe('paused');
  await act(async()=>finishStatus({phase:'active'}));
  expect(hook.result.current.status.phase).toBe('paused');
});
test('replacement identities and restored counters refresh even when the counter decreases', async () => {
  let value={identity:'first',revision:10}; const normal=api.getMockImplementation();
  api.mockImplementation((command,args)=>command==='observed_library_revision'?Promise.resolve(value):normal(command,args));
  const changed=vi.fn();renderHook(()=>useIndexing({ready:true,onChanged:changed}));
  await act(async()=>vi.advanceTimersByTimeAsync(3000));
  value={identity:'first',revision:1}; await act(async()=>vi.advanceTimersByTimeAsync(3000));
  value={identity:'replacement',revision:1}; await act(async()=>vi.advanceTimersByTimeAsync(3000));
  expect(changed).toHaveBeenCalledTimes(3);
});

test('readiness changes retain ownership of unfinished reads and hand off pending hints', async () => {
  let finishStatus, statuses=0; const normal=api.getMockImplementation();
  api.mockImplementation((command,args)=>command==='indexing_status' && ++statuses===1 ? new Promise(resolve=>{finishStatus=resolve;}) : normal(command,args));
  const changed=vi.fn();
  const hook=renderHook(({ready})=>useIndexing({ready,onChanged:changed}),{initialProps:{ready:true}});
  await act(async()=>vi.advanceTimersByTimeAsync(3000));
  await act(async()=>hook.rerender({ready:false})); await act(async()=>hook.rerender({ready:true}));
  await act(async()=>{events.callback({payload:{}});await vi.advanceTimersByTimeAsync(3000);});
  expect(statuses).toBe(1); expect(changed).not.toHaveBeenCalled();
  await act(async()=>finishStatus({phase:'active'}));
  await act(async()=>vi.advanceTimersByTimeAsync(750));
  expect(statuses).toBe(2);expect(changed).toHaveBeenCalledOnce();
});
test('failed controls release admission and can retry without claiming a pause', async () => {
  let fail=true;const normal=api.getMockImplementation();
  api.mockImplementation((command,args)=>command==='pause_indexing' && fail ? Promise.reject(Error('Native control busy')) : normal(command,args));
  const hook=renderHook(()=>useIndexing({ready:true}));await act(async()=>{});
  await act(async()=>hook.result.current.toggle());
  expect(hook.result.current.error).toBe('Native control busy'); expect(hook.result.current.status.phase).toBe('active');
  expect(hook.result.current.busy).toBe(false);
  fail=false;await act(async()=>hook.result.current.retry());
  expect(hook.result.current.status.phase).toBe('paused');expect(hook.result.current.error).toBe('');
});
