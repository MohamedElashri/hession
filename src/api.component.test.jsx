import { expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { api } from './api.js';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
test('IPC normalization preserves structured syntax and cursor errors', async () => {
  invoke.mockRejectedValue({ kind: 'syntax', message: 'Unknown field', position: 4 });
  await expect(api('developer_search', {})).rejects.toMatchObject({ kind: 'syntax', message: 'Unknown field', position: 4 });
  invoke.mockRejectedValue({ kind: 'stale_cursor', message: 'Library changed', position: null });
  await expect(api('developer_search', {})).rejects.toMatchObject({ kind: 'stale_cursor', message: 'Library changed' });
});
