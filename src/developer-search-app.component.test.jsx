import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { api } from './api.js';
const boot = vi.hoisted(() => ({ tree: null }));
vi.mock('react-dom/client', async importOriginal => {
  const actual = await importOriginal();
  return { ...actual, createRoot: (container, ...args) => container ? actual.createRoot(container, ...args) : { render: tree => { boot.tree = tree; } } };
});
vi.mock('./api.js', () => ({ api: vi.fn(), hasTauriRuntime: () => false }));
import './main.jsx';
const sessions = [{ id: 1, title: 'Initial history', agent: 'codex', tags: [] }, { id: 2, title: 'Other history', agent: 'claude', tags: [] }];
const result = { sessionId: 2, entryId: 900, ordinal: 9000, position: 501, title: 'Matching passage', agent: 'claude', excerpt: 'Indexed result', highlights: [], starred: false, tags: ['review'] };
beforeEach(() => {
  window.history.replaceState(null, '', '/');
  const storage = new Map();
  vi.stubGlobal('localStorage', { getItem: key => storage.get(key) ?? null, setItem: (key, value) => storage.set(key, String(value)), removeItem: key => storage.delete(key) });
  window.matchMedia = vi.fn(() => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  Element.prototype.scrollIntoView = vi.fn();
  api.mockReset();
  api.mockImplementation(async (command, args) => {
    if (command === 'get_optional_features') return { continuationEnabled: false };
    if (['get_agents', 'get_projects', 'get_sync_checkpoints'].includes(command)) return [];
    if (command === 'get_stats') return { sessions: 2, entries: 1000, agents: 2, files: 0 };
    if (command === 'get_facets') return { models: [], branches: [], tools: [] };
    if (command === 'get_sync_status') return { running: false };
    if (command === 'search_sessions') return { sessions, total: 2 };
    if (command === 'developer_search') return { results: [result], countLowerBound: 1, nextCursor: null };
    if (command === 'get_session') return { session: sessions.find(session => String(session.id) === args.id),
      entries: args.id === '2' && args.page === '6' ? [{ id: 900, ordinal: 9000, kind: 'message', role: 'assistant', parts: [{ kind: 'text', text: 'Target matching passage' }], tools: [] }] : [],
      total: 1000, limit: Number(args.limit), offset: (Number(args.page) - 1) * Number(args.limit) };
    if (command === 'update_session_annotations') return { starred: args.starred, tags: [] };
    throw new Error(`Unexpected IPC: ${command}`);
  });
});

test('developer result opens its exact entry using row position rather than ordinal', async () => {
  const user = userEvent.setup(); render(boot.tree);
  await screen.findByRole('heading', { name: 'Initial history' });
  await user.type(screen.getByRole('textbox', { name: 'Search sessions', exact: true }), 'alpha beta');
  await user.click(await screen.findByRole('button', { name: /Matching passage.*Open matching entry 502 in full log/ }));
  await screen.findByText('Target matching passage');
  expect(screen.getByRole('tab', { name: 'Full log' }).getAttribute('aria-selected')).toBe('true');
  expect(within(screen.getByRole('navigation', { name: 'Transcript pages' })).getByText('Page 6 of 10')).toBeTruthy();
  expect(document.getElementById('entry-900').classList.contains('search-target')).toBe(true);
  expect(api.mock.calls).toContainEqual(['get_session', { id: '2', limit: '100', page: '6' }]);
  expect(api.mock.calls.some(([command, args]) => command === 'get_session' && Number(args.page) > 6)).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Legacy search', exact: true }));
  await user.click(await screen.findByRole('button', { name: /^Codex.*Initial history/ }));
  await screen.findByRole('heading', { name: 'Initial history' });
  await user.click(screen.getByRole('button', { name: /^Claude Code.*Other history/ }));
  await screen.findByRole('heading', { name: 'Other history' });
  expect(within(screen.getByRole('navigation', { name: 'Transcript pages' })).getByText('Page 1 of 10')).toBeTruthy();
  expect(document.getElementById('entry-900')).toBeNull();
}, 10000);

test('legacy search remains accessible and independent starred filter is retained', async () => {
  const user = userEvent.setup(); render(boot.tree);
  await screen.findByRole('heading', { name: 'Initial history' });
  await user.click(screen.getByRole('button', { name: 'Starred', exact: true }));
  await user.type(screen.getByRole('textbox', { name: 'Search sessions', exact: true }), 'alpha beta');
  await screen.findByText('Matching passage');
  expect(api.mock.calls.filter(([command]) => command === 'developer_search').at(-1)[1].request.filters.starred).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Legacy search', exact: true }));
  await waitFor(() => expect(api.mock.calls.filter(([command]) => command === 'search_sessions').at(-1)[1]).toMatchObject({ q: 'alpha beta', starred: 'true' }));
  expect(screen.queryByRole('group', { name: 'Search result type' })).toBeNull();
}, 10000);

test('developer search preserves annotation actions and prevents mismatched legacy filtered exports', async () => {
  const user = userEvent.setup(); render(boot.tree);
  await screen.findByRole('heading', { name: 'Initial history' });
  await user.type(screen.getByRole('textbox', { name: 'Search sessions', exact: true }), 'alpha beta');
  await user.click(await screen.findByRole('button', { name: 'Star Matching passage' }));
  expect(api.mock.calls).toContainEqual(['update_session_annotations', { id: 2, starred: true }]);
  await user.click(screen.getByRole('button', { name: 'Export this session' }));
  expect(screen.getByText(/Switch to Legacy search for filtered transcript exports/)).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Export scope' }));
  const filtered = screen.getByRole('menuitemradio', { name: 'Filtered view (Legacy search only)' }); expect(filtered.disabled).toBe(true);
}, 10000);
