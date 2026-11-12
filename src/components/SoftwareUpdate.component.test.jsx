import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, test, vi } from 'vitest';
import { UpdateController } from '../update-controller.js';
import SoftwareUpdate from './SoftwareUpdate.jsx';
import SettingsButton from './SettingsButton.jsx';
import SettingsDialog from './SettingsDialog.jsx';
import { api } from '../api.js';

const store = vi.hoisted(() => ({ current: null }));
vi.mock('../updates.js', () => ({ updates: {
  subscribe: listener => store.current.subscribe(listener),
  getSnapshot: () => store.current.getSnapshot(),
  loadInfo: () => store.current.loadInfo(),
  check: () => store.current.check(),
  install: () => store.current.install(),
  restart: () => store.current.restart(),
} }));
vi.mock('../api.js', () => ({ api: vi.fn() }));

let services, pkg, download, finishDownload;
beforeEach(() => {
  api.mockResolvedValue({ continuationEnabled: false });
  download = new Promise(resolve => { finishDownload = resolve; });
  pkg = {
    version: '0.4.0', body: 'A newer release.',
    download: vi.fn(async report => {
      report({ event: 'Started', data: { contentLength: 100 } });
      report({ event: 'Progress', data: { chunkLength: 40 } });
      await download;
      report({ event: 'Finished' });
    }),
    install: vi.fn(async () => {}), close: vi.fn(async () => {}),
  };
  services = {
    info: vi.fn(async () => ({ version: '0.3.0', supported: true })),
    check: vi.fn(async () => pkg), restart: vi.fn(async () => {}),
  };
  store.current = new UpdateController(services);
});

test('the complete update flow survives closing the dialog and waits for an explicit restart', async () => {
  const user = userEvent.setup();
  const view = render(<><SettingsButton onClick={vi.fn()}/><SoftwareUpdate/></>);
  await screen.findByText('Hession · Version 0.3.0');
  expect(services.check).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Check for updates' }));
  await user.click(await screen.findByRole('button', { name: 'Download and install' }));
  expect(screen.getByRole('progressbar', { name: 'Update download' }).value).toBe(40);
  // Keep the sidebar mounted while the dialog is closed.
  view.rerender(<SettingsButton onClick={vi.fn()}/>);
  expect(screen.getByText('Downloading update…')).toBeTruthy();
  await act(async () => finishDownload());
  expect(screen.getByText('Update ready to restart')).toBeTruthy();
  expect(pkg.install).toHaveBeenCalledOnce();
  expect(services.restart).not.toHaveBeenCalled();
  view.rerender(<><SettingsButton onClick={vi.fn()}/><SoftwareUpdate/></>);
  expect(screen.getByRole('heading', { name: 'Ready to restart' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Restart Hession' }));
  expect(services.restart).toHaveBeenCalledOnce();
  expect(services.check).toHaveBeenCalledOnce();
  expect(pkg.download).toHaveBeenCalledOnce();
});

test('restart waits for sync and a rejected restart remains retryable', async () => {
  const user = userEvent.setup();
  finishDownload();
  await store.current.check(); await store.current.install();
  const view = render(<SoftwareUpdate syncRunning/>);
  expect(screen.getByRole('button', { name: 'Restart Hession' }).disabled).toBe(true);
  await user.click(screen.getByRole('button', { name: 'Restart Hession' }));
  expect(services.restart).not.toHaveBeenCalled();
  services.restart.mockRejectedValueOnce(new Error('Saved-data operations are still running.'));
  view.rerender(<SoftwareUpdate syncRunning={false}/>);
  await user.click(screen.getByRole('button', { name: 'Restart Hession' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Saved-data operations');
  await user.click(screen.getByRole('button', { name: 'Restart Hession' }));
  expect(services.restart).toHaveBeenCalledTimes(2);
});

test('a rejected download never offers restart or installs the package', async () => {
  const user = userEvent.setup();
  pkg.download.mockRejectedValueOnce(new Error('Signature verification failed.'));
  render(<SoftwareUpdate/>);
  await user.click(screen.getByRole('button', { name: 'Check for updates' }));
  await user.click(await screen.findByRole('button', { name: 'Download and install' }));
  expect((await screen.findByRole('alert')).textContent).toContain('Signature verification failed');
  expect(screen.getByRole('button', { name: 'Try again' })).toBeTruthy();
  expect(screen.queryByRole('button', { name: 'Restart Hession' })).toBeNull();
  expect(pkg.install).not.toHaveBeenCalled();
});

test('source installations explain the manual fallback without contacting the updater', async () => {
  services.info.mockResolvedValue({ version: '0.3.0', supported: false, reason: 'Rebuild source installations from source.' });
  render(<SoftwareUpdate/>);
  await screen.findByText('Rebuild source installations from source.');
  await waitFor(() => expect(screen.queryByRole('button', { name: 'Check for updates' })).toBeNull());
  expect(services.check).not.toHaveBeenCalled();
});

test('a ready update keeps About’s accessible name and restart action inside Settings', async () => {
  finishDownload();
  await store.current.check(); await store.current.install();
  render(<SettingsDialog initialTab="about" onClose={vi.fn()}/>);
  expect(screen.getByRole('tab', { name: 'About', exact: true })).toBeTruthy();
  await screen.findByRole('button', { name: 'Restart Hession' });
  expect(screen.getAllByRole('dialog')).toHaveLength(1);
  expect(screen.getByRole('dialog', { name: 'Settings' })).toBeTruthy();
});
