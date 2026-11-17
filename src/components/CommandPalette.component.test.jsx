import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { api } from '../api.js';
import { createCommandRegistry } from '../commands.js';
import CommandPalette from './CommandPalette.jsx';
import ShortcutHelp from './ShortcutHelp.jsx';

vi.mock('../api.js', () => ({ api: vi.fn() }));
beforeEach(() => { api.mockReset(); api.mockResolvedValue({ sessions: [] }); });

function props(overrides = {}) {
  return { registry: createCommandRegistry([
    { id: 'search', title: 'Search sessions', shortcuts: ['/'], run: vi.fn() },
    { id: 'resume', title: 'Resume session', shortcuts: ['Mod+f'], disabled: () => 'Open a loaded session first.', run: vi.fn() },
  ]), context: { mac: true }, projects: [{ path: '/work/demo', display_name: 'Demo workspace' }],
  onClose: vi.fn(), onSession: vi.fn(), onProject: vi.fn(), onExecute: vi.fn(), ...overrides };
}

test('combobox supports arrow/Home/End, disabled reasons, selection, and focus restoration', async () => {
  const user = userEvent.setup();
  const previous = document.createElement('button');
  document.body.append(previous); previous.focus();
  const callbacks = props();
  const { unmount } = render(<CommandPalette {...callbacks}/>);
  const input = screen.getByRole('combobox');
  expect(document.activeElement).toBe(input);
  expect(input.getAttribute('aria-controls')).toBe(screen.getByRole('listbox').id);
  await user.keyboard('{ArrowDown}{Enter}');
  expect(callbacks.onExecute).not.toHaveBeenCalled();
  const disabled = screen.getByRole('option', { name: /Resume session/ });
  expect(disabled.getAttribute('aria-disabled')).toBe('true');
  expect(disabled.textContent).toContain('Open a loaded session first.');
  expect(disabled.textContent).toContain('Cmd+F');
  await user.keyboard('{End}{Enter}');
  expect(callbacks.onProject).toHaveBeenCalledWith(callbacks.projects[0]);
  await user.keyboard('{Home}{Enter}');
  expect(callbacks.onExecute).toHaveBeenCalledWith('search');
  fireEvent.keyDown(input, { key: 'Enter', isComposing: true });
  expect(callbacks.onExecute).toHaveBeenCalledTimes(1);
  await user.keyboard('{Tab}');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Close command palette' }));
  await user.keyboard('{Tab}');
  expect(document.activeElement).toBe(input);
  await user.keyboard('{Escape}');
  expect(callbacks.onClose).toHaveBeenCalledOnce();
  unmount();
  expect(document.activeElement).toBe(previous);
  previous.remove();
});

test('session discovery is debounced, bounded and ignores late results', async () => {
  const user = userEvent.setup();
  let resolveOld;
  api.mockImplementation((command, args) => args.q === '' ? new Promise(resolve => { resolveOld = resolve; })
    : Promise.resolve({ sessions: Array.from({ length: 20 }, (_, index) => ({ id: index + 1, title: `Needle session ${index + 1}` })) }));
  const callbacks = props({ projects: [] });
  render(<CommandPalette {...callbacks}/>);
  await waitFor(() => expect(api).toHaveBeenCalled());
  await user.type(screen.getByRole('combobox'), 'Needle');
  expect(screen.getByRole('status').textContent).toContain('Searching sessions');
  await screen.findByRole('option', { name: /Needle session 1$/ });
  expect(screen.getAllByRole('option')).toHaveLength(12);
  expect(api).toHaveBeenLastCalledWith('search_sessions', { q: 'Needle', limit: '12', offset: '0', sort: 'recent' });
  resolveOld({ sessions: [{ id: 99, title: 'Outdated session' }] });
  await waitFor(() => expect(screen.queryByText('Outdated session')).toBeNull());
  await user.keyboard('{Enter}');
  expect(callbacks.onSession).toHaveBeenCalledWith(1);
});

test('search failure, empty results and Linux shortcut help are explicit', async () => {
  const user = userEvent.setup();
  api.mockRejectedValue(new Error('Library unavailable'));
  const callbacks = props({ projects: [] });
  const { unmount } = render(<CommandPalette {...callbacks}/>);
  await user.type(screen.getByRole('combobox'), 'unmatched');
  await screen.findByText('Session search unavailable: Library unavailable');
  expect(screen.queryAllByRole('option')).toHaveLength(0);
  await user.clear(screen.getByRole('combobox'));
  api.mockResolvedValue({ sessions: [] });
  await user.type(screen.getByRole('combobox'), 'absent');
  await screen.findByText('No matching commands, sessions, or workspaces.');
  unmount();
  render(<ShortcutHelp {...callbacks} context={{ mac: false }}/>);
  expect(screen.getByText('Ctrl+F')).toBeTruthy();
  expect(screen.getByRole('dialog').textContent).toContain('Open a loaded session first.');
  await user.keyboard('{Escape}');
  expect(callbacks.onClose).toHaveBeenCalledOnce();
});

test.each([{ sessions: {} }, { sessions: [null] }, { sessions: [{ id: -1 }] }, { sessions: [{ id: 1, title: {} }] }])('malformed IPC response stays inside the palette error boundary: %j', async value => {
  api.mockResolvedValue(value);
  render(<CommandPalette {...props()}/>);
  await screen.findByText('Session search unavailable: Invalid session search response.');
  expect(screen.getByRole('option', { name: /Search sessions/ })).toBeTruthy();
});

test('modified editing keys and repeated activation do not run palette actions; help traps focus', async () => {
  const user = userEvent.setup();
  const callbacks = props();
  const { unmount } = render(<CommandPalette {...callbacks}/>);
  const input = screen.getByRole('combobox');
  const initial = input.getAttribute('aria-activedescendant');
  fireEvent.keyDown(input, { key: 'End', ctrlKey: true });
  fireEvent.keyDown(input, { key: 'ArrowDown', shiftKey: true });
  expect(input.getAttribute('aria-activedescendant')).toBe(initial);
  fireEvent.keyDown(input, { key: 'Enter', repeat: true });
  fireEvent.keyDown(input, { key: 'Enter', keyCode: 229 });
  fireEvent.keyDown(input, { key: 'Escape', repeat: true });
  expect(callbacks.onExecute).not.toHaveBeenCalled();
  expect(callbacks.onClose).not.toHaveBeenCalled();
  await user.keyboard('{Shift>}{Tab}{/Shift}');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Close command palette' }));
  await user.keyboard('{Enter}');
  expect(callbacks.onClose).toHaveBeenCalledOnce();
  expect(callbacks.onExecute).not.toHaveBeenCalled();
  unmount();
  render(<ShortcutHelp {...callbacks}/>);
  const close = screen.getByRole('button', { name: 'Close keyboard shortcuts' });
  expect(document.activeElement).toBe(close);
  await user.keyboard('{Shift>}{Tab}{/Shift}');
  expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Done' }));
  await user.keyboard('{Tab}');
  expect(document.activeElement).toBe(close);
  fireEvent.keyDown(close, { key: 'Escape', keyCode: 229 });
  expect(callbacks.onClose).toHaveBeenCalledOnce();
});
