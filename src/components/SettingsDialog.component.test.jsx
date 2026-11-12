import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { api } from '../api.js';
import SettingsDialog from './SettingsDialog.jsx';

vi.mock('../api.js', () => ({ api: vi.fn() }));

beforeEach(() => {
  api.mockReset();
  api.mockImplementation(async command => {
    if (command === 'get_optional_features') return { continuationEnabled: false };
    if (command === 'get_device_sync_settings') return { configured: false, enabled: false, scope: [], workspaces: [], status: {} };
    throw new Error(`Unexpected IPC call: ${command}`);
  });
});

function mount(initialTab = 'continuation') {
  const callbacks = { onClose: vi.fn(), onTerminal: vi.fn(), onFeaturesChanged: vi.fn(), onLibraryChanged: vi.fn() };
  render(<SettingsDialog initialTab={initialTab} {...callbacks}/>);
  return callbacks;
}

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
