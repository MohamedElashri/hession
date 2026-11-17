import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, test, vi } from 'vitest';
import FeatureDocumentation from './FeatureDocumentation.jsx';

test('focused continuation help follows canonical privacy and MCP links offline', async () => {
  const user = userEvent.setup();
  const fetch = vi.spyOn(globalThis, 'fetch').mockRejectedValue(new Error('Offline'));
  const close = vi.fn();
  render(<FeatureDocumentation topic="continuation" onClose={close} />);
  expect(screen.getByRole('heading', { level: 1, name: 'Cross-harness continuation' })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'Import histories' })).toBeNull();
  await user.click(screen.getByRole('link', { name: 'Security' }));
  expect(screen.getByRole('heading', { level: 1, name: 'Security and privacy' })).toBeTruthy();
  await user.click(screen.getByRole('link', { name: 'MCP server permissions' }));
  expect(screen.getByRole('heading', { level: 1, name: 'Terminal commands and read-only MCP' })).toBeTruthy();
  expect(screen.getByRole('heading', { level: 2, name: 'MCP server permissions' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Back to settings' }));
  expect(close).toHaveBeenCalledOnce();
  expect(fetch).not.toHaveBeenCalled();
  expect(window.location.hash).toBe('');
});

test('hosting links return to focused setup and expose the bundled protocol', async () => {
  const user = userEvent.setup();
  render(<FeatureDocumentation topic="sync" onClose={() => {}} />);
  await user.click(screen.getByRole('button', { name: 'Self-hosting guide' }));
  await user.click(screen.getAllByRole('link', { name: 'Private sync' })[0]);
  expect(screen.getByRole('heading', { level: 1, name: 'Private device sync' })).toBeTruthy();
  await user.click(screen.getByRole('button', { name: 'Self-hosting guide' }));
  await user.click(screen.getByRole('link', { name: 'relay API' }));
  expect(screen.getByRole('heading', { level: 1, name: /Private sync protocol/ })).toBeTruthy();
  expect(screen.queryByRole('heading', { name: 'Import histories' })).toBeNull();
});
