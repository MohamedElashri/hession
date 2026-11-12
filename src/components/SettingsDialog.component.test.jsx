import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { save } from '@tauri-apps/plugin-dialog';
import { api } from '../api.js';
import SettingsDialog from './SettingsDialog.jsx';
import { DEFAULT_APPEARANCE } from '../appearance.js';

vi.mock('../api.js', () => ({ api: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(), save: vi.fn() }));
afterEach(() => vi.unstubAllGlobals());

beforeEach(() => {
  api.mockReset();
  invoke.mockReset(); save.mockReset();
  api.mockImplementation(async command => {
    if (command === 'app_version') return { version: '0.3.0', supported: true };
    if (command === 'list_terminals') return { saved: 'default', terminals: [] };
    if (command === 'get_optional_features') return { continuationEnabled: false };
    if (command === 'get_device_sync_settings') return { configured: false, enabled: false, scope: [], workspaces: [], status: {} };
    throw new Error(`Unexpected IPC call: ${command}`);
  });
});

function mount(initialTab = 'continuation') {
  const callbacks = { onClose: vi.fn(), onAppearanceChange: vi.fn(), onAutoSyncChange: vi.fn(), onSectionChange: vi.fn(), onFeaturesChanged: vi.fn(), onLibraryChanged: vi.fn(), onManagedDataChanged: vi.fn(), onRemotesChanged: vi.fn() };
  render(<SettingsDialog initialTab={initialTab} preferences={DEFAULT_APPEARANCE} {...callbacks}/>);
  return callbacks;
}

test('About contains the updater without opening another dialog or checking for a release', async () => {
  const user = userEvent.setup();
  mount('appearance');
  await user.click(screen.getByRole('tab', { name: 'About', exact: true }));
  await screen.findByText('Hession · Version 0.3.0');
  expect(screen.getByRole('button', { name: 'Check for updates' })).toBeTruthy();
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  expect(api.mock.calls.map(([command]) => command)).toEqual(['get_optional_features', 'app_version']);
});

test('keyboard navigation stays in one dialog and only loads sections when visited', async () => {
  const user = userEvent.setup();
  mount('continuation');
  screen.getByRole('tab', { name: 'Continuation' }).focus();
  await user.keyboard('{Home}');
  await screen.findByRole('radio', { name: /Dark/ });
  expect(screen.getByRole('tab', { name: 'Appearance' }).getAttribute('aria-selected')).toBe('true');
  expect(api.mock.calls.map(([command]) => command)).toEqual(['get_optional_features']);
  await user.keyboard('{ArrowDown}');
  await screen.findByRole('combobox', { name: 'Terminal', exact: true });
  expect(document.activeElement).toBe(screen.getByRole('tab', { name: 'Terminal' }));
  await user.keyboard('{End}');
  await screen.findByRole('button', { name: 'Check for updates' });
  expect(screen.getByRole('tabpanel', { name: 'About' })).toBeTruthy();
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  expect(invoke).not.toHaveBeenCalled();
});

test('terminal drafts survive section changes and saving keeps Settings open', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  let finish;
  api.mockImplementation((command, args) => command === 'set_terminal_preference'
    ? new Promise(resolve => { finish = resolve; }) : handler(command, args));
  const callbacks = mount('terminal');
  await user.selectOptions(await screen.findByRole('combobox', { name: 'Terminal', exact: true }), 'custom');
  await user.type(screen.getByRole('textbox', { name: 'Executable path' }), '/opt/My Terminal/bin/terminal');
  await user.click(screen.getByRole('tab', { name: 'Appearance' }));
  await screen.findByRole('radio', { name: /Dark/ });
  await user.click(screen.getByRole('tab', { name: 'Terminal' }));
  expect(screen.getByRole('textbox', { name: 'Executable path' }).value).toBe('/opt/My Terminal/bin/terminal');
  await user.click(screen.getByRole('button', { name: 'Save', exact: true }));
  expect(api).toHaveBeenLastCalledWith('set_terminal_preference', {
    terminal: 'custom', customPath: '/opt/My Terminal/bin/terminal', customArgs: ['-e', 'sh', '{script}'],
  });
  expect(screen.getByRole('tab', { name: 'Appearance' }).disabled).toBe(true);
  fireEvent(screen.getByRole('dialog'), new Event('cancel', { cancelable: true }));
  expect(callbacks.onClose).not.toHaveBeenCalled();
  await act(async () => finish({ ok: true }));
  await screen.findByText('Terminal preference saved.');
  expect(screen.getByRole('tab', { name: 'Appearance' }).disabled).toBe(false);
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  expect(callbacks.onClose).not.toHaveBeenCalled();
});

test('sync setup drafts survive section and guide navigation without enabling sync', async () => {
  const user = userEvent.setup();
  mount('sync');
  await user.type(await screen.findByLabelText('Relay address'), 'https://relay.example.test');
  await user.type(screen.getByLabelText('Relay setup token'), 'synthetic-test-token');
  await user.click(screen.getByRole('tab', { name: 'Terminal' }));
  await screen.findByRole('combobox', { name: 'Terminal', exact: true });
  await user.click(screen.getByRole('tab', { name: 'Private sync' }));
  await user.click(screen.getByRole('button', { name: 'Setup and self-hosting guide' }));
  await user.click(screen.getByRole('button', { name: 'Back to settings' }));
  expect(screen.getByLabelText('Relay address').value).toBe('https://relay.example.test');
  expect(screen.getByLabelText('Relay setup token').value).toBe('synthetic-test-token');
  expect(api.mock.calls.map(([command]) => command).sort()).toEqual(['get_device_sync_settings', 'get_optional_features', 'list_terminals']);
});

test('backup operations block navigation and closing until the file picker finishes', async () => {
  const user = userEvent.setup();
  vi.stubGlobal('__TAURI_INTERNALS__', { invoke: vi.fn() });
  invoke.mockResolvedValue({ sessions: 2, entries: 4, tools: 0, totalSessions: 2, databaseBytes: 1000,
    databasePath: '/synthetic/sessions.sqlite', token: 'test', options: { agents: [], workspaces: [], tags: [] } });
  let finish;
  save.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  const callbacks = mount('data');
  const exportButton = await screen.findByRole('button', { name: 'Export SQLite backup' });
  await waitFor(() => expect(exportButton.disabled).toBe(false));
  await user.click(exportButton);
  expect(screen.getByRole('tab', { name: 'About' }).disabled).toBe(true);
  expect(screen.getByRole('button', { name: 'Close settings' }).disabled).toBe(true);
  fireEvent(screen.getByRole('dialog'), new Event('cancel', { cancelable: true }));
  expect(callbacks.onClose).not.toHaveBeenCalled();
  await act(async () => finish(null));
  expect(screen.getByRole('tab', { name: 'About' }).disabled).toBe(false);
  expect(invoke.mock.calls.map(([command]) => command)).toEqual(['preview_saved_data']);
});

test('opening continuation settings remains off and contacts no relay or harness', async () => {
  mount();
  const checkbox = screen.getByRole('checkbox', { name: 'Enable cross-harness continuation' });
  await waitFor(() => expect(checkbox.disabled).toBe(false));
  expect(checkbox.checked).toBe(false);
  expect(api.mock.calls.map(([command]) => command)).toEqual(['get_optional_features']);
  expect(screen.getByRole('dialog', { name: 'Settings' }).open).toBe(true);
});

test('continuation opt-in and opt-out change only the continuation setting', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => command === 'set_continuation_enabled'
    ? { continuationEnabled: args.enabled } : handler(command, args));
  const callbacks = mount();
  const checkbox = screen.getByRole('checkbox', { name: 'Enable cross-harness continuation' });
  await waitFor(() => expect(checkbox.disabled).toBe(false));
  await user.click(checkbox);
  await waitFor(() => expect(checkbox.checked).toBe(true));
  expect(callbacks.onFeaturesChanged).toHaveBeenLastCalledWith({ continuationEnabled: true });
  await user.click(checkbox);
  await waitFor(() => expect(checkbox.checked).toBe(false));
  expect(callbacks.onFeaturesChanged).toHaveBeenLastCalledWith({ continuationEnabled: false });
  expect(api.mock.calls.map(([command]) => command)).toEqual(['get_optional_features', 'set_continuation_enabled', 'set_continuation_enabled']);
});

test('failed settings reads fail closed, and cleanup remains available while disabled', async () => {
  const user = userEvent.setup();
  api.mockImplementation(async command => {
    if (command === 'get_optional_features') throw new Error('Settings are invalid.');
    if (command === 'cleanup_continuation_packages') return { removed: 2 };
    throw new Error(`Unexpected IPC call: ${command}`);
  });
  mount();
  expect((await screen.findByRole('alert')).textContent).toContain('Settings are invalid.');
  const checkbox = screen.getByRole('checkbox', { name: 'Enable cross-harness continuation' });
  expect(checkbox.disabled).toBe(true);
  expect(checkbox.checked).toBe(false);
  await user.click(screen.getByRole('button', { name: 'Delete saved handoff packages' }));
  await screen.findByText(/Deleted 2 saved handoff packages/);
  expect(api.mock.calls.map(([command]) => command)).toEqual(['get_optional_features', 'cleanup_continuation_packages']);
});

test('a pending consent write blocks close and Escape until its outcome is known', async () => {
  const user = userEvent.setup();
  let finish;
  const handler = api.getMockImplementation();
  api.mockImplementation((command, args) => command === 'set_continuation_enabled'
    ? new Promise(resolve => { finish = resolve; }) : handler(command, args));
  const callbacks = mount();
  const checkbox = screen.getByRole('checkbox', { name: 'Enable cross-harness continuation' });
  await waitFor(() => expect(checkbox.disabled).toBe(false));
  await user.click(checkbox);
  const dialog = screen.getByRole('dialog', { name: 'Settings' });
  expect(screen.getByRole('button', { name: 'Close settings' }).disabled).toBe(true);
  fireEvent(dialog, new Event('cancel', { cancelable: true }));
  expect(callbacks.onClose).not.toHaveBeenCalled();
  await act(async () => finish({ continuationEnabled: true }));
  expect(screen.getByRole('button', { name: 'Close settings' }).disabled).toBe(false);
  fireEvent(dialog, new Event('cancel', { cancelable: true }));
  expect(callbacks.onClose).toHaveBeenCalledOnce();
});

test('opening bundled guides and following their links stays inside the offline help view', async () => {
  const user = userEvent.setup();
  const fetch = vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('Network is forbidden in offline help tests.'));
  mount('sync');
  await screen.findByLabelText('Relay address');
  await user.click(screen.getByRole('button', { name: 'Setup and self-hosting guide' }));
  await screen.findByRole('heading', { level: 1, name: 'Private device sync' });
  await user.click(screen.getAllByRole('link', { name: 'Self-hosting' })[0]);
  await screen.findByRole('heading', { level: 1, name: /Self-hosting Hession/ });
  await user.click(screen.getByRole('button', { name: 'Device setup' }));
  await user.click(screen.getAllByRole('link', { name: 'Security' })[0]);
  await screen.findByRole('heading', { level: 1, name: 'Security and privacy' });
  expect(fetch).not.toHaveBeenCalled();
  expect(api.mock.calls.every(([command]) => ['get_optional_features', 'get_device_sync_settings'].includes(command))).toBe(true);
  expect(window.location.hash).toBe('');
});
