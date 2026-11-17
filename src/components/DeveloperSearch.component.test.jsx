import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { api } from '../api.js';
import DeveloperSearch from './DeveloperSearch.jsx';
vi.mock('../api.js', () => ({ api: vi.fn() }));
const hit = { sessionId: 7, workspaceId: 2, entryId: 70, ordinal: 900, position: 501, title: 'Compiler repair',
  agent: 'codex', project: '/work/project', excerpt: '🦀abc <script>alert(1)</script>', highlights: [{ start: 1, end: 4 }],
  matchingLocations: ['entry'], starred: true, tags: ['review'] };
const page = (results = [hit], nextCursor = null) => ({ results, nextCursor, countLowerBound: results.length + Number(Boolean(nextCursor)), revision: { identity: 'fixture', revision: 1 } });
const props = { query: 'alpha beta', filters: {}, selected: null, onOpen: vi.fn() };
beforeEach(() => { api.mockReset(); api.mockResolvedValue(page()); });

test('sends the shared typed request and safely renders scalar highlights as text', async () => {
  const user = userEvent.setup(); const onOpen = vi.fn();
  const filters = { agent: 'codex', project: '/work/project', models: ['model'], tools: ['shell'], branch: 'feature', host: '@local', since: '2026-01-01', starred: true, tags: ['review'] };
  const { container } = render(<DeveloperSearch {...props} filters={filters} onOpen={onOpen}/>);
  const result = await screen.findByRole('button', { name: /Compiler repair.*Open matching entry/ });
  expect(api).toHaveBeenCalledWith('developer_search', { request: { query: 'alpha beta', mode: 'sessions', limit: 20, cursor: null, visibility: 'all_indexed_text', filters: { ...filters, since: new Date(2026, 0, 1).toISOString() } } });
  expect(container.querySelector('mark').textContent).toBe('abc'); expect(container.querySelector('script')).toBeNull();
  expect(result.textContent).toContain('<script>alert(1)</script>');
  expect(result.hasAttribute('data-session-navigation')).toBe(true); expect(result.closest('[data-session-list]')).toBeTruthy();
  await user.click(result); expect(onOpen).toHaveBeenCalledWith(hit);
});

test('cursor pages retain the identical absolute cutoff and support previous pages', async () => {
  const user = userEvent.setup(); api.mockImplementation(async (_, { request }) => page([{ ...hit, title: request.cursor ? 'Second page' : 'First page' }], request.cursor ? null : 'cursor-one'));
  render(<DeveloperSearch {...props} filters={{ since: 'week' }}/>);
  await screen.findByRole('button', { name: /First page.*Open matching entry/ });
  const cutoff = api.mock.calls[0][1].request.filters.since; expect(Number.isNaN(Date.parse(cutoff))).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Next developer results' }));
  await screen.findByRole('button', { name: /Second page.*Open matching entry/ });
  expect(api.mock.calls.at(-1)[1].request).toMatchObject({ cursor: 'cursor-one', filters: { since: cutoff } });
  expect(screen.getByText('Page 2')).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Previous developer results' }));
  await screen.findByRole('button', { name: /First page.*Open matching entry/ }); expect(api.mock.calls.at(-1)[1].request.cursor).toBeNull();
});

test('message mode persists across query changes and always starts at the first page', async () => {
  const user = userEvent.setup(); const { rerender } = render(<DeveloperSearch {...props}/>);
  await screen.findByRole('button', { name: /Compiler repair.*Open matching entry/ });
  const messagesButton = screen.getByRole('button', { name: 'Messages', exact: true });
  await user.click(messagesButton);
  expect(document.activeElement).toBe(messagesButton);
  await waitFor(() => expect(api.mock.calls.at(-1)[1].request.mode).toBe('messages'));
  rerender(<DeveloperSearch {...props} query="other query"/>);
  await waitFor(() => expect(api.mock.calls.at(-1)[1].request).toMatchObject({ query: 'other query', mode: 'messages', cursor: null }));
});

test('syntax errors retain positions and are distinguishable from stale cursors', async () => {
  api.mockRejectedValue(Object.assign(new Error('Unknown field at byte 3'), { kind: 'syntax', position: 3 }));
  render(<DeveloperSearch {...props} query="α wat:value"/>);
  const alert = await screen.findByRole('alert'); expect(alert.textContent).toContain('Query position: byte 3');
  expect(screen.queryByRole('button', { name: 'Retry search' })).toBeNull();
});

test('a changed library requires an explicit restart instead of silently shifting pages', async () => {
  const user = userEvent.setup(); let restarted = false;
  api.mockImplementation(async (_, { request }) => {
    if (request.cursor) throw Object.assign(new Error('Expired'), { kind: 'stale_cursor' });
    return page([{ ...hit, title: restarted ? 'Fresh result' : 'Initial result' }], 'old-cursor');
  });
  render(<DeveloperSearch {...props}/>); await screen.findByRole('button', { name: /Initial result.*Open matching entry/ });
  await user.click(screen.getByRole('button', { name: 'Next developer results' }));
  expect((await screen.findByRole('alert')).textContent).toContain('The library changed'); restarted = true;
  await user.click(screen.getByRole('button', { name: 'Restart search', exact: true }));
  await screen.findByRole('button', { name: /Fresh result.*Open matching entry/ }); expect(screen.getByText('Page 1')).toBeTruthy(); expect(api.mock.calls.at(-1)[1].request.cursor).toBeNull();
});

test('late responses cannot replace a newer query and metadata matches have session actions', async () => {
  let resolveOld; api.mockImplementationOnce(() => new Promise(resolve => { resolveOld = resolve; })).mockResolvedValue(page([{ ...hit, entryId: null, ordinal: null, position: null, excerpt: null, title: 'New metadata' }]));
  const { rerender } = render(<DeveloperSearch {...props}/>); await waitFor(() => expect(api).toHaveBeenCalledTimes(1));
  rerender(<DeveloperSearch {...props} query="new query"/>);
  await screen.findByRole('button', { name: /New metadata.*metadata match/ });
  resolveOld(page([{ ...hit, title: 'Old response' }]));
  await waitFor(() => expect(screen.queryByText('Old response')).toBeNull());
});

test('annotations are available without moving selection and invalid spans cannot inject markup', async () => {
  const user = userEvent.setup(); const onToggleStar = vi.fn();
  api.mockResolvedValue(page([{ ...hit, highlights: [{ start: -1, end: 10 }, { start: 1, end: 99 }] }]));
  const { container, rerender } = render(<DeveloperSearch {...props} onToggleStar={onToggleStar} starPending={new Set()}/>);
  await screen.findByText('Compiler repair'); expect(container.querySelector('mark')).toBeNull();
  await user.click(screen.getByRole('button', { name: 'Unstar Compiler repair' })); expect(onToggleStar).toHaveBeenCalledWith({ id: 7, starred: true });
  rerender(<DeveloperSearch {...props} onToggleStar={onToggleStar} starPending={new Set([7])}/>);
  expect(screen.getByRole('button', { name: 'Unstar Compiler repair' }).disabled).toBe(true);
});


test('background refresh preserves the focused result and guards pending activations', async () => {
  const onOpen = vi.fn(); const { rerender } = render(<DeveloperSearch {...props} onOpen={onOpen} refreshKey={1}/>);
  const result = await screen.findByRole('button', { name: /Compiler repair.*Open matching entry/ }); result.focus();
  rerender(<DeveloperSearch {...props} onOpen={onOpen} refreshKey={2}/>);
  expect(document.activeElement).toBe(result); expect(result.getAttribute('aria-disabled')).toBe('true');
  result.click(); expect(onOpen).not.toHaveBeenCalled();
  await waitFor(() => expect(result.getAttribute('aria-disabled')).toBe('false')); expect(document.activeElement).toBe(result);
});
