import { test } from 'node:test';
import assert from 'node:assert/strict';
import { toolPresentation } from './tool-presentation.js';
import { buildConversation } from './transcript.js';

test('command presentation preserves exact shell text and all auxiliary arguments', () => {
  const command = "printf '%s\\n' 'a b'\ncat <<'EOF'\n$HOME `literal`\nEOF";
  const view = toolPresentation({ tool_name: 'exec', arguments_json: JSON.stringify({ cmd: command, workdir: '/a b', timeout: 30 }), result_text: '' });
  assert.equal(view.command, command);
  assert.equal(JSON.parse(view.args).workdir, '/a b');
  assert.equal(view.result, '');
  assert.equal(view.outcome, 'Output recorded');
});

test('argument arrays are displayed without inventing shell quoting', () => {
  const args = { command: ['bash', '-lc', 'echo "two words"'], cwd: '/tmp' };
  const view = toolPresentation({ arguments_json: JSON.stringify(args) });
  assert.equal(view.command, null);
  assert.deepEqual(JSON.parse(view.args), args);
});

test('missing output stays passive, malformed arguments remain visible, and summary is bounded', () => {
  const view = toolPresentation({ arguments_json: 'not-json', command_text: 'x'.repeat(30000) });
  assert.equal(view.result, null);
  assert.equal(view.outcome, 'Invoked');
  assert.equal(view.args, 'not-json');
  assert.ok(view.summary.length < 330);
  assert.equal(view.command.length, 30000);
});

test('exit failures survive conversation grouping and take priority over a success status', () => {
  const [turn] = buildConversation([{ role: 'assistant', tools: [{ id: 1, status: 'completed', exit_code: 2, result_text: 'failure' }] }]);
  const view = toolPresentation(turn.items[0].calls[0]);
  assert.equal(view.failed, true);
  assert.equal(view.outcome, 'Exit 2');
  assert.equal(toolPresentation({ exit_code: 0 }).failed, false);
  assert.equal(toolPresentation({ status: 'error' }).failed, true);
});
