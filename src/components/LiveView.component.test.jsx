import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { api } from '../api.js';
import { listen } from '@tauri-apps/api/event';
import LiveView, { LiveTail } from './LiveView.jsx';

const bus = vi.hoisted(() => ({ handlers: new Set() }));
vi.mock('../api.js', () => ({ api: vi.fn(), hasTauriRuntime: () => true }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

const session = (id = 1, status = 'recent') => ({ id, title: `Local session ${id}`, agent: 'codex', product: 'Codex', project: '/work/demo', lastEventAt: '2026-11-17T12:00:00Z', status,
  latestEvent: { id: id * 10, ordinal: 900, position: 500, kind: 'message', role: 'assistant', summary: 'Recorded work', occurredAt: '2026-11-17T12:00:00Z' } });
const entry = (id, ordinal = id, position = id - 1) => ({ id, ordinal, position, kind: 'message', role: 'assistant', occurred_at: '2026-11-17T12:00:00Z', parts: [{ kind: 'text', text: `Event ${id}` }], tools: [] });
const delta = (entries, seq = 1, extra = {}) => ({ revision: { identity: 'library', revision: seq }, cursor: { identity: 'library', epoch: 1, seq, sessionId: 1 }, reset: false, deleted: false, entries, hasMore: false, ...extra });
const emit = () => { for (const handler of bus.handlers) handler({ payload: { revision: 5 } }); };
const tick = async (ms = 0) => { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); await act(async () => { await vi.advanceTimersByTimeAsync(0); }); };
const observer = () => ({ available: true, enabled: true, busy: false, status: { phase: 'active' }, toggle: vi.fn() });

beforeEach(() => {
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-11-17T12:01:00Z'));
  bus.handlers.clear(); api.mockReset(); listen.mockReset();
  listen.mockImplementation(async (name, handler) => { bus.handlers.add(handler); return () => bus.handlers.delete(handler); });
  api.mockImplementation(async (command, args) => {
    if (command === 'recent_local_sessions') return { sessions: [session()], hasMore: false, nextOffset: null, revision: 1 };
    if (command === 'session_delta') return delta([entry(args.sessionId * 10)], 1, { reset: !args.cursor });
    throw new Error(`Unexpected IPC: ${command}`);
  });
});
afterEach(() => { vi.useRealTimers(); });

test('local activity uses passive labels, bounded filters, exact latest-event jumps, and supplied indexing controls', async () => {
  const onOpen = vi.fn(), control = observer();
  render(<LiveView agents={[{ family: 'codex', product: 'Codex' }]} projects={[{ path: '/work/demo', display_name: 'Demo' }]} observer={control} onSelectSession={onOpen}/>);
  await tick();
  expect(screen.getByRole('heading', { name: 'Live local activity' })).toBeTruthy();
  expect(screen.getAllByText(/Recently updated/).length).toBeGreaterThan(0);
  expect(api).toHaveBeenCalledWith('recent_local_sessions', { agent: '', project: '', since: '2026-11-17T11:46:00.000Z', limit: 50, offset: 0 });
  fireEvent.change(screen.getByRole('combobox', { name: 'Live time window' }), { target: { value: 'all' } }); await tick();
  fireEvent.change(screen.getByRole('combobox', { name: 'Live project' }), { target: { value: '/work/demo' } }); await tick();
  fireEvent.change(screen.getByRole('combobox', { name: 'Live agent' }), { target: { value: 'codex' } }); await tick();
  expect(api).toHaveBeenCalledWith('recent_local_sessions', { agent: 'codex', project: '/work/demo', since: '', limit: 50, offset: 0 });
  fireEvent.click(screen.getByRole('button', { name: 'Pause automatic indexing' }));
  expect(control.toggle).toHaveBeenCalledOnce();
  const header = screen.getByRole('heading', { name: 'Local session 1' }).closest('header');
  fireEvent.click(within(header).getByRole('button', { name: 'Open full history' }));
  expect(onOpen).toHaveBeenCalledWith(1, { entryId: 10, position: 500 });
  expect(new Set(api.mock.calls.map(([command]) => command))).toEqual(new Set(['recent_local_sessions', 'session_delta']));
});

test('late tool updates preserve expansion and manual scroll; stable IDs prevent duplicate cards; reset replaces the tail', async () => {
  const onOpen = vi.fn();
  const first = { ...entry(1, 10, 0), tools: [{ id: 1, tool_name: 'shell', result_text: 'Original output' }] };
  api.mockResolvedValueOnce(delta([first, entry(2, 900, 500)], 1, { reset: true }));
  render(<LiveTail session={session()} available onSelectSession={onOpen}/>); await tick();
  const article = screen.getByText('Event 1').closest('article');
  const toggle = within(article).getByRole('button', { name: /assistant · message/ });
  fireEvent.click(toggle); toggle.focus();
  expect(screen.getByText('Original output')).toBeTruthy();
  const area = screen.getByRole('region', { name: 'Live transcript' }); area.scrollTop = 73;
  api.mockResolvedValueOnce(delta([{ ...first, tools: [{ id: 1, tool_name: 'shell', result_text: 'Late completed output' }] }, entry(3, 1000, 501)], 2));
  act(emit); await tick(250);
  expect(within(article).getByRole('button', { name: /assistant · message/ }).getAttribute('aria-expanded')).toBe('true');
  expect(screen.getByText('Late completed output')).toBeTruthy();
  expect(document.activeElement).toBe(toggle);
  expect(area.scrollTop).toBe(73);
  expect(document.querySelectorAll('[data-live-entry="1"]')).toHaveLength(1);
  expect(screen.getByRole('button', { name: '1 new event · Jump to latest' })).toBeTruthy();
  fireEvent.click(within(document.querySelector('[data-live-entry="2"]')).getByRole('button', { name: 'Open full history' }));
  expect(onOpen).toHaveBeenCalledWith(1, { entryId: 2, position: 500 });
  api.mockResolvedValueOnce(delta([entry(4, 4000, 900)], 3, { reset: true }));
  act(emit); await tick(250);
  expect(screen.queryByText('Event 1')).toBeNull(); expect(screen.getByText('Event 4')).toBeTruthy();
  expect(screen.getByText(/source history changed/)).toBeTruthy();
});

test('follow starts off, scroll-away suspends it, and pausing live never changes indexing', async () => {
  api.mockResolvedValue(delta([entry(1)], 1, { reset: true }));
  render(<LiveTail session={session()} available onSelectSession={vi.fn()}/>); await tick();
  const area = screen.getByRole('region', { name: 'Live transcript' });
  Object.defineProperties(area, { scrollHeight: { configurable: true, value: 1000 }, clientHeight: { configurable: true, value: 200 } });
  area.scrollTo = vi.fn(); area.scrollTop = 40;
  api.mockResolvedValueOnce(delta([entry(2)], 2)); act(emit); await tick(250);
  expect(area.scrollTop).toBe(40);
  fireEvent.click(screen.getByRole('button', { name: 'Follow latest events' }));
  expect(area.scrollTo).toHaveBeenCalledWith({ top: 1000, behavior: 'instant' });
  api.mockResolvedValueOnce(delta([entry(3)], 3)); act(emit); await tick(250);
  expect(area.scrollTop).toBe(1000);
  area.scrollTop = 100; fireEvent.scroll(area);
  expect(screen.getByRole('button', { name: 'Follow latest events' }).getAttribute('aria-pressed')).toBe('false');
  expect(screen.getByText(/Following paused after you scrolled away/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Pause live updates' }));
  const count = api.mock.calls.length; act(() => { for (let i = 0; i < 100; i++) emit(); }); await tick(6000);
  expect(api.mock.calls).toHaveLength(count);
  fireEvent.click(screen.getByRole('button', { name: 'Resume live updates' })); await tick();
  expect(screen.getByRole('button', { name: 'Follow latest events' }).getAttribute('aria-pressed')).toBe('false');
  expect(api.mock.calls.every(([command]) => command === 'session_delta')).toBe(true);
});

test('event storms and session changes keep one request in flight and ignore obsolete responses', async () => {
  let resolveFirst;
  api.mockImplementation((command, args) => args.sessionId === 1 ? new Promise(resolve => { resolveFirst = resolve; })
    : Promise.resolve(delta([entry(20)], 2, { reset: true })));
  const { rerender } = render(<LiveTail session={session(1)} available onSelectSession={vi.fn()}/>); await tick();
  rerender(<LiveTail session={session(2)} available onSelectSession={vi.fn()}/>);
  act(() => { for (let i = 0; i < 500; i++) emit(); }); await tick(1000);
  expect(api.mock.calls).toHaveLength(1);
  await act(async () => resolveFirst(delta([entry(10)], 1, { reset: true }))); await tick(150);
  expect(api.mock.calls).toHaveLength(2);
  expect(api.mock.calls[1][1]).toEqual({ sessionId: 2, cursor: null, limit: 100 });
  expect(screen.queryByText('Event 10')).toBeNull(); expect(screen.getByText('Event 20')).toBeTruthy();
  expect(screen.getByRole('button', { name: 'Follow latest events' }).getAttribute('aria-pressed')).toBe('false');
});

test('updates above the viewport preserve the visible entry anchor', async () => {
  const old = { ...entry(1), tools: [{ tool_name: 'shell', result_text: 'Old result' }] };
  api.mockResolvedValueOnce(delta([old, entry(2)], 1, { reset: true }));
  render(<LiveTail session={session()} available onSelectSession={vi.fn()}/>); await tick();
  const area = screen.getByRole('region', { name: 'Live transcript' });
  const first = document.querySelector('[data-live-entry="1"]'), second = document.querySelector('[data-live-entry="2"]');
  fireEvent.click(within(first).getByRole('button', { name: /assistant · message/ }));
  area.scrollTop = 70;
  area.getBoundingClientRect = () => ({ top: 0 });
  first.getBoundingClientRect = () => ({ top: -200, bottom: area.textContent.includes('Late result') ? 80 : -20 });
  second.getBoundingClientRect = () => ({ top: area.textContent.includes('Late result') ? 110 : 10, bottom: 180 });
  api.mockResolvedValueOnce(delta([{ ...old, tools: [{ tool_name: 'shell', result_text: 'Late result' }] }], 2));
  act(emit); await tick(250);
  expect(area.scrollTop).toBe(170);
});

test('event notification failure falls back to bounded polling', async () => {
  listen.mockRejectedValue(new Error('No event host'));
  api.mockResolvedValue(delta([entry(1)], 1, { reset: true }));
  render(<LiveTail session={session()} available onSelectSession={vi.fn()}/>); await tick();
  expect(screen.getByText('Live notifications are unavailable; polling continues.')).toBeTruthy();
  expect(api).toHaveBeenCalledOnce(); await tick(2000);
  expect(api).toHaveBeenCalledTimes(2);
});

test('jumping to latest acknowledges pending events even while another delta is in flight', async () => {
  api.mockResolvedValueOnce(delta([entry(1)], 1, { reset: true }));
  render(<LiveTail session={session()} available onSelectSession={vi.fn()}/>); await tick();
  api.mockResolvedValueOnce(delta([entry(2)], 2)); act(emit); await tick(250);
  let finish;
  api.mockReturnValueOnce(new Promise(resolve => { finish = resolve; })); act(emit); await tick(250);
  fireEvent.click(screen.getByRole('button', { name: '1 new event · Jump to latest' }));
  expect(screen.getByRole('button', { name: 'Jump to latest' })).toBeTruthy();
  await act(async () => finish(delta([entry(3)], 3))); await tick();
  expect(screen.getByRole('button', { name: '1 new event · Jump to latest' })).toBeTruthy();
});

test('delta catch-up and retained state stay bounded, and missing positions never become ordinal-based jumps', async () => {
  let page = 0;
  api.mockImplementation(async () => {
    const start = page++ * 100;
    return delta(Array.from({ length: 100 }, (_, n) => entry(start + n + 1, (start + n + 1) * 10, null)), page, { reset: page === 1, hasMore: page < 4 });
  });
  const onOpen = vi.fn();
  render(<LiveTail session={session()} available onSelectSession={onOpen}/>); await tick(); await tick(300);
  expect(document.querySelectorAll('[data-live-entry]')).toHaveLength(300);
  expect(document.querySelector('[data-live-entry="1"]')).toBeNull();
  expect(screen.getByText(/Older events left/)).toBeTruthy();
  const first = document.querySelector('[data-live-entry="101"]');
  fireEvent.click(within(first).getByRole('button', { name: 'Open full history' }));
  expect(onOpen).toHaveBeenCalledWith(1, { entryId: 101, position: null });
  expect(api.mock.calls).toHaveLength(4);
});

test('stale health, empty activity, unavailable sources and deletion have explicit recoverable states', async () => {
  const control = observer(); control.status.phase = 'degraded';
  api.mockImplementation(async command => command === 'recent_local_sessions'
    ? { sessions: [session(1, 'stale')], hasMore: false } : Promise.reject(new Error('Source unavailable')));
  render(<LiveView observer={control} onSelectSession={vi.fn()}/>); await tick();
  expect(screen.getByText(/recovery polling/)).toBeTruthy();
  expect(screen.getAllByText(/May be stale/).length).toBeGreaterThan(0);
  expect(screen.getByRole('alert').textContent).toContain('Source unavailable');
  api.mockResolvedValueOnce(delta([], 2, { deleted: true }));
  fireEvent.click(screen.getByRole('button', { name: 'Retry live events' })); await tick();
  expect(screen.getByText('This session is no longer indexed.')).toBeTruthy();
  api.mockResolvedValueOnce({ sessions: [], hasMore: false });
  fireEvent.change(screen.getByRole('combobox', { name: 'Live time window' }), { target: { value: 'all' } }); await tick();
  expect(screen.getByText('No local sessions recorded in this window.')).toBeTruthy();
  expect(screen.getByText('Select a local session to inspect its recorded events.')).toBeTruthy();
});

test('unmounted views dispose late event subscriptions and never commit late responses', async () => {
  let resolveListen, resolveDelta;
  const stop = vi.fn(); listen.mockReturnValue(new Promise(resolve => { resolveListen = resolve; }));
  api.mockReturnValue(new Promise(resolve => { resolveDelta = resolve; }));
  const { unmount } = render(<LiveTail session={session()} available onSelectSession={vi.fn()}/>); await tick();
  unmount(); await act(async () => { resolveListen(stop); resolveDelta(delta([entry(1)])); }); await tick(5000);
  expect(stop).toHaveBeenCalledOnce(); expect(api).toHaveBeenCalledOnce();
});

test('private event payloads stay collapsed, actual tool arguments expand explicitly, and reset clears reused IDs', async () => {
  const privateEntry = { ...entry(1), kind: 'reasoning', search_text: 'Combined private payload', parts: [{ kind: 'text', text: 'Private reasoning' }],
    tools: [{ id: 1, tool_name: 'shell', arguments_json: '{"command":"private argument"}', result_text: 'Private tool output' }] };
  api.mockResolvedValueOnce(delta([privateEntry], 1, { reset: true }));
  render(<LiveTail session={session()} available onSelectSession={vi.fn()}/>); await tick();
  expect(screen.queryByText('Combined private payload')).toBeNull();
  expect(screen.queryByText(/The source history changed/)).toBeNull();
  expect(screen.queryByText('Private reasoning')).toBeNull(); expect(screen.queryByText('Private tool output')).toBeNull();
  expect(screen.getByText('Recorded tool activity: shell')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: /assistant · reasoning/ }));
  expect(screen.getByText('{"command":"private argument"}')).toBeTruthy();
  expect(screen.getByText('Private tool output')).toBeTruthy();
  api.mockResolvedValueOnce(delta([{ ...privateEntry, tools: [{ tool_name: 'shell', result_text: 'Different source output' }] }], 2, { reset: true }));
  act(emit); await tick(250);
  expect(screen.getByRole('button', { name: /assistant · reasoning/ }).getAttribute('aria-expanded')).toBe('false');
  expect(screen.queryByText('Different source output')).toBeNull();
  expect(screen.queryByText('Private reasoning')).toBeNull();
});
