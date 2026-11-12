import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { api } from './api.js';

// Capture the real entry tree while letting Testing Library own its DOM root.
const boot = vi.hoisted(() => ({ tree: null }));
vi.mock('react-dom/client', async importOriginal => {
  const actual = await importOriginal();
  return { ...actual, createRoot: (container, ...args) => container
    ? actual.createRoot(container, ...args) : { render: tree => { boot.tree = tree; } } };
});
vi.mock('./api.js', () => ({ api: vi.fn(), hasTauriRuntime: () => false }));
import './main.jsx';

const sessions = [
  { id: 1, title: 'First session', agent: 'codex', cwd: '/work', tags: [] },
  { id: 2, title: 'Selected session', agent: 'claude', cwd: '/work', tags: [] },
];

beforeEach(() => {
  window.history.replaceState(null, '', '/');
  const storage = new Map();
  vi.stubGlobal('localStorage', {
    getItem: key => storage.get(key) ?? null,
    setItem: (key, value) => storage.set(key, String(value)),
    removeItem: key => storage.delete(key), clear: () => storage.clear(),
  });
  window.matchMedia = vi.fn(() => ({ matches: false, addEventListener() {}, removeEventListener() {} }));
  api.mockReset();
  api.mockImplementation(async (command, args) => {
    if (command === 'get_optional_features') return { continuationEnabled: false };
    if (command === 'get_agents' || command === 'get_projects' || command === 'get_sync_checkpoints') return [];
    if (command === 'get_stats') return { sessions: 2, entries: 1000, agents: 2, files: 2 };
    if (command === 'get_facets') return { models: [], branches: [], tools: [] };
    if (command === 'get_sync_status') return { running: false };
    if (command === 'search_sessions') return { sessions, total: 2 };
    if (command === 'get_session') return {
      session: sessions.find(s => String(s.id) === args.id), entries: [], total: 1000, limit: 500,
    };
    if (command === 'get_device_sync_settings') return {
      configured: true, enabled: true, deviceName: 'Test device', relayUrl: 'https://relay.example.test',
      workspaces: [], scope: [], status: { running: false },
    };
    throw new Error(`Unexpected IPC call: ${command}`);
  });
});

test.each(['success', 'failure'])('a sync %s preserves the selected session and transcript page while refreshing imports', async outcome => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => {
    if (command === 'run_device_sync') {
      if (outcome === 'failure') throw new Error('Could not reach the relay');
      return { status: { uploaded: 0, downloaded: 1 } };
    }
    return handler(command, args);
  });
  render(boot.tree);
  await user.click(await screen.findByRole('button', { name: /^Claude Code.*Selected session/ }));
  await screen.findByRole('heading', { name: 'Selected session' });
  await user.click(screen.getByRole('button', { name: 'Next transcript page' }));
  await screen.findByRole('textbox', { name: 'Go to transcript page, current page 2 of 2' });
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  await user.click(await screen.findByRole('button', { name: 'Private sync' }));
  const callsBefore = api.mock.calls.length;
  await user.click(await screen.findByRole('button', { name: 'Sync now' }));
  await screen.findByText(outcome === 'failure' ? 'Could not reach the relay' : /Sync complete/);
  await waitFor(() => expect(api.mock.calls.slice(callsBefore)).toContainEqual([
    'get_session', { id: '2', limit: '500', page: '2' },
  ]));
  expect(new URL(window.location.href).searchParams.get('session')).toBe('2');
  await user.click(screen.getByRole('button', { name: 'Done' }));
  expect(screen.getByRole('heading', { name: 'Selected session' })).toBeTruthy();
  expect(screen.getByRole('textbox', { name: 'Go to transcript page, current page 2 of 2' })).toBeTruthy();
});
