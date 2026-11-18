import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import DeviceSyncSettings from './components/DeviceSyncSettings.jsx';
import { api } from './api.js';
vi.mock('./api.js', () => ({ api: vi.fn() }));
afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks(); });
test('private sync polls only lightweight progress, single-flight, and stops on unmount', async () => {
  vi.useFakeTimers(); let finishSync, finishStatus, busy = 0, peak = 0;
  const settings = { configured:true, enabled:true, deviceName:'Synthetic', workspaces:[],scope:[],status:{} };
  api.mockImplementation((command) => {
    if (command==='get_device_sync_settings') return Promise.resolve(settings);
    if (command==='run_device_sync') return new Promise(resolve => { finishSync=resolve; });
    if (command==='get_device_sync_status') { busy++; peak=Math.max(peak,busy); return new Promise(resolve=>{finishStatus=()=>{busy--;resolve({phase:'Uploading',uploaded:1});};}); }
    throw new Error(command);
  });
  const changed = vi.fn(), onBusyChange = vi.fn();
  const view=render(<DeviceSyncSettings onLibraryChanged={changed} onBusyChange={onBusyChange}/>);
  await act(async()=>{}); fireEvent.click(screen.getByRole('button',{name:'Sync now'}));
  await act(async()=>vi.advanceTimersByTimeAsync(4000));
  expect(api.mock.calls.filter(([command])=>command==='get_device_sync_settings')).toHaveLength(1);
  expect(api.mock.calls.filter(([command])=>command==='get_device_sync_status')).toHaveLength(1);
  await act(async()=>finishStatus());
  expect(screen.getByText(/Syncing · Uploading/)).toBeTruthy();
  await act(async()=>vi.advanceTimersByTimeAsync(800)); expect(peak).toBe(1);
  await act(async()=>{finishStatus();finishSync(settings);});
  expect(changed).toHaveBeenCalledOnce();
  expect(api.mock.calls.filter(([command])=>command==='get_device_sync_settings')).toHaveLength(2);
  view.unmount(); const calls=api.mock.calls.length;
  await act(async()=>vi.advanceTimersByTimeAsync(4000)); expect(api.mock.calls).toHaveLength(calls);
});
