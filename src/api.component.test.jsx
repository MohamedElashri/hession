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

test('reading output references restore the full body for display and copy', async () => {
  const text = 'Full output: café.\n'.repeat(1000);
  const value = { entries: [
    { id: 1, tools: [{ id: 42, result_text: text }], parts: [] },
    { id: 2, tools: [], parts: [{ kind: 'tool_result', text: null, tool_result_id: 42, data_json: '{"call_id":"call"}' }] },
  ] };
  invoke.mockResolvedValue(value);
  const result = await api('get_session', { id: '1' });
  expect(result).toBe(value);
  expect(result.entries[1].parts[0].text).toBe(text);
  expect(result.entries[1].parts[0].tool_result_id).toBeUndefined();
  expect(result.entries[1].parts[0].data_json).toBe('{"call_id":"call"}');
  invoke.mockResolvedValue({ unchanged: true });
  expect(await api('get_session')).toEqual({ unchanged: true });
});

test('a missing canonical tool output fails explicitly rather than displaying incomplete history', async () => {
  invoke.mockResolvedValue({ entries: [{ id: 2, tools: [], parts: [{ kind: 'tool_result', text: null, tool_result_id: 42 }] }] });
  await expect(api('get_session')).rejects.toThrow('Tool output changed');
});
