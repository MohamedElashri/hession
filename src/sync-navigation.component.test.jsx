import { render, screen, waitFor, fireEvent, within } from '@testing-library/react';
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
const transcriptPages = () => within(screen.getByRole('navigation', { name: 'Transcript pages' }));

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
      session: sessions.find(s => String(s.id) === args.id), entries: [], total: 1000, limit: Number(args.limit),
      offset: (Number(args.page) - 1) * Number(args.limit),
    };
    if (command === 'get_device_sync_settings') return {
      configured: true, enabled: true, deviceName: 'Test device', relayUrl: 'https://relay.example.test',
      workspaces: [], scope: [], status: { running: false },
    };
    throw new Error(`Unexpected IPC call: ${command}`);
  });
});

test('a 126-page transcript opens either end and jumps to a numbered page with one read', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  let finishLast;
  api.mockImplementation(async (command, args) => {
    if (command !== 'get_session') return handler(command, args);
    const page = args.page === 'last' ? 126 : Number(args.page);
    if (args.page === 'last') await new Promise(resolve => { finishLast = resolve; });
    return {
      session: sessions.find(session => String(session.id) === args.id),
      total: 12_501, limit: 100, offset: (page - 1) * 100,
      entries: [{ id: page, ordinal: page * 100, kind: 'message', role: 'assistant',
        parts: [{ kind: 'text', text: `Reply from page ${page}` }], tools: [] }],
    };
  });
  render(boot.tree);
  await screen.findByText('Reply from page 1');
  const last = transcriptPages().getByRole('button', { name: 'Last transcript page' });
  await user.click(last);
  expect(document.activeElement).toBe(last);
  expect(transcriptPages().getByRole('button', { name: 'Last transcript page' })).toBe(last);
  expect(last.getAttribute('aria-disabled')).toBe('true');
  await user.click(last);
  finishLast();
  await screen.findByText('Reply from page 126');
  expect(transcriptPages().getByText('Page 126 of 126')).toBeTruthy();
  await user.click(transcriptPages().getByRole('button', { name: 'Previous transcript page' }));
  await screen.findByText('Reply from page 125');
  await user.type(transcriptPages().getByRole('textbox', { name: 'Transcript page number' }), '73');
  await user.click(transcriptPages().getByRole('button', { name: 'Go to transcript page' }));
  await screen.findByText('Reply from page 73');
  await user.click(transcriptPages().getByRole('button', { name: 'First transcript page' }));
  await screen.findByText('Reply from page 1');
  expect(api.mock.calls.filter(([command]) => command === 'get_session').map(([, args]) => args.page))
    .toEqual(['1', 'last', '125', '73', '1']);
});

test('Last reaches the end from a numbered final page and can be used again', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => {
    if (command === 'get_session' && args.page === 'last') {
      return handler(command, { ...args, page: '10' });
    }
    return handler(command, args);
  });
  render(boot.tree);
  await screen.findByRole('heading', { name: 'First session' });
  await user.type(transcriptPages().getByRole('textbox', { name: 'Transcript page number' }), '10');
  await user.click(transcriptPages().getByRole('button', { name: 'Go to transcript page' }));
  await transcriptPages().findByText('Page 10 of 10');

  const transcript = screen.getByRole('region', { name: 'Transcript' });
  Object.defineProperty(transcript, 'scrollHeight', { value: 750 });
  transcript.scrollTop = 20;
  const last = transcriptPages().getByRole('button', { name: 'Last transcript page' });
  await user.click(last);
  await waitFor(() => expect(api.mock.calls).toContainEqual(
    ['get_session', { id: '1', limit: '100', page: 'last' }],
  ));
  await waitFor(() => expect(transcript.scrollTop).toBe(750));

  transcript.scrollTop = 10;
  await user.click(last);
  expect(transcript.scrollTop).toBe(750);
  expect(api.mock.calls.filter(([command, args]) => command === 'get_session' && args.page === 'last')).toHaveLength(1);
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
  await screen.findByText('Version 0.3.0');
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
  await transcriptPages().findByText('Page 2 of 10');
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  await user.click(await screen.findByRole('tab', { name: 'Private sync' }));
  const callsBefore = api.mock.calls.length;
  await user.click(await screen.findByRole('button', { name: 'Sync now' }));
  await screen.findByText(outcome === 'failure' ? 'Could not reach the relay' : /Sync complete/);
  await waitFor(() => expect(api.mock.calls.slice(callsBefore)).toContainEqual([
    'get_session', { id: '2', limit: '100', page: '2' },
  ]));
  expect(new URL(window.location.href).searchParams.get('session')).toBe('2');
  await user.click(screen.getByRole('button', { name: 'Done' }));
  expect(screen.getByRole('heading', { name: 'Selected session' })).toBeTruthy();
  expect(transcriptPages().getByText('Page 2 of 10')).toBeTruthy();
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
  await transcriptPages().findByText('Page 2 of 10');
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
  await waitFor(() => expect(api).toHaveBeenCalledWith('get_session', { id: '2', limit: '100', page: '8' }));
  await screen.findByRole('heading', { name: 'Selected session' });
  expect(transcriptPages().getByText('Page 8 of 10')).toBeTruthy();
  expect(new URL(window.location.href).searchParams.get('session')).toBe('2');
}, 15000);

test('focus mode restores the surrounding layout and keeps the same transcript and page', async () => {
  const user = userEvent.setup();
  render(boot.tree);
  await screen.findByRole('heading', { name: 'First session' });
  await user.click(screen.getByRole('button', { name: 'Next transcript page' }));
  await transcriptPages().findByText('Page 2 of 10');
  const transcript = screen.getByRole('region', { name: 'Transcript' });
  transcript.scrollTop = 120;
  const reads = api.mock.calls.filter(([command]) => command === 'get_session').length;
  await user.click(screen.getByRole('button', { name: 'Focus mode' }));
  expect(document.querySelector('.app-shell').classList.contains('reader-focus')).toBe(true);
  expect(screen.getByRole('region', { name: 'Transcript' })).toBe(transcript);
  expect(transcript.scrollTop).toBe(120);
  expect(transcriptPages().getByText('Page 2 of 10')).toBeTruthy();
  await user.keyboard('{Escape}');
  expect(document.querySelector('.app-shell').classList.contains('reader-focus')).toBe(false);
  expect(screen.getByRole('region', { name: 'Transcript' })).toBe(transcript);
  await user.click(screen.getByRole('button', { name: 'Focus mode' }));
  await user.keyboard('{Control>}b{/Control}');
  expect(document.querySelector('.app-shell').classList.contains('reader-focus')).toBe(false);
  expect(document.querySelector('.app-shell').classList.contains('sidebar-hidden')).toBe(false);
  expect(api.mock.calls.filter(([command]) => command === 'get_session')).toHaveLength(reads);
});

test('layout controls persist widths, keyboard resize panels, and preserve reading preferences across focus', async () => {
  const user = userEvent.setup();
  render(boot.tree);
  await screen.findByRole('heading', { name: 'First session' });
  const transcript = screen.getByRole('region', { name: 'Transcript' });
  const separator = screen.getByRole('separator', { name: 'Session list width' });
  fireEvent.keyDown(separator, { key: 'ArrowRight', shiftKey: true });
  expect(document.querySelector('.app-shell').style.getPropertyValue('--list-w')).toBe('410px');
  await user.click(screen.getByLabelText('Reading layout'));
  await user.click(screen.getByRole('checkbox', { name: 'Hide session details' }));
  await user.click(screen.getByRole('checkbox', { name: 'Use available reading width' }));
  expect(document.querySelector('.app-shell').style.getPropertyValue('--reading-w')).toBe('none');
  expect(screen.getByRole('slider', { name: 'Reading width' }).disabled).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Focus mode' }));
  await user.click(screen.getByRole('button', { name: 'Exit focus' }));
  expect(document.querySelector('.app-shell').classList.contains('reader-header-hidden')).toBe(true);
  expect(screen.getByRole('region', { name: 'Transcript' })).toBe(transcript);
  expect(JSON.parse(localStorage.getItem(storageKey('reading-layout')))).toEqual(expect.objectContaining({ listWidth: 410, headerHidden: true, fullWidth: true }));
});

test('a background refresh reuses the selected transcript and retains the list on transient query failure', async () => {
  const user=userEvent.setup(); const handler=api.getMockImplementation(); let refreshing=false;
  api.mockImplementation(async(command,args)=>{
    if(command==='run_device_sync'){refreshing=true;return {status:{uploaded:0,downloaded:1}};}
    if(command==='search_sessions'&&refreshing)throw new Error('Temporary query failure');
    if(command==='get_session')return args.knownToken?{unchanged:true,readToken:'same-session-page'}:{...await handler(command,args),readToken:'same-session-page'};
    return handler(command,args);
  });
  render(boot.tree); await screen.findByRole('heading',{name:'First session'});
  const transcript=screen.getByRole('region',{name:'Transcript'});
  await user.click(screen.getByRole('button',{name:'Settings'}));
  await user.click(await screen.findByRole('tab',{name:'Private sync'}));
  await user.click(await screen.findByRole('button',{name:'Sync now'}));
  await screen.findByText('Temporary query failure');
  await waitFor(()=>expect(api.mock.calls).toContainEqual(['get_session',{id:'1',limit:'100',page:'1',knownToken:'same-session-page'}]));
  await user.click(screen.getByRole('button',{name:'Done'}));
  expect(screen.getByRole('region',{name:'Transcript'})).toBe(transcript);
  expect(screen.getByRole('button',{name:/^Claude Code.*Selected session/})).toBeTruthy();
});

test('a library update during a pending Last page read refreshes that page once it loads', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  let finishLast;
  let synced = false;
  let listRefreshed = false;
  const lastPage = (text, total, offset, readToken) => ({
    session: sessions[0], total, offset, limit: 100, readToken,
    entries: [{ id: total, ordinal: total - 1, kind: 'message', role: 'assistant',
      parts: [{ kind: 'text', text }], tools: [] }],
  });
  api.mockImplementation(async (command, args) => {
    if (command === 'run_device_sync') {
      synced = true;
      return { status: { uploaded: 0, downloaded: 1 } };
    }
    if (command === 'search_sessions' && synced) listRefreshed = true;
    if (command === 'get_session' && args.page === 'last') {
      if (args.knownToken === 'old-tail') {
        return lastPage('New reply on the last page', 12_501, 12_500, 'new-tail');
      }
      return new Promise(resolve => {
        finishLast = () => resolve(lastPage('Earlier reply', 12_500, 12_400, 'old-tail'));
      });
    }
    return handler(command, args);
  });
  render(boot.tree);
  await screen.findByRole('heading', { name: 'First session' });
  await user.click(transcriptPages().getByRole('button', { name: 'Last transcript page' }));
  await waitFor(() => expect(finishLast).toBeDefined());
  await user.click(screen.getByRole('button', { name: 'Settings' }));
  await user.click(await screen.findByRole('tab', { name: 'Private sync' }));
  await user.click(await screen.findByRole('button', { name: 'Sync now' }));
  await waitFor(() => expect(listRefreshed).toBe(true));
  finishLast();

  await screen.findByText('New reply on the last page');
  expect(transcriptPages().getByText('Page 126 of 126')).toBeTruthy();
  expect(api.mock.calls.filter(([command, args]) => command === 'get_session' && args.page === 'last'))
    .toEqual([
      ['get_session', { id: '1', limit: '100', page: 'last' }],
      ['get_session', { id: '1', limit: '100', page: 'last', knownToken: 'old-tail' }],
    ]);
});
