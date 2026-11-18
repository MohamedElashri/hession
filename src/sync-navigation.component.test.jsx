import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { api } from './api.js';
import { storageKey } from './brand.js';

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

test('one Settings entry opens About and its updater inside the same window', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => command === 'app_version'
    ? { version: '0.3.0', supported: true } : handler(command, args));
  render(boot.tree);
  expect(screen.getAllByRole('button', { name: 'Settings' })).toHaveLength(1);
  expect(screen.queryByRole('button', { name: 'Appearance' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Remote hosts' })).toBeNull();
  await user.click(await screen.findByRole('button', { name: 'Settings' }));
  // The first lazy Settings import includes transforms; give loaded CI workers
  // a bounded wait without changing the production loading behavior.
  await user.click(await screen.findByRole('tab', { name: 'About' }, { timeout: 10000 }));
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  await screen.findByText('Hession · Version 0.3.0');
  expect(screen.getByRole('button', { name: 'Check for updates' })).toBeTruthy();
  expect(api.mock.calls.filter(([command]) => command.includes('update') || command === 'app_version'))
    .toEqual([['app_version']]);
}, 15000);

test('subagents are hidden by default and the toolbar includes them in both search engines', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  const helper = { id: 3, title: 'Background approval review', agent: 'codex', is_subagent: 1, tags: [] };
  api.mockImplementation(async (command, args) => {
    if (command === 'search_sessions') return { sessions: args.includeSubagents ? [...sessions, helper] : sessions, total: args.includeSubagents ? 3 : 2 };
    if (command === 'developer_search') return { results: [], countLowerBound: 0 };
    return handler(command, args);
  });
  render(boot.tree);
  await screen.findByRole('heading', { name: 'First session' });
  expect(api.mock.calls).toContainEqual(['search_sessions', expect.objectContaining({ includeSubagents: false })]);
  const toggle = screen.getByRole('button', { name: 'Show subagents' });
  expect(toggle.getAttribute('aria-pressed')).toBe('false');
  expect(screen.queryByText(helper.title)).toBeNull();
  await user.click(toggle);
  expect(await screen.findByText(helper.title)).toBeTruthy();
  expect(screen.getByText('Subagent')).toBeTruthy();
  await user.type(screen.getByRole('textbox', { name: 'Search sessions' }), 'approval');
  await waitFor(() => expect(api.mock.calls).toContainEqual(['developer_search', expect.objectContaining({ request: expect.objectContaining({ filters: expect.objectContaining({ excludeSubagents: false }) }) })]));
  await user.click(toggle);
  await waitFor(() => expect(api.mock.calls).toContainEqual(['developer_search', expect.objectContaining({ request: expect.objectContaining({ filters: expect.objectContaining({ excludeSubagents: true }) }) })]));
});

test('history refresh preferences are saved through Settings and the last section reopens', async () => {
  const user = userEvent.setup();
  render(boot.tree);
  await user.click(screen.getByRole('button', { name: 'Settings', exact: true }));
  await user.click(await screen.findByRole('tab', { name: 'History refresh' }));
  const checkbox = screen.getByRole('checkbox', { name: 'Refresh histories on startup' });
  expect(checkbox.checked).toBe(true);
  await user.click(checkbox);
  expect(checkbox.checked).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Done' }));
  await user.click(screen.getByRole('button', { name: 'Settings', exact: true }));
  expect((await screen.findByRole('checkbox', { name: 'Refresh histories on startup' })).checked).toBe(false);
  expect(screen.getByRole('tab', { name: 'History refresh' }).getAttribute('aria-selected')).toBe('true');
  expect(api.mock.calls.some(([command]) => command === 'start_sync' || command === 'run_device_sync')).toBe(false);
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
  await user.click(await screen.findByRole('tab', { name: 'Private sync' }));
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

test('keyboard commands navigate sessions and transcript without hijacking typing or dialogs', async () => {
  const user = userEvent.setup();
  render(boot.tree);
  await screen.findByRole('heading', { name: 'First session' });
  fireEvent.keyDown(window, { key: '/', isComposing: true });
  expect(document.activeElement).not.toBe(screen.getByRole('textbox', { name: 'Search sessions' }));
  await user.keyboard('/');
  const search = screen.getByRole('textbox', { name: 'Search sessions' });
  await waitFor(() => expect(document.activeElement).toBe(search));
  await user.type(search, 'j/?');
  expect(search.value).toBe('j/?');
  expect(screen.queryByRole('dialog')).toBeNull();
  await user.clear(search);
  await waitFor(() => expect(screen.queryByText('Loading…')).toBeNull());
  await user.keyboard('{Control>}{Shift>}l{/Shift}{/Control}');
  const first = screen.getByRole('button', { name: /^Codex.*First session/ });
  await waitFor(() => expect(document.activeElement).toBe(first));
  await user.keyboard('j');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: /^Claude Code.*Selected session/ }));
  expect(screen.getByRole('heading', { name: 'First session' })).toBeTruthy();
  await user.keyboard('{Enter}');
  await screen.findByRole('heading', { name: 'Selected session' });
  expect(new URL(window.location.href).searchParams.get('session')).toBe('2');
  await user.keyboard('{Control>}f{/Control}');
  await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('searchbox', { name: 'Search this session' })));
  await user.keyboard('{Control>}k{/Control}');
  const palette = await screen.findByRole('combobox');
  await user.type(palette, 'Next transcript page');
  await user.keyboard('{Enter}');
  await screen.findByRole('textbox', { name: 'Go to transcript page, current page 2 of 2' });
  await user.click(screen.getByRole('button', { name: 'Settings', exact: true }));
  await screen.findByRole('dialog');
  await user.keyboard('{Control>}k{/Control}');
  expect(screen.queryByRole('combobox', { name: 'Search commands, sessions, and workspaces' })).toBeNull();
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: 'Done' }));
  expect(api.mock.calls.some(([command]) => ['resume_session', 'run_device_sync', 'launch_continuation'].includes(command))).toBe(false);
});

test('macOS uses Command shortcuts and reduced motion keeps keyboard movement immediate', async () => {
  vi.spyOn(window.navigator, 'platform', 'get').mockReturnValue('MacIntel');
  localStorage.setItem(storageKey('appearance'), JSON.stringify({ reduceMotion: true }));
  const scroll = vi.fn();
  Object.defineProperty(Element.prototype, 'scrollIntoView', { configurable: true, writable: true, value: scroll });
  const user = userEvent.setup();
  render(boot.tree);
  await screen.findByRole('heading', { name: 'First session' });
  expect(document.documentElement.dataset.reduceMotion).toBe('true');
  expect(screen.getByRole('button', { name: 'Open command palette' }).textContent).toContain('Cmd+K');
  await user.keyboard('{Control>}k{/Control}');
  expect(screen.queryByRole('combobox')).toBeNull();
  await user.keyboard('{Meta>}k{/Meta}');
  expect(await screen.findByRole('combobox')).toBeTruthy();
  await user.keyboard('{Escape}');
  await user.keyboard('{Meta>}{Shift>}l{/Shift}{/Meta}');
  await waitFor(() => expect(document.activeElement.classList.contains('session-card')).toBe(true));
  await user.keyboard('j');
  expect(scroll).toHaveBeenLastCalledWith({ block: 'nearest', behavior: 'instant' });
  await user.keyboard('{Meta>}b{/Meta}');
  expect(screen.getByRole('button', { name: 'Show navigation' })).toBeTruthy();
  await user.keyboard('{Meta>}b{/Meta}');
  expect(screen.queryByRole('button', { name: 'Show navigation' })).toBeNull();
});

test('palette opens real export and Settings actions, Escape restores focus, sidebar keys stay scoped', async () => {
  const user = userEvent.setup();
  render(boot.tree);
  await screen.findByRole('heading', { name: 'First session' });
  const trigger = screen.getByRole('button', { name: 'Open command palette' });
  await user.click(trigger);
  await user.keyboard('{Escape}');
  expect(document.activeElement).toBe(trigger);
  await user.keyboard('{Control>}k{/Control}');
  await user.type(await screen.findByRole('combobox'), 'Export sessions');
  await user.keyboard('{Enter}');
  await screen.findByRole('dialog', { name: 'Export sessions' });
  await user.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  await user.keyboard('{Control>}k{/Control}');
  await user.type(await screen.findByRole('combobox'), 'Open settings');
  await user.keyboard('{Enter}');
  await screen.findByRole('dialog', { name: 'Settings' });
  await user.click(screen.getByRole('button', { name: 'Done' }));
  const sessionsButton = screen.getByRole('button', { name: /^Sessions/ });
  sessionsButton.focus();
  await user.keyboard('{ArrowDown}');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Live', exact: true }));
  await user.keyboard('j');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Live', exact: true }));
  await user.keyboard('?');
  expect(await screen.findByRole('dialog', { name: 'Keyboard shortcuts' })).toBeTruthy();
});

test('palette session and workspace results use existing navigation and Resume keeps its review step', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => {
    if (command === 'get_projects') return [{ path: '/work/demo', display_name: 'Demo workspace' }];
    if (command === 'list_terminals') return { terminals: [], saved: 'default' };
    if (command === 'get_resume_info') return { displayCommand: 'codex resume example' };
    return handler(command, args);
  });
  render(boot.tree);
  await screen.findByRole('heading', { name: 'First session' });
  await user.keyboard('{Control>}k{/Control}');
  await user.type(await screen.findByRole('combobox'), 'Selected session');
  await user.click(await screen.findByRole('option', { name: /Session Selected session/ }));
  await screen.findByRole('heading', { name: 'Selected session' });
  expect(new URL(window.location.href).searchParams.get('session')).toBe('2');
  await user.keyboard('{Control>}k{/Control}');
  await user.type(await screen.findByRole('combobox'), 'Resume selected');
  await user.keyboard('{Enter}');
  expect(await screen.findByRole('dialog', { name: 'Resume session' })).toBeTruthy();
  await screen.findByText('codex resume example');
  expect(api.mock.calls).toContainEqual(['get_resume_info', { id: '2' }]);
  expect(api.mock.calls.some(([command]) => command === 'resume_session')).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Close resume' }));
  await user.keyboard('{Control>}k{/Control}');
  await user.type(await screen.findByRole('combobox'), 'Demo workspace');
  await user.keyboard('{Enter}');
  await waitFor(() => expect(api.mock.calls.some(([command, args]) => command === 'search_sessions' && args.project === '/work/demo')).toBe(true));
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(screen.getByRole('button', { name: 'Remove filter demo' })).toBeTruthy();
});

test('Live navigation uses the registry and browser history while preserving the selected session', async () => {
  const user = userEvent.setup();
  render(boot.tree);
  await user.click(await screen.findByRole('button', { name: /^Claude Code.*Selected session/ }));
  await screen.findByRole('heading', { name: 'Selected session' });
  await user.keyboard('{Control>}k{/Control}');
  await user.type(await screen.findByRole('combobox'), 'Show live local activity');
  await user.keyboard('{Enter}');
  const heading = await screen.findByRole('heading', { name: 'Live local activity' });
  await waitFor(() => expect(document.activeElement).toBe(heading));
  expect(new URL(window.location.href).searchParams.get('view')).toBe('live');
  expect(screen.getByText('Live local activity is available in the desktop app.')).toBeTruthy();
  await user.keyboard('{Control>}k{/Control}');
  await user.type(await screen.findByRole('combobox', { name: 'Search commands, sessions, and workspaces' }), 'Show Files and Git');
  expect(screen.getByRole('option', { name: /Show Files and Git/ }).getAttribute('aria-disabled')).toBe('false');
  await user.keyboard('{Escape}');
  expect(document.activeElement).toBe(heading);
  await user.click(screen.getByRole('button', { name: /^Sessions/ }));
  await screen.findByRole('heading', { name: 'Selected session' });
  expect(new URL(window.location.href).searchParams.get('session')).toBe('2');
  expect(api.mock.calls.some(([command]) => ['session_delta', 'recent_local_sessions', 'pause_indexing', 'start_indexing'].includes(command))).toBe(false);
});

test('file evidence opens its source session and true transcript page despite gapped ordinals', async () => {
  const user = userEvent.setup(); const original = api.getMockImplementation();
  const project = { workspace_id: 1, recorded_path: '/work', display_name: 'work', session_count: 2, local_session_count: 2, synced_session_count: 0 };
  api.mockImplementation(async (command, args) => {
    if (command === 'project_catalog') return { projects: [project], has_more: false, next_offset: null };
    if (command === 'project_files') return { files: [], has_more: false, next_offset: null };
    if (command === 'recorded_git') return { contexts: [], entry_positions: {}, has_more: false, next_offset: null };
    if (command === 'project_activity') return { evidence: [{ id: 1, reported_path: 'src/a.rs', category: 'reported_write_intent', source: { session_id: 2, entry_id: 777, entry_ordinal: 9999 }, agent_family: 'codex' }], entry_positions: { 777: 750 }, has_more: false, next_offset: null };
    return original(command, args);
  });
  render(boot.tree);
  await user.click(screen.getByRole('button', { name: 'Files and Git' }));
  expect(new URL(window.location.href).searchParams.get('view')).toBe('projects');
  await user.click(await screen.findByRole('button', { name: 'Open session 2, event 9999' }));
  await waitFor(() => expect(api).toHaveBeenCalledWith('get_session', { id: '2', limit: '500', page: '2' }));
  await screen.findByRole('heading', { name: 'Selected session' });
  expect(screen.getByRole('textbox', { name: 'Go to transcript page, current page 2 of 2' })).toBeTruthy();
  expect(new URL(window.location.href).searchParams.get('session')).toBe('2');
}, 15000);
