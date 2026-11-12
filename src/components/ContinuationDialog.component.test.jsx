import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { api } from '../api.js';
import ContinuationDialog from './ContinuationDialog.jsx';

vi.mock('../api.js', () => ({ api: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

function preview(overrides = {}) {
  return {
    source: { title: 'Test task', family: 'claude', importedAt: '2026-11-12T10:00:00Z' },
    workspace: { path: '/work/test', exists: true, dirty: false, repositoryRoot: '/work/test', branch: 'main', commit: 'test-commit', statusKnown: true },
    checkpoint: 'reviewed-checkpoint', content: 'Historical context for a test task.', omissions: ['No private reasoning or credentials.'],
    warnings: [], cliFound: true, cliVersion: 'test-cli', canLaunch: true,
    providerNotice: 'The destination may send this context to its model provider.', ...overrides,
  };
}
const launched = { ok: true, terminal: 'Fake terminal', packagePath: '/tmp/test-context.md', destinationSessionId: null };
const requests = command => api.mock.calls.filter(([name]) => name === command);

beforeEach(() => {
  api.mockReset();
  api.mockImplementation(async command => {
    if (command === 'get_continuation_info') return preview();
    if (command === 'launch_continuation') return launched;
    throw new Error(`Unexpected IPC call: ${command}`);
  });
});

test('opening a handoff only previews, with tool payloads excluded by default', async () => {
  render(<ContinuationDialog sessionId="1" onClose={vi.fn()}/>);
  await screen.findByText('Test task');
  expect(requests('get_continuation_info')).toEqual([['get_continuation_info', { id: '1', target: 'codex', cwd: null, note: null, includeTools: false }]]);
  expect(requests('launch_continuation')).toHaveLength(0);
  expect(screen.getByRole('checkbox', { name: 'Include recent tool inputs and outputs' }).checked).toBe(false);
  expect(screen.getByText(/destination may send this context/)).toBeTruthy();
});

test('editing context invalidates launch until a new preview is reviewed', async () => {
  const user = userEvent.setup();
  let count = 0;
  api.mockImplementation(async command => {
    if (command === 'get_continuation_info') return preview({ checkpoint: `checkpoint-${++count}` });
    if (command === 'launch_continuation') return launched;
    throw new Error(`Unexpected IPC call: ${command}`);
  });
  render(<ContinuationDialog sessionId="1" onClose={vi.fn()}/>);
  await screen.findByText('Test task');
  await user.type(screen.getByRole('textbox', { name: /Handoff note/ }), 'Keep the public API.');
  expect(screen.getByRole('button', { name: 'Continue in Codex' }).disabled).toBe(true);
  await user.click(screen.getByRole('checkbox', { name: 'Include recent tool inputs and outputs' }));
  await user.click(screen.getByRole('button', { name: 'Refresh preview' }));
  await screen.findByText('Test task');
  expect(api).toHaveBeenLastCalledWith('get_continuation_info', {
    id: '1', target: 'codex', cwd: '/work/test', note: 'Keep the public API.', includeTools: true,
  });
  await user.click(screen.getByRole('button', { name: 'Continue in Codex' }));
  expect(api).toHaveBeenCalledWith('launch_continuation', {
    id: '1', target: 'codex', cwd: '/work/test', note: 'Keep the public API.', includeTools: true,
    checkpoint: 'checkpoint-2', terminal: null,
  });
  await screen.findByText(/Opened a new session/);
});

test('a double activation opens only one terminal even before a React state update', async () => {
  let finish;
  api.mockImplementation(async command => {
    if (command === 'get_continuation_info') return preview();
    if (command === 'launch_continuation') return new Promise(resolve => { finish = resolve; });
    throw new Error(`Unexpected IPC call: ${command}`);
  });
  render(<ContinuationDialog sessionId="1" onClose={vi.fn()}/>);
  await screen.findByText('Test task');
  const button = screen.getByRole('button', { name: 'Continue in Codex' });
  act(() => { fireEvent.click(button); fireEvent.click(button); });
  expect(requests('launch_continuation')).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Close continuation' }).disabled).toBe(true);
  await act(async () => finish(launched));
  await screen.findByText(/Opened a new session/);
});

test('late results for another source cannot replace the displayed checkpoint', async () => {
  const pending = {};
  api.mockImplementation((command, args) => {
    if (command === 'get_continuation_info') return new Promise(resolve => { pending[args.id] = resolve; });
    throw new Error(`Unexpected IPC call: ${command}`);
  });
  const view = render(<ContinuationDialog sessionId="1" onClose={vi.fn()}/>);
  view.rerender(<ContinuationDialog sessionId="2" onClose={vi.fn()}/>);
  await act(async () => pending['2'](preview({ source: { title: 'New source', family: 'codex' }, checkpoint: 'new' })));
  await screen.findByText('New source');
  await act(async () => pending['1'](preview({ source: { title: 'Old source', family: 'claude' }, checkpoint: 'old' })));
  expect(screen.queryByText('Old source')).toBeNull();
  expect(screen.getByText('New source')).toBeTruthy();
});

test('a new source resets the previous draft, harness choice, and tool opt-in', async () => {
  const user = userEvent.setup();
  api.mockImplementation(async (command, args) => {
    if (command === 'get_continuation_info') return preview({ source: { title: args.id === '1' ? 'First source' : 'Second source', family: 'claude' } });
    throw new Error(`Unexpected IPC call: ${command}`);
  });
  const view = render(<ContinuationDialog sessionId="1" onClose={vi.fn()}/>);
  await screen.findByText('First source');
  await user.selectOptions(screen.getByRole('combobox', { name: 'Destination harness' }), 'claude');
  await user.type(screen.getByRole('textbox', { name: /Handoff note/ }), 'Private draft for the first source.');
  await user.click(screen.getByRole('checkbox', { name: 'Include recent tool inputs and outputs' }));
  view.rerender(<ContinuationDialog sessionId="2" onClose={vi.fn()}/>);
  await screen.findByText('Second source');
  expect(screen.getByRole('combobox', { name: 'Destination harness' }).value).toBe('codex');
  expect(screen.getByRole('textbox', { name: /Handoff note/ }).value).toBe('');
  expect(screen.getByRole('checkbox', { name: 'Include recent tool inputs and outputs' }).checked).toBe(false);
  expect(screen.queryByText('First source')).toBeNull();
  expect(api).toHaveBeenLastCalledWith('get_continuation_info', { id: '2', target: 'codex', cwd: null, note: null, includeTools: false });
});

test('a context edit and immediate activation cannot launch the old reviewed request', async () => {
  render(<ContinuationDialog sessionId="1" onClose={vi.fn()}/>);
  await screen.findByText('Test task');
  const button = screen.getByRole('button', { name: 'Continue in Codex' });
  const note = screen.getByRole('textbox', { name: /Handoff note/ });
  act(() => {
    fireEvent.change(note, { target: { value: 'This edit has not been reviewed yet.' } });
    fireEvent.click(button);
  });
  expect(requests('launch_continuation')).toHaveLength(0);
  expect(button.disabled).toBe(true);
});

test('stale launch rejection requires another preview and never retries automatically', async () => {
  const user = userEvent.setup();
  api.mockImplementation(async command => {
    if (command === 'get_continuation_info') return preview();
    if (command === 'launch_continuation') throw new Error('The source changed. Refresh and review again.');
    throw new Error(`Unexpected IPC call: ${command}`);
  });
  render(<ContinuationDialog sessionId="1" onClose={vi.fn()}/>);
  await screen.findByText('Test task');
  await user.click(screen.getByRole('button', { name: 'Continue in Codex' }));
  expect((await screen.findByRole('alert')).textContent).toContain('The source changed.');
  expect(screen.getByRole('button', { name: 'Continue in Codex' }).disabled).toBe(true);
  expect(requests('launch_continuation')).toHaveLength(1);
  expect(requests('get_continuation_info')).toHaveLength(1);
  await user.click(screen.getByRole('button', { name: 'Refresh preview' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue in Codex' }).disabled).toBe(false));
  expect(requests('launch_continuation')).toHaveLength(1);
});
