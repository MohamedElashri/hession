import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, test, vi } from 'vitest';
import ToolActivity, { TextPanel } from './ToolActivity.jsx';
import { Conversation } from './Conversation.jsx';
import { buildConversation } from '../transcript.js';

test('tools have separate command, complete arguments and output with exact copy', async () => {
  const writeText = vi.fn().mockResolvedValue();
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  const command = "echo 'two words'\nprintf '%s\\n' '$HOME'";
  const { container } = render(<ToolActivity tool={{ tool_name: 'exec', arguments_json: JSON.stringify({ cmd: command, cwd: '/work/demo' }), result_text: 'two words\n$HOME', exit_code: 0 }}/>);
  expect(screen.getByText('Tool call')).toBeTruthy();
  expect(screen.queryByRole('region', { name: 'Command' })).toBeNull();
  fireEvent.click(container.querySelector('summary'));
  const panel = await screen.findByRole('region', { name: 'Command' });
  expect(within(panel).getByLabelText('Command text').textContent).toBe(command);
  expect(screen.getByRole('region', { name: 'Output' }).textContent).toContain('$HOME');
  expect(screen.getByText('Exit 0')).toBeTruthy();
  fireEvent.click(within(panel).getByRole('button', { name: 'Copy command' }));
  expect(writeText).toHaveBeenCalledWith(command);
  expect(await within(panel).findByText('Copied')).toBeTruthy();
  expect(container.querySelector('.tool-arguments').textContent).toContain('/work/demo');
});

test('long output has an explicit preview, expands fully, and copies unseen text', async () => {
  const writeText = vi.fn().mockResolvedValue();
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  const output = `${'output line\n'.repeat(150)}END_OF_OUTPUT`;
  render(<TextPanel label="Output" text={output}/>);
  const pre = screen.getByLabelText('Output text');
  expect(pre.textContent).not.toContain('END_OF_OUTPUT');
  expect(screen.getByText(/Preview ·/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Copy output' }));
  expect(writeText).toHaveBeenCalledWith(output);
  fireEvent.click(screen.getByRole('button', { name: /Show all/ }));
  await vi.waitFor(() => expect(pre.textContent).toBe(output));
  fireEvent.click(screen.getByRole('button', { name: 'Show preview' }));
  await vi.waitFor(() => expect(pre.textContent).not.toContain('END_OF_OUTPUT'));
});

test('a long single-line command wraps locally and can scroll without changing its contents', () => {
  const command = `echo ${'x'.repeat(18000)}END`;
  render(<ToolActivity tool={{ tool_name: 'shell', command_text: command }} initiallyOpen/>);
  const panel = screen.getByRole('region', { name: 'Command' });
  const pre = within(panel).getByLabelText('Command text');
  expect(pre.className).toContain('is-wrapped');
  fireEvent.click(within(panel).getByRole('button', { name: 'Wrap command' }));
  expect(pre.className).toContain('is-unwrapped');
  expect(screen.getByText('No output recorded.')).toBeTruthy();
  expect(screen.getByRole('button', { name: /Show all/ })).toBeTruthy();
});

test('empty output, failures and missing output are distinguished; untrusted output stays text', () => {
  const { container } = render(<><ToolActivity tool={{ tool_name: 'bad', status: 'completed', exit_code: 1, result_text: '<img src=x onerror=alert(1)>' }} initiallyOpen/><ToolActivity tool={{ tool_name: 'empty', result_text: '' }} initiallyOpen/></>);
  expect(container.querySelector('.tool-failed').textContent).toContain('Exit 1');
  expect(container.querySelector('img')).toBeNull();
  expect(screen.getByText('Empty output.')).toBeTruthy();
  expect(screen.queryByText('No output recorded.')).toBeNull();
});

test('clipboard failures are reported and updated live output does not claim an old copy', async () => {
  const writeText = vi.fn().mockRejectedValue(Error('denied'));
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  const { rerender } = render(<TextPanel label="Output" text="old"/>);
  fireEvent.click(screen.getByRole('button', { name: 'Copy output' }));
  expect(await screen.findByText('Copy failed')).toBeTruthy();
  rerender(<TextPanel label="Output" text="new"/>);
  expect(screen.queryByText('Copy failed')).toBeNull();
});

test('code arguments retain their line breaks and failed calls stay visible in folded runs', () => {
  const turns = buildConversation([{ id: 1, role: 'assistant', tools: Array.from({ length: 10 }, (_, id) => ({ id, tool_name: `tool-${id}`, exit_code: id === 8 ? 9 : 0 })) }]);
  const { container } = render(<><Conversation turns={turns} agentName="Codex"/><ToolActivity tool={{ tool_name: 'javascript', arguments_json: JSON.stringify({ code: 'const a = 1;\nreturn a;', timeout: 5 }) }} initiallyOpen/></>);
  expect(screen.getByText('tool-8')).toBeTruthy();
  expect(screen.queryByText('tool-7')).toBeNull();
  expect(screen.getByLabelText('Code text').textContent).toBe('const a = 1;\nreturn a;');
  expect(container.querySelector('.tool-failed').textContent).toContain('Exit 9');
});

test('a Unicode preview boundary never leaves a dangling surrogate', () => {
  render(<TextPanel label="Output" text={`${'a'.repeat(11999)}😀tail`}/>);
  expect(screen.getByLabelText('Output text').textContent).toBe('a'.repeat(11999));
});

test('conversation separates speakers, preserves turn anchors and folds long tool runs', () => {
  const turns = buildConversation([{ id: 1, role: 'user', parts: [{ kind: 'text', text: 'Please check the build.' }] },
    { id: 2, role: 'assistant', parts: [{ kind: 'text', text: 'I will inspect the build.' }], tools: Array.from({ length: 9 }, (_, id) => ({ id, tool_name: `tool-${id}`, result_text: 'ok' })) },
    { id: 3, role: 'assistant', parts: [{ kind: 'text', text: 'The build passed.' }] }]);
  const { container } = render(<Conversation turns={turns} agentName="Codex" agentColor="blue"/>);
  expect(screen.getByRole('article', { name: 'Your message' })).toBeTruthy();
  expect(screen.getByRole('article', { name: 'Codex response' })).toBeTruthy();
  expect(screen.getByText('Agent')).toBeTruthy();
  expect(container.querySelector('#turn-1')).toBeTruthy();
  expect(container.querySelectorAll('.tool-activity')).toHaveLength(4);
  fireEvent.click(screen.getByRole('button', { name: 'Show 5 more tool calls' }));
  expect(container.querySelectorAll('.tool-activity')).toHaveLength(9);
  expect(screen.getByText('The build passed.')).toBeTruthy();
});
