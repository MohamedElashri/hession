import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { api } from '../api.js';
import DeviceSyncSettings from './DeviceSyncSettings.jsx';

vi.mock('../api.js', () => ({ api: vi.fn() }));

const defaults = {
  enabled: false, configured: false, setupPending: false, relayUrl: '', deviceName: '', scope: [],
  workspaces: [{ path: '/work/A', count: 2 }, { path: '/work/B', count: 3 }],
  keyFingerprint: 'test-fingerprint', status: { running: false, phase: 'Disabled', uploaded: 0, downloaded: 0 },
};
let current;
const commands = () => api.mock.calls.map(([command]) => command);
const clone = value => structuredClone(value);

beforeEach(() => {
  current = clone(defaults);
  api.mockReset();
  api.mockImplementation(async (command, args) => {
    if (command === 'get_device_sync_settings') return clone(current);
    if (command === 'preview_device_sync') return { sessions: args.workspaces.length * 2, entries: 8, estimatedBytes: 512 };
    throw new Error(`Unexpected IPC call: ${command}`);
  });
});

function mount() {
  const callbacks = { onLibraryChanged: vi.fn().mockResolvedValue(undefined), onBusyChange: vi.fn(), onHelp: vi.fn() };
  render(<DeviceSyncSettings {...callbacks}/>);
  return callbacks;
}

test('opening private settings reads only local state and never starts setup or transfer', async () => {
  mount();
  await screen.findByLabelText('Relay address');
  expect(commands()).toEqual(['get_device_sync_settings']);
  expect(screen.getByRole('button', { name: 'Create encrypted library' }).disabled).toBe(true);
  expect(screen.getByRole('checkbox', { name: /\/work\/A/ }).checked).toBe(false);
  expect(screen.queryByRole('button', { name: 'Enable device sync' })).toBeNull();
});

test('testing a relay is an explicit action and does not create keys or upload histories', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => command === 'test_device_sync_relay'
    ? { protocolVersion: 1, initialized: false } : handler(command, args));
  mount();
  await user.type(await screen.findByLabelText('Relay address'), 'https://relay.example.test');
  expect(commands()).toEqual(['get_device_sync_settings']);
  await user.click(screen.getByRole('button', { name: 'Test connection' }));
  await screen.findByText(/Relay reachable/);
  expect(api).toHaveBeenLastCalledWith('test_device_sync_relay', { relayUrl: 'https://relay.example.test' });
  expect(commands()).toEqual(['get_device_sync_settings', 'test_device_sync_relay']);
});

test('creation requires a preview and recovery backup, then enables without transferring', async () => {
  const user = userEvent.setup();
  const defaultHandler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => {
    if (command === 'setup_device_sync') {
      current = { ...current, configured: true, relayUrl: args.relayUrl, deviceName: args.deviceName, scope: args.workspaces };
      return { settings: clone(current), recoveryKey: 'hession1:fake-test-key' };
    }
    if (command === 'set_device_sync_enabled') { current.enabled = args.enabled; return clone(current); }
    return defaultHandler(command, args);
  });
  mount();
  await user.type(await screen.findByLabelText('Relay address'), 'https://relay.example.test');
  await user.type(screen.getByLabelText('Device name'), 'Test laptop');
  await user.type(screen.getByLabelText('Relay setup token'), 'test-only-administrator-token');
  await user.click(screen.getByRole('checkbox', { name: /\/work\/A/ }));
  expect(screen.getByRole('button', { name: 'Create encrypted library' }).disabled).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Preview selected histories' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Create encrypted library' }).disabled).toBe(false));
  await user.click(screen.getByRole('button', { name: 'Create encrypted library' }));
  const enable = await screen.findByRole('button', { name: 'Enable device sync' });
  expect(enable.disabled).toBe(true);
  expect(api).toHaveBeenCalledWith('setup_device_sync', expect.objectContaining({ workspaces: ['/work/A'] }));
  await user.click(screen.getByRole('checkbox', { name: 'I have saved my recovery key' }));
  expect(enable.disabled).toBe(false);
  await user.click(enable);
  await screen.findByRole('button', { name: 'Disable device sync' });
  expect(commands()).not.toContain('run_device_sync');
  expect(commands()).not.toContain('join_device_sync');
});

test('exporting an existing library key cannot bypass trusted fingerprint verification', async () => {
  const user = userEvent.setup();
  current = { ...current, configured: true, relayUrl: 'https://relay.example.test', deviceName: 'Joined laptop', scope: ['/work/A'] };
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => command === 'export_device_sync_recovery_key'
    ? { recoveryKey: 'hession1:fake-test-key' } : handler(command, args));
  mount();
  const enable = await screen.findByRole('button', { name: 'Enable device sync' });
  await user.click(screen.getByRole('button', { name: 'Preview selected histories' }));
  await user.click(screen.getByRole('button', { name: 'Show recovery key' }));
  await user.click(await screen.findByRole('checkbox', { name: 'I have saved my recovery key' }));
  expect(enable.disabled).toBe(true);
  await user.click(screen.getByRole('checkbox', { name: /I verified this fingerprint/ }));
  expect(enable.disabled).toBe(false);
  expect(commands()).not.toContain('set_device_sync_enabled');
});

test('joining is download-only until explicitly scoped, verified, and enabled', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => {
    if (command === 'join_device_sync') {
      current = { ...current, configured: true, relayUrl: args.relayUrl, deviceName: args.deviceName, scope: args.workspaces };
      return clone(current);
    }
    return handler(command, args);
  });
  mount();
  await screen.findByLabelText('Relay address');
  await user.click(screen.getByRole('button', { name: 'Join library' }));
  await user.type(screen.getByLabelText('Relay address'), 'https://relay.example.test');
  await user.type(screen.getByLabelText('Device name'), 'Second device');
  await user.type(screen.getByLabelText('Pairing invitation'), 'test-invitation');
  await user.type(screen.getByLabelText('Library recovery key'), 'hession1:fake-key');
  await user.click(screen.getByRole('button', { name: 'Preview selected histories' }));
  await user.click(screen.getByRole('button', { name: 'Connect this device' }));
  const enable = await screen.findByRole('button', { name: 'Enable device sync' });
  expect(enable.disabled).toBe(true);
  expect(api).toHaveBeenCalledWith('join_device_sync', {
    relayUrl: 'https://relay.example.test', invitation: 'test-invitation', recoveryKey: 'hession1:fake-key',
    deviceName: 'Second device', workspaces: [],
  });
  expect(screen.queryByLabelText('Library recovery key')).toBeNull();
  expect(screen.queryByText('hession1:fake-key')).toBeNull();
  expect(commands()).not.toContain('run_device_sync');
  expect(commands()).not.toContain('set_device_sync_enabled');
  expect(commands()).not.toContain('setup_device_sync');
});

test('an interrupted enrollment exposes recovery and blocks disconnect until the key is saved', async () => {
  const user = userEvent.setup();
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => {
    if (command === 'setup_device_sync') {
      current = { ...current, setupPending: true, relayUrl: args.relayUrl, deviceName: args.deviceName };
      throw new Error('Enrollment timed out after the relay committed.');
    }
    if (command === 'export_device_sync_recovery_key') return { recoveryKey: 'hession1:pending-test-key' };
    if (command === 'disconnect_device_sync') { current = clone(defaults); return clone(current); }
    return handler(command, args);
  });
  mount();
  await user.type(await screen.findByLabelText('Relay address'), 'https://relay.example.test');
  await user.type(screen.getByLabelText('Device name'), 'Test laptop');
  await user.type(screen.getByLabelText('Relay setup token'), 'test-only-setup-token');
  await user.click(screen.getByRole('button', { name: 'Preview selected histories' }));
  await user.click(screen.getByRole('button', { name: 'Create encrypted library' }));
  await screen.findByText(/Setup was interrupted/);
  expect(screen.queryByRole('button', { name: 'Enable device sync' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Create encrypted library' })).toBeNull();
  const disconnect = screen.getByRole('button', { name: 'Disconnect relay' });
  expect(disconnect.disabled).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Show recovery key' }));
  expect(disconnect.disabled).toBe(true);
  await user.click(await screen.findByRole('checkbox', { name: 'I have saved my recovery key' }));
  expect(disconnect.disabled).toBe(false);
  await user.click(disconnect);
  await screen.findByLabelText('Relay address');
  expect(commands()).not.toContain('set_device_sync_enabled');
  expect(commands()).not.toContain('run_device_sync');
});

test('changing selection invalidates a reviewed scope, including download-only selection', async () => {
  const user = userEvent.setup();
  current = { ...current, configured: true, scope: ['/work/A'] };
  mount();
  const enable = await screen.findByRole('button', { name: 'Enable device sync' });
  await user.click(screen.getByRole('checkbox', { name: /I verified this fingerprint/ }));
  await user.click(screen.getByRole('button', { name: 'Preview selected histories' }));
  await waitFor(() => expect(enable.disabled).toBe(false));
  await user.click(screen.getByRole('checkbox', { name: /\/work\/A/ }));
  expect(enable.disabled).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Preview selected histories' }));
  await waitFor(() => expect(enable.disabled).toBe(false));
  expect(api).toHaveBeenLastCalledWith('preview_device_sync', { workspaces: [] });
});

test('cancelled sync still refreshes completed imports and keeps controls busy until refresh finishes', async () => {
  const user = userEvent.setup();
  current = { ...current, configured: true, enabled: true, scope: ['/work/A'] };
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => {
    if (command === 'run_device_sync') throw new Error('Device sync stopped after one download.');
    return handler(command, args);
  });
  const callbacks = mount();
  let finishRefresh;
  callbacks.onLibraryChanged.mockImplementation(() => new Promise(resolve => { finishRefresh = resolve; }));
  await user.click(await screen.findByRole('button', { name: 'Sync now' }));
  await waitFor(() => expect(callbacks.onLibraryChanged).toHaveBeenCalledOnce());
  expect(screen.getByRole('button', { name: 'Sync now' }).disabled).toBe(true);
  expect(callbacks.onBusyChange).toHaveBeenLastCalledWith(true);
  await act(async () => finishRefresh());
  await screen.findByRole('alert');
  expect(screen.getByRole('button', { name: 'Sync now' }).disabled).toBe(false);
  expect(callbacks.onBusyChange).toHaveBeenLastCalledWith(false);
});

test('a library refresh failure is shown rather than becoming an unhandled rejection', async () => {
  const user = userEvent.setup();
  current = { ...current, configured: true, enabled: true };
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => command === 'run_device_sync' ? clone(current) : handler(command, args));
  const callbacks = mount();
  callbacks.onLibraryChanged.mockRejectedValue(new Error('Could not repaint imported histories.'));
  await user.click(await screen.findByRole('button', { name: 'Sync now' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Could not repaint imported histories.');
  expect(screen.getByRole('button', { name: 'Sync now' }).disabled).toBe(false);
});

test('pairing displays Unix-second expiry correctly and does not reveal the recovery key automatically', async () => {
  const user = userEvent.setup();
  current = { ...current, configured: true, enabled: true };
  const handler = api.getMockImplementation();
  const expiresAt = 1_794_500_000;
  api.mockImplementation(async (command, args) => command === 'create_device_sync_invitation'
    ? { invitation: 'test-only-invitation', expiresAt, keyFingerprint: 'test-fingerprint' } : handler(command, args));
  mount();
  await user.click(await screen.findByRole('button', { name: 'Pair another device' }));
  await screen.findByText('test-only-invitation');
  expect(screen.getByText(/Expires/).textContent).toContain(new Date(expiresAt * 1000).toLocaleString());
  expect(commands()).not.toContain('export_device_sync_recovery_key');
  expect(commands()).not.toContain('run_device_sync');
});

test('relay restore repair requires pausing and never starts a transfer by itself', async () => {
  const user = userEvent.setup();
  current = { ...current, configured: true, enabled: true, scope: ['/work/A'] };
  const handler = api.getMockImplementation();
  api.mockImplementation(async (command, args) => {
    if (command === 'set_device_sync_enabled') { current.enabled = args.enabled; return clone(current); }
    if (command === 'reset_device_sync_checkpoint') return clone(current);
    return handler(command, args);
  });
  mount();
  await screen.findByRole('button', { name: 'Disable device sync' });
  await user.click(screen.getByText('Repair after restoring a relay backup'));
  const repair = screen.getByRole('button', { name: 'Replay after relay restore' });
  expect(repair.disabled).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Disable device sync' }));
  await waitFor(() => expect(repair.disabled).toBe(false));
  await user.click(repair);
  await screen.findByText(/Checkpoint reset/);
  expect(commands()).not.toContain('run_device_sync');
  expect(commands()).not.toContain('disconnect_device_sync');
  expect(api).toHaveBeenCalledWith('set_device_sync_enabled', { enabled: false, workspaces: ['/work/A'] });
});

test('double activation cannot start two transfers or release busy state early', async () => {
  current = { ...current, configured: true, enabled: true };
  const handler = api.getMockImplementation();
  const pending = [];
  api.mockImplementation((command, args) => command === 'run_device_sync'
    ? new Promise(resolve => pending.push(resolve)) : handler(command, args));
  const callbacks = mount();
  const button = await screen.findByRole('button', { name: 'Sync now' });
  act(() => { fireEvent.click(button); fireEvent.click(button); });
  try {
    expect(commands().filter(command => command === 'run_device_sync')).toHaveLength(1);
    expect(callbacks.onBusyChange).toHaveBeenLastCalledWith(true);
  } finally {
    await act(async () => pending.forEach(finish => finish(clone(current))));
  }
  await waitFor(() => expect(button.disabled).toBe(false));
});
